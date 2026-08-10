//! Waveform peaks: min/max pairs per fixed bin of frames, computed
//! incrementally by the take writer (it already sees every sample) and
//! stored as a sidecar per track. The sidecar is header + pairs to EOF —
//! no trailing count, so a crash mid-take leaves a readable file.
//!
//! Sidecar layout (little-endian): `b"TPK1"` · `samples_per_bin: u32` ·
//! `(min: i16, max: i16)` pairs. Pairs span all channels of the track,
//! scaled by 32767 with saturation.

use std::io::{self, Read, Write};
use std::path::Path;

/// Frames per bin: ~94 bins/sec at 48 kHz — one bin per pixel at the
/// deepest zoom, decimated client-side for wider views.
pub const PEAK_SAMPLES_PER_BIN: u32 = 512;

const MAGIC: &[u8; 4] = b"TPK1";

fn scale(v: f32) -> i16 {
    (v.clamp(-1.0, 1.0) * f32::from(i16::MAX)) as i16
}

/// Incremental min/max over one track's interleaved samples. A bin closes
/// every `PEAK_SAMPLES_PER_BIN` frames (all channels folded together).
pub struct PeakAccum {
    samples_per_bin: u32,
    filled: u32,
    min: f32,
    max: f32,
}

impl PeakAccum {
    pub fn new(channels: u16) -> Self {
        PeakAccum {
            samples_per_bin: PEAK_SAMPLES_PER_BIN * u32::from(channels),
            filled: 0,
            min: 0.0,
            max: 0.0,
        }
    }

    /// Returns the finished pair when this sample closes a bin.
    #[inline]
    pub fn push(&mut self, sample: f32) -> Option<(i16, i16)> {
        if self.filled == 0 {
            self.min = sample;
            self.max = sample;
        } else {
            self.min = self.min.min(sample);
            self.max = self.max.max(sample);
        }
        self.filled += 1;
        (self.filled == self.samples_per_bin).then(|| {
            self.filled = 0;
            (scale(self.min), scale(self.max))
        })
    }

    /// The partial tail bin, if any — call once at finalize.
    pub fn flush(&mut self) -> Option<(i16, i16)> {
        (self.filled > 0).then(|| {
            self.filled = 0;
            (scale(self.min), scale(self.max))
        })
    }
}

pub fn write_header(out: &mut impl Write) -> io::Result<()> {
    out.write_all(MAGIC)?;
    out.write_all(&PEAK_SAMPLES_PER_BIN.to_le_bytes())
}

pub fn write_pair(out: &mut impl Write, pair: (i16, i16)) -> io::Result<()> {
    out.write_all(&pair.0.to_le_bytes())?;
    out.write_all(&pair.1.to_le_bytes())
}

/// Read a sidecar: `(samples_per_bin, pairs)`. Trailing partial bytes (a
/// crash mid-pair) are dropped, not fatal.
pub fn read_sidecar(path: &Path) -> io::Result<(u32, Vec<(i16, i16)>)> {
    let mut bytes = Vec::new();
    std::fs::File::open(path)?.read_to_end(&mut bytes)?;
    if bytes.len() < 8 || &bytes[..4] != MAGIC {
        return Err(io::Error::other(format!(
            "{} is not a peaks sidecar",
            path.display()
        )));
    }
    let samples_per_bin = u32::from_le_bytes(bytes[4..8].try_into().expect("4 bytes"));
    let pairs = bytes[8..]
        .chunks_exact(4)
        .map(|c| {
            (
                i16::from_le_bytes([c[0], c[1]]),
                i16::from_le_bytes([c[2], c[3]]),
            )
        })
        .collect();
    Ok((samples_per_bin, pairs))
}

