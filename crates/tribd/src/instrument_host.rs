//! The instrument orchestrator: a dedicated thread that owns the SoundFont
//! cache, the MIDI port connections, and rack construction.
//!
//! It is a separate thread from the device orchestrator on purpose. That
//! one is entirely about audio-device identity — renames, card profiles,
//! slot widths — and its loop calls into the backend, which shells out to
//! `pactl`. A MIDI port has none of those problems, and coupling a
//! keyboard's liveness to `pactl`'s latency would be a real regression.
//! What it does copy is the doctrine: open on demand, refresh only when
//! asked, never guess at a port whose name changed.
//!
//! It owns soundfont loading because parsing one costs hundreds of
//! milliseconds and hundreds of megabytes. That cannot happen on the
//! control task (which owes the console a live fader) or on the audio
//! thread (which owes it no allocation at all), so it happens here and the
//! finished rack crosses as a box.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::mpsc::{Receiver, Sender};
use std::sync::{Arc, Mutex};

use rtrb::Producer;

use serde::Serialize;
use tokio::sync::oneshot;
use trib_core::InstrumentState;
use trib_engine::{
    Instrument, InstrumentRack, KeyFilter, MidiBinding, MidiEvent, SoundFont, VoiceSpec,
};
use utoipa::ToSchema;

use crate::soundfonts;

/// Why an instrument is or is not making sound.
///
/// This is machinery, not document — the same split that keeps the
/// transport out of the reducer. It lives on the instruments report
/// alongside a sentence, because "the strip is silent" must always have an
/// answer somewhere in the console.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum InstrumentStatus {
    /// Loaded and bound: it will sound when it is played.
    Live,
    /// Loaded, but nothing is routed to it yet.
    Ready,
    /// Its soundfont or its port could not be found.
    Missing,
    /// Its soundfont was found and refused to load.
    Failed,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, ToSchema)]
pub struct InstrumentReport {
    pub id: u32,
    pub name: String,
    pub status: InstrumentStatus,
    /// The daemon's own sentence for why, in the console's voice. `None`
    /// when there is nothing to explain.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    /// Presets the loaded soundfont carries, so the console can say
    /// whether a preset number means anything.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub presets: Option<u16>,
    /// Resident sample data, so a Pi's memory budget is legible.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub resident_bytes: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, ToSchema)]
pub struct MidiPortReport {
    /// Stable across a replug: the port's name, not its ALSA client
    /// number, which the kernel hands out afresh every time.
    pub id: String,
    pub name: String,
    /// True when an instrument is bound to it and it is connected.
    pub connected: bool,
    /// An instrument references it but the system does not offer it.
    pub absent: bool,
}

/// What the instrument host knows about MIDI, in one answer.
///
/// A named struct rather than a tuple: three parallel lists crossing a
/// channel is where a reader stops being able to tell which is which.
#[derive(Debug, Default, Clone)]
pub struct MidiReport {
    pub instruments: Vec<InstrumentReport>,
    pub inputs: Vec<MidiPortReport>,
    pub outputs: Vec<crate::midi_out::MidiOutPortReport>,
}

pub enum InstrumentMsg {
    /// The console changed: rebuild the rack from this document.
    InstrumentsChanged(Vec<InstrumentState>),
    Report {
        reply: oneshot::Sender<MidiReport>,
    },
    /// Re-enumerate ports and retry anything that failed, then rebuild.
    Refresh {
        reply: oneshot::Sender<MidiReport>,
    },
    /// The MIDI routes changed.
    ///
    /// Its OWN message rather than a widened `InstrumentsChanged`: folding
    /// them together would make editing a route reload a SoundFont, which
    /// is hundreds of milliseconds and hundreds of megabytes.
    RoutesChanged(Vec<trib_core::MidiRoute>),
    /// The soundfont library as it stands.
    Library {
        reply: oneshot::Sender<Vec<soundfonts::SoundfontInfo>>,
    },
    /// One note on, then off, on an instrument — the "is this thing making
    /// sound?" button. Answers whether the instrument exists at all.
    TestNote {
        id: u32,
        reply: oneshot::Sender<bool>,
    },
    /// Silence every held note everywhere.
    Panic,
}

#[derive(Clone)]
pub struct InstrumentHandle {
    tx: Sender<InstrumentMsg>,
}

impl InstrumentHandle {
    pub fn new(tx: Sender<InstrumentMsg>) -> Self {
        InstrumentHandle { tx }
    }

