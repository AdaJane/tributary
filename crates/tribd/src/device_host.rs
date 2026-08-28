//! The device orchestrator: a dedicated thread that owns the audio
//! backend's stream handle, the input slot map, and the device state
//! machine. Everything it does may block (enumeration, opening hardware),
//! which is exactly why it is NOT the control task — the control task
//! sends it wanted-set diffs and receives slot maps back.
//!
//! Refresh (user-initiated, no background retry): re-enumerate, re-run
//! name reconciliation, close-and-release failed streams, retry every
//! wanted-but-unopened device.
//!
//! One exception, and it is the backend's, not a device's: when the
//! backend's OWN device set changes — the exclusive layer's card coming
//! up after a boot without it, or coming back after a death — the
//! orchestrator heals as if Refresh had been pressed. It learns that from
//! a generation counter the backend bumps, polled once a second without
//! enumerating, so a headless appliance recovers with nobody at the
//! console. A device that failed to open on a healthy backend still waits
//! for Refresh, exactly as before.

use std::collections::{BTreeSet, HashMap};
use std::sync::Arc;
use std::sync::mpsc::{Receiver, Sender};

use serde::Serialize;
use tokio::sync::oneshot;
use trib_audio::{
    AudioBackend, InputDeviceInfo, OpenInput, OpenOutput, OutputDeviceInfo, StreamHandle,
};
use trib_engine::{InputSlots, OutputSlots};
use utoipa::ToSchema;

use crate::device_match::reconcile_names;
use crate::engine_host::ControlHandle;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum DeviceStatus {
    /// Stream running, feeding its slot.
    Open,
    /// Present but nothing patched to it.
    Available,
    /// Wanted, but the stream failed to open or errored since.
    Failed,
    /// Wanted (a strip references it) but not on this system right now.
    Absent,
}

/// One profile a device's card can be switched into.
#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct ProfileReport {
    /// `pro-audio` — what a change request names.
    pub name: String,
    /// "Pro Audio" — what the system sound panel prints.
    pub description: String,
}

/// One row of the patchbay device document.
#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct DeviceReport {
    pub name: String,
    /// The friendly print the system sound menu shows, when the source
    /// layer provides one.
    pub label: Option<String>,
    /// What the device exposes RIGHT NOW — a property of the card's active
    /// profile, not of the hardware. 0 = unknown (an absent device was
    /// never enumerated this boot).
    pub channels: u16,
    /// The OS default input — what `device: null` patches feed from.
    pub active: bool,
    pub status: DeviceStatus,
    /// Some strip references it (directly, via the default, or via an alias).
    pub patched: bool,
    pub underruns: u64,
    /// Whole frames dropped because the engine wasn't draining fast enough.
    pub overruns: u64,
    /// The card this device belongs to. A profile change names the CARD.
    pub card: Option<String>,
    /// The card's active profile, when it has one.
    pub profile: Option<String>,
    /// Profiles this device's card can be switched into. More than one
    /// means the channel count above is a choice, not a hardware limit.
    pub profiles: Vec<ProfileReport>,
    /// The source's channel positions ("aux0,aux1,…"). NOT decoration:
    /// the capture opens in exactly these positions, because the server
    /// routes by position name and will synthesise a surround map that
    /// drops most of a pro-audio source's inputs otherwise.
    pub channel_map: Option<String>,
    /// The source layer muted this input. It still opens, still streams,
    /// and every meter fed from it reads silence — so a dead meter has an
    /// explanation here that exists nowhere else in the report.
    pub muted: bool,
    /// The quietest channel's volume as a percentage of unity, when the
    /// source layer says. None = unknown (raw ALSA, or an absent device),
    /// which is deliberately distinct from 0.
    pub volume_percent: Option<u32>,
    /// Why capture isn't running, when the failure happened at open time
    /// (post-open stream deaths land in the journal only).
    pub error: Option<String>,
    /// The stored project name this device was matched from, when name
    /// reconciliation adopted it under a changed name.
    pub reconciled_from: Option<String>,
}

pub enum DeviceMsg {
    /// The set of device identities strips reference (None = default).
    WantedChanged(BTreeSet<Option<String>>),
    /// The set of output device identities the patch bay references.
    WantedOutputsChanged(BTreeSet<Option<String>>),
    /// Fresh output enumeration + state join. No side effects.
    ListOutputs {
        reply: oneshot::Sender<Vec<OutputDeviceReport>>,
    },
    /// Output list + full reconcile: open the wanted, close the unwanted,
    /// retry the failed.
    RefreshOutputs {
        reply: oneshot::Sender<Vec<OutputDeviceReport>>,
    },
    /// Fresh enumeration + state join. No side effects.
    List {
        reply: oneshot::Sender<Vec<DeviceReport>>,
    },
    /// List + full reconcile: open the wanted, close the unwanted, retry
    /// the failed, re-run name reconciliation.
    Refresh {
        reply: oneshot::Sender<Vec<DeviceReport>>,
    },
    /// Switch a card's profile, then reconcile as for a Refresh — the
    /// card's devices change name, width and map, so nothing read before
    /// the switch survives it.
    SetProfile {
        card: String,
        profile: String,
        reply: oneshot::Sender<Result<Vec<DeviceReport>, String>>,
    },
}

/// Channels of a device the patch bay may aim at.
///
/// A device that carries the monitor gives up its first channels to it —
/// the real-time backend shares one card between the control-room feed and
/// everything else. Subtracted here rather than in the backend so the
/// console can still print the device's true width beside it.
fn patchable_channels(info: &OutputDeviceInfo) -> u16 {
    info.channels.saturating_sub(info.monitor_channels)
}

/// One row of the output patch bay device document. The mirror of
/// [`DeviceReport`], carrying the same honest-failure fields for the same
/// reasons — read the other way round.
#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct OutputDeviceReport {
    pub name: String,
    pub label: Option<String>,
    /// What the device exposes RIGHT NOW. 0 = unknown (absent, never
    /// enumerated this boot).
    pub channels: u16,
    /// How many of `channels` the monitor has taken and the patch bay may
    /// not offer.
    pub monitor_channels: u16,
    /// The OS default output.
    pub active: bool,
    pub status: DeviceStatus,
    /// Some patch references it.
    pub patched: bool,
    pub underruns: u64,
    pub overruns: u64,
    /// ALSA xruns recovered on this device. Always 0 off the real-time
    /// backend, which is the only one that opens a `hw:` PCM.
    pub xruns: u64,
    /// Worst engine block this second, in microseconds. 0 where the
    /// backend does not measure it.
    pub worst_block_us: u32,
    /// The sink layer muted this output. It still opens, still streams,
    /// and the room hears nothing — and an output has no meter to show it,
    /// so this field is the only place a dead PA is explainable.
    pub muted: bool,
    pub volume_percent: Option<u32>,
    pub error: Option<String>,
}

#[derive(Clone)]
pub struct DeviceHandle {
    tx: Sender<DeviceMsg>,
}

impl DeviceHandle {
    /// The channel is created by the caller so the handle can be handed to
    /// the control task before the orchestrator thread spins up.
    pub fn new(tx: Sender<DeviceMsg>) -> Self {
        DeviceHandle { tx }
    }

    /// Sync and non-blocking (unbounded channel) — safe from async context.
    pub fn wanted_changed(&self, wanted: BTreeSet<Option<String>>) {
        let _ = self.tx.send(DeviceMsg::WantedChanged(wanted));
    }

    pub fn wanted_outputs_changed(&self, wanted: BTreeSet<Option<String>>) {
        let _ = self.tx.send(DeviceMsg::WantedOutputsChanged(wanted));
    }