/// Backfill for takes recorded before peaks existed (or whose sidecar
/// tore): derive the pairs from the audio file and cache them as a sidecar
/// (temp + rename, so a torn write never masquerades as a good cache).
pub fn compute_from_audio(
    audio: &Path,
    format: crate::format::RecordFormat,
    sidecar: &Path,
) -> io::Result<Vec<(i16, i16)>> {
    use crate::format::RecordFormat;

    let mut accum;
    let mut pairs = Vec::new();
    match format {
        RecordFormat::Wav16 | RecordFormat::Wav24 | RecordFormat::Wav32Float => {
            let mut reader = hound::WavReader::open(audio).map_err(io::Error::other)?;
            accum = PeakAccum::new(reader.spec().channels);
            // Peaks scale by i16::MAX either way; f32 keeps one decode path.
            let scale = match format {
                RecordFormat::Wav32Float => 1.0,
                RecordFormat::Wav16 => 1.0 / 32_768.0,
                _ => 1.0 / 8_388_608.0,
            };
            if format.is_float() {
                for sample in reader.samples::<f32>() {
                    if let Some(pair) = accum.push(sample.map_err(io::Error::other)?) {
                        pairs.push(pair);
                    }
                }
            } else {
                for sample in reader.samples::<i32>() {
                    let v = sample.map_err(io::Error::other)? as f32 * scale;
                    if let Some(pair) = accum.push(v) {
                        pairs.push(pair);
                    }
                }
            }
        }
        RecordFormat::Flac16 | RecordFormat::Flac24 => {
            let (mut samples, info) = crate::flac::FlacSamples::open(audio, 0)?;
            accum = PeakAccum::new(info.channels);
            while let Some(sample) = samples.next_sample() {
                if let Some(pair) = accum.push(sample) {
                    pairs.push(pair);
                }
            }
        }
    }
    if let Some(pair) = accum.flush() {
        pairs.push(pair);
    }

    let tmp = sidecar.with_extension("peaks.tmp");
    let mut out = io::BufWriter::new(std::fs::File::create(&tmp)?);
    write_header(&mut out)?;
    for &pair in &pairs {
        write_pair(&mut out, pair)?;
    }
    out.flush()?;
    std::fs::rename(&tmp, sidecar)?;
    Ok(pairs)
}