    /// Sync and non-blocking (unbounded channel) — safe from async context.
    pub fn routes_changed(&self, routes: Vec<trib_core::MidiRoute>) {
        let _ = self.tx.send(InstrumentMsg::RoutesChanged(routes));
    }

    pub fn instruments_changed(&self, instruments: Vec<InstrumentState>) {
        let _ = self.tx.send(InstrumentMsg::InstrumentsChanged(instruments));
    }

    pub async fn report(&self) -> MidiReport {
        self.request(|reply| InstrumentMsg::Report { reply }).await
    }

    pub async fn refresh(&self) -> MidiReport {
        self.request(|reply| InstrumentMsg::Refresh { reply }).await
    }

    pub async fn library(&self) -> Vec<soundfonts::SoundfontInfo> {
        let (reply, response) = oneshot::channel();
        if self.tx.send(InstrumentMsg::Library { reply }).is_err() {
            return Vec::new();
        }
        response.await.unwrap_or_default()
    }

    /// `false` = no such instrument.
    pub async fn test_note(&self, id: u32) -> bool {
        let (reply, response) = oneshot::channel();
        if self.tx.send(InstrumentMsg::TestNote { id, reply }).is_err() {
            return false;
        }
        response.await.unwrap_or(false)
    }

    pub fn panic(&self) {
        let _ = self.tx.send(InstrumentMsg::Panic);
    }

    async fn request(
        &self,
        make: impl FnOnce(oneshot::Sender<MidiReport>) -> InstrumentMsg,
    ) -> MidiReport {
        let (reply, response) = oneshot::channel();
        if self.tx.send(make(reply)).is_err() {
            return MidiReport::default();
        }
        response.await.unwrap_or_default()
    }
}

/// What the host publishes when it has built a rack.
pub type RackReady = Box<dyn Fn(Box<InstrumentRack>) + Send>;

pub fn spawn(
    rx: Receiver<InstrumentMsg>,
    library: crate::soundfonts::Library,
    sample_rate: u32,
    midi_tx: Producer<MidiEvent>,
    out: Arc<crate::midi_out::OutPorts>,
    rack_ready: RackReady,
) {
    std::thread::Builder::new()
        .name("trib-instruments".into())
        .spawn(move || {
            Host::new(library, sample_rate, midi_tx, out, rack_ready).run(rx);
        })
        .expect("instrument host thread spawns");
}

/// One instrument's build outcome, kept beside the document so the report
/// can explain a silence without re-reading the disk.
struct Built {
    state: InstrumentState,
    status: InstrumentStatus,
    reason: Option<String>,
    presets: Option<u16>,
    resident_bytes: Option<u64>,
}

struct Host {
    library: crate::soundfonts::Library,
    sample_rate: u32,
    mounts: Vec<(PathBuf, String)>,
    /// Loaded soundfonts by library id.
    ///
    /// Load-bearing for memory, not speed: a rebuild that re-read the file
    /// would hold the old rack's copy and the new one at the same time, and
    /// on a 512 MB Pi that is the difference between a preset change and an
    /// OOM kill.
    cache: HashMap<String, Arc<SoundFont>>,
    built: Vec<Built>,
    ports: crate::midi_in::Ports,
    /// MIDI OUT. Owned here rather than on a thread of its own, because
    /// "what MIDI is connected" would otherwise have two owners and drift.
    out: Arc<crate::midi_out::OutPorts>,
    /// What the echo closure reads. Shared with the midir input callbacks,
    /// which run on their own threads per port.
    echo: Arc<Mutex<EchoState>>,
    routes: Vec<trib_core::MidiRoute>,
    rack_ready: RackReady,
}

/// The document the echo fans out against.
#[derive(Debug, Default)]
struct EchoState {
    routes: Vec<trib_core::MidiRoute>,
    instruments: Vec<InstrumentState>,
    /// Input port index → name, the reverse of what `index_ports` built.
    port_names: Vec<String>,
}

impl Host {
    fn new(
        library: crate::soundfonts::Library,
        sample_rate: u32,
        midi_tx: Producer<MidiEvent>,
        out: Arc<crate::midi_out::OutPorts>,
        rack_ready: RackReady,
    ) -> Self {
        let echo: Arc<Mutex<EchoState>> = Arc::default();
        // The echo closure the midir input callbacks run. It reads the
        // document under a lock the audio thread never touches, decides
        // purely (`midi_echo::fan_out`), and writes to the ports.
        let echo_fn: crate::midi_in::Echo = {
            let echo = Arc::clone(&echo);
            let out = Arc::clone(&out);
            Arc::new(move |event| {
                let state = echo.lock().expect("echo lock");
                if state.routes.is_empty() {
                    return;
                }
                for outgoing in crate::midi_echo::fan_out(
                    &state.routes,
                    &state.instruments,
                    &state.port_names,
                    event,
                ) {
                    out.send(&outgoing);
                }
            })
        };
        Host {
            library,
            sample_rate,
            mounts: Vec::new(),
            cache: HashMap::new(),
            built: Vec::new(),
            ports: crate::midi_in::Ports::new(midi_tx, echo_fn),
            out,
            echo,
            routes: Vec::new(),
            rack_ready,
        }
    }

