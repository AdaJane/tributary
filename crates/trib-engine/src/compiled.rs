//! The compiled graph: `MixerState` flattened into preallocated processors
//! the audio thread can run without allocation, locks, or lookups.
//!
//! Stage order is the cycle rule made physical: strips → buses (groups mix,
//! aux collect sends) → FX (fed by aux, returning to master) → master.
//!
//! Compilation happens control-side. On a topology change the control task
//! compiles a fresh graph (smoothers seeded from the authoritative state,
//! filters and FX tails starting clean) and swaps it in at a block
//! boundary. Mid-gesture smoother positions and filter history are not
//! carried across a swap: structural edits during a fade are rare, and the
//! worst case is one 10 ms re-ramp — revisit only if it's ever audible.

use std::collections::HashMap;

use trib_core::{
    BusId, BusKind, EqBandKind, FxId, FxParams, MeterKey, MixerState, SendTap, StripId,
    db_to_linear,
};
use trib_dsp::{
    BandFilter, Biquad, Coefficients, Delay, MeterAccum, Reverb, SmoothedParam, band_coefficients,
    pan_gains,
};

use crate::rings::{EqIx, FlagIx, MAX_METERS, ParamIx};
use crate::slots::InputSlots;

/// Where a `SetParam` lands. Index in this table IS the `ParamIx`.
#[derive(Debug, Clone, Copy)]
enum ParamSlot {
    StripGain(u16),
    StripFader(u16),
    StripPan(u16),
    StripMute(u16),
    StripSend(u16, u16),
    BusFader(u16),
    BusMute(u16),
    FxRoom(u16),
    FxDamp(u16),
    FxTime(u16),
    FxFeedback(u16),
    FxReturn(u16),
    MasterFader,
}

#[derive(Debug, Clone, Copy)]
enum FlagSlot {
    StripPfl(u16),
    StripEq(u16),
    StripSendPre(u16, u16),
    BusPfl(u16),
}

#[derive(Debug, Clone, Copy)]
struct EqSlot {
    strip: u16,
    band: u8,
}

struct SendSlot {
    /// Index of the destination aux in `buses`.
    bus: u16,
    level: SmoothedParam,
    pre: bool,
}

struct StripNode {
    input_channel: Option<u16>,
    gain: SmoothedParam,
    eq_enabled: bool,
    /// low, mid, high — run in series.
    eq: [BandFilter; 3],
    /// Mute is a smoothed 0/1 gain: click-free by construction.
    mute: SmoothedParam,
    fader: SmoothedParam,
    /// Pan position (-1..1), smoothed in the position domain.
    pan: SmoothedParam,
    pfl: bool,
    /// Main destination: None = master, Some = index into `buses` (a group).
    route: Option<u16>,
    /// One slot per aux bus, always present — a quiet send is just quiet.
    sends: Vec<SendSlot>,
    meter: MeterAccum,
}

struct BusNode {
    kind: BusKind,
    l: Vec<f32>,
    /// Unused (stays zero) for aux buses — sends are mono.
    r: Vec<f32>,
    fader: SmoothedParam,
    mute: SmoothedParam,
    pfl: bool,
}

enum FxProcessor {
    Reverb(Reverb),
    Delay(Delay),
}

struct FxNode {
    /// Index of the feeding aux in `buses`; None = a dangling reference in
    /// a hand-edited manifest, which degrades to silence.
    input: Option<u16>,
    processor: FxProcessor,
    return_level: SmoothedParam,
}

pub struct CompiledGraph {
    block_size: usize,
    strips: Vec<StripNode>,
    buses: Vec<BusNode>,
    fx: Vec<FxNode>,
    master_fader: SmoothedParam,
    master_meter: MeterAccum,
    /// Mix accumulators, preallocated at `block_size`.
    master_l: Vec<f32>,
    master_r: Vec<f32>,
    pfl_bus: Vec<f32>,
    params: Vec<ParamSlot>,
    flags: Vec<FlagSlot>,
    eqs: Vec<EqSlot>,
    /// Meter slots: strip i at index i, master last.
    meter_count: usize,
}

