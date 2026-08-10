//! The audio-thread engine: drains commands, runs the compiled graph,
//! pushes telemetry, and returns retired graphs to the control side for
//! dropping. Grown from the M1 tracer; the boundary contracts are the same.

use std::sync::atomic::Ordering;

use rtrb::{Consumer, Producer, RingBuffer};

use crate::compiled::CompiledGraph;
use crate::playback::PlaybackSet;
use crate::record::RecordSet;
use crate::rings::{
    CMD_RING_CAPACITY, EngineCommand, MAX_METERS, METER_RING_CAPACITY, MONITOR_RING_CAPACITY,
    MeterBlock, MonitorTarget, RETIRE_RING_CAPACITY, Retired,
};

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
}

/// The audio side. `process` runs on the audio callback: no allocation, no
/// locks, no syscalls.
pub struct GraphEngine {
    graph: Box<CompiledGraph>,
    record: Option<Box<RecordSet>>,
    playback: Option<Box<PlaybackSet>>,
    monitor: MonitorTarget,
    /// Scratch for the stream branch — preallocated, one block of stereo.
    monitor_buf: Vec<f32>,
    monitor_tx: Producer<f32>,
    /// Retirees waiting for ring space. Preallocated; never grows.
    parked: Vec<Retired>,
    frame: u64,
    cmd_rx: Consumer<EngineCommand>,
    meter_tx: Producer<MeterBlock>,
    retire_tx: Producer<Retired>,
}

pub fn engine_pair(initial: Box<CompiledGraph>) -> (EngineHandle, GraphEngine) {
    let (cmd_tx, cmd_rx) = RingBuffer::new(CMD_RING_CAPACITY);
    let (meter_tx, meter_rx) = RingBuffer::new(METER_RING_CAPACITY);
    let (retire_tx, retire_rx) = RingBuffer::new(RETIRE_RING_CAPACITY);
    let (monitor_tx, monitor_rx) = RingBuffer::new(MONITOR_RING_CAPACITY);
    let monitor_buf = vec![0.0; initial.block_size() * 2];
    (
        EngineHandle {
            cmd_tx,
            meter_rx,
            retire_rx,
            monitor_rx,
        },
        GraphEngine {
            graph: initial,
            record: None,
            playback: None,
            monitor: MonitorTarget::Hardware,
            monitor_buf,
            monitor_tx,
            parked: Vec::with_capacity(MAX_PARKED),
            frame: 0,
            cmd_rx,
            meter_tx,
            retire_tx,
        },
    )
}

impl GraphEngine {
    /// One callback's work. `input` is interleaved `input_channels`;
    /// `output` interleaved stereo. Arbitrary callback sizes are handled by
    /// sub-blocking at the compiled block size.
    pub fn process(&mut self, input: &[f32], input_channels: usize, output: &mut [f32]) {
        self.drain_commands();
        self.flush_parked();

        let block = self.graph.block_size();
        let frames = output.len() / 2;
        debug_assert_eq!(input.len(), frames * input_channels);

        let mut done = 0;
        while done < frames {
            let chunk = (frames - done).min(block);
            let mut peaks = [0.0f32; MAX_METERS];
            let mut clip_bits = 0u64;
            self.graph.process_chunk(
                &input[done * input_channels..(done + chunk) * input_channels],
                input_channels,
                &mut output[done * 2..(done + chunk) * 2],
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
            // mix (the PFL precedent one stage up); on the stream monitor it
            // fills the browser ring and the wire keeps the live mix. Meters
            // always show the live mix.
            let mut finished = false;
            if let Some(playback) = &mut self.playback {
                let popped = match self.monitor {
                    MonitorTarget::Hardware => {
                        playback.mix_chunk(&mut output[done * 2..(done + chunk) * 2])
                    }
                    MonitorTarget::Stream => {
                        let buf = &mut self.monitor_buf[..chunk * 2];
                        let popped = playback.mix_chunk(buf);
                        for &sample in &buf[..popped * 2] {
                            // Full ring = a stalled pump; stream audio is
                            // droppable by doctrine.
                            let _ = self.monitor_tx.push(sample);
                        }
                        popped
                    }
                };
                playback
                    .shared
                    .position
                    .fetch_add(popped as u64, Ordering::Relaxed);
                finished = playback.shared.finished.load(Ordering::Relaxed);
            }
            if finished && let Some(set) = self.playback.take() {
                self.retire(Retired::Playback(set));
            }
            self.frame += 1;
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
                Ok(EngineCommand::StartRecord { set }) => {
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
    use trib_core::{InputAssign, MixerState, StripId, StripState};

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
        strip.input = Some(InputAssign {
            device: None,
            device_channel: 0,
        });
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
        let compiled = compile(&state(), SR, BLOCK, &slots());
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
        let mut output = vec![0.0; BLOCK * 2];
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
        let compiled = compile(&state(), SR, BLOCK, &slots());
        let (mut handle, mut engine) = engine_pair(compiled.graph);
        // 3.5 blocks in one callback.
        let frames = BLOCK * 7 / 2;
        let input = sine(0.5, frames);
        let mut output = vec![0.0; frames * 2];
        engine.process(&input, 1, &mut output);
        let mut blocks = 0;
        while handle.meter_rx.pop().is_ok() {
            blocks += 1;
        }
        assert_eq!(blocks, 4, "3.5 blocks = 3 full chunks + 1 remainder");
    }

    #[test]
    fn swap_retires_the_old_graph_to_the_control_side() {
        let compiled = compile(&state(), SR, BLOCK, &slots());
        let (mut handle, mut engine) = engine_pair(compiled.graph);
        let replacement = compile(&state(), SR, BLOCK, &slots());
        handle
            .cmd_tx
            .push(EngineCommand::SwapGraph {
                graph: replacement.graph,
            })
            .unwrap();
        let input = sine(0.5, BLOCK);
        let mut output = vec![0.0; BLOCK * 2];
        engine.process(&input, 1, &mut output);
        assert!(
            handle.retire_rx.pop().is_ok(),
            "old graph came back for dropping"
        );
    }

    #[test]
    fn the_process_path_never_allocates_even_across_a_swap() {
        let compiled = compile(&state(), SR, BLOCK, &slots());
        let gain_ix = compiled.params.gain[&StripId(0)];
        let (mut handle, mut engine) = engine_pair(compiled.graph);
        let replacement = compile(&state(), SR, BLOCK, &slots());
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
        let mut output = vec![0.0; BLOCK * 2];
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
    fn playback_replaces_the_live_mix_and_advances_position() {
        let compiled = compile(&state(), SR, BLOCK, &slots());
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
        let mut output = vec![0.0; BLOCK * 2];
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
        let compiled = compile(&state(), SR, BLOCK, &slots());
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
        let mut output = vec![0.0; BLOCK * 2];
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
        let compiled = compile(&state(), SR, BLOCK, &slots());
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
        let mut output = vec![0.0; BLOCK * 2];
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