    /// Removable volumes that might carry soundfonts.
    ///
    /// Enumerated here rather than pushed from the mount watcher: that
    /// watcher skips enumeration entirely unless somebody is subscribed to
    /// the destinations channel, so a stick plugged in with only the
    /// Instruments tab open would never have been noticed. Blocking on
    /// `lsblk` is exactly what this thread exists to absorb.
    fn rescan_mounts(&mut self) {
        self.mounts = crate::destinations::enumerate()
            .into_iter()
            .filter(|drive| drive.removable)
            .filter_map(|drive| {
                let mount = drive.mount_point?;
                Some((PathBuf::from(mount), drive.label))
            })
            .collect();
    }

    fn run(mut self, rx: Receiver<InstrumentMsg>) {
        while let Ok(msg) = rx.recv() {
            match msg {
                InstrumentMsg::InstrumentsChanged(instruments) => self.rebuild(&instruments),
                InstrumentMsg::Report { reply } => {
                    let _ = reply.send(self.report());
                }
                InstrumentMsg::Refresh { reply } => {
                    self.ports.refresh();
                    self.out.refresh();
                    self.out.bind(&self.routes);
                    self.rescan_mounts();
                    let document: Vec<InstrumentState> =
                        self.built.iter().map(|b| b.state.clone()).collect();
                    self.rebuild(&document);
                    let _ = reply.send(self.report());
                }
                InstrumentMsg::Library { reply } => {
                    let _ = reply.send(self.library.list(&self.mounts));
                }
                InstrumentMsg::TestNote { id, reply } => {
                    let exists = self.built.iter().any(|b| b.state.id.0 == id);
                    if exists {
                        self.ports.test_note(id);
                    }
                    let _ = reply.send(exists);
                }
                InstrumentMsg::RoutesChanged(routes) => self.set_routes(routes),
                // One button, both directions. A hanging note on an
                // EXTERNAL synth is the worse case — nothing else can stop
                // it — so a panic that silenced only the rack would be a
                // trap rather than a convenience.
                InstrumentMsg::Panic => {
                    self.ports.panic();
                    self.out.panic();
                }
            }
        }
    }

