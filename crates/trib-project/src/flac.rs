//! FLAC for take tracks: a streaming per-block encoder on `flacenc`, and a
//! seekable f32 sample reader on `symphonia`. Both stay behind the writer
//! and reader — nothing else in the crate knows the codec.

use std::fs::File;
use std::io::{self, BufWriter, Seek, SeekFrom, Write};
use std::path::Path;

use flacenc::component::BitRepr;
use flacenc::error::Verify;
use flacenc::source::Fill;

use crate::format::quantize;

/// Inter-channel frames per FLAC frame — the standard block size; the
/// encoder buffers at most one block between ring drains.
const BLOCK_FRAMES: usize = 4096;

/// STREAMINFO payload location: `fLaC` magic (4) + block header (4).
const STREAMINFO_OFFSET: u64 = 8;
const STREAMINFO_LEN: usize = 34;

fn encode_err(e: impl std::fmt::Debug) -> io::Error {
    io::Error::other(format!("flac encode: {e:?}"))
}

/// Serialize one `BitRepr` component to its byte form.
fn to_bytes(part: &impl BitRepr) -> io::Result<Vec<u8>> {
    let mut sink = flacenc::bitsink::ByteSink::new();
    part.write(&mut sink).map_err(encode_err)?;
    Ok(sink.as_slice().to_vec())
}

/// Streaming FLAC writer for one track: quantizes f32, encodes a frame per
/// full block, and patches STREAMINFO (totals, sizes, MD5) on finalize.
pub(crate) struct FlacTrackEncoder {
    out: BufWriter<File>,
    config: flacenc::error::Verified<flacenc::config::Encoder>,
    stream_info: flacenc::component::StreamInfo,
    framebuf: flacenc::source::FrameBuf,
    context: flacenc::source::Context,
    /// Interleaved quantized samples, less than one block.
    pending: Vec<i32>,
    channels: usize,
    bits: u16,
    frame_number: usize,
}

impl FlacTrackEncoder {
    pub fn create(path: &Path, channels: u16, sample_rate: u32, bits: u16) -> io::Result<Self> {
        let channels = usize::from(channels);
        let stream_info =
            flacenc::component::StreamInfo::new(sample_rate as usize, channels, usize::from(bits))
                .map_err(encode_err)?;
        let config = flacenc::config::Encoder::default()
            .into_verified()
            .map_err(encode_err)?;
        let framebuf =
            flacenc::source::FrameBuf::with_size(channels, BLOCK_FRAMES).map_err(encode_err)?;

        let mut out = BufWriter::new(File::create(path)?);
        out.write_all(b"fLaC")?;
        // Metadata block header: is-last · type STREAMINFO · 24-bit length.
        out.write_all(&[0x80, 0, 0, STREAMINFO_LEN as u8])?;
        out.write_all(&to_bytes(&stream_info)?)?;

        Ok(FlacTrackEncoder {
            out,
            config,
            stream_info,
            framebuf,
            context: flacenc::source::Context::new(usize::from(bits), channels),
            pending: Vec::with_capacity(BLOCK_FRAMES * channels),
            channels,
            bits,
            frame_number: 0,
        })
    }

    pub fn write_sample(&mut self, sample: f32) -> io::Result<()> {
        self.pending.push(quantize(sample, self.bits));
        if self.pending.len() == BLOCK_FRAMES * self.channels {
            self.encode_pending()?;
        }
        Ok(())
    }

    fn encode_pending(&mut self) -> io::Result<()> {
        if self.pending.is_empty() {
            return Ok(());
        }
        self.framebuf
            .fill_interleaved(&self.pending)
            .map_err(encode_err)?;
        self.context
            .fill_interleaved(&self.pending)
            .map_err(encode_err)?;
        let frame = flacenc::encode_fixed_size_frame(
            &self.config,
            &self.framebuf,
            self.frame_number,
            &self.stream_info,
        )
        .map_err(encode_err)?;
        self.stream_info.update_frame_info(&frame);
        self.out.write_all(&to_bytes(&frame)?)?;
        self.frame_number += 1;
        self.pending.clear();
        Ok(())
    }

