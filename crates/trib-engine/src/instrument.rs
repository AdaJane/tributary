//! Virtual instruments: SoundFont synthesis on the render thread.
//!
//! An instrument is an input source that the box makes itself. The rack
//! renders each one into its own stereo pair of a small interleaved frame,
//! and `compile()` resolves an instrument patch to a pair of indices into
//! it — so from the strip pass downward an instrument is indistinguishable
//! from a microphone, and metering, PFL, the record tap, takes and lanes
//! all work with no knowledge of it.
//!
//! Two decisions worth keeping:
//!
//! The rack is clocked by the engine, never by a timer thread of its own.
//! A synth has no external clock to follow, so a free-running producer
//! feeding a ring would drift against the engine exactly the way the parec
//! capture path does — and would add a ring's latency to buy that drift.
//!
//! The rack renders into its OWN buffer rather than into the device input
//! frame. Writing into the 64-wide frame would need a bounds guard against
//! `input_channels`, and the fake backend runs 1 channel wide — instruments
//! would silently never render under `--no-default-features`, which is most
//! of the test suite.

use rtrb::Consumer;
use rustysynth::{SoundFont, Synthesizer, SynthesizerSettings};
use std::sync::Arc;

/// Channels the whole rack may occupy — the sibling of the 64-wide
/// hardware input frame, and sized to match it.
///
/// A split drum kit spends one channel per piece, so a six-piece kit plus a
/// couple of stereo keyboards is an ordinary session. The buffer this
/// implies is 64 floats a frame, which is nothing; the real ceiling is CPU,
/// and that is measured on the box, not asserted here.
pub const MAX_INSTRUMENT_CHANNELS: usize = 64;

/// A stereo (unsplit) instrument's channel count.
pub const INSTRUMENT_CHANNELS: usize = 2;

/// How often MIDI is applied inside a graph block: every 64 frames, so
/// 1.33 ms at 48 kHz.
///
/// This is `rustysynth`'s own internal render granularity, so draining
/// events between sub-chunks costs nothing. Splitting the *graph* block at
/// event offsets would buy sample accuracy at the price of one `MeterBlock`
/// per event into a 64-deep ring, and a frame counter that no longer counts
/// blocks.
pub const SYNTH_SUB_BLOCK: usize = 64;

/// Depth of the MIDI event ring. Far above any human playing rate; a full
/// ring means the audio thread stalled, and a dropped note-off is a note
/// that hangs forever — so the rack answers a drop with all-notes-off
/// rather than pretending it did not happen.
pub const MIDI_RING_CAPACITY: usize = 1024;

/// One MIDI message crossing onto the audio thread.
///
/// Fixed-size and `Copy`: the port thread never allocates and the rack
/// never parses. `port` is an index resolved control-side, so no `String`
/// ever reaches the audio thread — the same discipline as the assembler's
/// detach-by-offset.
///
/// Channel-voice messages only. Anything `>= 0xF0` (SysEx, clock, reset) is
/// dropped at the port thread: none of it reaches a SoundFont voice.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MidiEvent {
    pub port: u8,
    pub channel: u8,
    /// Status nibble only: 0x80 note off, 0x90 note on, 0xB0 CC,
    /// 0xC0 program, 0xE0 pitch bend.
    pub status: u8,
    pub data1: u8,
    pub data2: u8,
}

/// Which MIDI traffic an instrument answers to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct MidiBinding {
    /// Port index, resolved control-side. `None` = bound to nothing, which
    /// is silence with a reason rather than an error.
    pub port: Option<u8>,
    /// `None` = omni: every channel on that port.
    pub channel: Option<u8>,
}

impl MidiBinding {
    fn accepts(&self, event: &MidiEvent) -> bool {
        self.port == Some(event.port) && self.channel.is_none_or(|channel| channel == event.channel)
    }
}

/// Which keys a voice answers to, and how many mixer channels it writes.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct KeyFilter {
    /// Inclusive ranges. Empty = the whole keyboard, and stereo output.
    pub ranges: Vec<(u8, u8)>,
}

impl KeyFilter {
    pub fn all() -> Self {
        KeyFilter { ranges: Vec::new() }
    }

    pub fn is_split(&self) -> bool {
        !self.ranges.is_empty()
    }