    pub async fn list_outputs(&self) -> Vec<OutputDeviceReport> {
        let (reply, response) = oneshot::channel();
        if self.tx.send(DeviceMsg::ListOutputs { reply }).is_err() {
            return Vec::new();
        }
        response.await.unwrap_or_default()
    }

    pub async fn refresh_outputs(&self) -> Vec<OutputDeviceReport> {
        let (reply, response) = oneshot::channel();
        if self.tx.send(DeviceMsg::RefreshOutputs { reply }).is_err() {
            return Vec::new();
        }
        response.await.unwrap_or_default()
    }

    pub async fn list(&self) -> Vec<DeviceReport> {
        let (reply, response) = oneshot::channel();
        if self.tx.send(DeviceMsg::List { reply }).is_err() {
            return Vec::new();
        }
        response.await.unwrap_or_default()
    }

    pub async fn refresh(&self) -> Vec<DeviceReport> {
        let (reply, response) = oneshot::channel();
        if self.tx.send(DeviceMsg::Refresh { reply }).is_err() {
            return Vec::new();
        }
        response.await.unwrap_or_default()
    }

    /// Put `card` into `profile` and return the document that results.
    pub async fn set_profile(
        &self,
        card: String,
        profile: String,
    ) -> Result<Vec<DeviceReport>, String> {
        let (reply, response) = oneshot::channel();
        self.tx
            .send(DeviceMsg::SetProfile {
                card,
                profile,
                reply,
            })
            .map_err(|_| "device orchestrator is not running".to_owned())?;
        response
            .await
            .map_err(|_| "device orchestrator dropped the request".to_owned())?
    }
}

pub fn spawn(
    rx: Receiver<DeviceMsg>,
    backend: Arc<dyn AudioBackend>,
    stream: Option<Box<dyn StreamHandle>>,
    control: ControlHandle,
) {
    std::thread::Builder::new()
        .name("trib-devices".into())
        .spawn(move || {
            let outputs_control = control.clone();
            Orchestrator::new(
                backend,
                stream,
                Box::new(move |slots| control.input_slots_changed_blocking(slots)),
                Box::new(move |slots| outputs_control.output_slots_changed_blocking(slots)),
            )
            .run(rx);
        })
        .expect("device orchestrator thread spawns");
}

/// How long to wait for a card profile switch to land, as attempts of
/// [`PROFILE_SETTLE_STEP`]. Generous: a USB interface renegotiating its
/// alt-setting is slower than a built-in codec.
const PROFILE_SETTLE_ATTEMPTS: u32 = 40;
const PROFILE_SETTLE_STEP: std::time::Duration = std::time::Duration::from_millis(50);

/// A running input stream, keyed by the STORED identity strips patch with.
struct OpenEntry {
    /// The name the stream actually opened under (alias-resolved).
    resolved: Option<String>,
    channels: u16,
    label: Option<String>,
}

/// How a stored identity opens right now.
struct Resolved {
    resolved: Option<String>,
    channels: u16,
    pulse: bool,
    label: Option<String>,
    /// The source's own channel positions, carried into the open so the
    /// capture asks for them by name. Routing, not decoration.
    channel_map: Option<String>,
}

struct Orchestrator {
    backend: Arc<dyn AudioBackend>,
    /// None = the audio backend failed at boot; everything reports failed.
    stream: Option<Box<dyn StreamHandle>>,
    /// Publishes slot-map changes to the control task.
    slots_changed: Box<dyn Fn(InputSlots) + Send>,
    slots: InputSlots,
    wanted: BTreeSet<Option<String>>,
    /// stored project name → present device name (name reconciliation).
    aliases: HashMap<String, String>,
    /// Renames this daemon PERFORMED by switching a card profile. Kept
    /// apart from `aliases` because they are known, not inferred: a switch
    /// rewrites `…analog-stereo` into `…pro-audio`, which shares no token
    /// with its old name and so is invisible to the matching heuristic.
    profile_aliases: HashMap<String, String>,
    open: HashMap<Option<String>, OpenEntry>,
    open_errors: HashMap<Option<String>, String>,
    /// The output half. Deliberately in the same struct on the same
    /// thread: everything here blocks (enumeration, opening hardware), the
    /// stream handle below is ONE object shared by both directions, and a
    /// card's profile decides its input and output widths together — so
    /// splitting the two halves across threads would make one profile
    /// switch a cross-thread transaction.
    out_slots: OutputSlots,
    out_slots_changed: Box<dyn Fn(OutputSlots) + Send>,
    wanted_outputs: BTreeSet<Option<String>>,
    open_outputs: HashMap<Option<String>, OutOpenEntry>,
    output_errors: HashMap<Option<String>, String>,
    /// The backend generation last reconciled against.
    backend_generation: u64,
}

/// How often the orchestrator looks at the backend's generation while
/// idle. A lock read, not an enumeration — enumerating happens only when
/// the number moved.
const BACKEND_POLL: std::time::Duration = std::time::Duration::from_secs(1);

/// What the orchestrator knows about one open output.
struct OutOpenEntry {
    /// The present device name this was opened against, when it differs
    /// from the stored one (`None` follows the system default).
    resolved: Option<String>,
    channels: u16,
}

impl Orchestrator {
    fn new(
        backend: Arc<dyn AudioBackend>,
        stream: Option<Box<dyn StreamHandle>>,
        slots_changed: Box<dyn Fn(InputSlots) + Send>,
        out_slots_changed: Box<dyn Fn(OutputSlots) + Send>,
    ) -> Self {
        let backend_generation = backend.generation();
        Orchestrator {
            backend,
            stream,
            slots_changed,
            slots: InputSlots::default(),
            wanted: BTreeSet::new(),
            aliases: HashMap::new(),
            profile_aliases: HashMap::new(),
            open: HashMap::new(),
            open_errors: HashMap::new(),
            out_slots: OutputSlots::with_monitor(),
            out_slots_changed,
            wanted_outputs: BTreeSet::new(),
            open_outputs: HashMap::new(),
            output_errors: HashMap::new(),
            backend_generation,
        }
    }

    /// Heal after the backend's own device set changed, if it has.
    /// Returns whether it had.
    fn heal_if_backend_changed(&mut self) -> bool {
        let generation = self.backend.generation();
        if generation == self.backend_generation {
            return false;
        }
        self.backend_generation = generation;
        tracing::info!(generation, "audio backend devices changed; re-patching");
        let present = self.backend.input_devices();
        self.reconcile(&present, true);
        let present = self.backend.output_devices();
        self.reconcile_outputs(&present, true);
        true
    }