    pub fn finalize(mut self) -> io::Result<()> {
        self.encode_pending()?;
        // Declare a fixed-blocksize stream (reference-encoder behavior):
        // the partial tail must not fold into min_block_size, or readers
        // treat the stream as variable and reject frame-number addressing.
        self.stream_info
            .set_block_sizes(BLOCK_FRAMES, BLOCK_FRAMES)
            .map_err(encode_err)?;
        self.stream_info.set_md5_digest(&self.context.md5_digest());
        let info = to_bytes(&self.stream_info)?;
        let mut file = self.out.into_inner().map_err(io::Error::other)?;
        file.seek(SeekFrom::Start(STREAMINFO_OFFSET))?;
        file.write_all(&info)?;
        file.sync_all()
    }
}

/// What the file itself claims to be — validated by the caller against
/// the take manifest.
#[derive(Debug, Clone, Copy)]
pub(crate) struct FlacInfo {
    pub frames: u64,
    pub channels: u16,
    pub sample_rate: u32,
    pub bits: u16,
}

/// Seekable f32 sample stream over one FLAC track, interleaved like the
/// hound iterators the feeder uses. Decode errors end the stream (the
/// feeder degrades a failed track to silence).
pub(crate) struct FlacSamples {
    reader: symphonia::default::formats::FlacReader<'static>,
    decoder: symphonia::default::codecs::FlacDecoder,
    track_id: u32,
    /// Interleaved samples of the current packet.
    buf: Vec<f32>,
    pos: usize,
    /// Samples to discard after a coarse seek landed early.
    skip: u64,
}

impl FlacSamples {
    /// Open one FLAC file and seek: the `open_samples` twin for FLAC.
    /// Returns the stream plus what the file claims to be.
    pub fn open(path: &Path, start_frame: u64) -> io::Result<(Self, FlacInfo)> {
        use symphonia::core::formats::{FormatOptions, FormatReader, TrackType};
        use symphonia::core::io::MediaSourceStream;

        let file = File::open(path)?;
        let mss = MediaSourceStream::new(Box::new(file), Default::default());
        let reader =
            symphonia::default::formats::FlacReader::try_new(mss, FormatOptions::default())
                .map_err(io::Error::other)?;
        let track = reader
            .default_track(TrackType::Audio)
            .ok_or_else(|| io::Error::other("flac file has no audio track"))?;
        let track_id = track.id;
        let frames = track
            .num_frames
            .ok_or_else(|| io::Error::other("flac file has no frame count"))?;
        let params = match &track.codec_params {
            Some(symphonia::core::codecs::CodecParameters::Audio(params)) => params.clone(),
            _ => return Err(io::Error::other("flac track has no audio params")),
        };
        let info = FlacInfo {
            frames,
            channels: params
                .channels
                .as_ref()
                .map(|c| c.count() as u16)
                .unwrap_or(1),
            sample_rate: params
                .sample_rate
                .ok_or_else(|| io::Error::other("flac track has no sample rate"))?,
            bits: params
                .bits_per_sample
                .ok_or_else(|| io::Error::other("flac track has no bit depth"))?
                as u16,
        };
        let decoder =
            symphonia::default::codecs::FlacDecoder::try_new(&params, &Default::default())
                .map_err(io::Error::other)?;

        let mut samples = FlacSamples {
            reader,
            decoder,
            track_id,
            buf: Vec::new(),
            pos: 0,
            skip: 0,
        };
        let start = start_frame.min(frames);
        if start > 0 {
            samples.seek_to(start, u64::from(info.channels))?;
        }
        Ok((samples, info))
    }

    fn seek_to(&mut self, frame: u64, channels: u64) -> io::Result<()> {
        use symphonia::core::codecs::audio::AudioDecoder;
        use symphonia::core::formats::{FormatReader, SeekMode, SeekTo};
        use symphonia::core::units::Timestamp;

        let seeked = self
            .reader
            .seek(
                SeekMode::Accurate,
                SeekTo::Timestamp {
                    ts: Timestamp::new(frame as i64),
                    track_id: self.track_id,
                },
            )
            .map_err(io::Error::other)?;
        self.decoder.reset();
        // Accurate mode lands at or before the target; skip the difference.
        let landed = seeked.actual_ts.get().max(0) as u64;
        self.skip = frame.saturating_sub(landed) * channels;
        Ok(())
    }