    fn covers(&self, key: u8) -> bool {
        self.ranges.is_empty()
            || self
                .ranges
                .iter()
                .any(|(lo, hi)| (*lo..=*hi).contains(&key))
    }
}

/// One loaded instrument. Construction — parsing the SoundFont, building
/// the voice pool — happens control-side; the audio thread only renders.
///
/// A split kit is several of these over one shared `Arc<SoundFont>`: each
/// hears only its own keys, so the total voices in flight are the same as
/// one synth would carry, and the samples are loaded once.
pub struct Instrument {
    synth: Synthesizer,
    binding: MidiBinding,
    keys: KeyFilter,
}

/// Everything needed to build one voice, gathered so the constructor reads
/// as one decision rather than eight positional arguments.
#[derive(Debug, Clone)]
pub struct VoiceSpec {
    pub sample_rate: u32,
    pub polyphony: u16,
    pub effects: bool,
    pub bank: u16,
    pub program: u8,
    pub binding: MidiBinding,
    /// Which keys this voice answers to — and therefore whether it is one
    /// mono split or the instrument's whole stereo output.
    pub keys: KeyFilter,
}

impl Instrument {
    /// Build a playable instrument. `soundfont` is shared: several
    /// instruments on one file must hold the same `Arc`, or a rack rebuild
    /// transiently doubles hundreds of megabytes of sample data.
    ///
    /// `bank`/`program` are applied as MIDI messages because that is the
    /// only way a SoundFont player selects a preset — bank select MSB/LSB
    /// then program change, exactly as a sequencer would.
    pub fn new(
        soundfont: &Arc<SoundFont>,
        spec: &VoiceSpec,
    ) -> Result<Self, rustysynth::SynthesizerError> {
        let mut settings = SynthesizerSettings::new(spec.sample_rate as i32);
        settings.block_size = SYNTH_SUB_BLOCK;
        settings.maximum_polyphony = spec.polyphony as usize;
        settings.enable_reverb_and_chorus = spec.effects;

        let mut synth = Synthesizer::new(soundfont, &settings)?;
        // Bank select MSB/LSB, then program change. Channel 0 throughout:
        // the rack routes by binding and re-addresses every event to one
        // channel, so an instrument is one timbre no matter what the
        // keyboard transmits on.
        synth.process_midi_message(0, 0xB0, 0x00, (spec.bank >> 7) as i32 & 0x7F);
        synth.process_midi_message(0, 0xB0, 0x20, (spec.bank & 0x7F) as i32);
        synth.process_midi_message(0, 0xC0, spec.program as i32, 0);
        Ok(Self {
            synth,
            binding: spec.binding,
            keys: spec.keys.clone(),
        })
    }

    /// Apply one event; `false` when this voice ignored it.
    fn apply(&mut self, event: &MidiEvent) -> bool {
        // Note messages are filtered by key; everything else — sustain,
        // pitch bend, program change — reaches every split, because those
        // are properties of the performance rather than of one drum.
        if matches!(event.status, 0x80 | 0x90 | 0xA0) && !self.keys.covers(event.data1) {
            return false;
        }
        self.synth.process_midi_message(
            0,
            event.status as i32,
            event.data1 as i32,
            event.data2 as i32,
        );
        true
    }
}

/// The engine's instrument rack. Built control-side and moved in whole
/// through the command ring, so a SoundFont is never parsed — nor freed —
/// on the audio thread.
#[derive(Default)]
pub struct InstrumentRack {
    /// One slot per instrument in the console document, in document order —
    /// `None` where the soundfont would not load.
    ///
    /// A failed instrument keeps its slot rather than being skipped,
    /// because `compile()` resolves an instrument patch by position. Closing
    /// the gap would silently move every later instrument's audio onto the
    /// wrong strip, which is a far worse failure than the one that caused
    /// it.
    instruments: Vec<Option<Instrument>>,
    /// Channels each slot writes, parallel to `instruments`. Held
    /// separately so a slot that failed to load still reserves the width
    /// the console laid out for it.
    widths: Vec<u8>,
    /// Planar scratch for one sub-chunk, preallocated. `rustysynth` renders
    /// left and right separately; the rack interleaves.
    left: Vec<f32>,
    right: Vec<f32>,
}