/// Control-side lookup from domain identity to engine dispatch index.
/// Rebuilt on every compile; the old one dies with the old graph.
#[derive(Debug, Default)]
pub struct ParamMap {
    pub gain: HashMap<StripId, ParamIx>,
    pub fader: HashMap<StripId, ParamIx>,
    pub pan: HashMap<StripId, ParamIx>,
    pub mute: HashMap<StripId, ParamIx>,
    pub send: HashMap<(StripId, BusId), ParamIx>,
    pub send_pre: HashMap<(StripId, BusId), FlagIx>,
    pub bus_fader: HashMap<BusId, ParamIx>,
    pub bus_mute: HashMap<BusId, ParamIx>,
    pub bus_pfl: HashMap<BusId, FlagIx>,
    pub fx_room: HashMap<FxId, ParamIx>,
    pub fx_damp: HashMap<FxId, ParamIx>,
    pub fx_time: HashMap<FxId, ParamIx>,
    pub fx_feedback: HashMap<FxId, ParamIx>,
    pub fx_return: HashMap<FxId, ParamIx>,
    pub master_fader: Option<ParamIx>,
    pub pfl: HashMap<StripId, FlagIx>,
    pub eq_enabled: HashMap<StripId, FlagIx>,
    pub eq_band: HashMap<(StripId, EqBandKind), EqIx>,
}

pub struct CompileOutput {
    pub graph: Box<CompiledGraph>,
    pub params: ParamMap,
    /// `meter_keys[i]` names meter slot i of every `MeterBlock` this graph
    /// emits — the pump's decoding table.
    pub meter_keys: Vec<MeterKey>,
}