    /// Next interleaved sample, or None at end-of-stream / decode failure.
    pub fn next_sample(&mut self) -> Option<f32> {
        use symphonia::core::codecs::audio::AudioDecoder;
        use symphonia::core::formats::FormatReader;

        loop {
            if self.pos < self.buf.len() {
                let sample = self.buf[self.pos];
                self.pos += 1;
                if self.skip > 0 {
                    self.skip -= 1;
                    continue;
                }
                return Some(sample);
            }
            let packet = match self.reader.next_packet() {
                Ok(Some(packet)) => packet,
                _ => return None,
            };
            if packet.track_id != self.track_id {
                continue;
            }
            let decoded = match self.decoder.decode(&packet) {
                Ok(decoded) => decoded,
                Err(_) => return None,
            };
            self.buf.resize(decoded.samples_interleaved(), 0.0);
            decoded.copy_to_slice_interleaved(&mut self.buf);
            self.pos = 0;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A deterministic full-scale-ish test signal.
    fn signal(frames: usize, channels: usize) -> Vec<f32> {
        (0..frames * channels)
            .map(|i| ((i as f32) * 0.001).sin() * 0.8)
            .collect()
    }

    fn encode(path: &Path, samples: &[f32], channels: u16, bits: u16) {
        let mut enc = FlacTrackEncoder::create(path, channels, 48_000, bits).unwrap();
        for &s in samples {
            enc.write_sample(s).unwrap();
        }
        enc.finalize().unwrap();
    }

    fn decode_all(path: &Path) -> Vec<f32> {
        let (mut samples, _) = FlacSamples::open(path, 0).unwrap();
        let mut out = Vec::new();
        while let Some(s) = samples.next_sample() {
            out.push(s);
        }
        out
    }

    fn assert_round_trip(samples: &[f32], decoded: &[f32], bits: u16) {
        assert_eq!(decoded.len(), samples.len());
        let scale = (1_i64 << (bits - 1)) as f32;
        for (i, (&input, &output)) in samples.iter().zip(decoded).enumerate() {
            let expected = quantize(input, bits) as f32 / scale;
            assert!(
                (output - expected).abs() < 1e-4,
                "sample {i}: {output} != {expected}"
            );
        }
    }

    #[test]
    fn a_multi_block_stereo_take_round_trips_at_16_bits() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("t.flac");
        // 2.5 blocks: multi-frame plus a partial tail.
        let samples = signal(BLOCK_FRAMES * 5 / 2, 2);
        encode(&path, &samples, 2, 16);
        assert_round_trip(&samples, &decode_all(&path), 16);
    }

    #[test]
    fn a_mono_take_round_trips_at_24_bits() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("t.flac");
        let samples = signal(BLOCK_FRAMES + 100, 1);
        encode(&path, &samples, 1, 24);
        assert_round_trip(&samples, &decode_all(&path), 24);
    }

    #[test]
    fn a_short_take_is_a_single_partial_frame() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("t.flac");
        let samples = signal(100, 1);
        encode(&path, &samples, 1, 16);
        assert_round_trip(&samples, &decode_all(&path), 16);
    }

    #[test]
    fn finalize_patches_the_frame_count_into_streaminfo() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("t.flac");
        let frames = BLOCK_FRAMES * 2 + 7;
        encode(&path, &signal(frames, 2), 2, 16);
        let (_, info) = FlacSamples::open(&path, 0).unwrap();
        assert_eq!(info.frames, frames as u64);
        assert_eq!(info.channels, 2);
        assert_eq!(info.sample_rate, 48_000);
        assert_eq!(info.bits, 16);
    }

    #[test]
    fn opening_at_a_start_frame_seeks_to_it() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("t.flac");
        // A ramp readable by position: frame i quantizes to i (16-bit).
        let frames = BLOCK_FRAMES * 2;
        let samples: Vec<f32> = (0..frames).map(|i| i as f32 / 32_767.0).collect();
        encode(&path, &samples, 1, 16);

        for start in [0_u64, 1, 4095, 4096, 5000] {
            let (mut stream, _) = FlacSamples::open(&path, start).unwrap();
            let first = stream.next_sample().unwrap();
            let frame = (first * 32_768.0).round() as u64;
            assert_eq!(frame, start, "seek to {start} landed at {frame}");
        }
    }

    #[test]
    fn the_header_is_a_flac_magic() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("t.flac");
        encode(&path, &signal(10, 1), 1, 16);
        let bytes = std::fs::read(&path).unwrap();
        assert_eq!(&bytes[..4], b"fLaC");
        assert_eq!(bytes[4], 0x80, "one metadata block: STREAMINFO, last");
    }
}