impl InstrumentRack {
    /// `widths` is the channel count of each slot as the console laid it
    /// out — supplied rather than derived, because a slot whose soundfont
    /// failed has no voice to ask, and its channels must exist anyway.
    pub fn new(instruments: Vec<Option<Instrument>>, widths: Vec<u8>) -> Self {
        debug_assert_eq!(instruments.len(), widths.len());
        Self {
            instruments,
            widths,
            left: vec![0.0; SYNTH_SUB_BLOCK],
            right: vec![0.0; SYNTH_SUB_BLOCK],
        }
    }

    pub fn is_empty(&self) -> bool {
        self.instruments.is_empty()
    }

    /// Channels the rack writes: one per split voice, two per stereo one.
    ///
    /// A slot that failed to load still declares its width, so the layout
    /// matches the document even when nothing in it will sound.
    pub fn channels(&self) -> usize {
        self.widths.iter().map(|w| *w as usize).sum()
    }

    /// Silence every held note. Called on a rack swap and whenever the
    /// event ring drops — a lost note-off hangs forever otherwise.
    pub fn all_notes_off(&mut self) {
        for instrument in self.instruments.iter_mut().flatten() {
            instrument.synth.note_off_all(false);
        }
    }

    /// Render `frames` into `buf` (interleaved, `channels()` wide) while
    /// applying MIDI at sub-chunk boundaries, and return the channel count.
    ///
    /// Allocation-free by contract: everything it touches was sized when the
    /// rack was built. An empty rack returns 0 immediately, so a console
    /// with no instruments pays nothing.
    pub fn render_into(
        &mut self,
        buf: &mut [f32],
        frames: usize,
        midi: &mut Consumer<MidiEvent>,
        capture: Option<(&mut crate::record::MidiCapture, u64)>,
    ) -> usize {
        let mut capture = capture;
        let channels = self.channels();
        if channels == 0 {
            // Still drain, or events pile up until the ring blocks the
            // port thread and the first instrument added hears history.
            while midi.pop().is_ok() {}
            return 0;
        }
        debug_assert!(buf.len() >= frames * channels);

        let mut done = 0;
        while done < frames {
            let chunk = (frames - done).min(SYNTH_SUB_BLOCK);
            // Events are stamped at the sub-chunk they are applied in —
            // the same 1.33 ms grid the audio hears them on, so the
            // sidecar and the waveform agree.
            let at = capture
                .as_ref()
                .map(|(_, position)| *position + done as u64);
            self.drain(midi, capture.as_mut().map(|(c, _)| &mut **c), at);
            let mut base = 0usize;
            for (slot, width) in self.instruments.iter_mut().zip(&self.widths) {
                let width = *width as usize;
                match slot {
                    Some(instrument) => {
                        instrument
                            .synth
                            .render(&mut self.left[..chunk], &mut self.right[..chunk]);
                        for frame in 0..chunk {
                            let out = (done + frame) * channels + base;
                            if width == 1 {
                                // A split is mono: average rather than sum,
                                // so a centred drum keeps its level instead
                                // of gaining 6 dB for being centred.
                                buf[out] = 0.5 * (self.left[frame] + self.right[frame]);
                            } else {
                                buf[out] = self.left[frame];
                                buf[out + 1] = self.right[frame];
                            }
                        }
                    }
                    // A slot that failed to load is silence, and must be
                    // written as silence: the buffer is reused every block.
                    None => {
                        for frame in 0..chunk {
                            let out = (done + frame) * channels + base;
                            buf[out..out + width].fill(0.0);
                        }
                    }
                }
                base += width;
            }
            done += chunk;
        }
        channels
    }

    /// Apply every pending event to the instruments bound to it. An event
    /// no instrument claims is dropped, not broadcast: a keyboard on the
    /// wrong channel must stay silent, or the mistake is unfindable.
    fn drain(
        &mut self,
        midi: &mut Consumer<MidiEvent>,
        mut capture: Option<&mut crate::record::MidiCapture>,
        at: Option<u64>,
    ) {
        while let Ok(event) = midi.pop() {
            for (index, instrument) in self.instruments.iter_mut().enumerate() {
                let Some(instrument) = instrument else {
                    continue;
                };
                if !instrument.binding.accepts(&event) {
                    continue;
                }
                let applied = instrument.apply(&event);
                // Only what actually reached a voice is captured: a key
                // outside a split's range never sounded, so writing it into
                // that split's sidecar would be a lie about the take.
                if applied && let (Some(capture), Some(sample)) = (capture.as_deref_mut(), at) {
                    capture.push(trib_core::CapturedMidi {
                        sample,
                        instrument: index as u16,
                        status: event.status,
                        data1: event.data1,
                        data2: event.data2,
                    });
                }
            }
        }
    }
}

