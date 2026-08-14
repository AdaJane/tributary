//! The audio-thread engine: drains commands, runs the compiled graph,
//! pushes telemetry, and returns retired graphs to the control side for
//! dropping. Grown from the M1 tracer; the boundary contracts are the same.

use std::sync::atomic::Ordering;

use rtrb::{Consumer, Producer, RingBuffer};

use crate::compiled::CompiledGraph;
use crate::instrument::{InstrumentRack, MAX_INSTRUMENT_CHANNELS, MIDI_RING_CAPACITY, MidiEvent};
use crate::playback::PlaybackSet;
use crate::record::RecordSet;
use crate::rings::{
    CMD_RING_CAPACITY, EngineCommand, MAX_METERS, METER_RING_CAPACITY, MONITOR_RING_CAPACITY,
    MeterBlock, MonitorTarget, RETIRE_RING_CAPACITY, Retired,
};
use crate::slots::{MAX_OUTPUT_CHANNELS, MONITOR_OUT};

/// Commands drained per block. Bounds per-callback work; the rest waits one
/// block (~5 ms) — invisible at gesture rates.
const MAX_CMDS_PER_BLOCK: usize = 64;

/// Retired graphs the audio thread can hold while the retire ring is full.
/// The pump drains every 50 ms, so this never fills in practice; the
/// capacity only exists so `parked.push` cannot allocate.
const MAX_PARKED: usize = 8;

/// The control side's end of the rings.
pub struct EngineHandle {
    pub cmd_tx: Producer<EngineCommand>,
    pub meter_rx: Consumer<MeterBlock>,
    /// Old graphs and finished record sets come back here to be dropped
    /// control-side.
    pub retire_rx: Consumer<Retired>,
    /// The playback mix while the monitor targets the browser stream.
    pub monitor_rx: Consumer<f32>,
    /// MIDI into the rack. Written by the port threads, not the control
    /// task: an event's whole job is to arrive quickly, and routing it
    /// through the control loop would put a fader drag between a key and
    /// its note.
    pub midi_tx: Producer<MidiEvent>,
}

/// The audio side. `process` runs on the audio callback: no allocation, no
/// locks, no syscalls.
pub struct GraphEngine {
    graph: Box<CompiledGraph>,
    /// Virtual instruments. Empty until the instrument host publishes one,
    /// and an empty rack short-circuits, so a console with no instruments
    /// pays nothing for their existence.
    rack: Box<InstrumentRack>,
    /// Scratch the rack renders into — preallocated at the widest the rack
    /// may ever be, so adding an instrument never allocates here.
    instrument_buf: Vec<f32>,
    midi_rx: Consumer<MidiEvent>,
    record: Option<Box<RecordSet>>,
    playback: Option<Box<PlaybackSet>>,
    monitor: MonitorTarget,
    /// Scratch for the stream branch — preallocated, one block of stereo.
    monitor_buf: Vec<f32>,
    monitor_tx: Producer<f32>,
    /// Retirees waiting for ring space. Preallocated; never grows.
    parked: Vec<Retired>,
    frame: u64,
    /// Frames rendered since the engine started. Unlike `frame`, which
    /// counts blocks for the meter pump, this counts samples — the unit a
    /// MIDI capture has to stamp events in.
    sample_frame: u64,
    cmd_rx: Consumer<EngineCommand>,
    meter_tx: Producer<MeterBlock>,
    retire_tx: Producer<Retired>,
}

