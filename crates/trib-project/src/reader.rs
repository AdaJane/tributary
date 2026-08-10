//! The take feeder: the writer's mirror. One thread per track streams that
//! track's file into its ring; the engine pops the rings in lockstep, so
//! every ring must carry the identical frame sequence. Short tracks are
//! padded with silence so every lane spans the take, and a loop region
//! wraps by reopening the reader at the loop start. Tracks never block
//! each other on a slow decode.

use std::io;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use rtrb::{Consumer, Producer, RingBuffer};

use crate::flac::FlacSamples;
use crate::format::RecordFormat;

/// Ring seconds per playback track: slack for a busy disk before the
/// output degrades to (aligned) silence.
pub const PLAYBACK_RING_SECS: usize = 4;

/// Top-up cadence, matching the writer's drain cadence.
const FEED_INTERVAL: Duration = Duration::from_millis(50);

#[derive(Debug, Clone)]
pub struct TrackFile {
    /// File name within the take directory ("ch01-vocal.wav").
    pub file: String,
    pub channels: u16,
}

/// What to play: which files, over which span, wrapped where.
#[derive(Debug, Clone)]
pub struct PlaybackSource {
    pub take_dir: PathBuf,
    pub tracks: Vec<TrackFile>,
    pub sample_rate: u32,
    pub format: RecordFormat,
    /// The take's length — the longest track; shorter ones pad to it.
    pub total_frames: u64,
    pub start_frame: u64,
    /// Wrap `[start, end)` forever instead of ending the session.
    pub loop_region: Option<(u64, u64)>,
}

pub struct FeederHandle {
    stop: Arc<AtomicBool>,
}

impl FeederHandle {
    pub fn stop(&self) {
        self.stop.store(true, Ordering::Relaxed);
    }
}

/// A dropped session must not leave threads streaming from disk.
impl Drop for FeederHandle {
    fn drop(&mut self) {
        self.stop();
    }
}

type WavSamples<S> = hound::WavIntoSamples<io::BufReader<std::fs::File>, S>;

/// One track's decoded f32 stream, uniform over containers. The FLAC
/// decoder is boxed: it dwarfs the hound iterators.
enum TrackSamples {
    WavF32(WavSamples<f32>),
    WavI16(WavSamples<i16>),
    WavI24(WavSamples<i32>),
    Flac(Box<FlacSamples>),
}

impl TrackSamples {
    /// Next interleaved sample; None at end-of-stream or read failure.
    fn next_sample(&mut self) -> Option<f32> {
        match self {
            TrackSamples::WavF32(s) => s.next()?.ok(),
            TrackSamples::WavI16(s) => s.next()?.ok().map(|v| f32::from(v) / 32_768.0),
            TrackSamples::WavI24(s) => s.next()?.ok().map(|v| v as f32 / 8_388_608.0),
            TrackSamples::Flac(s) => s.next_sample(),
        }
    }
}

struct FeederTrack {
    path: PathBuf,
    channels: usize,
    format: RecordFormat,
    sample_rate: u32,
    /// Real frames in the file; positions past this feed silence.
    frames: u64,
    samples: TrackSamples,
    /// A damaged tail degrades to silence — alignment over fidelity.
    read_failed: bool,
    tx: Producer<f32>,
}

impl FeederTrack {
    /// Feed exactly `count` frames starting at absolute frame `pos`.
    /// The caller guarantees ring space and lockstep positioning.
    fn feed(&mut self, pos: u64, count: usize) {
        for i in 0..count {
            let real = (pos + i as u64) < self.frames && !self.read_failed;
            for _ in 0..self.channels {
                let sample = if real {
                    match self.samples.next_sample() {
                        Some(s) => s,
                        None => {
                            self.read_failed = true;
                            0.0
                        }
                    }
                } else {
                    0.0
                };
                let _ = self.tx.push(sample);
            }
        }
    }

    /// Reposition by reopening — a wrap happens at most once per loop pass.
    fn reopen_at(&mut self, frame: u64) -> io::Result<()> {
        let (samples, frames) = open_samples(
            &self.path,
            frame,
            self.channels as u16,
            self.sample_rate,
            self.format,
        )?;
        self.samples = samples;
        self.frames = frames;
        self.read_failed = false;
        Ok(())
    }
}