/// Peaks for every track of a take, sidecar-first with decode backfill —
/// backfills run in parallel across tracks (a legacy take decodes every
/// file in full).
pub fn read_or_compute(
    take_dir: &Path,
    format: crate::format::RecordFormat,
    files: &[String],
) -> io::Result<Vec<Vec<(i16, i16)>>> {
    use rayon::prelude::*;

    files
        .par_iter()
        .map(|file| {
            let sidecar = take_dir.join(file).with_extension("peaks");
            match read_sidecar(&sidecar) {
                Ok((_, pairs)) => Ok(pairs),
                Err(_) => compute_from_audio(&take_dir.join(file), format, &sidecar),
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bins_close_every_512_frames_scaled_and_saturated() {
        let mut accum = PeakAccum::new(1);
        let mut pairs = Vec::new();
        for i in 0..1024 {
            let v = if i == 100 {
                -2.0 // out of range: saturates
            } else if i == 700 {
                0.5
            } else {
                0.0
            };
            if let Some(pair) = accum.push(v) {
                pairs.push(pair);
            }
        }
        assert_eq!(pairs.len(), 2);
        assert_eq!(pairs[0], (i16::MIN + 1, 0), "-2.0 clamps to -1.0");
        assert_eq!(pairs[1], (0, (0.5f32 * 32767.0) as i16));
    }

    #[test]
    fn a_stereo_track_folds_both_channels_into_one_bin() {
        let mut accum = PeakAccum::new(2);
        let mut pairs = Vec::new();
        // 512 frames = 1024 samples: L=0.25, R=-0.75.
        for i in 0..1024 {
            let v = if i % 2 == 0 { 0.25 } else { -0.75 };
            if let Some(pair) = accum.push(v) {
                pairs.push(pair);
            }
        }
        assert_eq!(pairs.len(), 1);
        let (min, max) = pairs[0];
        assert!((f32::from(min) / 32767.0 + 0.75).abs() < 1e-3);
        assert!((f32::from(max) / 32767.0 - 0.25).abs() < 1e-3);
    }

    #[test]
    fn flush_emits_the_partial_tail_once() {
        let mut accum = PeakAccum::new(1);
        for _ in 0..100 {
            assert!(accum.push(0.9).is_none());
        }
        assert_eq!(
            accum.flush(),
            Some(((0.9f32 * 32767.0) as i16, (0.9f32 * 32767.0) as i16))
        );
        assert_eq!(accum.flush(), None, "the tail does not repeat");
    }

    #[test]
    fn sidecar_bytes_round_trip_and_tolerate_a_torn_tail() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("ch01.peaks");
        let mut out = Vec::new();
        write_header(&mut out).unwrap();
        write_pair(&mut out, (-100, 200)).unwrap();
        write_pair(&mut out, (-300, 400)).unwrap();
        out.push(0xFF); // torn write: half a pair
        std::fs::write(&path, &out).unwrap();

        let (spb, pairs) = read_sidecar(&path).unwrap();
        assert_eq!(spb, PEAK_SAMPLES_PER_BIN);
        assert_eq!(pairs, vec![(-100, 200), (-300, 400)]);
    }

    #[test]
    fn backfill_from_wav_matches_the_incremental_accumulator() {
        let dir = tempfile::tempdir().unwrap();
        let wav = dir.path().join("t.wav");
        let spec = hound::WavSpec {
            channels: 1,
            sample_rate: 48_000,
            bits_per_sample: 32,
            sample_format: hound::SampleFormat::Float,
        };
        let mut writer = hound::WavWriter::create(&wav, spec).unwrap();
        let mut accum = PeakAccum::new(1);
        let mut expected = Vec::new();
        for i in 0..1500 {
            let v = ((i as f32) / 1500.0) * (if i % 3 == 0 { -1.0 } else { 1.0 });
            writer.write_sample(v).unwrap();
            if let Some(pair) = accum.push(v) {
                expected.push(pair);
            }
        }
        if let Some(pair) = accum.flush() {
            expected.push(pair);
        }
        writer.finalize().unwrap();

        let sidecar = dir.path().join("t.peaks");
        let pairs =
            compute_from_audio(&wav, crate::format::RecordFormat::Wav32Float, &sidecar).unwrap();
        assert_eq!(pairs, expected, "1500 frames = 2 full bins + tail");
        assert_eq!(pairs.len(), 3);
        let (_, cached) = read_sidecar(&sidecar).unwrap();
        assert_eq!(cached, pairs, "the sidecar cache holds the same pairs");
    }

    #[test]
    fn backfill_decodes_flac_and_matches_the_float_pairs() {
        let dir = tempfile::tempdir().unwrap();
        let signal: Vec<f32> = (0..1500).map(|i| ((i as f32) * 0.02).sin() * 0.9).collect();

        let mut enc =
            crate::flac::FlacTrackEncoder::create(&dir.path().join("t.flac"), 1, 48_000, 16)
                .unwrap();
        let mut accum = PeakAccum::new(1);
        let mut expected = Vec::new();
        for &v in &signal {
            enc.write_sample(v).unwrap();
            // The decoded value is the quantized one; peaks match within
            // one LSB of the i16 scale.
            if let Some(pair) = accum.push(crate::format::quantize(v, 16) as f32 / 32_768.0) {
                expected.push(pair);
            }
        }
        if let Some(pair) = accum.flush() {
            expected.push(pair);
        }
        enc.finalize().unwrap();

        let sidecar = dir.path().join("t.peaks");
        let pairs = compute_from_audio(
            &dir.path().join("t.flac"),
            crate::format::RecordFormat::Flac16,
            &sidecar,
        )
        .unwrap();
        assert_eq!(pairs.len(), expected.len());
        for (got, want) in pairs.iter().zip(&expected) {
            assert!((i32::from(got.0) - i32::from(want.0)).abs() <= 1);
            assert!((i32::from(got.1) - i32::from(want.1)).abs() <= 1);
        }
    }

    #[test]
    fn read_or_compute_prefers_sidecars_and_backfills_the_rest() {
        let dir = tempfile::tempdir().unwrap();
        let spec = hound::WavSpec {
            channels: 1,
            sample_rate: 48_000,
            bits_per_sample: 32,
            sample_format: hound::SampleFormat::Float,
        };
        for file in ["a.wav", "b.wav"] {
            let mut writer = hound::WavWriter::create(dir.path().join(file), spec).unwrap();
            for _ in 0..600 {
                writer.write_sample(0.5f32).unwrap();
            }
            writer.finalize().unwrap();
        }
        // a has a (deliberately different) sidecar; b must backfill.
        let mut out = Vec::new();
        write_header(&mut out).unwrap();
        write_pair(&mut out, (-42, 42)).unwrap();
        std::fs::write(dir.path().join("a.peaks"), &out).unwrap();

        let tracks = ["a.wav".to_string(), "b.wav".to_string()];
        let pairs =
            read_or_compute(dir.path(), crate::format::RecordFormat::Wav32Float, &tracks).unwrap();
        assert_eq!(pairs[0], vec![(-42, 42)], "the sidecar wins");
        assert_eq!(pairs[1].len(), 2, "600 frames = one bin + tail");
        assert!(
            dir.path().join("b.peaks").exists(),
            "the backfill is cached"
        );
    }
}