pub fn engine_pair(initial: Box<CompiledGraph>) -> (EngineHandle, GraphEngine) {
    let (cmd_tx, cmd_rx) = RingBuffer::new(CMD_RING_CAPACITY);
    let (meter_tx, meter_rx) = RingBuffer::new(METER_RING_CAPACITY);
    let (retire_tx, retire_rx) = RingBuffer::new(RETIRE_RING_CAPACITY);
    let (monitor_tx, monitor_rx) = RingBuffer::new(MONITOR_RING_CAPACITY);
    let (midi_tx, midi_rx) = RingBuffer::new(MIDI_RING_CAPACITY);
    let monitor_buf = vec![0.0; initial.block_size() * 2];
    let instrument_buf = vec![0.0; initial.block_size() * MAX_INSTRUMENT_CHANNELS];
    (
        EngineHandle {
            cmd_tx,
            meter_rx,
            retire_rx,
            monitor_rx,
            midi_tx,
        },
        GraphEngine {
            graph: initial,
            rack: Box::default(),
            instrument_buf,
            midi_rx,
            record: None,
            playback: None,
            monitor: MonitorTarget::Hardware,
            monitor_buf,
            monitor_tx,
            parked: Vec::with_capacity(MAX_PARKED),
            frame: 0,
            sample_frame: 0,
            cmd_rx,
            meter_tx,
            retire_tx,
        },
    )
}

impl GraphEngine {
    /// One callback's work. `input` is interleaved `input_channels`;
    /// `output` is the interleaved [`MAX_OUTPUT_CHANNELS`] plane, built by
    /// [`crate::output_buffer`]. Arbitrary callback sizes are handled by
    /// sub-blocking at the compiled block size.
    ///
    /// Channels [`MONITOR_OUT`] are the control-room feed, which PFL and
    /// the tape return may take over. Everything above them belongs to
    /// direct outs, which neither may touch.
    pub fn process(&mut self, input: &[f32], input_channels: usize, output: &mut [f32]) {
        self.drain_commands();
        self.flush_parked();

        let block = self.graph.block_size();
        // Not a `debug_assert`: a backend that hands us a narrower buffer
        // would render every direct out into silence, and silence is
        // exactly the failure this is insurance against. A panicking audio
        // thread is loud; a quietly dead output jack is not.
        //
        // "Multiple of the stride" alone is far too weak to be that
        // insurance — a stereo-sized buffer at a 256-frame block is 512
        // samples, which IS a multiple of 64, and would quietly render 8
        // frames instead of 256. So the two frames must also agree with
        // each other, which is what actually pins the length.
        let frames = output.len() / MAX_OUTPUT_CHANNELS;
        assert!(
            output.len().is_multiple_of(MAX_OUTPUT_CHANNELS)
                && input.len() == frames * input_channels,
            "the engine output is a fixed {MAX_OUTPUT_CHANNELS}-channel plane \
             and must describe the same frame count as the input: got {} output \
             samples ({frames} frames) against {} input samples at {input_channels} \
             channels — build the plane with `trib_engine::output_buffer`",
            output.len(),
            input.len(),
        );

        let mut done = 0;
        while done < frames {
            let chunk = (frames - done).min(block);
            let mut peaks = [0.0f32; MAX_METERS];
            let mut clip_bits = 0u64;
            // Instruments render first: they are an input source, and the
            // strip pass reads them exactly the way it reads a device.
            // The sidecar's clock is the take's, not the daemon's uptime.
            let capture = self.record.as_deref_mut().and_then(|set| {
                let start = set.midi.as_ref()?.start_sample;
                let elapsed = self.sample_frame.saturating_sub(start);
                Some((set.midi.as_deref_mut()?, elapsed))
            });
            let inst_channels = self.rack.render_into(
                &mut self.instrument_buf[..chunk * MAX_INSTRUMENT_CHANNELS],
                chunk,
                &mut self.midi_rx,
                capture,
            );
            self.graph.process_chunk(
                &input[done * input_channels..(done + chunk) * input_channels],
                input_channels,
                &self.instrument_buf[..chunk * inst_channels.max(1)],
                inst_channels,
                &mut output[done * MAX_OUTPUT_CHANNELS..(done + chunk) * MAX_OUTPUT_CHANNELS],
                &mut peaks,
                &mut clip_bits,
                self.record.as_deref_mut(),
            );
            let mut meter_block = MeterBlock::empty(self.frame);
            meter_block.count = self.graph.meter_count() as u8;
            meter_block.peak = peaks;
            meter_block.clip_bits = clip_bits;
            // Full ring = a stalled pump; meters are droppable by doctrine.
            let _ = self.meter_tx.push(meter_block);
            // The tape return: on the hardware monitor it replaces the live
            // mix in the MONITOR PAIR (the PFL precedent one stage up); on
            // the stream monitor it fills the browser ring and the wire
            // keeps the live mix. Meters always show the live mix, and
            // direct outs are untouched either way — a review convenience
            // must not reach a feed to the PA.
            //
            // Both arms render into `monitor_buf` rather than into the
            // plane: `mix_chunk` zeroes what it cannot fill and strides by
            // two, so handing it a slice of a 64-wide plane would wipe
            // every direct out on every block.
            let mut finished = false;
            if let Some(playback) = &mut self.playback {
                let buf = &mut self.monitor_buf[..chunk * 2];
                let popped = playback.mix_chunk(buf);
                playback
                    .shared
                    .position
                    .fetch_add(popped as u64, Ordering::Relaxed);
                finished = playback.shared.finished.load(Ordering::Relaxed);
                match self.monitor {
                    // `mix_chunk` leaves the buffer untouched in exactly
                    // one case — the tape has run out — so copying then
                    // would paste a stale block over the live mix that is
                    // supposed to return in the same block.
                    MonitorTarget::Hardware if !finished => {
                        for i in 0..chunk {
                            let base = (done + i) * MAX_OUTPUT_CHANNELS;
                            output[base + MONITOR_OUT[0] as usize] = buf[i * 2];
                            output[base + MONITOR_OUT[1] as usize] = buf[i * 2 + 1];
                        }
                    }
                    MonitorTarget::Hardware => {}
                    MonitorTarget::Stream => {
                        for &sample in &buf[..popped * 2] {
                            // Full ring = a stalled pump; stream audio is
                            // droppable by doctrine.
                            let _ = self.monitor_tx.push(sample);
                        }
                    }
                }
            }
            if finished && let Some(set) = self.playback.take() {
                self.retire(Retired::Playback(set));
            }
            self.frame += 1;
            self.sample_frame += chunk as u64;
            done += chunk;
        }
    }