    /// Load what the document asks for and publish a fresh rack.
    ///
    /// Always publishes, even when nothing loaded: an empty rack is how the
    /// engine learns that the last instrument went away.
    fn rebuild(&mut self, document: &[InstrumentState]) {
        self.ports.bind(document);
        // The echo matches by port NAME and the rack by index, so the echo
        // needs the same index→name table the rack was built from.
        {
            let mut echo = self.echo.lock().expect("echo lock");
            echo.instruments = document.to_vec();
            echo.port_names = crate::midi_in::index_ports(document);
        }
        let mut instruments = Vec::with_capacity(document.len());
        let mut built = Vec::with_capacity(document.len());

        let mut widths = Vec::with_capacity(document.len());
        for state in document {
            let (voices, outcome) = self.build_one(state);
            // A split kit is several voices sharing one soundfont, each
            // hearing only its own keys — so the rack sees N mono slots,
            // not one instrument with N outputs. The channel layout is the
            // same either way, which is what `compile` resolves against.
            for voice in voices {
                widths.push(if state.splits.is_empty() { 2 } else { 1 });
                instruments.push(voice);
            }
            built.push(outcome);
        }

        // Drop cache entries nothing references any more — otherwise a
        // soundfont stays resident forever after the instrument that
        // wanted it is deleted.
        let wanted: Vec<&String> = document
            .iter()
            .filter_map(|s| s.soundfont.as_ref())
            .collect();
        self.cache.retain(|id, _| wanted.contains(&id));

        self.built = built;
        (self.rack_ready)(Box::new(InstrumentRack::new(instruments, widths)));
    }

    /// Build one instrument, or a silent placeholder that knows why.
    ///
    /// A failure is never fatal and never skips the slot: the rack's Nth
    /// instrument must stay the document's Nth instrument, because the
    /// compiled graph resolves patches by position.
    fn build_one(&mut self, state: &InstrumentState) -> (Vec<Option<Instrument>>, Built) {
        // Every slot the console laid out must exist even when nothing
        // loads: the compiled graph resolves patches by position, so a
        // short rack would move a later instrument's audio onto the wrong
        // strip.
        let empty = || {
            (0..state.channels_slots())
                .map(|_| None)
                .collect::<Vec<_>>()
        };
        let silent = |status, reason: &str| Built {
            state: state.clone(),
            status,
            reason: Some(reason.to_owned()),
            presets: None,
            resident_bytes: None,
        };

        let Some(id) = state.soundfont.as_deref() else {
            return (
                empty(),
                silent(InstrumentStatus::Missing, "no soundfont chosen"),
            );
        };

        let soundfont = match self.load(id) {
            Ok(sf) => sf,
            Err(reason) => return (empty(), silent(InstrumentStatus::Failed, &reason)),
        };

        let binding = MidiBinding {
            port: self.ports.index_of(state.port.as_deref()),
            channel: state.midi_channel,
        };
        // One voice for the whole keyboard, or one per split. They share the
        // `Arc<SoundFont>`, so a six-piece kit costs no extra sample memory,
        // and each voice only ever sounds its own keys, so the voices in
        // flight are the same as one synth would carry.
        let filters: Vec<KeyFilter> = if state.splits.is_empty() {
            vec![KeyFilter::all()]
        } else {
            state
                .splits
                .iter()
                .map(|split| KeyFilter {
                    ranges: split.ranges.clone(),
                })
                .collect()
        };

        let mut voices = Vec::with_capacity(filters.len());
        for keys in filters {
            let spec = VoiceSpec {
                sample_rate: self.sample_rate,
                polyphony: state.polyphony,
                effects: state.effects,
                bank: state.bank,
                program: state.program,
                binding,
                keys,
            };
            match Instrument::new(&soundfont, &spec) {
                Ok(voice) => voices.push(Some(voice)),
                Err(err) => {
                    return (
                        empty(),
                        silent(InstrumentStatus::Failed, &format!("{id}: {err}")),
                    );
                }
            }
        }

        let presets = u16::try_from(soundfont.get_presets().len()).ok();
        let resident_bytes = resident_bytes(&soundfont);
        let (status, reason) = match (&state.port, binding.port) {
            (None, _) => (
                InstrumentStatus::Ready,
                Some("no MIDI input chosen".to_owned()),
            ),
            (Some(name), None) => (
                InstrumentStatus::Missing,
                Some(format!("its MIDI input “{name}” is not connected")),
            ),
            (Some(_), Some(_)) => (InstrumentStatus::Live, None),
        };

        (
            voices,
            Built {
                state: state.clone(),
                status,
                reason,
                presets,
                resident_bytes,
            },
        )
    }