/// Open one track file, validate it against the take manifest, and seek.
/// Returns the owned sample stream plus the file's real frame count. This
/// is the hard wall: a file whose spec disagrees with the manifest (or the
/// engine rate) is refused, never resampled or guessed at.
fn open_samples(
    path: &Path,
    start_frame: u64,
    channels: u16,
    sample_rate: u32,
    format: RecordFormat,
) -> io::Result<(TrackSamples, u64)> {
    match format {
        RecordFormat::Wav16 | RecordFormat::Wav24 | RecordFormat::Wav32Float => {
            let mut reader = hound::WavReader::open(path).map_err(io::Error::other)?;
            let spec = reader.spec();
            let expected = if format.is_float() {
                hound::SampleFormat::Float
            } else {
                hound::SampleFormat::Int
            };
            if spec.channels != channels
                || spec.sample_rate != sample_rate
                || spec.sample_format != expected
                || spec.bits_per_sample != format.bits_per_sample()
            {
                return Err(io::Error::other(format!(
                    "{} does not match its manifest (spec {spec:?}, format {format:?})",
                    path.display()
                )));
            }
            let frames = u64::from(reader.duration());
            let seek_to = start_frame.min(frames);
            let seek_to =
                u32::try_from(seek_to).map_err(|_| io::Error::other("take too long to seek"))?;
            reader.seek(seek_to).map_err(io::Error::other)?;
            let samples = match format {
                RecordFormat::Wav32Float => TrackSamples::WavF32(reader.into_samples()),
                RecordFormat::Wav16 => TrackSamples::WavI16(reader.into_samples()),
                _ => TrackSamples::WavI24(reader.into_samples()),
            };
            Ok((samples, frames))
        }
        RecordFormat::Flac16 | RecordFormat::Flac24 => {
            let (samples, info) = FlacSamples::open(path, start_frame)?;
            if info.channels != channels
                || info.sample_rate != sample_rate
                || info.bits != format.bits_per_sample()
            {
                return Err(io::Error::other(format!(
                    "{} does not match its manifest ({info:?}, format {format:?})",
                    path.display()
                )));
            }
            Ok((TrackSamples::Flac(Box::new(samples)), info.frames))
        }
    }
}

/// One track's feed loop state: its own position and wrap handling. All
/// tracks walk the identical `[start, end)` (+ loop) sequence, so the
/// rings stay in lockstep without sharing a position.
struct TrackFeeder {
    track: FeederTrack,
    /// Next absolute frame to feed.
    pos: u64,
    /// Feed boundary: the loop end while looping, else the take end.
    end: u64,
    loop_region: Option<(u64, u64)>,
}

impl TrackFeeder {
    /// One pass: feed as much as the ring can take, wrapping at the loop.
    /// Returns true when the (non-looping) end was reached.
    fn top_up(&mut self) -> io::Result<bool> {
        loop {
            let space = self.track.tx.slots() / self.track.channels;
            let count = space.min(self.end.saturating_sub(self.pos) as usize);
            self.track.feed(self.pos, count);
            self.pos += count as u64;
            if self.pos >= self.end {
                let Some((loop_start, _)) = self.loop_region else {
                    return Ok(true);
                };
                self.track.reopen_at(loop_start)?;
                self.pos = loop_start;
                if count == 0 && space == 0 {
                    return Ok(false); // ring full at the wrap; resume next tick
                }
                continue;
            }
            return Ok(false);
        }
    }
}