    fn drain_commands(&mut self) {
        for _ in 0..MAX_CMDS_PER_BLOCK {
            match self.cmd_rx.pop() {
                Ok(EngineCommand::SetParam { param, target }) => {
                    self.graph.set_param(param, target);
                }
                Ok(EngineCommand::SetFlag { flag, on }) => self.graph.set_flag(flag, on),
                Ok(EngineCommand::SetEqCoeffs { eq, coeffs }) => {
                    self.graph.set_eq_coeffs(eq, coeffs);
                }
                Ok(EngineCommand::SwapGraph { graph }) => {
                    let old = std::mem::replace(&mut self.graph, graph);
                    self.retire(Retired::Graph(old));
                }
                Ok(EngineCommand::SwapRack { rack }) => {
                    let mut old = std::mem::replace(&mut self.rack, rack);
                    // The outgoing synths are about to lose the only thing
                    // that could ever release their voices.
                    old.all_notes_off();
                    self.retire(Retired::Rack(old));
                }
                Ok(EngineCommand::StartRecord { mut set }) => {
                    // Stamp the take's zero here rather than control-side:
                    // the control task cannot see the audio thread's clock,
                    // and a sidecar offset by the command's queue time
                    // would drift the notes off the waveform.
                    if let Some(midi) = set.midi.as_deref_mut() {
                        midi.start_sample = self.sample_frame;
                    }
                    if let Some(old) = self.record.replace(set) {
                        self.retire(Retired::Record(old));
                    }
                }
                Ok(EngineCommand::StopRecord) => {
                    if let Some(set) = self.record.take() {
                        self.retire(Retired::Record(set));
                    }
                }
                Ok(EngineCommand::StartPlayback { set }) => {
                    if let Some(old) = self.playback.replace(set) {
                        self.retire(Retired::Playback(old));
                    }
                }
                Ok(EngineCommand::StopPlayback) => {
                    if let Some(set) = self.playback.take() {
                        self.retire(Retired::Playback(set));
                    }
                }
                Ok(EngineCommand::SetPlaybackSolo { track, on }) => {
                    if let Some(playback) = &mut self.playback {
                        playback.set_solo(track, on);
                    }
                }
                Ok(EngineCommand::SetPlaybackMute { track, on }) => {
                    if let Some(playback) = &mut self.playback {
                        playback.set_mute(track, on);
                    }
                }
                Ok(EngineCommand::SetMonitorTarget { target }) => {
                    self.monitor = target;
                }
                Err(_) => break,
            }
        }
    }