    fn load(&mut self, id: &str) -> Result<Arc<SoundFont>, String> {
        if let Some(hit) = self.cache.get(id) {
            return Ok(Arc::clone(hit));
        }
        let path = self
            .library
            .resolve(&self.mounts, id)
            .ok_or_else(|| format!("“{id}” is not in the soundfont library"))?;
        let mut file =
            std::fs::File::open(&path).map_err(|e| format!("“{id}” could not be opened: {e}"))?;
        let soundfont = SoundFont::new(&mut file)
            .map_err(|e| format!("“{id}” is not a SoundFont this daemon can read: {e}"))?;
        let soundfont = Arc::new(soundfont);
        self.cache.insert(id.to_owned(), Arc::clone(&soundfont));
        Ok(soundfont)
    }

    fn report(&self) -> MidiReport {
        let instruments = self
            .built
            .iter()
            .map(|b| InstrumentReport {
                id: b.state.id.0,
                name: b.state.name.clone(),
                status: b.status.clone(),
                reason: b.reason.clone(),
                presets: b.presets,
                resident_bytes: b.resident_bytes,
            })
            .collect();
        let wanted: Vec<&str> = self
            .built
            .iter()
            .filter_map(|b| b.state.port.as_deref())
            .collect();
        MidiReport {
            instruments,
            inputs: self.ports.report(&wanted),
            outputs: self.out.report(&self.routes),
        }
    }

    /// Publish the routes to the ports and to the echo closure.
    fn set_routes(&mut self, routes: Vec<trib_core::MidiRoute>) {
        self.out.bind(&routes);
        self.routes = routes.clone();
        self.echo.lock().expect("echo lock").routes = routes;
    }
}

/// Sample bytes a loaded SoundFont holds resident. `wave_data` is `i16`, so
/// two bytes a frame — this is the number that decides whether a Pi copes.
fn resident_bytes(soundfont: &SoundFont) -> Option<u64> {
    u64::try_from(soundfont.get_wave_data().len())
        .ok()
        .map(|frames| frames * 2)
}

#[cfg(test)]
mod tests {
    use super::*;
    use trib_core::InstrumentId;

    /// A host whose user library is `root` and which ships no built-ins.
    /// `builtin_host` covers the other arrangement.
    fn host(root: PathBuf) -> (Host, rtrb::Consumer<MidiEvent>) {
        library_host(crate::soundfonts::Library {
            builtin: PathBuf::from("/nonexistent-builtin-library"),
            user: root,
        })
    }

    fn library_host(library: crate::soundfonts::Library) -> (Host, rtrb::Consumer<MidiEvent>) {
        let (tx, rx) = rtrb::RingBuffer::new(64);
        let host = Host::new(
            library,
            48_000,
            tx,
            Arc::new(crate::midi_out::OutPorts::new()),
            Box::new(|_| {}),
        );
        (host, rx)
    }

    fn with_soundfont(id: &str) -> (tempfile::TempDir, InstrumentState) {
        let root = tempfile::tempdir().unwrap();
        std::fs::write(
            root.path().join(id),
            trib_engine::test_support::sine_soundfont(),
        )
        .unwrap();
        let mut state = InstrumentState::new(InstrumentId(0), "Rhodes".into());
        state.soundfont = Some(id.to_owned());
        (root, state)
    }

    #[test]
    fn an_instrument_with_no_soundfont_builds_silent_and_says_so() {
        let (mut host, _rx) = host(PathBuf::from("/nowhere"));
        let state = InstrumentState::new(InstrumentId(0), "Empty".into());
        let (voices, built) = host.build_one(&state);
        assert!(voices.iter().all(Option::is_none));
        assert_eq!(built.status, InstrumentStatus::Missing);
        assert_eq!(built.reason.as_deref(), Some("no soundfont chosen"));
    }