pub mod fixture {
    //! A minimal but genuinely valid SoundFont, built in memory.
    //!
    //! Generated rather than committed as a binary blob: a 4 KB `.sf2` in
    //! the tree is unreviewable, and every field here is load-bearing for
    //! some test. Debian's `TimGM6mb.sf2` is deliberately not used — it is
    //! GPL, and this repo is MIT.
    //!
    //! Public (not `#[cfg(test)]`) because the daemon's own tests need a
    //! real soundfont to exercise loading, caching and upload validation,
    //! and a second copy of this generator would be the duplication it
    //! exists to avoid. It costs a few hundred bytes of release binary.

    /// 750 Hz at 48 kHz — a 64-sample period, so the loop is seamless.
    const PERIOD: usize = 64;
    const SAMPLE_FRAMES: usize = 1024;
    /// The spec's inter-sample guard, and what makes the loader's sanity
    /// check pass: it demands the sample END sit strictly inside the data.
    const GUARD_FRAMES: usize = 46;

    fn chunk(id: &[u8; 4], payload: &[u8]) -> Vec<u8> {
        // Every chunk this fixture emits is even-sized, so RIFF's pad byte
        // never applies — and must not be written, since the reader takes
        // the size at its word.
        assert_eq!(payload.len() % 2, 0, "{id:?} would need a RIFF pad byte");
        let mut out = Vec::with_capacity(8 + payload.len());
        out.extend_from_slice(id);
        out.extend_from_slice(&(payload.len() as u32).to_le_bytes());
        out.extend_from_slice(payload);
        out
    }

    fn list(form: &[u8; 4], payload: &[u8]) -> Vec<u8> {
        let mut inner = form.to_vec();
        inner.extend_from_slice(payload);
        chunk(b"LIST", &inner)
    }

    fn name20(name: &str) -> [u8; 20] {
        let mut out = [0u8; 20];
        let bytes = name.as_bytes();
        out[..bytes.len()].copy_from_slice(bytes);
        out
    }

    fn generator(kind: u16, value: u16) -> [u8; 4] {
        let mut out = [0u8; 4];
        out[..2].copy_from_slice(&kind.to_le_bytes());
        out[2..].copy_from_slice(&value.to_le_bytes());
        out
    }

    fn zone(generator_index: u16, modulator_index: u16) -> [u8; 4] {
        let mut out = [0u8; 4];
        out[..2].copy_from_slice(&generator_index.to_le_bytes());
        out[2..].copy_from_slice(&modulator_index.to_le_bytes());
        out
    }