    /// Never drop a retiree here — deallocation belongs to the control side.
    fn retire(&mut self, old: Retired) {
        if let Err(rtrb::PushError::Full(old)) = self.retire_tx.push(old) {
            if self.parked.len() < MAX_PARKED {
                self.parked.push(old);
            } else {
                // Unreachable with a live pump; dropping here is the
                // bounded emergency exit, not the design.
                debug_assert!(false, "retire ring and parking both full");
            }
        }
    }

    fn flush_parked(&mut self) {
        while let Some(old) = self.parked.pop() {
            if let Err(rtrb::PushError::Full(old)) = self.retire_tx.push(old) {
                self.parked.push(old);
                break;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use trib_core::{InputAssign, InstrumentId, InstrumentState, MixerState, StripId, StripState};

    use super::*;
    use crate::compiled::compile;
    use crate::slots::InputSlots;

    // Any allocation inside `assert_no_alloc` aborts the test: the process
    // path must stay allocation-free.
    #[global_allocator]
    static GUARD: assert_no_alloc::AllocDisabler = assert_no_alloc::AllocDisabler;

    const SR: u32 = 48_000;
    const BLOCK: usize = 256;

    fn slots() -> InputSlots {
        InputSlots::single_default(1)
    }

    fn state() -> MixerState {
        let mut strip = StripState::new(StripId(0), "Ch 1".into());
        strip.input = Some(InputAssign::device(None, 0));
        strip.fader_db = 0.0;
        MixerState {
            strips: vec![strip],
            ..MixerState::default()
        }
    }

    fn sine(amplitude: f32, len: usize) -> Vec<f32> {
        (0..len)
            .map(|i| amplitude * (i as f32 / SR as f32 * 440.0 * std::f32::consts::TAU).sin())
            .collect()
    }

    #[test]
    fn a_fader_command_changes_the_level_after_the_ramp() {
        let compiled = compile(
            &state(),
            SR,
            BLOCK,
            &slots(),
            &crate::slots::OutputSlots::with_monitor(),
        );
        let fader_ix = compiled.params.fader[&StripId(0)];
        let (mut handle, mut engine) = engine_pair(compiled.graph);
        handle
            .cmd_tx
            .push(EngineCommand::SetParam {
                param: fader_ix,
                target: 0.5,
            })
            .unwrap();
        let input = sine(0.8, BLOCK);
        let mut output = crate::slots::output_buffer(BLOCK);
        for _ in 0..3 {
            engine.process(&input, 1, &mut output);
        }
        let peak = output
            .chunks_exact(2)
            .map(|f| f[0].abs())
            .fold(0.0f32, f32::max);
        let expected = 0.8 * 0.5 * core::f32::consts::FRAC_1_SQRT_2;
        assert!((peak - expected).abs() < 0.01, "peak {peak} vs {expected}");
    }

    #[test]
    fn variable_callback_sizes_are_sub_blocked() {
        let compiled = compile(
            &state(),
            SR,
            BLOCK,
            &slots(),
            &crate::slots::OutputSlots::with_monitor(),
        );
        let (mut handle, mut engine) = engine_pair(compiled.graph);
        // 3.5 blocks in one callback.
        let frames = BLOCK * 7 / 2;
        let input = sine(0.5, frames);
        let mut output = crate::slots::output_buffer(frames);
        engine.process(&input, 1, &mut output);
        let mut blocks = 0;
        while handle.meter_rx.pop().is_ok() {
            blocks += 1;
        }
        assert_eq!(blocks, 4, "3.5 blocks = 3 full chunks + 1 remainder");
    }

    #[test]
    fn swap_retires_the_old_graph_to_the_control_side() {
        let compiled = compile(
            &state(),
            SR,
            BLOCK,
            &slots(),
            &crate::slots::OutputSlots::with_monitor(),
        );
        let (mut handle, mut engine) = engine_pair(compiled.graph);
        let replacement = compile(
            &state(),
            SR,
            BLOCK,
            &slots(),
            &crate::slots::OutputSlots::with_monitor(),
        );
        handle
            .cmd_tx
            .push(EngineCommand::SwapGraph {
                graph: replacement.graph,
            })
            .unwrap();
        let input = sine(0.5, BLOCK);
        let mut output = crate::slots::output_buffer(BLOCK);
        engine.process(&input, 1, &mut output);
        assert!(
            handle.retire_rx.pop().is_ok(),
            "old graph came back for dropping"
        );
    }

    #[test]
    fn the_process_path_never_allocates_even_across_a_swap() {
        let compiled = compile(
            &state(),
            SR,
            BLOCK,
            &slots(),
            &crate::slots::OutputSlots::with_monitor(),
        );
        let gain_ix = compiled.params.gain[&StripId(0)];
        let (mut handle, mut engine) = engine_pair(compiled.graph);
        let replacement = compile(
            &state(),
            SR,
            BLOCK,
            &slots(),
            &crate::slots::OutputSlots::with_monitor(),
        );
        handle
            .cmd_tx
            .push(EngineCommand::SetParam {
                param: gain_ix,
                target: 0.7,
            })
            .unwrap();
        handle
            .cmd_tx
            .push(EngineCommand::SwapGraph {
                graph: replacement.graph,
            })
            .unwrap();
        // An active tape return rides along: its pops and retire on finish
        // must be allocation-free too.
        let (playback_tx, playback_rx) = RingBuffer::new(BLOCK * 4);
        handle
            .cmd_tx
            .push(EngineCommand::StartPlayback {
                set: Box::new(playback_set(playback_rx)),
            })
            .unwrap();
        handle
            .cmd_tx
            .push(EngineCommand::SetPlaybackSolo { track: 0, on: true })
            .unwrap();
        let mut playback_tx = playback_tx;
        for _ in 0..BLOCK * 4 {
            playback_tx.push(0.25).unwrap();
        }
        drop(playback_tx); // rings drain mid-run, so the finish path is covered
        let input = sine(0.5, BLOCK);
        let mut output = crate::slots::output_buffer(BLOCK);
        assert_no_alloc::assert_no_alloc(|| {
            for _ in 0..8 {
                engine.process(&input, 1, &mut output);
            }
        });
    }

    /// A one-instrument console with the strip patched to its left channel.
    fn instrument_state() -> MixerState {
        let mut strip = StripState::new(StripId(0), "Keys".into());
        strip.input = Some(InputAssign::instrument(InstrumentId(0), 0));
        strip.fader_db = 0.0;
        MixerState {
            strips: vec![strip],
            instruments: vec![InstrumentState::new(InstrumentId(0), "Rhodes".into())],
            ..MixerState::default()
        }
    }

    fn loaded_rack() -> Box<InstrumentRack> {
        let mut bytes = std::io::Cursor::new(crate::instrument::fixture::sine_soundfont());
        let sf = std::sync::Arc::new(rustysynth::SoundFont::new(&mut bytes).unwrap());
        let binding = crate::instrument::MidiBinding {
            port: Some(0),
            channel: None,
        };
        Box::new(InstrumentRack::new(
            vec![Some(
                crate::instrument::Instrument::new(
                    &sf,
                    &crate::instrument::VoiceSpec {
                        sample_rate: SR,
                        polyphony: 32,
                        effects: false,
                        bank: 0,
                        program: 0,
                        binding,
                        keys: crate::instrument::KeyFilter::all(),
                    },
                )
                .unwrap(),
            )],
            vec![2],
        ))
    }

    fn note_on(key: u8) -> MidiEvent {
        MidiEvent {
            port: 0,
            channel: 0,
            status: 0x90,
            data1: key,
            data2: 100,
        }
    }

    // The tracer bullet, end to end inside the engine: a MIDI event enters
    // the ring, the rack synthesises it, and it leaves through a strip in
    // the master mix — with no device, no backend and no hardware anywhere.
    #[test]
    fn a_midi_note_reaches_the_master_mix_through_an_instrument_and_a_strip() {
        let compiled = compile(
            &instrument_state(),
            SR,
            BLOCK,
            &slots(),
            &crate::slots::OutputSlots::with_monitor(),
        );
        let (mut handle, mut engine) = engine_pair(compiled.graph);
        handle
            .cmd_tx
            .push(EngineCommand::SwapRack {
                rack: loaded_rack(),
            })
            .unwrap();
        handle.midi_tx.push(note_on(78)).unwrap();

        let input = vec![0.0; BLOCK];
        let mut output = crate::slots::output_buffer(BLOCK);
        let mut peak = 0.0f32;
        for _ in 0..4 {
            engine.process(&input, 1, &mut output);
            peak = peak.max(output.iter().map(|s| s.abs()).fold(0.0, f32::max));
        }
        assert!(peak > 0.0, "the note never reached the mix");
    }

    #[test]
    fn a_console_with_no_instruments_hears_nothing_from_the_rack() {
        // The rack is loaded but no strip is patched to it: an instrument
        // must not leak into the mix just by existing.
        let compiled = compile(
            &state(),
            SR,
            BLOCK,
            &slots(),
            &crate::slots::OutputSlots::with_monitor(),
        );
        let (mut handle, mut engine) = engine_pair(compiled.graph);
        handle
            .cmd_tx
            .push(EngineCommand::SwapRack {
                rack: loaded_rack(),
            })
            .unwrap();
        handle.midi_tx.push(note_on(78)).unwrap();

        let input = vec![0.0; BLOCK];
        let mut output = crate::slots::output_buffer(BLOCK);
        for _ in 0..4 {
            engine.process(&input, 1, &mut output);
        }
        assert!(output.iter().all(|s| s.abs() < 1e-6));
    }

    #[test]
    fn a_rack_swap_retires_the_old_rack_to_the_control_side() {
        // Freeing a SoundFont on the audio thread is exactly what the
        // retire ring exists to prevent.
        let compiled = compile(
            &instrument_state(),
            SR,
            BLOCK,
            &slots(),
            &crate::slots::OutputSlots::with_monitor(),
        );
        let (mut handle, mut engine) = engine_pair(compiled.graph);
        handle
            .cmd_tx
            .push(EngineCommand::SwapRack {
                rack: loaded_rack(),
            })
            .unwrap();
        let input = vec![0.0; BLOCK];
        let mut output = crate::slots::output_buffer(BLOCK);
        engine.process(&input, 1, &mut output);
        // The first swap replaces the empty default rack.
        assert!(matches!(handle.retire_rx.pop(), Ok(Retired::Rack(_))));

        handle
            .cmd_tx
            .push(EngineCommand::SwapRack {
                rack: loaded_rack(),
            })
            .unwrap();
        engine.process(&input, 1, &mut output);
        assert!(matches!(handle.retire_rx.pop(), Ok(Retired::Rack(_))));
    }

    #[test]
    fn the_process_path_never_allocates_with_a_live_instrument_rack() {
        let compiled = compile(
            &instrument_state(),
            SR,
            BLOCK,
            &slots(),
            &crate::slots::OutputSlots::with_monitor(),
        );
        let (mut handle, mut engine) = engine_pair(compiled.graph);
        handle
            .cmd_tx
            .push(EngineCommand::SwapRack {
                rack: loaded_rack(),
            })
            .unwrap();
        // Install the rack before the guarded run: the swap itself retires
        // a box, which is control-side work the guard would flag.
        let input = vec![0.0; BLOCK];
        let mut output = crate::slots::output_buffer(BLOCK);
        engine.process(&input, 1, &mut output);
        while handle.retire_rx.pop().is_ok() {}

        for key in 60..72u8 {
            handle.midi_tx.push(note_on(key)).unwrap();
        }
        assert_no_alloc::assert_no_alloc(|| {
            for _ in 0..8 {
                engine.process(&input, 1, &mut output);
            }
        });
    }

    fn playback_set(rx: rtrb::Consumer<f32>) -> crate::playback::PlaybackSet {
        crate::playback::PlaybackSet::new(
            vec![crate::playback::PlaybackTrack {
                rx,
                channels: 1,
                is_master: false,
                solo: false,
                mute: false,
                gate: trib_dsp::SmoothedParam::new(0.0, SR),
            }],
            false,
        )
    }

    #[test]
    fn the_tape_return_replaces_the_monitor_pair_and_leaves_direct_outs_alone() {
        // The second isolation invariant. `mix_chunk` zeroes what it cannot
        // fill and strides by two, so handing it a slice of the 64-wide
        // plane would wipe every direct out on every block — the front of
        // house dying because somebody reviewed a take.
        use crate::slots::{MONITOR_CHANNELS, MONITOR_OUT, OutputSlots};
        use trib_core::{OutputJack, OutputPatch, OutputSource, SendTap};

        let mut plane = OutputSlots::with_monitor();
        plane.allocate(Some("outs"), 8).unwrap();
        let mut state = state();
        let mut patch = OutputPatch::new(
            OutputSource::Master,
            0,
            OutputJack {
                device: Some("outs".into()),
                channel: 0,
            },
        );
        patch.tap = SendTap::PostFader;
        state.outputs = vec![patch];

        let compiled = compile(&state, SR, BLOCK, &slots(), &plane);
        let (mut handle, mut engine) = engine_pair(compiled.graph);
        let (mut tx, rx) = RingBuffer::new(BLOCK * 8);
        for _ in 0..BLOCK * 8 {
            tx.push(0.4).unwrap();
        }
        handle
            .cmd_tx
            .push(EngineCommand::StartPlayback {
                set: Box::new(playback_set(rx)),
            })
            .unwrap();

        let input = sine(0.8, BLOCK);
        let mut output = crate::slots::output_buffer(BLOCK);
        for _ in 0..3 {
            engine.process(&input, 1, &mut output);
        }

        let frame = 4;
        let base = frame * MAX_OUTPUT_CHANNELS;
        let take = 0.4 * core::f32::consts::FRAC_1_SQRT_2;
        assert!(
            (output[base + MONITOR_OUT[0] as usize] - take).abs() < 1e-3,
            "the monitor pair carries the take"
        );
        let direct = output[base + usize::from(MONITOR_CHANNELS)];
        assert!(
            direct.abs() > 1e-3 && (direct - take).abs() > 1e-3,
            "the direct out still carries the LIVE mix, not the tape and not \
             silence: got {direct}"
        );
    }

    #[test]
    fn playback_replaces_the_live_mix_and_advances_position() {
        let compiled = compile(
            &state(),
            SR,
            BLOCK,
            &slots(),
            &crate::slots::OutputSlots::with_monitor(),
        );
        let (mut handle, mut engine) = engine_pair(compiled.graph);
        let (mut tx, rx) = RingBuffer::new(BLOCK * 8);
        for _ in 0..BLOCK * 8 {
            tx.push(0.4).unwrap();
        }
        let set = playback_set(rx);
        let shared = set.shared.clone();
        handle
            .cmd_tx
            .push(EngineCommand::StartPlayback { set: Box::new(set) })
            .unwrap();
        let input = sine(0.8, BLOCK);
        let mut output = crate::slots::output_buffer(BLOCK);
        for _ in 0..3 {
            engine.process(&input, 1, &mut output);
        }
        let expected = 0.4 * core::f32::consts::FRAC_1_SQRT_2;
        assert!(
            (output[BLOCK] - expected).abs() < 1e-3,
            "output is the take, not the live sine: {}",
            output[BLOCK]
        );
        assert_eq!(
            shared.position.load(std::sync::atomic::Ordering::Relaxed),
            BLOCK as u64 * 3
        );
    }

    #[test]
    fn the_stream_monitor_keeps_the_live_mix_and_fills_the_ring() {
        let compiled = compile(
            &state(),
            SR,
            BLOCK,
            &slots(),
            &crate::slots::OutputSlots::with_monitor(),
        );
        let (mut handle, mut engine) = engine_pair(compiled.graph);
        let (mut tx, rx) = RingBuffer::new(BLOCK * 8);
        for _ in 0..BLOCK * 8 {
            tx.push(0.4).unwrap();
        }
        handle
            .cmd_tx
            .push(EngineCommand::StartPlayback {
                set: Box::new(playback_set(rx)),
            })
            .unwrap();
        handle
            .cmd_tx
            .push(EngineCommand::SetMonitorTarget {
                target: MonitorTarget::Stream,
            })
            .unwrap();
        let input = sine(0.8, BLOCK);
        let mut output = crate::slots::output_buffer(BLOCK);
        assert_no_alloc::assert_no_alloc(|| {
            for _ in 0..3 {
                engine.process(&input, 1, &mut output);
            }
        });
        let live_peak = output
            .chunks_exact(2)
            .map(|f| f[0].abs())
            .fold(0.0f32, f32::max);
        let expected_live = 0.8 * core::f32::consts::FRAC_1_SQRT_2;
        assert!(
            (live_peak - expected_live).abs() < 0.02,
            "the wire still carries the live mix: {live_peak}"
        );
        let mut streamed = 0;
        let mut stream_peak = 0.0f32;
        while let Ok(sample) = handle.monitor_rx.pop() {
            streamed += 1;
            stream_peak = stream_peak.max(sample.abs());
        }
        assert_eq!(streamed, BLOCK * 3 * 2, "every playback frame hit the ring");
        let expected_stream = 0.4 * core::f32::consts::FRAC_1_SQRT_2;
        assert!(
            (stream_peak - expected_stream).abs() < 1e-3,
            "the ring carries the take: {stream_peak}"
        );
    }

    #[test]
    fn a_finished_playback_retires_itself() {
        let compiled = compile(
            &state(),
            SR,
            BLOCK,
            &slots(),
            &crate::slots::OutputSlots::with_monitor(),
        );
        let (mut handle, mut engine) = engine_pair(compiled.graph);
        let (mut tx, rx) = RingBuffer::new(BLOCK * 2);
        for _ in 0..100 {
            tx.push(0.1).unwrap();
        }
        drop(tx); // the feeder is done: short take
        handle
            .cmd_tx
            .push(EngineCommand::StartPlayback {
                set: Box::new(playback_set(rx)),
            })
            .unwrap();
        let input = sine(0.5, BLOCK);
        let mut output = crate::slots::output_buffer(BLOCK);
        engine.process(&input, 1, &mut output);
        engine.process(&input, 1, &mut output);
        assert!(
            matches!(handle.retire_rx.pop(), Ok(Retired::Playback(_))),
            "the drained set came back for dropping"
        );
        let peak = output.iter().fold(0.0f32, |a, s| a.max(s.abs()));
        assert!(peak > 0.0, "the live mix is back after auto-stop");
    }
}