    #[test]
    fn a_soundfont_that_is_not_in_the_library_reports_its_name() {
        let (mut host, _rx) = host(PathBuf::from("/nowhere"));
        let mut state = InstrumentState::new(InstrumentId(0), "Rhodes".into());
        state.soundfont = Some("missing.sf2".into());
        let (voices, built) = host.build_one(&state);
        assert!(voices.iter().all(Option::is_none));
        assert_eq!(built.status, InstrumentStatus::Failed);
        assert!(built.reason.unwrap().contains("missing.sf2"));
    }

    #[test]
    fn a_loaded_instrument_with_no_port_is_ready_not_live() {
        // "Ready" and "Live" are different sentences on purpose: one is
        // waiting to be wired, the other is waiting to be played.
        let (root, state) = with_soundfont("sine.sf2");
        let (mut host, _rx) = host(root.path().to_path_buf());
        let (voices, built) = host.build_one(&state);
        assert_eq!(voices.len(), 1, "an unsplit instrument is one stereo voice");
        assert!(voices[0].is_some());
        assert_eq!(built.status, InstrumentStatus::Ready);
        assert_eq!(built.reason.as_deref(), Some("no MIDI input chosen"));
        assert_eq!(built.presets, Some(1));
        assert!(built.resident_bytes.unwrap() > 0);
    }

    #[test]
    fn an_instrument_naming_a_port_that_is_not_there_reports_the_port() {
        let (root, mut state) = with_soundfont("sine.sf2");
        state.port = Some("nanoKEY2 MIDI 1".into());
        let (mut host, _rx) = host(root.path().to_path_buf());
        let (voices, built) = host.build_one(&state);
        // It still builds: the sound is loaded and ready the moment the
        // keyboard is plugged back in.
        assert!(voices[0].is_some());
        assert_eq!(built.status, InstrumentStatus::Missing);
        assert!(built.reason.unwrap().contains("nanoKEY2 MIDI 1"));
    }

    #[test]
    fn two_instruments_on_one_soundfont_share_one_load() {
        let (root, _) = with_soundfont("sine.sf2");
        let (mut host, _rx) = host(root.path().to_path_buf());
        let first = host.load("sine.sf2").unwrap();
        let second = host.load("sine.sf2").unwrap();
        assert!(
            Arc::ptr_eq(&first, &second),
            "a second instrument must not read the file again"
        );
        assert_eq!(host.cache.len(), 1);
    }

    #[test]
    fn a_rebuild_reuses_the_cached_soundfont_rather_than_loading_a_second_copy() {
        // The one that matters on a 512 MB Pi: without this, a preset
        // change holds the old rack's samples and the new rack's at once.
        let (root, state) = with_soundfont("sine.sf2");
        let (mut host, _rx) = host(root.path().to_path_buf());
        host.rebuild(std::slice::from_ref(&state));
        let first = Arc::clone(host.cache.get("sine.sf2").unwrap());
        host.rebuild(std::slice::from_ref(&state));
        let second = Arc::clone(host.cache.get("sine.sf2").unwrap());
        assert!(Arc::ptr_eq(&first, &second));
    }

    #[test]
    fn a_soundfont_nothing_references_any_more_stops_being_resident() {
        let (root, state) = with_soundfont("sine.sf2");
        let (mut host, _rx) = host(root.path().to_path_buf());
        host.rebuild(std::slice::from_ref(&state));
        assert_eq!(host.cache.len(), 1);
        host.rebuild(&[]);
        assert!(host.cache.is_empty(), "the samples must not stay resident");
    }

    #[test]
    fn a_failed_instrument_still_holds_its_slot_in_the_rack() {
        // Positional resolution: the compiled graph patches by index, so a
        // failure that shifted its neighbours would move audio to the
        // wrong strip.
        let (root, good) = with_soundfont("sine.sf2");
        let mut broken = InstrumentState::new(InstrumentId(1), "Broken".into());
        broken.soundfont = Some("missing.sf2".into());
        let mut second = good.clone();
        second.id = InstrumentId(2);

        let (mut host, _rx) = host(root.path().to_path_buf());
        host.rebuild(&[good, broken, second]);
        assert_eq!(host.built.len(), 3);
        assert_eq!(host.built[1].status, InstrumentStatus::Failed);
        assert_eq!(host.built[2].status, InstrumentStatus::Ready);
    }
}