    /// Which present device a stored output name refers to. `None` follows
    /// the system default, exactly as it does on the way in.
    fn resolve_output<'a>(
        &self,
        stored: &Option<String>,
        present: &'a [OutputDeviceInfo],
    ) -> Option<&'a OutputDeviceInfo> {
        match stored {
            None => present.iter().find(|d| d.active),
            Some(name) => present.iter().find(|d| &d.name == name),
        }
    }

    /// Open what the patch bay wants, close what it no longer does.
    ///
    /// `heal` additionally retries devices whose stream died — the
    /// Refresh gesture, and the whole recovery story for an output that
    /// was unplugged and put back.
    fn reconcile_outputs(&mut self, present: &[OutputDeviceInfo], heal: bool) {
        let before = self.out_slots.clone();
        if heal {
            let failed: Vec<Option<String>> = self
                .stream
                .as_ref()
                .map(|s| s.output_status())
                .unwrap_or_default()
                .into_iter()
                .filter(|s| s.failed)
                .map(|s| s.device)
                .collect();
            for device in failed {
                self.close_output(&device);
            }
            self.output_errors.clear();
        }

        // Close anything unwanted, gone, or resized. A width change has to
        // go through close-then-open: the slot allocator refuses to resize
        // under a live stream, and rightly so.
        for device in self.open_outputs.keys().cloned().collect::<Vec<_>>() {
            let width = self
                .resolve_output(&device, present)
                .map(patchable_channels);
            let held = self.open_outputs[&device].channels;
            if !self.wanted_outputs.contains(&device) || width != Some(held) {
                self.close_output(&device);
            }
        }

        for device in self.wanted_outputs.clone() {
            if self.open_outputs.contains_key(&device) {
                continue;
            }
            let Some(info) = self.resolve_output(&device, present) else {
                continue; // absent: the report says so, and Refresh retries
            };
            let channels = patchable_channels(info);
            if channels == 0 {
                self.output_errors.insert(
                    device.clone(),
                    "this device exposes no patchable output channels".to_owned(),
                );
                continue;
            }
            let resolved = (device.is_some() || info.active).then(|| info.name.clone());
            let pulse = info.pulse;
            let channel_map = info.channel_map.clone();
            let offset = match self.out_slots.allocate(device.as_deref(), channels) {
                Ok(offset) => offset,
                Err(e) => {
                    self.output_errors
                        .insert(device.clone(), format!("output plane full: {e:?}"));
                    continue;
                }
            };
            let req = OpenOutput {
                // `None` stays `None` so the backend tracks the system
                // default live rather than pinning the name it had today.
                device: device.clone(),
                channels,
                offset,
                pulse,
                channel_map,
            };
            match self.stream.as_ref().map(|s| s.open_output(req)) {
                Some(Ok(())) => {
                    self.open_outputs
                        .insert(device.clone(), OutOpenEntry { resolved, channels });
                    self.output_errors.remove(&device);
                }
                Some(Err(e)) => {
                    self.out_slots.release(device.as_deref());
                    self.output_errors.insert(device.clone(), e.to_string());
                }
                None => {
                    self.out_slots.release(device.as_deref());
                    self.output_errors
                        .insert(device.clone(), "audio backend not running".to_owned());
                }
            }
        }

        if self.out_slots != before {
            (self.out_slots_changed)(self.out_slots.clone());
        }
    }

    fn close_output(&mut self, stored: &Option<String>) {
        if let Some(entry) = self.open_outputs.remove(stored) {
            if let Some(stream) = &self.stream {
                let _ = stream.close_output(entry.resolved.as_deref().or(stored.as_deref()));
            }
            self.out_slots.release(stored.as_deref());
        }
    }

    /// One row per output device, joined with what we know about it.
    fn output_report(&self, present: &[OutputDeviceInfo]) -> Vec<OutputDeviceReport> {
        let status = self
            .stream
            .as_ref()
            .map(|s| s.output_status())
            .unwrap_or_default();
        let mut rows: Vec<OutputDeviceReport> = present
            .iter()
            .map(|info| {
                let stored = if info.active {
                    None
                } else {
                    Some(info.name.clone())
                };
                let key = self
                    .open_outputs
                    .keys()
                    .find(|k| match k {
                        None => info.active,
                        Some(name) => name == &info.name,
                    })
                    .cloned();
                let live = key
                    .as_ref()
                    .and_then(|k| status.iter().find(|s| s.device.as_deref() == k.as_deref()))
                    .or_else(|| {
                        status
                            .iter()
                            .find(|s| s.device.as_deref() == Some(info.name.as_str()))
                    });
                let wanted = self.wanted_outputs.contains(&stored)
                    || self.wanted_outputs.contains(&Some(info.name.clone()));
                let error = self
                    .output_errors
                    .get(&stored)
                    .or_else(|| self.output_errors.get(&Some(info.name.clone())))
                    .cloned();
                OutputDeviceReport {
                    label: info.description.clone(),
                    name: info.name.clone(),
                    channels: info.channels,
                    monitor_channels: info.monitor_channels,
                    active: info.active,
                    status: if error.is_some() {
                        DeviceStatus::Failed
                    } else if key.is_some() {
                        DeviceStatus::Open
                    } else {
                        DeviceStatus::Available
                    },
                    patched: wanted,
                    underruns: live.map_or(0, |s| s.underruns),
                    overruns: live.map_or(0, |s| s.overruns),
                    xruns: live.map_or(0, |s| s.xruns),
                    worst_block_us: live.map_or(0, |s| s.worst_block_us),
                    muted: info.muted,
                    volume_percent: info.volume_percent,
                    error,
                }
            })
            .collect();

        // Devices a patch names that nothing enumerated. Drawn rather than
        // dropped, for the reason the input board already learned: a strip
        // that goes nowhere has to be explainable from the patch bay.
        for stored in &self.wanted_outputs {
            let known = rows.iter().any(|r| match stored {
                None => r.active,
                Some(name) => &r.name == name,
            });
            if known {
                continue;
            }
            rows.push(OutputDeviceReport {
                name: stored
                    .clone()
                    .unwrap_or_else(|| "System default output".to_owned()),
                label: None,
                channels: 0,
                monitor_channels: 0,
                active: stored.is_none(),
                status: DeviceStatus::Absent,
                patched: true,
                underruns: 0,
                overruns: 0,
                xruns: 0,
                worst_block_us: 0,
                muted: false,
                volume_percent: None,
                error: self.output_errors.get(stored).cloned(),
            });
        }
        rows
    }
    fn run(mut self, rx: Receiver<DeviceMsg>) {
        use std::sync::mpsc::RecvTimeoutError;
        // The channel closing is the shutdown signal: the thread exits and
        // the stream handle drops here, stopping all audio.
        loop {
            let msg = match rx.recv_timeout(BACKEND_POLL) {
                Ok(msg) => msg,
                Err(RecvTimeoutError::Timeout) => {
                    self.heal_if_backend_changed();
                    continue;
                }
                Err(RecvTimeoutError::Disconnected) => break,
            };
            // A message that arrives while the backend just changed sees
            // the healed state, not a stale one.
            self.heal_if_backend_changed();
            match msg {
                DeviceMsg::WantedChanged(wanted) => {
                    self.wanted = wanted;
                    let present = self.backend.input_devices();
                    self.reconcile(&present, false);
                }
                DeviceMsg::List { reply } => {
                    let present = self.backend.input_devices();
                    let _ = reply.send(self.report(&present));
                }
                DeviceMsg::Refresh { reply } => {
                    let present = self.backend.input_devices();
                    self.reconcile(&present, true);
                    let _ = reply.send(self.report(&present));
                }
                DeviceMsg::SetProfile {
                    card,
                    profile,
                    reply,
                } => {
                    let outcome = self.set_profile(&card, &profile);
                    let _ = reply.send(outcome);
                }
                DeviceMsg::WantedOutputsChanged(wanted) => {
                    self.wanted_outputs = wanted;
                    let present = self.backend.output_devices();
                    self.reconcile_outputs(&present, false);
                }
                DeviceMsg::ListOutputs { reply } => {
                    let present = self.backend.output_devices();
                    let _ = reply.send(self.output_report(&present));
                }
                DeviceMsg::RefreshOutputs { reply } => {
                    let present = self.backend.output_devices();
                    self.reconcile_outputs(&present, true);
                    let _ = reply.send(self.output_report(&present));
                }
            }
        }
    }

    /// Switch a card's profile and rebuild everything that hung off it.
    ///
    /// Heals like a Refresh on purpose: the switch renames the card's
    /// sources (`…analog-stereo` → `…pro-audio`), so stored patches only
    /// survive by going back through name reconciliation.
    fn set_profile(&mut self, card: &str, profile: &str) -> Result<Vec<DeviceReport>, String> {
        let before = self.backend.input_devices();
        let on_card = |devices: &[InputDeviceInfo]| -> Vec<String> {
            devices
                .iter()
                .filter(|d| d.card.as_deref() == Some(card))
                .map(|d| d.name.clone())
                .collect()
        };
        let devices_before = on_card(&before);
        // Which stored patch identities feed off this card right now —
        // resolved, so a device already adopted under another name counts.
        let patched_here: Vec<String> = self
            .wanted
            .iter()
            .filter(|stored| stored.is_some())
            .filter_map(|stored| {
                let resolved = self.resolve(stored, &before)?.resolved?;
                devices_before
                    .contains(&resolved)
                    .then(|| stored.clone().expect("filtered to named"))
            })
            .collect();

        self.backend
            .set_card_profile(card, profile)
            .map_err(|e| e.to_string())?;
        let present = self.settled(card, profile);

        // Carry the patches across the rename, but only where there is
        // exactly one input on this card at both ends. More than one and
        // the pairing would be a guess — and this codebase would rather
        // report a device absent than feed a strip the wrong microphone.
        let devices_after = on_card(&present);
        if let ([_], [after]) = (devices_before.as_slice(), devices_after.as_slice()) {
            for stored in patched_here.into_iter().filter(|s| s != after) {
                tracing::info!(%stored, renamed_to = %after, "profile switch carried a patch");
                self.profile_aliases.insert(stored, after.clone());
            }
        }

        // The card's old devices are definitively gone — we destroyed them.
        // `report` otherwise treats "open but unenumerable" as proof of
        // presence, which is right for a raw ALSA device that merely went
        // busy and wrong for these: their streams are dead.
        let vanished: Vec<Option<String>> = self
            .open
            .iter()
            .filter(|(_, entry)| {
                entry
                    .resolved
                    .as_ref()
                    .is_some_and(|r| devices_before.contains(r) && !devices_after.contains(r))
            })
            .map(|(stored, _)| stored.clone())
            .collect();
        for stored in vanished {
            self.close(&stored);
        }
        // The default-following stream carries no resolved name — parec
        // tracks the server default itself — so the `vanished` test above
        // can never match it, and `reconcile`'s width check misses it too
        // whenever the new profile happens to expose the same channel
        // count. Meanwhile the node it was reading really was destroyed,
        // and the server relocated the stream to whatever was default in
        // the interim, with nothing to ever move it back. Rebuild it.
        let default_was_on_this_card = before
            .iter()
            .find(|d| d.active)
            .is_some_and(|d| devices_before.contains(&d.name));
        if default_was_on_this_card {
            self.close(&None);
        }
        self.reconcile(&present, true);
        Ok(self.report(&present))
    }

    /// Enumerate once the switch has actually landed.
    ///
    /// The server tears down and recreates a card's nodes asynchronously,
    /// so enumerating straight after the call routinely returns the OLD
    /// sources or none at all. Bounded: on timeout the caller just gets a
    /// stale list it can Refresh, which beats blocking the thread forever.
    fn settled(&self, card: &str, profile: &str) -> Vec<InputDeviceInfo> {
        for _ in 0..PROFILE_SETTLE_ATTEMPTS {
            let switched = self
                .backend
                .input_cards()
                .iter()
                .any(|c| c.name == card && c.active_profile == profile);
            let present = self.backend.input_devices();
            if switched && present.iter().any(|d| d.card.as_deref() == Some(card)) {
                return present;
            }
            std::thread::sleep(PROFILE_SETTLE_STEP);
        }
        tracing::warn!(card, profile, "card profile did not settle in time");
        self.backend.input_devices()
    }

    /// Open the wanted, close the unwanted; on `heal`, also tear down
    /// failed streams so they reopen fresh. Publishes the slot map to the
    /// control task when it changed.
    fn reconcile(&mut self, present: &[InputDeviceInfo], heal: bool) {
        let before = self.slots.clone();
        let stored_names: Vec<String> = self.wanted.iter().filter_map(|w| w.clone()).collect();
        self.aliases = reconcile_names(&stored_names, present);
        // A rename we performed ourselves outranks the heuristic, which
        // cannot see through a profile switch. Entries whose target has
        // gone are stale and get forgotten here.
        self.profile_aliases
            .retain(|_, current| present.iter().any(|d| &d.name == current));
        for (stored, current) in &self.profile_aliases {
            if !present.iter().any(|d| &d.name == stored) {
                self.aliases.insert(stored.clone(), current.clone());
            }
        }

        if heal {
            let failed: Vec<Option<String>> = self
                .failed_resolved_names()
                .into_iter()
                .filter_map(|resolved| {
                    self.open
                        .iter()
                        .find(|(_, e)| e.resolved == resolved)
                        .map(|(stored, _)| stored.clone())
                })
                .collect();
            for stored in failed {
                self.close(&stored);
            }
            // A refresh forgets old failures: everything wanted retries.
            self.open_errors.clear();
        }

        // A device whose channel count changed under us — switching a card
        // profile is the usual cause — is still feeding a slot of the OLD
        // width. Close it so the open pass below rebuilds it at the new
        // one; left alone, the patchbay would draw the new channel count
        // with everything past the old width dead.
        let resized: Vec<Option<String>> = self
            .open
            .iter()
            .filter(|(stored, entry)| {
                self.resolve(stored, present)
                    .is_some_and(|found| found.channels != entry.channels)
            })
            .map(|(stored, _)| stored.clone())
            .collect();
        for stored in resized {
            tracing::info!(device = ?stored, "input channel count changed; reopening");
            self.close(&stored);
        }

        // Close whatever is no longer patched anywhere.
        let unwanted: Vec<Option<String>> = self
            .open
            .keys()
            .filter(|stored| !self.wanted.contains(*stored))
            .cloned()
            .collect();
        for stored in unwanted {
            self.close(&stored);
        }
        self.open_errors
            .retain(|stored, _| self.wanted.contains(stored));

        // Open what's newly wanted (or being retried).
        for stored in self.wanted.clone() {
            if self.open.contains_key(&stored) {
                continue;
            }
            if !heal && self.open_errors.contains_key(&stored) {
                continue; // failed before: only an explicit Refresh retries
            }
            let Some(found) = self.resolve(&stored, present) else {
                continue; // absent: no slot, patches read silence
            };
            let Some(stream) = &self.stream else {
                self.open_errors
                    .insert(stored.clone(), "audio backend not running".into());
                continue;
            };
            let offset = match self.slots.allocate(stored.as_deref(), found.channels) {
                Ok(offset) => offset,
                Err(e) => {
                    tracing::warn!(device = ?stored, error = ?e, "input slot allocation failed");
                    self.open_errors.insert(stored.clone(), format!("{e:?}"));
                    continue;
                }
            };
            match stream.open_input(OpenInput {
                device: found.resolved.clone(),
                channels: found.channels,
                offset,
                pulse: found.pulse,
                channel_map: found.channel_map.clone(),
            }) {
                Ok(()) => {
                    tracing::info!(
                        device = ?found.resolved,
                        channels = found.channels,
                        offset,
                        "input device opened"
                    );
                    self.open_errors.remove(&stored);
                    self.open.insert(
                        stored,
                        OpenEntry {
                            resolved: found.resolved,
                            channels: found.channels,
                            label: found.label,
                        },
                    );
                }
                Err(e) => {
                    tracing::warn!(device = ?found.resolved, %e, "input device failed to open");
                    self.slots.release(stored.as_deref());
                    self.open_errors.insert(stored, e.to_string());
                }
            }
        }

        if self.slots != before {
            (self.slots_changed)(self.slots.clone());
        }
    }

    fn close(&mut self, stored: &Option<String>) {
        if let Some(entry) = self.open.remove(stored) {
            if let Some(stream) = &self.stream {
                let _ = stream.close_input(entry.resolved.as_deref());
            }
            self.slots.release(stored.as_deref());
        }
    }

    /// Stored identity → how to open it, from the current enumeration and
    /// alias map. None = absent.
    fn resolve(&self, stored: &Option<String>, present: &[InputDeviceInfo]) -> Option<Resolved> {
        match stored {
            None => {
                let default = present.iter().find(|d| d.active)?;
                Some(Resolved {
                    // The default follows the system even for pulse: parec
                    // with no --device tracks the default source live.
                    resolved: None,
                    channels: default.channels,
                    pulse: default.pulse,
                    label: default.description.clone(),
                    channel_map: default.channel_map.clone(),
                })
            }
            Some(name) => {
                let resolved = if present.iter().any(|d| &d.name == name) {
                    name.clone()
                } else {
                    self.aliases.get(name)?.clone()
                };
                let device = present.iter().find(|d| d.name == resolved)?;
                Some(Resolved {
                    channels: device.channels,
                    pulse: device.pulse,
                    label: device.description.clone(),
                    channel_map: device.channel_map.clone(),
                    resolved: Some(resolved),
                })
            }
        }
    }

    fn failed_resolved_names(&self) -> Vec<Option<String>> {
        let Some(stream) = &self.stream else {
            return Vec::new();
        };
        stream
            .input_status()
            .into_iter()
            .filter(|s| s.failed)
            .map(|s| s.device)
            .collect()
    }

    /// The device document: every present device joined with orchestrator
    /// state, plus a row per wanted-but-absent stored name.
    fn report(&self, present: &[InputDeviceInfo]) -> Vec<DeviceReport> {
        let statuses: Vec<trib_audio::InputStreamStatus> = self
            .stream
            .as_ref()
            .map(|s| s.input_status())
            .unwrap_or_default();
        let resolved_open: HashMap<Option<&str>, &Option<String>> = self
            .open
            .iter()
            .map(|(stored, e)| (e.resolved.as_deref(), stored))
            .collect();
        let alias_targets: HashMap<&str, &str> = self
            .aliases
            .iter()
            .map(|(stored, current)| (current.as_str(), stored.as_str()))
            .collect();
        // Cards are what a profile change acts on; a device joins to one by
        // name. Platforms without profiles simply return an empty list.
        let cards: HashMap<String, trib_audio::CardInfo> = self
            .backend
            .input_cards()
            .into_iter()
            .map(|c| (c.name.clone(), c))
            .collect();

        let mut rows: Vec<DeviceReport> = present
            .iter()
            .map(|device| {
                // Which stored identity feeds from this device, if any.
                let stored = if device.active && self.open.contains_key(&None) {
                    resolved_open.get(&None).copied()
                } else {
                    resolved_open.get(&Some(device.name.as_str())).copied()
                };
                let status_entry = statuses.iter().find(|s| match &s.device {
                    None => device.active,
                    Some(name) => name == &device.name,
                });
                let patched = self.wanted.contains(&Some(device.name.clone()))
                    || (device.active && self.wanted.contains(&None))
                    || alias_targets.contains_key(device.name.as_str());
                let open_error = self.wanted_error_for(&device.name, device.active);
                let status = match (stored, status_entry) {
                    (Some(_), Some(s)) if s.failed => DeviceStatus::Failed,
                    (Some(_), _) => DeviceStatus::Open,
                    (None, _) if patched && open_error.is_some() => DeviceStatus::Failed,
                    (None, _) => DeviceStatus::Available,
                };
                let card = device.card.as_ref().and_then(|name| cards.get(name));
                DeviceReport {
                    name: device.name.clone(),
                    label: device.description.clone(),
                    channels: device.channels,
                    active: device.active,
                    status,
                    patched,
                    underruns: status_entry.map_or(0, |s| s.underruns),
                    overruns: status_entry.map_or(0, |s| s.overruns),
                    card: device.card.clone(),
                    profile: card.map(|c| c.active_profile.clone()),
                    profiles: card
                        .map(|c| {
                            c.profiles
                                .iter()
                                .map(|p| ProfileReport {
                                    name: p.name.clone(),
                                    description: p.description.clone(),
                                })
                                .collect()
                        })
                        .unwrap_or_default(),
                    channel_map: device.channel_map.clone(),
                    muted: device.muted,
                    volume_percent: device.volume_percent,
                    error: match status {
                        DeviceStatus::Failed => open_error.map(str::to_owned),
                        _ => None,
                    },
                    reconciled_from: alias_targets
                        .get(device.name.as_str())
                        .map(|s| (*s).to_owned()),
                }
            })
            .collect();

        // Devices we hold OPEN but enumeration can't see right now — raw
        // ALSA devices go busy once we stream from them. Streaming is the
        // strongest possible proof of presence.
        for (stored, entry) in &self.open {
            let Some(resolved) = &entry.resolved else {
                continue; // the default: its enumeration row is `active`
            };
            if present.iter().any(|d| &d.name == resolved) {
                continue;
            }
            let status_entry = statuses
                .iter()
                .find(|s| s.device.as_deref() == Some(resolved.as_str()));
            rows.push(DeviceReport {
                name: resolved.clone(),
                label: entry.label.clone(),
                channels: entry.channels,
                active: false,
                status: if status_entry.is_some_and(|s| s.failed) {
                    DeviceStatus::Failed
                } else {
                    DeviceStatus::Open
                },
                patched: true,
                underruns: status_entry.map_or(0, |s| s.underruns),
                overruns: status_entry.map_or(0, |s| s.overruns),
                // Held open but unenumerable, so there is nothing to join a
                // card against — profiles stay unoffered until it reappears.
                card: None,
                profile: None,
                profiles: Vec::new(),
                channel_map: None,
                // Unenumerable this pass, so mute and volume are unknown
                // rather than known-good — the last enumeration's answer
                // would be a guess dressed as a fact.
                muted: false,
                volume_percent: None,
                error: None,
                reconciled_from: stored.as_ref().filter(|s| *s != resolved).cloned(),
            });
        }

        // Wanted names that are neither present, reconciled, nor open:
        // absent, but still on the board so their patches stay explainable.
        for stored in &self.wanted {
            let Some(name) = stored else { continue };
            let covered = present.iter().any(|d| &d.name == name)
                || self.aliases.contains_key(name.as_str())
                || self.open.contains_key(stored);
            if !covered {
                rows.push(DeviceReport {
                    name: name.clone(),
                    label: None,
                    channels: 0,
                    active: false,
                    status: DeviceStatus::Absent,
                    patched: true,
                    underruns: 0,
                    overruns: 0,
                    // Unplugged: nothing to join, nothing to offer, and
                    // nothing to say about a mute that isn't there.
                    card: None,
                    profile: None,
                    profiles: Vec::new(),
                    channel_map: None,
                    muted: false,
                    volume_percent: None,
                    error: None,
                    reconciled_from: None,
                });
            }
        }
        rows
    }

    /// The recorded open-failure reason feeding this device row, if any:
    /// keyed by its stored name, or — for the active device — the default.
    fn wanted_error_for(&self, name: &str, active: bool) -> Option<&str> {
        self.open_errors
            .iter()
            .find_map(|(stored, e)| (stored.as_deref() == Some(name)).then_some(e.as_str()))
            .or_else(|| {
                active
                    .then(|| self.open_errors.get(&None).map(String::as_str))
                    .flatten()
            })
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use trib_audio::{AudioError, InputStreamStatus, StreamConfig};

    use super::*;

    /// Backend double with a swappable enumeration — the "replug".
    #[derive(Default)]
    struct TestBackend {
        devices: Mutex<Vec<InputDeviceInfo>>,
        cards: Mutex<Vec<trib_audio::CardInfo>>,
        /// What the card exposes once switched: profile → devices.
        on_switch: Mutex<HashMap<String, Vec<InputDeviceInfo>>>,
        switch_fails: Mutex<bool>,
        /// The backend's own-device-set counter — the exclusive layer's
        /// card coming and going.
        generation: Mutex<u64>,
        /// How many times the orchestrator enumerated; polling must not.
        enumerations: Mutex<u32>,
    }

    impl AudioBackend for TestBackend {
        fn name(&self) -> &'static str {
            "test"
        }
        fn input_devices(&self) -> Vec<InputDeviceInfo> {
            *self.enumerations.lock().unwrap() += 1;
            self.devices.lock().unwrap().clone()
        }
        fn generation(&self) -> u64 {
            *self.generation.lock().unwrap()
        }
        fn input_cards(&self) -> Vec<trib_audio::CardInfo> {
            self.cards.lock().unwrap().clone()
        }
        fn set_card_profile(&self, card: &str, profile: &str) -> Result<(), AudioError> {
            if *self.switch_fails.lock().unwrap() {
                return Err(AudioError::Device("scripted profile failure".into()));
            }
            // Stand in for the server: the card adopts the profile and its
            // devices come back changed.
            for entry in self.cards.lock().unwrap().iter_mut() {
                if entry.name == card {
                    entry.active_profile = profile.to_owned();
                }
            }
            if let Some(after) = self.on_switch.lock().unwrap().get(profile) {
                *self.devices.lock().unwrap() = after.clone();
            }
            Ok(())
        }
        fn start(
            &self,
            _config: &StreamConfig,
            _engine: trib_engine::GraphEngine,
        ) -> Result<Box<dyn StreamHandle>, AudioError> {
            unreachable!("orchestrator tests never start streams")
        }
    }

    /// Stream double: records every open/close, fails configured names.
    #[derive(Default)]
    struct TestStream {
        calls: Mutex<Vec<String>>,
        fail: Mutex<Vec<Option<String>>>,
    }

    impl StreamHandle for TestStream {
        fn open_input(&self, req: trib_audio::OpenInput) -> Result<(), AudioError> {
            self.calls.lock().unwrap().push(format!(
                "open {:?} ch{} @{}",
                req.device, req.channels, req.offset
            ));
            if self.fail.lock().unwrap().contains(&req.device) {
                return Err(AudioError::Device("scripted failure".into()));
            }
            Ok(())
        }
        fn close_input(&self, device: Option<&str>) -> Result<(), AudioError> {
            self.calls.lock().unwrap().push(format!("close {device:?}"));
            Ok(())
        }
        fn input_status(&self) -> Vec<InputStreamStatus> {
            Vec::new()
        }
    }

    fn dev(name: &str, channels: u16, active: bool) -> InputDeviceInfo {
        InputDeviceInfo {
            name: name.into(),
            description: None,
            channels,
            active,
            pulse: false,
            card: None,
            channel_map: None,
            muted: false,
            volume_percent: None,
        }
    }

    struct Rig {
        orchestrator: Orchestrator,
        calls: Arc<TestStream>,
        slot_updates: Arc<Mutex<Vec<InputSlots>>>,
        devices: Arc<TestBackend>,
    }

    fn rig(devices: Vec<InputDeviceInfo>) -> Rig {
        let backend = Arc::new(TestBackend {
            devices: Mutex::new(devices),
            ..TestBackend::default()
        });
        let stream = Arc::new(TestStream::default());
        let updates: Arc<Mutex<Vec<InputSlots>>> = Arc::default();
        let updates_sink = updates.clone();
        // Two Arcs to the same double: one inspected by the test, one
        // owned (boxed) by the orchestrator.
        struct Shared(Arc<TestStream>);
        impl StreamHandle for Shared {
            fn open_input(&self, req: trib_audio::OpenInput) -> Result<(), AudioError> {
                self.0.open_input(req)
            }
            fn close_input(&self, device: Option<&str>) -> Result<(), AudioError> {
                self.0.close_input(device)
            }
            fn input_status(&self) -> Vec<InputStreamStatus> {
                self.0.input_status()
            }
        }
        Rig {
            orchestrator: Orchestrator::new(
                backend.clone(),
                Some(Box::new(Shared(stream.clone()))),
                Box::new(move |slots| updates_sink.lock().unwrap().push(slots)),
                Box::new(|_| {}),
            ),
            calls: stream,
            slot_updates: updates,
            devices: backend,
        }
    }

    fn reconcile(rig: &mut Rig, wanted: &[Option<&str>], heal: bool) {
        rig.orchestrator.wanted = wanted.iter().map(|w| w.map(str::to_owned)).collect();
        let present = rig.orchestrator.backend.input_devices();
        rig.orchestrator.reconcile(&present, heal);
    }

    #[test]
    fn patching_opens_on_demand_and_unpatching_closes() {
        let mut rig = rig(vec![dev("default", 2, true), dev("dock", 2, false)]);
        reconcile(&mut rig, &[None], false);
        assert_eq!(
            rig.calls.calls.lock().unwrap().as_slice(),
            ["open None ch2 @0"]
        );
        assert_eq!(rig.slot_updates.lock().unwrap().len(), 1);

        reconcile(&mut rig, &[None, Some("dock")], false);
        assert_eq!(
            rig.calls.calls.lock().unwrap().last().unwrap(),
            "open Some(\"dock\") ch2 @2"
        );

        reconcile(&mut rig, &[Some("dock")], false);
        assert_eq!(
            rig.calls.calls.lock().unwrap().last().unwrap(),
            "close None",
            "the last default strip unpatched: its stream closes"
        );
        let slots = rig.slot_updates.lock().unwrap().last().unwrap().clone();
        assert_eq!(slots.resolve(Some("dock"), 0), Some(2));
        assert_eq!(slots.resolve(None, 0), None);
    }

    /// A device that belongs to a card, so profiles can be offered for it.
    fn carded(name: &str, channels: u16, card: &str) -> InputDeviceInfo {
        InputDeviceInfo {
            card: Some(card.into()),
            ..dev(name, channels, false)
        }
    }

    fn card(name: &str, active: &str, profiles: &[&str]) -> trib_audio::CardInfo {
        trib_audio::CardInfo {
            name: name.into(),
            active_profile: active.into(),
            profiles: profiles
                .iter()
                .map(|p| trib_audio::CardProfile {
                    name: (*p).into(),
                    description: (*p).into(),
                })
                .collect(),
        }
    }

    #[test]
    fn switching_a_card_profile_rebuilds_its_devices_at_the_new_width() {
        let mut rig = rig(vec![carded("umc.analog-stereo", 2, "card.umc")]);
        *rig.devices.cards.lock().unwrap() = vec![card(
            "card.umc",
            "input:analog-stereo",
            &["input:analog-stereo", "pro-audio"],
        )];
        // Switching renames the source as well as widening it — exactly
        // what makes name reconciliation load-bearing here.
        rig.devices.on_switch.lock().unwrap().insert(
            "pro-audio".into(),
            vec![carded("umc.pro-audio", 18, "card.umc")],
        );
        reconcile(&mut rig, &[Some("umc.analog-stereo")], false);
        assert_eq!(
            rig.calls.calls.lock().unwrap().as_slice(),
            ["open Some(\"umc.analog-stereo\") ch2 @0"]
        );

        let report = rig
            .orchestrator
            .set_profile("card.umc", "pro-audio")
            .expect("the switch succeeds");

        let row = report
            .iter()
            .find(|r| r.name == "umc.pro-audio")
            .expect("the renamed source is reported");
        assert_eq!(row.channels, 18);
        assert_eq!(row.profile.as_deref(), Some("pro-audio"));
        assert_eq!(row.profiles.len(), 2, "both profiles stay offered");
        assert_eq!(
            row.reconciled_from.as_deref(),
            Some("umc.analog-stereo"),
            "the stored patch follows the rename rather than going absent"
        );
        assert_eq!(
            rig.calls.calls.lock().unwrap().last().unwrap(),
            "open Some(\"umc.pro-audio\") ch18 @0"
        );
    }

    #[test]
    fn a_multi_input_card_never_guesses_which_patch_survives_a_rename() {
        let mut rig = rig(vec![
            carded("hda.mic", 2, "card.hda"),
            carded("hda.line", 2, "card.hda"),
        ]);
        *rig.devices.cards.lock().unwrap() = vec![card("card.hda", "HiFi", &["HiFi", "pro-audio"])];
        rig.devices.on_switch.lock().unwrap().insert(
            "pro-audio".into(),
            vec![
                carded("hda.pro.0", 2, "card.hda"),
                carded("hda.pro.1", 2, "card.hda"),
            ],
        );
        reconcile(&mut rig, &[Some("hda.mic")], false);

        let report = rig
            .orchestrator
            .set_profile("card.hda", "pro-audio")
            .expect("the switch succeeds");

        assert!(
            report.iter().all(|r| r.reconciled_from.is_none()),
            "two inputs before and after: pairing them would be a guess, and \
             guessing wrong feeds a strip the wrong microphone"
        );
        assert_eq!(
            report
                .iter()
                .find(|r| r.name == "hda.mic")
                .map(|r| r.status),
            Some(DeviceStatus::Absent),
            "the old patch is reported absent rather than silently rehomed"
        );
    }

    #[test]
    fn a_default_following_patch_is_rebuilt_across_a_profile_switch() {
        // The default stream carries no resolved name — parec follows the
        // server default itself — so the vanished check cannot see it, and
        // the width check misses it too when the new profile exposes the
        // same channel count. Its node was still destroyed underneath it.
        let umc = InputDeviceInfo {
            active: true,
            ..carded("umc.multichannel", 10, "card.umc")
        };
        let mut rig = rig(vec![umc]);
        *rig.devices.cards.lock().unwrap() = vec![card(
            "card.umc",
            "input:multichannel-input",
            &["input:multichannel-input", "pro-audio"],
        )];
        rig.devices.on_switch.lock().unwrap().insert(
            "pro-audio".into(),
            // Same width on purpose: that is what defeats the width check.
            vec![InputDeviceInfo {
                active: true,
                ..carded("umc.pro-input-0", 10, "card.umc")
            }],
        );
        reconcile(&mut rig, &[None], false);
        rig.calls.calls.lock().unwrap().clear();

        rig.orchestrator
            .set_profile("card.umc", "pro-audio")
            .expect("the switch succeeds");

        let calls = rig.calls.calls.lock().unwrap().clone();
        assert!(
            calls.contains(&"close None".to_owned()),
            "the default stream must be torn down: {calls:?}"
        );
        assert!(
            calls.iter().any(|c| c.starts_with("open None ch10")),
            "…and opened again on the card's new node: {calls:?}"
        );
    }

    #[test]
    fn a_profile_switch_elsewhere_leaves_the_default_stream_alone() {
        // Rebuilding is disruptive — audio drops for a moment — so it must
        // happen only when the default's own card was the one switched.
        let mut rig = rig(vec![
            InputDeviceInfo {
                active: true,
                ..dev("onboard", 2, true)
            },
            carded("umc.analog-stereo", 2, "card.umc"),
        ]);
        *rig.devices.cards.lock().unwrap() = vec![card(
            "card.umc",
            "input:analog-stereo",
            &["input:analog-stereo", "pro-audio"],
        )];
        rig.devices.on_switch.lock().unwrap().insert(
            "pro-audio".into(),
            vec![
                InputDeviceInfo {
                    active: true,
                    ..dev("onboard", 2, true)
                },
                carded("umc.pro-audio", 18, "card.umc"),
            ],
        );
        reconcile(&mut rig, &[None], false);
        rig.calls.calls.lock().unwrap().clear();

        rig.orchestrator
            .set_profile("card.umc", "pro-audio")
            .expect("the switch succeeds");

        let calls = rig.calls.calls.lock().unwrap().clone();
        assert!(
            !calls.contains(&"close None".to_owned()),
            "the default lives on another card and never lost its node: {calls:?}"
        );
    }

    #[test]
    fn a_refused_profile_switch_reports_why_and_changes_nothing() {
        let mut rig = rig(vec![carded("umc.analog-stereo", 2, "card.umc")]);
        *rig.devices.cards.lock().unwrap() =
            vec![card("card.umc", "input:analog-stereo", &["pro-audio"])];
        *rig.devices.switch_fails.lock().unwrap() = true;
        reconcile(&mut rig, &[Some("umc.analog-stereo")], false);
        let before = rig.calls.calls.lock().unwrap().len();

        let err = rig
            .orchestrator
            .set_profile("card.umc", "pro-audio")
            .expect_err("a refused switch is an error, not a silent no-op");

        assert!(err.contains("scripted profile failure"), "got: {err}");
        assert_eq!(
            rig.calls.calls.lock().unwrap().len(),
            before,
            "nothing was torn down on the way to failing"
        );
    }

    #[test]
    fn a_widened_device_reopens_at_its_new_channel_count() {
        // The interface starts in a stereo card profile.
        let mut rig = rig(vec![dev("umc", 2, false)]);
        reconcile(&mut rig, &[Some("umc")], false);
        assert_eq!(
            rig.calls.calls.lock().unwrap().as_slice(),
            ["open Some(\"umc\") ch2 @0"]
        );

        // The user switches it to pro-audio and it exposes eighteen.
        *rig.devices.devices.lock().unwrap() = vec![dev("umc", 18, false)];
        reconcile(&mut rig, &[Some("umc")], false);
        assert_eq!(
            rig.calls.calls.lock().unwrap()[1..],
            ["close Some(\"umc\")", "open Some(\"umc\") ch18 @0"],
            "the 2-wide stream and its slot must go before the wider one opens"
        );
        let slots = rig.slot_updates.lock().unwrap().last().unwrap().clone();
        assert_eq!(
            slots.resolve(Some("umc"), 17),
            Some(17),
            "the eighteenth channel carries audio rather than resolving to silence"
        );
    }

    #[test]
    fn an_absent_device_holds_no_slot_and_a_replug_plus_refresh_opens_it() {
        let mut rig = rig(vec![dev("default", 2, true)]);
        reconcile(&mut rig, &[Some("Scarlett 18i20 USB")], false);
        assert!(
            rig.calls.calls.lock().unwrap().is_empty(),
            "nothing to open"
        );
        assert!(rig.slot_updates.lock().unwrap().is_empty());
        let present = rig.orchestrator.backend.input_devices();
        let report = rig.orchestrator.report(&present);
        let absent = report
            .iter()
            .find(|r| r.name == "Scarlett 18i20 USB")
            .unwrap();
        assert_eq!(absent.status, DeviceStatus::Absent);
        assert!(absent.patched);

        // The interface gets plugged in; the user hits Refresh.
        rig.devices
            .devices
            .lock()
            .unwrap()
            .push(dev("Scarlett 18i20 USB", 8, false));
        reconcile(&mut rig, &[Some("Scarlett 18i20 USB")], true);
        assert_eq!(
            rig.calls.calls.lock().unwrap().last().unwrap(),
            "open Some(\"Scarlett 18i20 USB\") ch8 @0"
        );
    }

    #[test]
    fn the_backends_own_card_coming_up_re_patches_without_a_refresh() {
        // The appliance boots with the interface unplugged: the exclusive
        // backend enumerates nothing, and the default patch has nowhere to
        // go. Nobody is at the console to press Refresh.
        let mut rig = rig(Vec::new());
        reconcile(&mut rig, &[None], false);
        assert!(
            rig.calls.calls.lock().unwrap().is_empty(),
            "nothing to open"
        );
        let enumerated = *rig.devices.enumerations.lock().unwrap();

        // Idle polls with an unchanged generation must not enumerate.
        assert!(!rig.orchestrator.heal_if_backend_changed());
        assert_eq!(*rig.devices.enumerations.lock().unwrap(), enumerated);

        // The card comes up inside the backend, which says so.
        rig.devices
            .devices
            .lock()
            .unwrap()
            .push(dev("hw:1", 10, true));
        *rig.devices.generation.lock().unwrap() += 1;
        assert!(rig.orchestrator.heal_if_backend_changed());
        assert_eq!(
            rig.calls.calls.lock().unwrap().as_slice(),
            ["open None ch10 @0"],
            "the wanted default patch opens at the card's width, unprompted"
        );
        // Seen: a second look at the same generation is a no-op.
        assert!(!rig.orchestrator.heal_if_backend_changed());
    }

    #[test]
    fn a_failed_open_releases_its_slot_and_only_refresh_retries() {
        let mut rig = rig(vec![dev("default", 2, true), dev("flaky", 2, false)]);
        rig.calls.fail.lock().unwrap().push(Some("flaky".into()));
        reconcile(&mut rig, &[Some("flaky")], false);
        let updates = rig.slot_updates.lock().unwrap().len();
        assert_eq!(updates, 0, "failed open never published a slot map");

        // A wanted-set change does NOT retry a known failure…
        reconcile(&mut rig, &[Some("flaky"), None], false);
        let opens_for_flaky = rig
            .calls
            .calls
            .lock()
            .unwrap()
            .iter()
            .filter(|c| c.contains("flaky"))
            .count();
        assert_eq!(opens_for_flaky, 1);

        // …but Refresh does, and it succeeds once the fault clears.
        rig.calls.fail.lock().unwrap().clear();
        reconcile(&mut rig, &[Some("flaky"), None], true);
        let report = rig
            .orchestrator
            .report(&rig.orchestrator.backend.input_devices());
        let flaky = report.iter().find(|r| r.name == "flaky").unwrap();
        assert_eq!(flaky.status, DeviceStatus::Open);
    }

    #[test]
    fn a_renamed_device_reconciles_and_reports_its_old_name() {
        let mut rig = rig(vec![
            dev("default", 2, true),
            dev("ThinkPad Thunderbolt 4 Dock USB #2", 2, false),
        ]);
        reconcile(&mut rig, &[Some("ThinkPad Thunderbolt 4 Dock USB")], false);
        assert_eq!(
            rig.calls.calls.lock().unwrap().last().unwrap(),
            "open Some(\"ThinkPad Thunderbolt 4 Dock USB #2\") ch2 @0",
            "opened under the CURRENT name"
        );
        let slots = rig.slot_updates.lock().unwrap().last().unwrap().clone();
        assert_eq!(
            slots.resolve(Some("ThinkPad Thunderbolt 4 Dock USB"), 0),
            Some(0),
            "the slot keys by the STORED name so patches resolve"
        );
        let report = rig
            .orchestrator
            .report(&rig.orchestrator.backend.input_devices());
        let row = report
            .iter()
            .find(|r| r.name == "ThinkPad Thunderbolt 4 Dock USB #2")
            .unwrap();
        assert_eq!(row.status, DeviceStatus::Open);
        assert_eq!(
            row.reconciled_from.as_deref(),
            Some("ThinkPad Thunderbolt 4 Dock USB")
        );
    }

    #[test]
    fn a_failed_open_reports_its_reason() {
        let mut rig = rig(vec![dev("default", 2, true), dev("flaky", 2, false)]);
        rig.calls.fail.lock().unwrap().push(Some("flaky".into()));
        reconcile(&mut rig, &[Some("flaky")], false);
        let report = rig
            .orchestrator
            .report(&rig.orchestrator.backend.input_devices());
        let flaky = report.iter().find(|r| r.name == "flaky").unwrap();
        assert_eq!(flaky.status, DeviceStatus::Failed);
        assert_eq!(
            flaky.error.as_deref(),
            Some("no usable device: scripted failure")
        );
    }

    #[test]
    fn a_muted_device_reports_muted_while_still_opening_cleanly() {
        // The whole point: mute is invisible in every other field. An input
        // silenced by the source layer opens, streams, reports no error and
        // feeds silence to every meter — status alone cannot explain that.
        let muted = InputDeviceInfo {
            muted: true,
            volume_percent: Some(0),
            ..dev("default", 2, true)
        };
        let mut rig = rig(vec![muted]);
        reconcile(&mut rig, &[None], false);
        let report = rig
            .orchestrator
            .report(&rig.orchestrator.backend.input_devices());
        let row = report.iter().find(|r| r.name == "default").unwrap();
        assert_eq!(row.status, DeviceStatus::Open, "muted is not failed");
        assert_eq!(row.error, None, "muting is not an error, it is a setting");
        assert!(row.muted, "…so the report has to carry it separately");
        assert_eq!(row.volume_percent, Some(0));
    }

    #[test]
    fn an_unmuted_device_reports_its_volume_without_claiming_zero() {
        let mut rig = rig(vec![dev("default", 2, true)]);
        reconcile(&mut rig, &[None], false);
        let report = rig
            .orchestrator
            .report(&rig.orchestrator.backend.input_devices());
        let row = report.iter().find(|r| r.name == "default").unwrap();
        assert!(!row.muted);
        assert_eq!(
            row.volume_percent, None,
            "a backend that reports no volume must read as unknown, not silenced"
        );
    }

    #[test]
    fn without_a_running_backend_the_report_says_so() {
        let backend = Arc::new(TestBackend {
            devices: Mutex::new(vec![dev("usb", 2, true)]),
            ..TestBackend::default()
        });
        let mut orchestrator =
            Orchestrator::new(backend.clone(), None, Box::new(|_| {}), Box::new(|_| {}));
        orchestrator.wanted = [Some("usb".to_owned())].into_iter().collect();
        let present = backend.input_devices();
        orchestrator.reconcile(&present, true);
        let report = orchestrator.report(&present);
        let row = report.iter().find(|r| r.name == "usb").unwrap();
        assert_eq!(row.status, DeviceStatus::Failed);
        assert_eq!(row.error.as_deref(), Some("audio backend not running"));
    }
}