/// Flatten the document into a runnable graph. Pure with respect to IO.
pub fn compile(
    state: &MixerState,
    sample_rate: u32,
    block_size: usize,
    slots: &InputSlots,
) -> CompileOutput {
    let mut params = Vec::new();
    let mut flags = Vec::new();
    let mut eqs = Vec::new();
    let mut map = ParamMap::default();

    let bus_index: HashMap<BusId, u16> = state
        .buses
        .iter()
        .enumerate()
        .map(|(i, b)| (b.id, i as u16))
        .collect();
    let aux_buses: Vec<(BusId, u16)> = state
        .buses
        .iter()
        .filter(|b| b.kind == BusKind::Aux)
        .map(|b| (b.id, bus_index[&b.id]))
        .collect();

    let strips: Vec<StripNode> = state
        .strips
        .iter()
        .enumerate()
        .map(|(i, strip)| {
            let ix = i as u16;
            map.gain
                .insert(strip.id, push_param(&mut params, ParamSlot::StripGain(ix)));
            map.fader
                .insert(strip.id, push_param(&mut params, ParamSlot::StripFader(ix)));
            map.pan
                .insert(strip.id, push_param(&mut params, ParamSlot::StripPan(ix)));
            map.mute
                .insert(strip.id, push_param(&mut params, ParamSlot::StripMute(ix)));
            map.pfl
                .insert(strip.id, push_flag(&mut flags, FlagSlot::StripPfl(ix)));
            map.eq_enabled
                .insert(strip.id, push_flag(&mut flags, FlagSlot::StripEq(ix)));
            for (b, kind) in EqBandKind::ALL.iter().enumerate() {
                eqs.push(EqSlot {
                    strip: ix,
                    band: b as u8,
                });
                map.eq_band
                    .insert((strip.id, *kind), EqIx((eqs.len() - 1) as u16));
            }
            let sends = aux_buses
                .iter()
                .enumerate()
                .map(|(send_ix, (bus_id, bus_ix))| {
                    map.send.insert(
                        (strip.id, *bus_id),
                        push_param(&mut params, ParamSlot::StripSend(ix, send_ix as u16)),
                    );
                    map.send_pre.insert(
                        (strip.id, *bus_id),
                        push_flag(&mut flags, FlagSlot::StripSendPre(ix, send_ix as u16)),
                    );
                    let existing = strip.sends.iter().find(|s| s.dest == *bus_id);
                    SendSlot {
                        bus: *bus_ix,
                        level: SmoothedParam::new(
                            existing.map_or(0.0, |s| db_to_linear(s.level_db)),
                            sample_rate,
                        ),
                        pre: existing.is_some_and(|s| s.tap == SendTap::PreFader),
                    }
                })
                .collect();
            StripNode {
                // Device+channel resolves to a flat frame index here, at
                // compile time — unknown devices land on the silence path.
                input_channel: strip
                    .input
                    .as_ref()
                    .and_then(|a| slots.resolve(a.device.as_deref(), a.device_channel)),
                gain: SmoothedParam::new(db_to_linear(strip.gain_db), sample_rate),
                eq_enabled: strip.eq.enabled,
                eq: [
                    BandFilter::new(band_coefficients(&strip.eq.low, sample_rate)),
                    BandFilter::new(band_coefficients(&strip.eq.mid, sample_rate)),
                    BandFilter::new(band_coefficients(&strip.eq.high, sample_rate)),
                ],
                mute: SmoothedParam::new(if strip.mute { 0.0 } else { 1.0 }, sample_rate),
                fader: SmoothedParam::new(db_to_linear(strip.fader_db), sample_rate),
                pan: SmoothedParam::new(strip.pan, sample_rate),
                pfl: strip.pfl,
                route: match strip.route_to {
                    trib_core::RouteTarget::Master => None,
                    trib_core::RouteTarget::Bus { id } => bus_index.get(&id).copied(),
                },
                sends,
                meter: MeterAccum::default(),
            }
        })
        .collect();

    let buses: Vec<BusNode> = state
        .buses
        .iter()
        .enumerate()
        .map(|(i, bus)| {
            let ix = i as u16;
            map.bus_fader
                .insert(bus.id, push_param(&mut params, ParamSlot::BusFader(ix)));
            map.bus_mute
                .insert(bus.id, push_param(&mut params, ParamSlot::BusMute(ix)));
            map.bus_pfl
                .insert(bus.id, push_flag(&mut flags, FlagSlot::BusPfl(ix)));
            BusNode {
                kind: bus.kind,
                l: vec![0.0; block_size],
                r: vec![0.0; block_size],
                fader: SmoothedParam::new(db_to_linear(bus.fader_db), sample_rate),
                mute: SmoothedParam::new(if bus.mute { 0.0 } else { 1.0 }, sample_rate),
                pfl: bus.pfl,
            }
        })
        .collect();

    let fx: Vec<FxNode> = state
        .fx
        .iter()
        .enumerate()
        .map(|(i, unit)| {
            let ix = i as u16;
            map.fx_return
                .insert(unit.id, push_param(&mut params, ParamSlot::FxReturn(ix)));
            let processor = match unit.params {
                FxParams::Reverb { room_size, damping } => {
                    map.fx_room
                        .insert(unit.id, push_param(&mut params, ParamSlot::FxRoom(ix)));
                    map.fx_damp
                        .insert(unit.id, push_param(&mut params, ParamSlot::FxDamp(ix)));
                    FxProcessor::Reverb(Reverb::new(room_size, damping, sample_rate))
                }
                FxParams::Delay { time_ms, feedback } => {
                    map.fx_time
                        .insert(unit.id, push_param(&mut params, ParamSlot::FxTime(ix)));
                    map.fx_feedback
                        .insert(unit.id, push_param(&mut params, ParamSlot::FxFeedback(ix)));
                    FxProcessor::Delay(Delay::new(time_ms, feedback, sample_rate))
                }
            };
            FxNode {
                input: bus_index.get(&unit.input).copied(),
                processor,
                return_level: SmoothedParam::new(db_to_linear(unit.return_level_db), sample_rate),
            }
        })
        .collect();

    map.master_fader = Some(push_param(&mut params, ParamSlot::MasterFader));

    let mut meter_keys: Vec<MeterKey> = state
        .strips
        .iter()
        .map(|s| MeterKey::Strip { id: s.id })
        .collect();
    meter_keys.push(MeterKey::Master);
    let meter_count = meter_keys.len().min(MAX_METERS);

    let graph = Box::new(CompiledGraph {
        block_size,
        strips,
        buses,
        fx,
        master_fader: SmoothedParam::new(db_to_linear(state.master.fader_db), sample_rate),
        master_meter: MeterAccum::default(),
        master_l: vec![0.0; block_size],
        master_r: vec![0.0; block_size],
        pfl_bus: vec![0.0; block_size],
        params,
        flags,
        eqs,
        meter_count,
    });
    CompileOutput {
        graph,
        params: map,
        meter_keys,
    }
}