/// Open every track and prime its ring full (so PLAY starts with seconds
/// of slack) — in parallel, one thread per track, since priming a FLAC
/// track decodes seconds of audio. Playback starts only after every track
/// is primed. Then each track gets its own long-lived feeder thread; one
/// track's disk error stops the whole session (the engine finishes the
/// take), matching the single-feeder behavior.
pub fn open_and_prime(source: &PlaybackSource) -> io::Result<(Vec<Consumer<f32>>, FeederHandle)> {
    let ring_frames = PLAYBACK_RING_SECS * source.sample_rate as usize;
    let end = source
        .loop_region
        .map_or(source.total_frames, |(_, end)| end);

    let primed: io::Result<Vec<(TrackFeeder, Consumer<f32>)>> = std::thread::scope(|scope| {
        let handles: Vec<_> = source
            .tracks
            .iter()
            .map(|track| {
                scope.spawn(move || {
                    let (samples, frames) = open_samples(
                        &source.take_dir.join(&track.file),
                        source.start_frame,
                        track.channels,
                        source.sample_rate,
                        source.format,
                    )?;
                    let (tx, rx) = RingBuffer::new(ring_frames * track.channels as usize);
                    let mut feeder = TrackFeeder {
                        track: FeederTrack {
                            path: source.take_dir.join(&track.file),
                            channels: track.channels as usize,
                            format: source.format,
                            sample_rate: source.sample_rate,
                            frames,
                            samples,
                            read_failed: false,
                            tx,
                        },
                        pos: source.start_frame,
                        end,
                        loop_region: source.loop_region,
                    };
                    feeder.top_up()?;
                    Ok((feeder, rx))
                })
            })
            .collect();
        handles
            .into_iter()
            .map(|h| h.join().expect("prime thread does not panic"))
            .collect()
    });

    let (feeders, consumers): (Vec<TrackFeeder>, Vec<Consumer<f32>>) = primed?.into_iter().unzip();

    let stop = Arc::new(AtomicBool::new(false));
    for (index, mut feeder) in feeders.into_iter().enumerate() {
        let stop_flag = stop.clone();
        std::thread::Builder::new()
            .name(format!("trib-take-feeder-{index}"))
            .spawn(move || {
                while !stop_flag.load(Ordering::Relaxed) {
                    match feeder.top_up() {
                        Ok(true) => break, // fed to the end: producer drops, engine finishes
                        Ok(false) => std::thread::sleep(FEED_INTERVAL),
                        Err(e) => {
                            tracing::error!(%e, track = index, "take feeder failed; ending playback");
                            stop_flag.store(true, Ordering::Relaxed);
                            break;
                        }
                    }
                }
            })?;
    }
    Ok((consumers, FeederHandle { stop }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::flac::FlacTrackEncoder;

    /// Write a mono f32 WAV whose sample at frame i is i (scaled), so
    /// positions are directly readable in assertions.
    fn write_ramp_wav(dir: &Path, file: &str, frames: u32, channels: u16) {
        let spec = hound::WavSpec {
            channels,
            sample_rate: 48_000,
            bits_per_sample: 32,
            sample_format: hound::SampleFormat::Float,
        };
        let mut writer = hound::WavWriter::create(dir.join(file), spec).unwrap();
        for i in 0..frames {
            for c in 0..channels {
                writer.write_sample(i as f32 + c as f32 * 0.5).unwrap();
            }
        }
        writer.finalize().unwrap();
    }

    /// A 16-bit ramp: frame i holds the exact quantized value i, which
    /// decodes to i/32768 — positions stay readable through any container.
    fn ramp_frame(i: u64) -> f32 {
        i as f32 / 32_768.0
    }

    fn write_ramp_flac(dir: &Path, file: &str, frames: u64) {
        let mut enc = FlacTrackEncoder::create(&dir.join(file), 1, 48_000, 16).unwrap();
        for i in 0..frames {
            enc.write_sample(i as f32 / 32_767.0).unwrap();
        }
        enc.finalize().unwrap();
    }

    fn write_ramp_wav16(dir: &Path, file: &str, frames: u64) {
        let spec = hound::WavSpec {
            channels: 1,
            sample_rate: 48_000,
            bits_per_sample: 16,
            sample_format: hound::SampleFormat::Int,
        };
        let mut writer = hound::WavWriter::create(dir.join(file), spec).unwrap();
        for i in 0..frames {
            writer.write_sample(i as i16).unwrap();
        }
        writer.finalize().unwrap();
    }

    fn drain(rx: &mut Consumer<f32>) -> Vec<f32> {
        let mut out = Vec::new();
        while let Ok(s) = rx.pop() {
            out.push(s);
        }
        out
    }

    fn wait_abandoned(rx: &Consumer<f32>) {
        for _ in 0..200 {
            if rx.is_abandoned() {
                return;
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        panic!("feeder never finished");
    }

    fn source(dir: &Path, tracks: Vec<TrackFile>, total: u64) -> PlaybackSource {
        PlaybackSource {
            take_dir: dir.to_path_buf(),
            tracks,
            sample_rate: 48_000,
            format: RecordFormat::Wav32Float,
            total_frames: total,
            start_frame: 0,
            loop_region: None,
        }
    }

    fn mono(file: &str) -> TrackFile {
        TrackFile {
            file: file.into(),
            channels: 1,
        }
    }

    #[test]
    fn file_content_round_trips_through_the_rings() {
        let dir = tempfile::tempdir().unwrap();
        write_ramp_wav(dir.path(), "ch01-a.wav", 500, 1);
        let src = source(dir.path(), vec![mono("ch01-a.wav")], 500);
        let (mut consumers, _feeder) = open_and_prime(&src).unwrap();
        wait_abandoned(&consumers[0]);
        let samples = drain(&mut consumers[0]);
        assert_eq!(samples.len(), 500);
        assert_eq!(samples[0], 0.0);
        assert_eq!(samples[499], 499.0);
    }

    #[test]
    fn a_short_track_is_padded_with_silence_to_the_take_length() {
        let dir = tempfile::tempdir().unwrap();
        write_ramp_wav(dir.path(), "ch01-a.wav", 100, 1);
        write_ramp_wav(dir.path(), "master.wav", 300, 2);
        let src = source(
            dir.path(),
            vec![
                mono("ch01-a.wav"),
                TrackFile {
                    file: "master.wav".into(),
                    channels: 2,
                },
            ],
            300,
        );
        let (mut consumers, _feeder) = open_and_prime(&src).unwrap();
        wait_abandoned(&consumers[0]);
        wait_abandoned(&consumers[1]);
        let mono_samples = drain(&mut consumers[0]);
        let master = drain(&mut consumers[1]);
        assert_eq!(mono_samples.len(), 300, "padded to the take length");
        assert_eq!(mono_samples[99], 99.0);
        assert_eq!(mono_samples[100], 0.0, "the pad is silence");
        assert_eq!(master.len(), 600, "stereo: two samples per frame");
        assert_eq!(master[1], 0.5, "R of frame 0");
    }

    #[test]
    fn start_frame_seeks_into_the_file() {
        let dir = tempfile::tempdir().unwrap();
        write_ramp_wav(dir.path(), "ch01-a.wav", 400, 1);
        let mut src = source(dir.path(), vec![mono("ch01-a.wav")], 400);
        src.start_frame = 250;
        let (mut consumers, _feeder) = open_and_prime(&src).unwrap();
        wait_abandoned(&consumers[0]);
        let samples = drain(&mut consumers[0]);
        assert_eq!(samples.len(), 150);
        assert_eq!(samples[0], 250.0, "the first sample is the seek target");
    }

    /// The same seek, through every non-float container.
    #[test]
    fn start_frame_seeks_into_every_format() {
        type RampWriter = fn(&Path, &str, u64);
        let cases: [(RecordFormat, RampWriter); 2] = [
            (RecordFormat::Wav16, write_ramp_wav16),
            (RecordFormat::Flac16, write_ramp_flac),
        ];
        for (format, write) in cases {
            let dir = tempfile::tempdir().unwrap();
            let file = format!("ch01-a.{}", format.extension());
            write(dir.path(), &file, 9000);
            let mut src = source(dir.path(), vec![mono(&file)], 9000);
            src.format = format;
            src.start_frame = 5000;
            let (mut consumers, _feeder) = open_and_prime(&src).unwrap();
            wait_abandoned(&consumers[0]);
            let samples = drain(&mut consumers[0]);
            assert_eq!(samples.len(), 4000, "{format:?}");
            assert_eq!(samples[0], ramp_frame(5000), "{format:?} seek target");
            assert_eq!(samples[3999], ramp_frame(8999), "{format:?} tail");
        }
    }

    #[test]
    fn a_loop_region_wraps_seamlessly() {
        let dir = tempfile::tempdir().unwrap();
        write_ramp_wav(dir.path(), "ch01-a.wav", 400, 1);
        let mut src = source(dir.path(), vec![mono("ch01-a.wav")], 400);
        src.start_frame = 90;
        src.loop_region = Some((50, 120));
        let (mut consumers, feeder) = open_and_prime(&src).unwrap();
        // Priming fills the ring far beyond two passes of the 70-frame loop.
        let samples: Vec<f32> = (0..200).map(|_| consumers[0].pop().unwrap()).collect();
        feeder.stop();
        // From 90 up to the loop end, then wrapping [50, 120) forever.
        let mut expected: Vec<f32> = (90..120).map(|i| i as f32).collect();
        while expected.len() < 200 {
            expected.extend((50..120).map(|i| i as f32));
        }
        assert_eq!(samples, expected[..200]);
    }

    /// The wrap reopens and reseeks a FLAC decoder; positions must stay
    /// exact across passes.
    #[test]
    fn a_loop_region_wraps_a_flac_track() {
        let dir = tempfile::tempdir().unwrap();
        write_ramp_flac(dir.path(), "ch01-a.flac", 9000);
        let mut src = source(dir.path(), vec![mono("ch01-a.flac")], 9000);
        src.format = RecordFormat::Flac16;
        src.start_frame = 8990;
        src.loop_region = Some((8000, 9000));
        let (mut consumers, feeder) = open_and_prime(&src).unwrap();
        let samples: Vec<f32> = (0..2010).map(|_| consumers[0].pop().unwrap()).collect();
        feeder.stop();
        let mut expected: Vec<f32> = (8990..9000).map(ramp_frame).collect();
        while expected.len() < 2010 {
            expected.extend((8000..9000).map(ramp_frame));
        }
        assert_eq!(samples, expected[..2010]);
    }

    #[test]
    fn a_mismatched_spec_is_refused_up_front() {
        let dir = tempfile::tempdir().unwrap();
        write_ramp_wav(dir.path(), "ch01-a.wav", 100, 2);
        let src = source(
            dir.path(),
            vec![mono("ch01-a.wav")], // manifest says mono, file is stereo
            100,
        );
        assert!(open_and_prime(&src).is_err());
    }

    /// Every axis of the wall: bit depth, float/int, container, rate.
    #[test]
    fn a_format_mismatch_is_refused_per_field() {
        let dir = tempfile::tempdir().unwrap();
        write_ramp_wav(dir.path(), "f32.wav", 100, 1);
        write_ramp_wav16(dir.path(), "i16.wav", 100);
        write_ramp_flac(dir.path(), "t.flac", 100);

        // A 32f file refused when the manifest claims 16-bit, and so on.
        let cases = [
            ("f32.wav", RecordFormat::Wav16),
            ("i16.wav", RecordFormat::Wav32Float),
            ("i16.wav", RecordFormat::Wav24),
            ("t.flac", RecordFormat::Wav16),
            ("f32.wav", RecordFormat::Flac16),
            ("t.flac", RecordFormat::Flac24),
        ];
        for (file, format) in cases {
            let mut src = source(dir.path(), vec![mono(file)], 100);
            src.format = format;
            assert!(open_and_prime(&src).is_err(), "{file} as {format:?}");
        }
        // The engine-rate wall stays: right file, wrong rate.
        let mut src = source(dir.path(), vec![mono("i16.wav")], 100);
        src.format = RecordFormat::Wav16;
        src.sample_rate = 44_100;
        assert!(open_and_prime(&src).is_err(), "rate mismatch");
    }

    /// A whole multi-track FLAC session: every ring primes in parallel and
    /// the lanes stay in lockstep.
    #[test]
    fn a_multi_track_flac_session_primes_every_ring() {
        let dir = tempfile::tempdir().unwrap();
        for file in ["ch01-a.flac", "ch02-b.flac", "ch03-c.flac"] {
            write_ramp_flac(dir.path(), file, 6000);
        }
        let mut src = source(
            dir.path(),
            vec![
                mono("ch01-a.flac"),
                mono("ch02-b.flac"),
                mono("ch03-c.flac"),
            ],
            6000,
        );
        src.format = RecordFormat::Flac16;
        let (mut consumers, _feeder) = open_and_prime(&src).unwrap();
        for rx in &consumers {
            assert!(rx.slots() >= 6000, "primed full before return");
        }
        for rx in &mut consumers {
            wait_abandoned(rx);
            let samples = drain(rx);
            assert_eq!(samples.len(), 6000);
            assert_eq!(samples[4321], ramp_frame(4321), "lockstep content");
        }
    }

    #[test]
    fn stop_ends_the_feeder_before_the_take_does() {
        let dir = tempfile::tempdir().unwrap();
        // Longer than the ring so the feeder must stay alive after priming.
        write_ramp_wav(dir.path(), "ch01-a.wav", 300_000, 1);
        let src = source(dir.path(), vec![mono("ch01-a.wav")], 300_000);
        let (consumers, feeder) = open_and_prime(&src).unwrap();
        assert!(!consumers[0].is_abandoned(), "feeder is mid-take");
        drop(feeder); // dropping the handle stops the thread
        wait_abandoned(&consumers[0]);
    }
}