    /// One preset ("Sine", bank 0 program 0) playing one looped sine
    /// sample across the whole keyboard.
    pub fn sine_soundfont() -> Vec<u8> {
        // INFO: version, engine and bank name are the three the loader
        // insists on.
        let mut info = Vec::new();
        info.extend(chunk(b"ifil", &[2, 0, 1, 0]));
        info.extend(chunk(b"isng", b"EMU8000\0"));
        info.extend(chunk(b"INAM", b"Tributary test\0\0"));

        // sdta: the sine, then the guard frames.
        let mut samples = Vec::with_capacity((SAMPLE_FRAMES + GUARD_FRAMES) * 2);
        for frame in 0..SAMPLE_FRAMES {
            let phase = std::f64::consts::TAU * (frame % PERIOD) as f64 / PERIOD as f64;
            let value = (phase.sin() * f64::from(i16::MAX) * 0.5) as i16;
            samples.extend_from_slice(&value.to_le_bytes());
        }
        samples.extend(std::iter::repeat_n(0u8, GUARD_FRAMES * 2));
        let sdta = chunk(b"smpl", &samples);

        // pdta. Each list carries exactly one real record plus the
        // terminator the format requires.
        let mut phdr = Vec::new();
        phdr.extend_from_slice(&name20("Sine"));
        phdr.extend_from_slice(&0u16.to_le_bytes()); // program
        phdr.extend_from_slice(&0u16.to_le_bytes()); // bank
        phdr.extend_from_slice(&0u16.to_le_bytes()); // first bag
        phdr.extend_from_slice(&[0u8; 12]); // library, genre, morphology
        phdr.extend_from_slice(&name20("EOP"));
        phdr.extend_from_slice(&0u16.to_le_bytes());
        phdr.extend_from_slice(&0u16.to_le_bytes());
        phdr.extend_from_slice(&1u16.to_le_bytes()); // one bag consumed
        phdr.extend_from_slice(&[0u8; 12]);

        let mut pbag = Vec::new();
        pbag.extend_from_slice(&zone(0, 0));
        pbag.extend_from_slice(&zone(1, 0));

        let mut pgen = Vec::new();
        // Last generator of the zone must be INSTRUMENT, or the loader
        // reads the zone as a global one and the preset has no regions.
        pgen.extend_from_slice(&generator(41, 0)); // INSTRUMENT -> #0
        pgen.extend_from_slice(&generator(0, 0)); // terminator

        let mut inst = Vec::new();
        inst.extend_from_slice(&name20("Sine"));
        inst.extend_from_slice(&0u16.to_le_bytes());
        inst.extend_from_slice(&name20("EOI"));
        inst.extend_from_slice(&1u16.to_le_bytes());

        let mut ibag = Vec::new();
        ibag.extend_from_slice(&zone(0, 0));
        ibag.extend_from_slice(&zone(2, 0));

        let mut igen = Vec::new();
        igen.extend_from_slice(&generator(54, 1)); // SAMPLE_MODES: loop
        igen.extend_from_slice(&generator(53, 0)); // SAMPLE_ID -> #0, last
        igen.extend_from_slice(&generator(0, 0)); // terminator

        let end = SAMPLE_FRAMES as u32;
        let mut shdr = Vec::new();
        shdr.extend_from_slice(&name20("Sine"));
        shdr.extend_from_slice(&0u32.to_le_bytes()); // start
        shdr.extend_from_slice(&end.to_le_bytes()); // end
        shdr.extend_from_slice(&0u32.to_le_bytes()); // loop start
        shdr.extend_from_slice(&end.to_le_bytes()); // loop end
        shdr.extend_from_slice(&48_000u32.to_le_bytes());
        shdr.push(78); // original pitch: ~750 Hz is F#5
        shdr.push(0); // pitch correction
        shdr.extend_from_slice(&0u16.to_le_bytes()); // link
        shdr.extend_from_slice(&1u16.to_le_bytes()); // mono sample
        shdr.extend_from_slice(&name20("EOS"));
        shdr.extend_from_slice(&[0u8; 26]);

        let mut pdta = Vec::new();
        pdta.extend(chunk(b"phdr", &phdr));
        pdta.extend(chunk(b"pbag", &pbag));
        pdta.extend(chunk(b"pmod", &[0u8; 10]));
        pdta.extend(chunk(b"pgen", &pgen));
        pdta.extend(chunk(b"inst", &inst));
        pdta.extend(chunk(b"ibag", &ibag));
        pdta.extend(chunk(b"imod", &[0u8; 10]));
        pdta.extend(chunk(b"igen", &igen));
        pdta.extend(chunk(b"shdr", &shdr));

        let mut body = b"sfbk".to_vec();
        body.extend(list(b"INFO", &info));
        body.extend(list(b"sdta", &sdta));
        body.extend(list(b"pdta", &pdta));

        let mut out = b"RIFF".to_vec();
        out.extend_from_slice(&(body.len() as u32).to_le_bytes());
        out.extend(body);
        out
    }
}

#[cfg(test)]
mod tests {
    use super::fixture::sine_soundfont;
    use super::*;
    use rtrb::RingBuffer;

    // NOTE: no `#[global_allocator]` here — `engine.rs` already declares
    // `assert_no_alloc::AllocDisabler` for this crate's test binary, and a
    // second one is a compile error.

    const SR: u32 = 48_000;

    fn soundfont() -> Arc<SoundFont> {
        let mut bytes = std::io::Cursor::new(sine_soundfont());
        Arc::new(SoundFont::new(&mut bytes).expect("the fixture must be a valid SoundFont"))
    }

    fn spec(binding: MidiBinding, keys: KeyFilter) -> VoiceSpec {
        VoiceSpec {
            sample_rate: SR,
            polyphony: 32,
            effects: false,
            bank: 0,
            program: 0,
            binding,
            keys,
        }
    }

