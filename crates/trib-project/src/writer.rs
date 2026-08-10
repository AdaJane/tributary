//! The take writer: one thread per track draining its record ring into an
//! encoder, plus a coordinator that funnels live peak bins to the tap and
//! writes `take.toml` once every track has finalized. Dropped samples
//! become silence at the writer, so every track of a take stays
//! sample-aligned; the counts land in `take.toml` and mark the take
//! damaged. Tracks never block each other on a slow encode.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc;
use std::time::Duration;

use rtrb::Consumer;
use serde::Serialize;

use crate::flac::FlacTrackEncoder;
use crate::format::{RecordFormat, quantize};
use crate::peaks::PeakAccum;

/// Drain cadence. The record rings hold seconds; 50 ms keeps them nearly
/// empty without spinning.
const DRAIN_INTERVAL: Duration = Duration::from_millis(50);

pub struct TakeTrackSpec {
    /// File name within the take directory ("ch01-vocal.wav").
    pub file: String,
    pub channels: u16,
    /// The strip this track taps — None for the master mix. Manifested so
    /// the take↔console association outlives filename conventions.
    pub strip_id: Option<u32>,
}

pub struct TrackSink {
    pub spec: TakeTrackSpec,
    pub rx: Consumer<f32>,
    pub dropped: Arc<AtomicU64>,
}

/// Freshly closed peak bins for one track, handed out once per drain
/// cycle — the live-waveform feed.
pub struct PeakBatch {
    /// Index into the take's tracks (sink order).
    pub track: usize,
    pub start_bin: u64,
    /// Interleaved (min, max) pairs.
    pub bins: Vec<i16>,
}

pub type PeaksTap = Box<dyn FnMut(PeakBatch) + Send>;

#[derive(Serialize)]
struct TakeManifest {
    schema_version: u32,
    started_at_unix: u64,
    sample_rate: u32,
    format: RecordFormat,
    damaged: bool,
    tracks: Vec<TrackManifest>,
}

#[derive(Serialize)]
struct TrackManifest {
    file: String,
    channels: u16,
    frames: u64,
    dropped_samples: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    strip_id: Option<u32>,
}

/// One track's on-disk encoder, uniform over containers: takes f32, writes
/// whatever the format calls for.
enum TrackEncoder {
    Wav {
        writer: hound::WavWriter<std::io::BufWriter<std::fs::File>>,
        format: RecordFormat,
    },
    Flac(Box<FlacTrackEncoder>),
}

impl TrackEncoder {
    fn create(
        path: &Path,
        format: RecordFormat,
        channels: u16,
        sample_rate: u32,
    ) -> std::io::Result<Self> {
        match format {
            RecordFormat::Wav16 | RecordFormat::Wav24 | RecordFormat::Wav32Float => {
                let spec = hound::WavSpec {
                    channels,
                    sample_rate,
                    bits_per_sample: format.bits_per_sample(),
                    sample_format: if format.is_float() {
                        hound::SampleFormat::Float
                    } else {
                        hound::SampleFormat::Int
                    },
                };
                let writer = hound::WavWriter::create(path, spec).map_err(std::io::Error::other)?;
                Ok(TrackEncoder::Wav { writer, format })
            }
            RecordFormat::Flac16 | RecordFormat::Flac24 => Ok(TrackEncoder::Flac(Box::new(
                FlacTrackEncoder::create(path, channels, sample_rate, format.bits_per_sample())?,
            ))),
        }
    }

    /// Returns false when the sample failed to land on disk.
    fn write_sample(&mut self, sample: f32) -> bool {
        match self {
            TrackEncoder::Wav { writer, format } => match format {
                RecordFormat::Wav32Float => writer.write_sample(sample).is_ok(),
                RecordFormat::Wav16 => writer.write_sample(quantize(sample, 16) as i16).is_ok(),
                _ => writer.write_sample(quantize(sample, 24)).is_ok(),
            },
            TrackEncoder::Flac(encoder) => encoder.write_sample(sample).is_ok(),
        }
    }

    fn finalize(self) -> std::io::Result<()> {
        match self {
            TrackEncoder::Wav { writer, .. } => writer.finalize().map_err(std::io::Error::other),
            TrackEncoder::Flac(encoder) => encoder.finalize(),
        }
    }
}

