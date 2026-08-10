//! The device orchestrator: a dedicated thread that owns the audio
//! backend's stream handle, the input slot map, and the device state
//! machine. Everything it does may block (enumeration, opening hardware),
//! which is exactly why it is NOT the control task — the control task
//! sends it wanted-set diffs and receives slot maps back.
//!
//! Refresh (user-initiated, no background retry): re-enumerate, re-run
//! name reconciliation, close-and-release failed streams, retry every
//! wanted-but-unopened device.

use std::collections::{BTreeSet, HashMap};
use std::sync::Arc;
use std::sync::mpsc::{Receiver, Sender};

use serde::Serialize;
use tokio::sync::oneshot;
use trib_audio::{AudioBackend, InputDeviceInfo, OpenInput, StreamHandle};
use trib_engine::InputSlots;
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

/// One row of the patchbay device document.
#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct DeviceReport {
    pub name: String,
    /// The friendly print the system sound menu shows, when the source
    /// layer provides one.
    pub label: Option<String>,
    /// 0 = unknown (an absent device was never enumerated this boot).
    pub channels: u16,
    /// The OS default input — what `device: null` patches feed from.
    pub active: bool,
    pub status: DeviceStatus,
    /// Some strip references it (directly, via the default, or via an alias).
    pub patched: bool,
    pub underruns: u64,
    /// The stored project name this device was matched from, when name
    /// reconciliation adopted it under a changed name.
    pub reconciled_from: Option<String>,
}

pub enum DeviceMsg {
    /// The set of device identities strips reference (None = default).
    WantedChanged(BTreeSet<Option<String>>),
    /// Fresh enumeration + state join. No side effects.
    List {
        reply: oneshot::Sender<Vec<DeviceReport>>,
    },
    /// List + full reconcile: open the wanted, close the unwanted, retry
    /// the failed, re-run name reconciliation.
    Refresh {
        reply: oneshot::Sender<Vec<DeviceReport>>,
    },
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
            Orchestrator::new(
                backend,
                stream,
                Box::new(move |slots| control.input_slots_changed_blocking(slots)),
            )
            .run(rx);
        })
        .expect("device orchestrator thread spawns");
}

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
    open: HashMap<Option<String>, OpenEntry>,
    open_errors: HashMap<Option<String>, String>,
}

impl Orchestrator {
    fn new(
        backend: Arc<dyn AudioBackend>,
        stream: Option<Box<dyn StreamHandle>>,
        slots_changed: Box<dyn Fn(InputSlots) + Send>,
    ) -> Self {
        Orchestrator {
            backend,
            stream,
            slots_changed,
            slots: InputSlots::default(),
            wanted: BTreeSet::new(),
            aliases: HashMap::new(),
            open: HashMap::new(),
            open_errors: HashMap::new(),
        }
    }
    fn run(mut self, rx: Receiver<DeviceMsg>) {
        // The channel closing is the shutdown signal: the thread exits and
        // the stream handle drops here, stopping all audio.
        while let Ok(msg) = rx.recv() {
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
            }
        }
    }

    /// Open the wanted, close the unwanted; on `heal`, also tear down
    /// failed streams so they reopen fresh. Publishes the slot map to the
    /// control task when it changed.
    fn reconcile(&mut self, present: &[InputDeviceInfo], heal: bool) {
        let before = self.slots.clone();
        let stored_names: Vec<String> = self.wanted.iter().filter_map(|w| w.clone()).collect();
        self.aliases = reconcile_names(&stored_names, present);

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
                    self.open_errors.insert(stored.clone(), format!("{e:?}"));
                    continue;
                }
            };
            match stream.open_input(OpenInput {
                device: found.resolved.clone(),
                channels: found.channels,
                offset,
                pulse: found.pulse,
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
                let status = match (stored, status_entry) {
                    (Some(_), Some(s)) if s.failed => DeviceStatus::Failed,
                    (Some(_), _) => DeviceStatus::Open,
                    (None, _) if patched && self.wanted_error_for(&device.name, device.active) => {
                        DeviceStatus::Failed
                    }
                    (None, _) => DeviceStatus::Available,
                };
                DeviceReport {
                    name: device.name.clone(),
                    label: device.description.clone(),
                    channels: device.channels,
                    active: device.active,
                    status,
                    patched,
                    underruns: status_entry.map_or(0, |s| s.underruns),
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
                    reconciled_from: None,
                });
            }
        }
        rows
    }

    fn wanted_error_for(&self, name: &str, active: bool) -> bool {
        self.open_errors
            .keys()
            .any(|stored| stored.as_deref() == Some(name))
            || (active && self.open_errors.contains_key(&None))
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use trib_audio::{AudioError, InputStreamStatus, StreamConfig};

    use super::*;

    /// Backend double with a swappable enumeration — the "replug".
    struct TestBackend {
        devices: Mutex<Vec<InputDeviceInfo>>,
    }

    impl AudioBackend for TestBackend {
        fn name(&self) -> &'static str {
            "test"
        }
        fn input_devices(&self) -> Vec<InputDeviceInfo> {
            self.devices.lock().unwrap().clone()
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
}