    fn bound_to_port_0() -> MidiBinding {
        MidiBinding {
            port: Some(0),
            channel: None,
        }
    }

    fn rack_of(count: usize, binding: MidiBinding) -> InstrumentRack {
        let sf = soundfont();
        let instruments: Vec<Option<Instrument>> = (0..count)
            .map(|_| Some(Instrument::new(&sf, &spec(binding, KeyFilter::all())).expect("voice")))
            .collect();
        let widths = vec![2u8; instruments.len()];
        InstrumentRack::new(instruments, widths)
    }

    fn note_on(port: u8, channel: u8, key: u8) -> MidiEvent {
        MidiEvent {
            port,
            channel,
            status: 0x90,
            data1: key,
            data2: 100,
        }
    }

    fn peak(buf: &[f32], channels: usize, channel: usize) -> f32 {
        buf.chunks_exact(channels)
            .map(|frame| frame[channel].abs())
            .fold(0.0f32, f32::max)
    }

    #[test]
    fn the_fixture_is_a_valid_soundfont_with_one_preset() {
        let sf = soundfont();
        assert_eq!(sf.get_presets().len(), 1);
        assert_eq!(sf.get_presets()[0].get_name(), "Sine");
    }

    #[test]
    fn an_empty_rack_reports_no_channels_and_renders_nothing() {
        let mut rack = InstrumentRack::default();
        let (_tx, mut rx) = RingBuffer::<MidiEvent>::new(MIDI_RING_CAPACITY);
        let mut buf = [0.0f32; 256];
        assert_eq!(rack.render_into(&mut buf, 128, &mut rx, None), 0);
        assert!(buf.iter().all(|&s| s == 0.0));
    }

    #[test]
    fn an_empty_rack_still_drains_the_ring_rather_than_hoarding_history() {
        let mut rack = InstrumentRack::default();
        let (mut tx, mut rx) = RingBuffer::<MidiEvent>::new(MIDI_RING_CAPACITY);
        tx.push(note_on(0, 0, 60)).unwrap();
        let mut buf = [0.0f32; 256];
        rack.render_into(&mut buf, 128, &mut rx, None);
        assert!(rx.pop().is_err(), "the event should have been consumed");
    }

    #[test]
    fn a_note_on_reaches_only_its_own_instruments_stereo_pair() {
        let mut rack = rack_of(2, bound_to_port_0());
        // Re-bind the second instrument to a port nothing sends to.
        rack.instruments[1].as_mut().unwrap().binding = MidiBinding {
            port: Some(1),
            channel: None,
        };
        let (mut tx, mut rx) = RingBuffer::<MidiEvent>::new(MIDI_RING_CAPACITY);
        tx.push(note_on(0, 0, 78)).unwrap();

        let mut buf = vec![0.0f32; 256 * 4];
        let channels = rack.render_into(&mut buf, 256, &mut rx, None);

        assert_eq!(channels, 4);
        assert!(peak(&buf, channels, 0) > 0.0, "instrument 0 left is silent");
        assert!(
            peak(&buf, channels, 1) > 0.0,
            "instrument 0 right is silent"
        );
        assert_eq!(
            peak(&buf, channels, 2),
            0.0,
            "instrument 1 should not hear port 0"
        );
        assert_eq!(
            peak(&buf, channels, 3),
            0.0,
            "instrument 1 should not hear port 0"
        );
    }

    #[test]
    fn a_split_kit_puts_each_drum_on_its_own_mono_channel() {
        // The whole point of splits: hitting the kick must move the kick
        // fader and nothing else, so it can be panned and levelled alone.
        let sf = soundfont();
        let split = |ranges: &[(u8, u8)]| {
            let keys = KeyFilter {
                ranges: ranges.to_vec(),
            };
            Some(Instrument::new(&sf, &spec(bound_to_port_0(), keys)).unwrap())
        };
        // Kick on 36, hats on the interleaved 42/44/46 — the GM layout a
        // single contiguous range could not express.
        let mut rack = InstrumentRack::new(
            vec![split(&[(35, 36)]), split(&[(42, 42), (44, 44), (46, 46)])],
            vec![1, 1],
        );
        assert_eq!(rack.channels(), 2, "two drums, two mono faders");

        let (mut tx, mut rx) = RingBuffer::<MidiEvent>::new(MIDI_RING_CAPACITY);
        tx.push(note_on(0, 0, 36)).unwrap();
        let mut buf = vec![0.0f32; 256 * 2];
        let channels = rack.render_into(&mut buf, 256, &mut rx, None);

        assert!(peak(&buf, channels, 0) > 0.0, "the kick sounded");
        assert_eq!(peak(&buf, channels, 1), 0.0, "the hats stayed silent");

        // And the interleaved hat key reaches only the hat channel.
        buf.fill(0.0);
        tx.push(note_on(0, 0, 44)).unwrap();
        rack.render_into(&mut buf, 256, &mut rx, None);
        assert!(peak(&buf, channels, 1) > 0.0, "the hat sounded");
    }