struct ActiveTrack {
    spec: TakeTrackSpec,
    rx: Consumer<f32>,
    dropped: Arc<AtomicU64>,
    encoder: TrackEncoder,
    peaks: PeakAccum,
    peaks_out: std::io::BufWriter<std::fs::File>,
    /// Closed this drain cycle, awaiting the funnel.
    pending_bins: Vec<i16>,
    bins_emitted: u64,
    written_samples: u64,
    silenced_samples: u64,
    write_failed: bool,
}

impl ActiveTrack {
    /// Every sample — real or repair silence — lands in the file and the
    /// peaks sidecar together, so the waveform never drifts from the audio.
    /// Peaks see the raw f32, before any quantization.
    fn write(&mut self, sample: f32) {
        if !self.encoder.write_sample(sample) {
            self.write_failed = true;
        }
        self.written_samples += 1;
        if let Some(pair) = self.peaks.push(sample) {
            let _ = crate::peaks::write_pair(&mut self.peaks_out, pair);
            self.pending_bins.push(pair.0);
            self.pending_bins.push(pair.1);
        }
    }

    fn send_bins(&mut self, index: usize, batches: &Option<mpsc::Sender<PeakBatch>>) {
        let Some(tx) = batches else {
            self.pending_bins.clear();
            return;
        };
        if self.pending_bins.is_empty() {
            return;
        }
        let bins = std::mem::take(&mut self.pending_bins);
        let count = bins.len() as u64 / 2;
        let _ = tx.send(PeakBatch {
            track: index,
            start_bin: self.bins_emitted,
            bins,
        });
        self.bins_emitted += count;
    }

    /// One track's whole life: drain until the producer drops, then
    /// finalize. Encoder errors mark the track failed but never stop the
    /// drain — alignment and take completion beat fidelity.
    fn run(
        mut self,
        index: usize,
        batches: Option<mpsc::Sender<PeakBatch>>,
    ) -> (TrackManifest, bool) {
        loop {
            while let Ok(sample) = self.rx.pop() {
                self.write(sample);
            }
            // Overrun repair: silence keeps the timeline aligned.
            let dropped = self.dropped.load(Ordering::Relaxed);
            while self.silenced_samples < dropped {
                self.write(0.0);
                self.silenced_samples += 1;
            }
            self.send_bins(index, &batches);
            if self.rx.is_abandoned() && self.rx.is_empty() {
                break;
            }
            std::thread::sleep(DRAIN_INTERVAL);
        }

        if let Some(pair) = self.peaks.flush() {
            let _ = crate::peaks::write_pair(&mut self.peaks_out, pair);
            self.pending_bins.push(pair.0);
            self.pending_bins.push(pair.1);
        }
        self.send_bins(index, &batches);
        if let Err(e) = std::io::Write::flush(&mut self.peaks_out) {
            tracing::error!(%e, file = self.spec.file, "peaks flush failed");
        }
        if let Err(e) = self.encoder.finalize() {
            tracing::error!(%e, file = self.spec.file, "track finalize failed");
            self.write_failed = true;
        }
        let frames = self.written_samples / u64::from(self.spec.channels);
        (
            TrackManifest {
                file: self.spec.file,
                channels: self.spec.channels,
                frames,
                dropped_samples: self.silenced_samples,
                strip_id: self.spec.strip_id,
            },
            self.write_failed,
        )
    }
}