fn push_param(table: &mut Vec<ParamSlot>, slot: ParamSlot) -> ParamIx {
    table.push(slot);
    ParamIx((table.len() - 1) as u16)
}

fn push_flag(table: &mut Vec<FlagSlot>, slot: FlagSlot) -> FlagIx {
    table.push(slot);
    FlagIx((table.len() - 1) as u16)
}

impl CompiledGraph {
    pub fn block_size(&self) -> usize {
        self.block_size
    }

    pub fn meter_count(&self) -> usize {
        self.meter_count
    }

    /// Stale indices from a superseded compile are ignored, not corrupted:
    /// commands racing a swap may address the old layout for one block.
    pub fn set_param(&mut self, param: ParamIx, target: f32) {
        let Some(slot) = self.params.get(param.0 as usize) else {
            return;
        };
        match *slot {
            ParamSlot::StripGain(i) => self.strips[i as usize].gain.set_target(target),
            ParamSlot::StripFader(i) => self.strips[i as usize].fader.set_target(target),
            ParamSlot::StripPan(i) => self.strips[i as usize].pan.set_target(target),
            ParamSlot::StripMute(i) => self.strips[i as usize].mute.set_target(target),
            ParamSlot::StripSend(i, s) => {
                self.strips[i as usize].sends[s as usize]
                    .level
                    .set_target(target);
            }
            ParamSlot::BusFader(i) => self.buses[i as usize].fader.set_target(target),
            ParamSlot::BusMute(i) => self.buses[i as usize].mute.set_target(target),
            ParamSlot::FxRoom(i) => {
                if let FxProcessor::Reverb(reverb) = &mut self.fx[i as usize].processor {
                    reverb.set_room_size(target);
                }
            }
            ParamSlot::FxDamp(i) => {
                if let FxProcessor::Reverb(reverb) = &mut self.fx[i as usize].processor {
                    reverb.set_damping(target);
                }
            }
            ParamSlot::FxTime(i) => {
                if let FxProcessor::Delay(delay) = &mut self.fx[i as usize].processor {
                    delay.set_time_ms(target);
                }
            }
            ParamSlot::FxFeedback(i) => {
                if let FxProcessor::Delay(delay) = &mut self.fx[i as usize].processor {
                    delay.set_feedback(target);
                }
            }
            ParamSlot::FxReturn(i) => self.fx[i as usize].return_level.set_target(target),
            ParamSlot::MasterFader => self.master_fader.set_target(target),
        }
    }

    pub fn set_flag(&mut self, flag: FlagIx, on: bool) {
        let Some(slot) = self.flags.get(flag.0 as usize) else {
            return;
        };
        match *slot {
            FlagSlot::StripPfl(i) => self.strips[i as usize].pfl = on,
            FlagSlot::StripEq(i) => self.strips[i as usize].eq_enabled = on,
            FlagSlot::StripSendPre(i, s) => self.strips[i as usize].sends[s as usize].pre = on,
            FlagSlot::BusPfl(i) => self.buses[i as usize].pfl = on,
        }
    }

    pub fn set_eq_coeffs(&mut self, eq: EqIx, coeffs: Coefficients<f32>) {
        let Some(slot) = self.eqs.get(eq.0 as usize) else {
            return;
        };
        self.strips[slot.strip as usize].eq[slot.band as usize].update_coefficients(coeffs);
    }