    #[test]
    fn a_key_no_split_covers_is_silent_everywhere() {
        // Worth pinning: a pad outside the map does nothing, and the
        // console has to be able to say so rather than leave someone
        // hitting it.
        let sf = soundfont();
        let kick = Instrument::new(
            &sf,
            &spec(
                bound_to_port_0(),
                KeyFilter {
                    ranges: vec![(35, 36)],
                },
            ),
        )
        .unwrap();
        let mut rack = InstrumentRack::new(vec![Some(kick)], vec![1]);
        let (mut tx, mut rx) = RingBuffer::<MidiEvent>::new(MIDI_RING_CAPACITY);
        tx.push(note_on(0, 0, 60)).unwrap();
        let mut buf = vec![0.0f32; 256];
        let channels = rack.render_into(&mut buf, 256, &mut rx, None);
        assert_eq!(peak(&buf, channels, 0), 0.0);
    }

    #[test]
    fn an_instrument_that_failed_to_load_still_holds_its_slot() {
        // The compiled graph resolves an instrument patch by position, so
        // closing the gap left by a failed load would move every later
        // instrument's audio onto the wrong strip — a far worse failure
        // than the missing soundfont that caused it.
        let sf = soundfont();
        let live = |binding| Some(Instrument::new(&sf, &spec(binding, KeyFilter::all())).unwrap());
        let mut rack = InstrumentRack::new(vec![None, live(bound_to_port_0())], vec![2, 2]);
        assert_eq!(rack.channels(), 4, "the dead slot still costs two channels");

        let (mut tx, mut rx) = RingBuffer::<MidiEvent>::new(MIDI_RING_CAPACITY);
        tx.push(note_on(0, 0, 78)).unwrap();
        let mut buf = vec![0.0f32; 256 * 4];
        let channels = rack.render_into(&mut buf, 256, &mut rx, None);

        assert_eq!(peak(&buf, channels, 0), 0.0, "the dead slot is silent");
        assert_eq!(peak(&buf, channels, 1), 0.0);
        assert!(peak(&buf, channels, 2) > 0.0, "the live one kept its place");
    }

    #[test]
    fn a_dead_slot_is_written_as_silence_not_left_stale() {
        // The buffer is reused every block: skipping the write would leave
        // whatever the previous rack put there.
        let mut rack = InstrumentRack::new(vec![None], vec![2]);
        let (_tx, mut rx) = RingBuffer::<MidiEvent>::new(MIDI_RING_CAPACITY);
        let mut buf = vec![0.7f32; 256 * 2];
        rack.render_into(&mut buf, 256, &mut rx, None);
        assert!(buf.iter().all(|&s| s == 0.0));
    }

    #[test]
    fn an_event_on_an_unbound_port_or_channel_is_dropped_not_misrouted() {
        let mut rack = rack_of(
            1,
            MidiBinding {
                port: Some(0),
                channel: Some(3),
            },
        );
        let (mut tx, mut rx) = RingBuffer::<MidiEvent>::new(MIDI_RING_CAPACITY);
        tx.push(note_on(0, 9, 78)).unwrap(); // right port, wrong channel
        tx.push(note_on(1, 3, 78)).unwrap(); // right channel, wrong port

        let mut buf = vec![0.0f32; 256 * 2];
        let channels = rack.render_into(&mut buf, 256, &mut rx, None);
        assert_eq!(peak(&buf, channels, 0), 0.0);
    }