/// Start the writer for one take: a coordinator thread that spawns one
/// encoder thread per track, funnels their live peak bins to the tap, and
/// — once every producer has dropped and every track finalized — writes
/// `take.toml`, the take-level synchronization point.
pub fn spawn_writer(
    take_dir: PathBuf,
    sample_rate: u32,
    started_at_unix: u64,
    format: RecordFormat,
    sinks: Vec<TrackSink>,
    peaks_tap: Option<PeaksTap>,
) -> std::io::Result<std::thread::JoinHandle<()>> {
    std::fs::create_dir_all(&take_dir)?;
    let mut tracks = Vec::new();
    for sink in sinks {
        let encoder = TrackEncoder::create(
            &take_dir.join(&sink.spec.file),
            format,
            sink.spec.channels,
            sample_rate,
        )?;
        let sidecar = take_dir.join(&sink.spec.file).with_extension("peaks");
        let mut peaks_out = std::io::BufWriter::new(std::fs::File::create(sidecar)?);
        crate::peaks::write_header(&mut peaks_out)?;
        tracks.push(ActiveTrack {
            peaks: PeakAccum::new(sink.spec.channels),
            peaks_out,
            spec: sink.spec,
            rx: sink.rx,
            dropped: sink.dropped,
            encoder,
            pending_bins: Vec::new(),
            bins_emitted: 0,
            written_samples: 0,
            silenced_samples: 0,
            write_failed: false,
        });
    }

    std::thread::Builder::new()
        .name("trib-take-writer".into())
        .spawn(move || {
            // Panic insurance: enough of each spec to still manifest a
            // track whose thread died.
            let specs: Vec<(String, u16, Option<u32>)> = tracks
                .iter()
                .map(|t| (t.spec.file.clone(), t.spec.channels, t.spec.strip_id))
                .collect();
            let (batch_tx, batch_rx) = mpsc::channel::<PeakBatch>();
            let handles: Vec<_> = tracks
                .into_iter()
                .enumerate()
                .map(|(index, track)| {
                    let batches = peaks_tap.is_some().then(|| batch_tx.clone());
                    std::thread::Builder::new()
                        .name(format!("trib-track-writer-{index}"))
                        .spawn(move || track.run(index, batches))
                        .expect("track writer thread spawns")
                })
                .collect();
            drop(batch_tx);
            // The funnel: per-track order is the sender's FIFO; it closes
            // when the last track finishes, so joins below cannot block on
            // a still-draining sibling.
            if let Some(mut tap) = peaks_tap {
                for batch in batch_rx {
                    tap(batch);
                }
            }

            let mut damaged = false;
            let manifest_tracks: Vec<TrackManifest> = handles
                .into_iter()
                .zip(specs)
                .map(|(handle, (file, channels, strip_id))| match handle.join() {
                    Ok((manifest, write_failed)) => {
                        damaged |= manifest.dropped_samples > 0 || write_failed;
                        manifest
                    }
                    Err(_) => {
                        tracing::error!(file, "track writer thread panicked");
                        damaged = true;
                        TrackManifest {
                            file,
                            channels,
                            frames: 0,
                            dropped_samples: 0,
                            strip_id,
                        }
                    }
                })
                .collect();
            let manifest = TakeManifest {
                schema_version: 1,
                started_at_unix,
                sample_rate,
                format,
                damaged,
                tracks: manifest_tracks,
            };
            match toml::to_string_pretty(&manifest) {
                Ok(text) => {
                    if let Err(e) = std::fs::write(take_dir.join("take.toml"), text) {
                        tracing::error!(%e, "take manifest write failed");
                    }
                }
                Err(e) => tracing::error!(%e, "take manifest serialize failed"),
            }
            if damaged {
                tracing::warn!(dir = %take_dir.display(), "take damaged (dropped samples or write errors)");
            }
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sink(file: &str, channels: u16, strip_id: Option<u32>, rx: Consumer<f32>) -> TrackSink {
        TrackSink {
            spec: TakeTrackSpec {
                file: file.into(),
                channels,
                strip_id,
            },
            rx,
            dropped: Arc::new(AtomicU64::new(0)),
        }
    }

    #[test]
    fn writes_pushed_samples_and_finalizes_on_abandon() {
        let dir = tempfile::tempdir().unwrap();
        let (mut tx, rx) = rtrb::RingBuffer::new(1024);
        for i in 0..480 {
            tx.push(i as f32 / 480.0).unwrap();
        }
        let handle = spawn_writer(
            dir.path().to_path_buf(),
            48_000,
            1_000,
            RecordFormat::Wav32Float,
            vec![sink("ch01-test.wav", 1, Some(4), rx)],
            None,
        )
        .unwrap();
        drop(tx); // the take ends when producers drop
        handle.join().unwrap();

        let mut reader = hound::WavReader::open(dir.path().join("ch01-test.wav")).unwrap();
        assert_eq!(reader.spec().sample_rate, 48_000);
        assert_eq!(reader.len(), 480);
        let first: f32 = reader.samples::<f32>().next().unwrap().unwrap();
        assert_eq!(first, 0.0);
        let manifest = std::fs::read_to_string(dir.path().join("take.toml")).unwrap();
        assert!(manifest.contains("damaged = false"));
        assert!(manifest.contains("frames = 480"));
        assert!(manifest.contains("strip_id = 4"));
        assert!(manifest.contains("format = \"wav32_float\""));

        // The peaks sidecar rode along: 480 frames < one bin → one tail pair.
        let (spb, pairs) = crate::peaks::read_sidecar(&dir.path().join("ch01-test.peaks")).unwrap();
        assert_eq!(spb, crate::peaks::PEAK_SAMPLES_PER_BIN);
        assert_eq!(pairs.len(), 1);
        assert_eq!(pairs[0].0, 0, "ramp starts at zero");
        assert!(pairs[0].1 > 32_000, "ramp peaks near full scale");
    }

    #[test]
    fn the_peaks_tap_streams_every_bin_in_order() {
        let dir = tempfile::tempdir().unwrap();
        let (mut tx, rx) = rtrb::RingBuffer::new(4096);
        // 2.5 bins: two live pairs + a flushed tail.
        for _ in 0..1280 {
            tx.push(0.5).unwrap();
        }
        let seen: Arc<std::sync::Mutex<Vec<(usize, u64, usize)>>> = Arc::default();
        let tap_seen = seen.clone();
        let handle = spawn_writer(
            dir.path().to_path_buf(),
            48_000,
            1_000,
            RecordFormat::Wav32Float,
            vec![sink("ch01-test.wav", 1, Some(0), rx)],
            Some(Box::new(move |batch: PeakBatch| {
                tap_seen
                    .lock()
                    .unwrap()
                    .push((batch.track, batch.start_bin, batch.bins.len() / 2));
            })),
        )
        .unwrap();
        drop(tx);
        handle.join().unwrap();

        let batches = seen.lock().unwrap();
        let total: usize = batches.iter().map(|(_, _, n)| n).sum();
        assert_eq!(total, 3, "two full bins + the tail");
        assert_eq!(batches[0].0, 0, "track index rides along");
        assert_eq!(batches[0].1, 0, "bins start at zero");
        // start_bin is always the running total — batches never overlap.
        let mut expected_start = 0;
        for &(_, start, count) in batches.iter() {
            assert_eq!(start, expected_start);
            expected_start += count as u64;
        }
    }

    #[test]
    fn dropped_samples_become_silence_and_mark_the_take_damaged() {
        let dir = tempfile::tempdir().unwrap();
        let (mut tx, rx) = rtrb::RingBuffer::new(1024);
        let dropped = Arc::new(AtomicU64::new(0));
        for _ in 0..100 {
            tx.push(0.5).unwrap();
        }
        dropped.store(20, Ordering::Relaxed); // 20 samples lost to overrun
        let handle = spawn_writer(
            dir.path().to_path_buf(),
            48_000,
            1_000,
            RecordFormat::Wav32Float,
            vec![TrackSink {
                spec: TakeTrackSpec {
                    file: "master.wav".into(),
                    channels: 2,
                    strip_id: None,
                },
                rx,
                dropped,
            }],
            None,
        )
        .unwrap();
        drop(tx);
        handle.join().unwrap();

        let reader = hound::WavReader::open(dir.path().join("master.wav")).unwrap();
        assert_eq!(reader.len(), 120, "100 real + 20 silence");
        let manifest = std::fs::read_to_string(dir.path().join("take.toml")).unwrap();
        assert!(manifest.contains("damaged = true"));
        assert!(manifest.contains("dropped_samples = 20"));
    }

    /// Encode the same signal in every format; the audio must round-trip
    /// (within quantization) and the manifest must name the format.
    #[test]
    fn every_format_records_and_manifests() {
        let signal: Vec<f32> = (0..2000).map(|i| ((i as f32) * 0.01).sin() * 0.5).collect();
        for format in [
            RecordFormat::Wav16,
            RecordFormat::Wav24,
            RecordFormat::Wav32Float,
            RecordFormat::Flac16,
            RecordFormat::Flac24,
        ] {
            let dir = tempfile::tempdir().unwrap();
            let file = format!("ch01-test.{}", format.extension());
            let (mut tx, rx) = rtrb::RingBuffer::new(4096);
            for &s in &signal {
                tx.push(s).unwrap();
            }
            let handle = spawn_writer(
                dir.path().to_path_buf(),
                48_000,
                1_000,
                format,
                vec![sink(&file, 1, Some(0), rx)],
                None,
            )
            .unwrap();
            drop(tx);
            handle.join().unwrap();

            let manifest = std::fs::read_to_string(dir.path().join("take.toml")).unwrap();
            let name = toml::Value::try_from(format).unwrap();
            assert!(
                manifest.contains(&format!("format = {name}")),
                "{format:?} manifest"
            );
            assert!(manifest.contains("frames = 2000"), "{format:?} frames");
            assert!(manifest.contains("damaged = false"), "{format:?} clean");
            assert!(dir.path().join(&file).exists(), "{format:?} file");
        }
    }

    /// The peaks sidecar sees raw f32 before quantization: byte-identical
    /// across formats for the same input.
    #[test]
    fn peaks_sidecars_are_identical_across_formats() {
        let signal: Vec<f32> = (0..1500).map(|i| ((i as f32) * 0.02).sin()).collect();
        let mut sidecars = Vec::new();
        for format in [
            RecordFormat::Wav32Float,
            RecordFormat::Wav16,
            RecordFormat::Flac24,
        ] {
            let dir = tempfile::tempdir().unwrap();
            let file = format!("ch01-t.{}", format.extension());
            let (mut tx, rx) = rtrb::RingBuffer::new(2048);
            for &s in &signal {
                tx.push(s).unwrap();
            }
            let handle = spawn_writer(
                dir.path().to_path_buf(),
                48_000,
                1_000,
                format,
                vec![sink(&file, 1, None, rx)],
                None,
            )
            .unwrap();
            drop(tx);
            handle.join().unwrap();
            sidecars.push(std::fs::read(dir.path().join(&file).with_extension("peaks")).unwrap());
        }
        assert_eq!(sidecars[0], sidecars[1]);
        assert_eq!(sidecars[0], sidecars[2]);
    }

    /// A multi-track take where producers finish at different times: every
    /// file, sidecar, and manifest row must still land complete, and the
    /// manifest is written exactly once, after all tracks.
    #[test]
    fn a_multi_track_take_completes_every_row() {
        let dir = tempfile::tempdir().unwrap();
        let (mut tx_a, rx_a) = rtrb::RingBuffer::new(8192);
        let (mut tx_b, rx_b) = rtrb::RingBuffer::new(8192);
        let (mut tx_m, rx_m) = rtrb::RingBuffer::new(8192);
        for i in 0..600 {
            tx_a.push(i as f32 / 600.0).unwrap();
        }
        let handle = spawn_writer(
            dir.path().to_path_buf(),
            48_000,
            1_000,
            RecordFormat::Flac16,
            vec![
                sink("ch01-a.flac", 1, Some(0), rx_a),
                sink("ch02-b.flac", 1, Some(1), rx_b),
                sink("master.flac", 2, None, rx_m),
            ],
            None,
        )
        .unwrap();
        drop(tx_a); // track A ends first
        // B and master keep flowing while A is already finalizing.
        for i in 0..1200 {
            tx_b.push(-(i as f32) / 1200.0).unwrap();
        }
        for i in 0..2400 {
            tx_m.push((i as f32) / 2400.0).unwrap();
        }
        std::thread::sleep(Duration::from_millis(120));
        drop(tx_b);
        drop(tx_m);
        handle.join().unwrap();

        let text = std::fs::read_to_string(dir.path().join("take.toml")).unwrap();
        let parsed: toml::Value = toml::from_str(&text).unwrap();
        let tracks = parsed["tracks"].as_array().unwrap();
        assert_eq!(tracks.len(), 3);
        assert_eq!(tracks[0]["frames"].as_integer(), Some(600));
        assert_eq!(tracks[1]["frames"].as_integer(), Some(1200));
        assert_eq!(
            tracks[2]["frames"].as_integer(),
            Some(1200),
            "stereo: 2400 samples"
        );
        assert_eq!(parsed["damaged"].as_bool(), Some(false));
        for file in ["ch01-a.flac", "ch02-b.flac", "master.flac"] {
            assert!(dir.path().join(file).exists(), "{file}");
            assert!(
                dir.path().join(file).with_extension("peaks").exists(),
                "{file} sidecar"
            );
        }
    }

    #[test]
    fn an_unwritable_take_dir_is_refused_up_front() {
        let (_tx, rx) = rtrb::RingBuffer::new(16);
        let result = spawn_writer(
            PathBuf::from("/proc/no-such-place/take-001"),
            48_000,
            1_000,
            RecordFormat::Wav32Float,
            vec![sink("ch01-a.wav", 1, None, rx)],
            None,
        );
        assert!(result.is_err());
    }
}