    /// Process one chunk of at most `block_size` frames. `input` is
    /// interleaved with `input_channels`; `output` is interleaved stereo.
    /// A live `record` set receives dry strip taps and the stereo mix.
    pub fn process_chunk(
        &mut self,
        input: &[f32],
        input_channels: usize,
        output: &mut [f32],
        peaks: &mut [f32; MAX_METERS],
        clip_bits: &mut u64,
        mut record: Option<&mut crate::record::RecordSet>,
    ) {
        let frames = output.len() / 2;
        debug_assert!(frames <= self.block_size);

        self.master_l[..frames].fill(0.0);
        self.master_r[..frames].fill(0.0);
        self.pfl_bus[..frames].fill(0.0);
        for bus in &mut self.buses {
            bus.l[..frames].fill(0.0);
            bus.r[..frames].fill(0.0);
        }
        let mut any_pfl = false;

        // Stage 1: strips into master/groups/aux.
        for (strip_ix, strip) in self.strips.iter_mut().enumerate() {
            let channel = strip.input_channel.map(usize::from);
            if strip.pfl {
                any_pfl = true;
            }
            let track = record.as_deref_mut().and_then(|rec| {
                rec.strip_tracks
                    .get(strip_ix)
                    .copied()
                    .flatten()
                    .map(usize::from)
            });
            for i in 0..frames {
                let dry = match channel {
                    Some(c) if c < input_channels => input[i * input_channels + c],
                    _ => 0.0,
                };
                let mut x = dry * strip.gain.tick();
                if strip.eq_enabled {
                    x = strip.eq[0].run(x);
                    x = strip.eq[1].run(x);
                    x = strip.eq[2].run(x);
                }
                // Meter tap: post-gain post-EQ, pre-mute/fader — the level
                // check reading (what PFL listens to).
                strip.meter.accumulate(x);
                // Record tap sits at the same point: dry takes.
                if let Some(track_ix) = track
                    && let Some(rec) = record.as_deref_mut()
                {
                    rec.tracks[track_ix].push(x);
                }
                if strip.pfl {
                    self.pfl_bus[i] += x;
                }
                let level = x * strip.mute.tick() * strip.fader.tick();
                for send in &mut strip.sends {
                    let tap = if send.pre { x } else { level };
                    self.buses[send.bus as usize].l[i] += tap * send.level.tick();
                }
                let (l, r) = pan_gains(strip.pan.tick());
                match strip.route {
                    None => {
                        self.master_l[i] += level * l;
                        self.master_r[i] += level * r;
                    }
                    Some(bus) => {
                        let dest = &mut self.buses[bus as usize];
                        dest.l[i] += level * l;
                        dest.r[i] += level * r;
                    }
                }
            }
        }

        // Stage 2: buses. Groups mix into the master; aux scale in place
        // (their scaled signal is what feeds the FX stage).
        for bus in &mut self.buses {
            if bus.pfl {
                any_pfl = true;
            }
            for i in 0..frames {
                if bus.pfl {
                    // Pre-fade listen on the bus sum.
                    self.pfl_bus[i] += (bus.l[i] + bus.r[i]) * 0.5;
                }
                let level = bus.fader.tick() * bus.mute.tick();
                match bus.kind {
                    BusKind::Group => {
                        self.master_l[i] += bus.l[i] * level;
                        self.master_r[i] += bus.r[i] * level;
                    }
                    BusKind::Aux => {
                        bus.l[i] *= level;
                    }
                }
            }
        }

        // Stage 3: FX — aux in, wet return to the master, center.
        for fx in &mut self.fx {
            let Some(input_ix) = fx.input else { continue };
            for i in 0..frames {
                let dry = self.buses[input_ix as usize].l[i];
                let wet = match &mut fx.processor {
                    FxProcessor::Reverb(reverb) => reverb.run(dry),
                    FxProcessor::Delay(delay) => delay.run(dry),
                } * fx.return_level.tick();
                self.master_l[i] += wet;
                self.master_r[i] += wet;
            }
        }

        // Stage 4: master out (PFL replaces the feed, meters keep the mix).
        let master_track = record
            .as_deref_mut()
            .and_then(|rec| rec.master_track.map(usize::from));
        for i in 0..frames {
            let mf = self.master_fader.tick();
            let l = self.master_l[i] * mf;
            let r = self.master_r[i] * mf;
            self.master_meter
                .accumulate(if l.abs() > r.abs() { l } else { r });
            // The mix records post-fader — the take is what the room heard
            // (PFL is a monitoring convenience and never lands on tape).
            if let Some(track_ix) = master_track
                && let Some(rec) = record.as_deref_mut()
            {
                rec.tracks[track_ix].push(l);
                rec.tracks[track_ix].push(r);
            }
            if any_pfl {
                output[2 * i] = self.pfl_bus[i];
                output[2 * i + 1] = self.pfl_bus[i];
            } else {
                output[2 * i] = l;
                output[2 * i + 1] = r;
            }
        }

        for (i, strip) in self.strips.iter_mut().enumerate() {
            if i >= MAX_METERS - 1 {
                break;
            }
            let (peak, clipped) = strip.meter.take();
            peaks[i] = peaks[i].max(peak);
            *clip_bits |= u64::from(clipped) << i;
        }
        let master_ix = self.meter_count - 1;
        let (peak, clipped) = self.master_meter.take();
        peaks[master_ix] = peaks[master_ix].max(peak);
        *clip_bits |= u64::from(clipped) << master_ix;
    }
}