    #[test]
    fn an_omni_instrument_accepts_every_channel_on_its_port() {
        for channel in 0..16u8 {
            let mut rack = rack_of(1, bound_to_port_0());
            let (mut tx, mut rx) = RingBuffer::<MidiEvent>::new(MIDI_RING_CAPACITY);
            tx.push(note_on(0, channel, 78)).unwrap();
            let mut buf = vec![0.0f32; 256 * 2];
            let channels = rack.render_into(&mut buf, 256, &mut rx, None);
            assert!(
                peak(&buf, channels, 0) > 0.0,
                "omni should have accepted channel {channel}"
            );
        }
    }

    #[test]
    fn all_notes_off_silences_a_held_note() {
        let mut rack = rack_of(1, bound_to_port_0());
        let (mut tx, mut rx) = RingBuffer::<MidiEvent>::new(MIDI_RING_CAPACITY);
        tx.push(note_on(0, 0, 78)).unwrap();
        let mut buf = vec![0.0f32; 256 * 2];
        rack.render_into(&mut buf, 256, &mut rx, None);

        rack.all_notes_off();
        // The release stage is ~1 ms, so give it a block to fall away.
        rack.render_into(&mut buf, 256, &mut rx, None);
        buf.fill(0.0);
        let channels = rack.render_into(&mut buf, 256, &mut rx, None);
        assert_eq!(peak(&buf, channels, 0), 0.0);
    }

    // The gate this whole feature stands on: if SoundFont rendering
    // allocates, it cannot live on the render thread and the rack has to
    // become a demand-driven double buffer instead.
    #[test]
    fn rendering_a_held_note_never_allocates_on_the_audio_thread() {
        let mut rack = rack_of(2, bound_to_port_0());
        let (mut tx, mut rx) = RingBuffer::<MidiEvent>::new(MIDI_RING_CAPACITY);
        tx.push(note_on(0, 0, 78)).unwrap();
        let mut buf = vec![0.0f32; 256 * 4];

        assert_no_alloc::assert_no_alloc(|| {
            for _ in 0..8 {
                rack.render_into(&mut buf, 256, &mut rx, None);
            }
        });
    }

    #[test]
    fn applying_midi_never_allocates_on_the_audio_thread() {
        let mut rack = rack_of(1, bound_to_port_0());
        let (mut tx, mut rx) = RingBuffer::<MidiEvent>::new(MIDI_RING_CAPACITY);
        let mut buf = vec![0.0f32; 256 * 2];

        for key in 40..80u8 {
            tx.push(note_on(0, 0, key)).unwrap();
        }
        for key in 40..80u8 {
            tx.push(MidiEvent {
                port: 0,
                channel: 0,
                status: 0x80,
                data1: key,
                data2: 0,
            })
            .unwrap();
        }
        // Pitch bend and a controller too — the other messages a keyboard
        // sends without being asked.
        tx.push(MidiEvent {
            port: 0,
            channel: 0,
            status: 0xE0,
            data1: 0,
            data2: 96,
        })
        .unwrap();
        tx.push(MidiEvent {
            port: 0,
            channel: 0,
            status: 0xB0,
            data1: 0x40,
            data2: 127,
        })
        .unwrap();

        assert_no_alloc::assert_no_alloc(|| {
            rack.render_into(&mut buf, 256, &mut rx, None);
        });
    }

    #[test]
    fn all_notes_off_never_allocates_on_the_audio_thread() {
        let mut rack = rack_of(2, bound_to_port_0());
        let (mut tx, mut rx) = RingBuffer::<MidiEvent>::new(MIDI_RING_CAPACITY);
        tx.push(note_on(0, 0, 78)).unwrap();
        let mut buf = vec![0.0f32; 256 * 4];
        rack.render_into(&mut buf, 256, &mut rx, None);

        assert_no_alloc::assert_no_alloc(|| {
            rack.all_notes_off();
        });
    }

    #[test]
    fn a_render_shorter_than_a_sub_block_still_advances_the_synth() {
        let mut rack = rack_of(1, bound_to_port_0());
        let (mut tx, mut rx) = RingBuffer::<MidiEvent>::new(MIDI_RING_CAPACITY);
        tx.push(note_on(0, 0, 78)).unwrap();
        // 7 frames is neither a multiple of SYNTH_SUB_BLOCK nor of the
        // graph block; the callback sizes cpal hands us are arbitrary.
        let mut buf = vec![0.0f32; 7 * 2];
        let channels = rack.render_into(&mut buf, 7, &mut rx, None);
        assert_eq!(channels, 2);
    }
}