#[cfg(test)]
mod tests {
    use trib_core::{
        BusState, FaderTarget, FxState, InputAssign, MixCommand, RouteTarget, SendState,
        StripState, apply,
    };

    use super::*;

    const SR: u32 = 48_000;
    const BLOCK: usize = 256;

    fn slots() -> InputSlots {
        InputSlots::single_default(1)
    }

    fn state_with_input() -> MixerState {
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

    fn state_with_fx() -> MixerState {
        let mut state = state_with_input();
        state
            .buses
            .push(BusState::new(BusId(0), BusKind::Aux, "FX 1".into()));
        state.fx.push(FxState {
            id: FxId(0),
            name: "Echo".into(),
            input: BusId(0),
            params: FxParams::Delay {
                time_ms: 100.0,
                feedback: 0.0,
            },
            return_level_db: 0.0,
        });
        state
    }

    fn sine(amplitude: f32) -> Vec<f32> {
        (0..BLOCK)
            .map(|i| amplitude * (i as f32 / SR as f32 * 440.0 * std::f32::consts::TAU).sin())
            .collect()
    }

    fn run(
        graph: &mut CompiledGraph,
        input: &[f32],
        blocks: usize,
    ) -> (Vec<f32>, [f32; MAX_METERS], u64) {
        let mut output = vec![0.0; input.len() * 2];
        let mut peaks = [0.0; MAX_METERS];
        let mut clips = 0;
        for _ in 0..blocks {
            peaks = [0.0; MAX_METERS];
            clips = 0;
            graph.process_chunk(input, 1, &mut output, &mut peaks, &mut clips, None);
        }
        (output, peaks, clips)
    }

    fn peak_of(output: &[f32], side: usize) -> f32 {
        output
            .chunks_exact(2)
            .map(|f| f[side].abs())
            .fold(0.0f32, f32::max)
    }

    #[test]
    fn a_strip_on_a_second_device_reads_its_slot_offset() {
        let mut slots = InputSlots::default();
        slots.allocate(None, 2).unwrap();
        slots.allocate(Some("dock"), 2).unwrap();
        let mut state = state_with_input();
        state.strips[0].input = Some(InputAssign {
            device: Some("dock".into()),
            device_channel: 1,
        });
        let mut out = compile(&state, SR, BLOCK, &slots);
        // Four-channel frame: only flat channel 3 (dock ch 1) carries signal.
        let mut input = vec![0.0; BLOCK * 4];
        for frame in input.chunks_exact_mut(4) {
            frame[3] = 0.5;
        }
        let mut output = vec![0.0; BLOCK * 2];
        let mut peaks = [0.0; MAX_METERS];
        let mut clips = 0;
        for _ in 0..3 {
            peaks = [0.0; MAX_METERS];
            out.graph
                .process_chunk(&input, 4, &mut output, &mut peaks, &mut clips, None);
        }
        assert!(
            (peaks[0] - 0.5).abs() < 0.01,
            "the dock channel reached the strip"
        );
    }

    #[test]
    fn a_patch_to_an_unknown_device_is_silent() {
        let mut state = state_with_input();
        state.strips[0].input = Some(InputAssign {
            device: Some("unplugged interface".into()),
            device_channel: 0,
        });
        let mut out = compile(&state, SR, BLOCK, &slots());
        let (output, peaks, _) = run(&mut out.graph, &sine(0.5), 3);
        assert!(peak_of(&output, 0) < 1e-6);
        assert!(peaks[0] < 1e-6, "no meter movement either");
    }

    #[test]
    fn compile_maps_every_strip_control() {
        let out = compile(&state_with_input(), SR, BLOCK, &slots());
        assert!(out.params.gain.contains_key(&StripId(0)));
        assert!(
            out.params
                .eq_band
                .contains_key(&(StripId(0), EqBandKind::Peak))
        );
        assert_eq!(out.meter_keys.len(), 2, "strip meter + master");
        assert_eq!(out.meter_keys[1], MeterKey::Master);
    }

    #[test]
    fn a_unity_strip_reaches_the_master_at_pan_law_level() {
        let mut out = compile(&state_with_input(), SR, BLOCK, &slots());
        let input = sine(0.5);
        let (output, peaks, _) = run(&mut out.graph, &input, 3);
        assert!((peak_of(&output, 0) - 0.5 * core::f32::consts::FRAC_1_SQRT_2).abs() < 0.01);
        assert!((peaks[0] - 0.5).abs() < 0.01, "strip meter reads pre-fader");
    }

    #[test]
    fn hard_left_pan_leaves_the_right_bus_silent() {
        let mut state = state_with_input();
        state.strips[0].pan = -1.0;
        let mut out = compile(&state, SR, BLOCK, &slots());
        let (output, _, _) = run(&mut out.graph, &sine(0.5), 3);
        assert!(peak_of(&output, 1) < 1e-3);
    }

    #[test]
    fn eq_disabled_bypasses_the_filters() {
        let mut state = state_with_input();
        state.strips[0].eq.enabled = true; // defaults are OFF now
        state.strips[0].eq.high.gain_db = -15.0;
        state.strips[0].eq.high.freq_hz = 220.0;
        let mut cut = compile(&state, SR, BLOCK, &slots());
        let (_, peaks_cut, _) = run(&mut cut.graph, &sine(0.5), 4);
        state.strips[0].eq.enabled = false;
        let mut flat = compile(&state, SR, BLOCK, &slots());
        let (_, peaks_flat, _) = run(&mut flat.graph, &sine(0.5), 4);
        assert!(peaks_cut[0] < peaks_flat[0] * 0.5);
    }

    #[test]
    fn mute_command_silences_after_the_ramp() {
        let mut out = compile(&state_with_input(), SR, BLOCK, &slots());
        let mute_ix = out.params.mute[&StripId(0)];
        out.graph.set_param(mute_ix, 0.0);
        let (output, _, _) = run(&mut out.graph, &sine(0.8), 4);
        assert!(output.iter().all(|&s| s.abs() < 1e-3));
    }

    #[test]
    fn pfl_replaces_the_output_but_the_meters_keep_showing_the_mix() {
        let mut state = state_with_input();
        state.strips[0].fader_db = -90.0;
        state.strips[0].pfl = true;
        let mut out = compile(&state, SR, BLOCK, &slots());
        let (output, peaks, _) = run(&mut out.graph, &sine(0.5), 3);
        let out_peak = output.iter().fold(0.0f32, |a, &s| a.max(s.abs()));
        assert!((out_peak - 0.5).abs() < 0.01, "PFL is pre-fader listen");
        let master_ix = out.graph.meter_count() - 1;
        assert!(peaks[master_ix] < 1e-3, "mix meter stays down");
    }

    #[test]
    fn a_group_routed_strip_reaches_the_master_through_the_group_fader() {
        let mut state = state_with_input();
        state
            .buses
            .push(BusState::new(BusId(0), BusKind::Group, "Band".into()));
        state.strips[0].route_to = RouteTarget::Bus { id: BusId(0) };
        state.buses[0].fader_db = -6.0;
        let mut out = compile(&state, SR, BLOCK, &slots());
        let (output, _, _) = run(&mut out.graph, &sine(0.5), 3);
        let expected = 0.5 * core::f32::consts::FRAC_1_SQRT_2 * db_to_linear(-6.0);
        assert!((peak_of(&output, 0) - expected).abs() < 0.01);
    }

    #[test]
    fn a_post_fader_send_feeds_the_delay_and_returns_to_the_master() {
        let mut state = state_with_fx();
        state.strips[0].sends.push(SendState {
            dest: BusId(0),
            level_db: 0.0,
            tap: SendTap::PostFader,
        });
        let mut out = compile(&state, SR, BLOCK, &slots());
        // Impulse block, then silence: the echo must appear later.
        let mut impulse = vec![0.0f32; BLOCK];
        impulse[0] = 0.8;
        let silence = vec![0.0f32; BLOCK];
        let mut output = vec![0.0; BLOCK * 2];
        let mut peaks = [0.0; MAX_METERS];
        let mut clips = 0;
        out.graph
            .process_chunk(&impulse, 1, &mut output, &mut peaks, &mut clips, None);
        // 100 ms at 48k = 4800 samples ≈ 19 blocks. Echo lands in block 18.
        let mut echo_peak = 0.0f32;
        for _ in 0..20 {
            out.graph
                .process_chunk(&silence, 1, &mut output, &mut peaks, &mut clips, None);
            echo_peak = echo_peak.max(peak_of(&output, 0));
        }
        assert!(echo_peak > 0.3, "the echo came back ({echo_peak})");
    }

    #[test]
    fn a_pre_fader_send_survives_the_fader_being_down() {
        let mut state = state_with_fx();
        state.strips[0].fader_db = -90.0; // fader down…
        state.strips[0].sends.push(SendState {
            dest: BusId(0),
            level_db: 0.0,
            tap: SendTap::PreFader, // …but the send taps pre-fader
        });
        let mut out = compile(&state, SR, BLOCK, &slots());
        let mut impulse = vec![0.0f32; BLOCK];
        impulse[0] = 0.8;
        let silence = vec![0.0f32; BLOCK];
        let mut output = vec![0.0; BLOCK * 2];
        let mut peaks = [0.0; MAX_METERS];
        let mut clips = 0;
        out.graph
            .process_chunk(&impulse, 1, &mut output, &mut peaks, &mut clips, None);
        let mut echo_peak = 0.0f32;
        for _ in 0..20 {
            out.graph
                .process_chunk(&silence, 1, &mut output, &mut peaks, &mut clips, None);
            echo_peak = echo_peak.max(peak_of(&output, 0));
        }
        assert!(echo_peak > 0.3, "pre-fader send still fed the FX");
    }

    #[test]
    fn a_reducer_edit_recompiles_into_a_consistent_graph() {
        let state = state_with_input();
        let (next, _, _) = apply(
            &state,
            MixCommand::SetFader {
                target: FaderTarget::Strip { id: StripId(0) },
                level_db: -6.0,
            },
        )
        .unwrap();
        let mut out = compile(&next, SR, BLOCK, &slots());
        let (output, _, _) = run(&mut out.graph, &sine(0.5), 3);
        let expected = 0.5 * db_to_linear(-6.0) * core::f32::consts::FRAC_1_SQRT_2;
        assert!((peak_of(&output, 0) - expected).abs() < 0.01);
    }

    #[test]
    fn stale_dispatch_indices_are_ignored_not_fatal() {
        let mut out = compile(&state_with_input(), SR, BLOCK, &slots());
        out.graph.set_param(ParamIx(999), 1.0);
        out.graph.set_flag(FlagIx(999), true);
    }
}
