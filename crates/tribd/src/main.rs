//! tribd — the Tributary daemon. Owns all real-time audio; the web console
//! is a view onto it.

mod api;
mod console;
mod destinations;
mod device_host;
mod device_match;
mod engine_host;
mod format;
mod hub;
mod meter_pump;
mod monitor_pump;
mod mount_watch;
mod recording_prefs;
mod registry;
mod settings;

use std::path::PathBuf;
use std::sync::Arc;

use clap::{Parser, Subcommand};
use trib_audio::{AudioBackend, StreamConfig};
use trib_engine::{compile, engine_pair};

/// Hub backlog before a slow WS client starts skipping. Meters at 20 Hz are
/// the busiest stream, so this is ~12 s of headroom.
const HUB_CAPACITY: usize = 256;

#[derive(Parser)]
#[command(name = "tribd", about = "Tributary daemon — real-time mixing engine")]
struct Cli {
    /// Path to the base config file (missing file = defaults).
    #[arg(long, default_value = "config/tribd.toml")]
    config: PathBuf,
    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Subcommand)]
enum Command {
    /// Print the OpenAPI spec to stdout (the committed openapi.json).
    Openapi,
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let cli = Cli::parse();
    match cli.command {
        // Bare artifact on stdout: the command exists to be piped.
        Some(Command::Openapi) => {
            println!("{}", api::openapi_json());
            Ok(())
        }
        None => run(&cli.config).await,
    }
}

#[cfg(feature = "hardware")]
fn backend() -> Arc<dyn AudioBackend> {
    Arc::new(trib_audio::CpalBackend)
}

#[cfg(not(feature = "hardware"))]
fn backend() -> Arc<dyn AudioBackend> {
    tracing::warn!(
        "built without the `hardware` feature — running the FAKE audio backend \
         (440 Hz test tone, no devices). Install libasound2-dev and run with \
         `--features hardware` for real IO."
    );
    Arc::new(trib_audio::FakeBackend::default())
}

async fn run(config_path: &std::path::Path) -> Result<(), Box<dyn std::error::Error>> {
    // Every crate that can explain a silent input has to be named here.
    // `EnvFilter` enables only the targets it lists, so a bare "tribd=info"
    // dropped the whole audio layer on the floor: "no Pulse/PipeWire server
    // answered", "input stream error" and the monitor-output warnings were
    // all being written and none of them ever reached the journal. RUST_LOG
    // still overrides this wholesale.
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| {
                "tribd=info,trib_audio=info,trib_project=info,trib_engine=info".into()
            }),
        )
        .init();

    let settings = settings::load(config_path)?;
    let prefs_path = recording_prefs::prefs_path(config_path);
    let prefs = recording_prefs::RecordingPrefs::load(&prefs_path)?;
    let hub = hub::Hub::new(HUB_CAPACITY);
    let registry = registry::ChannelRegistry::default();

    // Roots go absolute up front: the settings API hands them back as
    // destination paths, and the PUT only accepts absolute ones.
    let default_root = std::path::absolute(&settings.projects.root)?;
    // The configured destination, if it's actually there. A missing or
    // unwritable drive falls back to the default root WITHOUT clearing the
    // pref — the settings API reports the divergence so the console can
    // show it.
    let projects_root = {
        let configured = std::path::absolute(prefs.effective_root(&settings))?;
        let usable =
            std::fs::create_dir_all(&configured).is_ok() && destinations::writable(&configured);
        if usable {
            configured
        } else {
            tracing::warn!(
                configured = %configured.display(),
                fallback = %default_root.display(),
                "configured destination unavailable; recording to the default root"
            );
            default_root.clone()
        }
    };
    let sample_rate = prefs.effective_sample_rate(&settings);

    // Reopen the latest project, or tear off fresh tape: one strip patched
    // to input 0 and the classic AUX 1/2 → reverb/delay loop pre-wired.
    // A restart is never an instruction to resume recording — arm flags
    // persist, but the transport always boots stopped.
    std::fs::create_dir_all(&projects_root)?;
    let (project, initial, project_created_at) = match trib_project::load_latest(&projects_root) {
        Some((project, manifest)) => {
            tracing::info!(name = %project.name, "reopened project");
            (project, manifest.mixer, manifest.created_at_unix)
        }
        None => {
            let template = console::fresh_console();
            let project = trib_project::create_project(&projects_root, "Session", &template)?;
            let manifest = trib_project::load_latest(&projects_root)
                .expect("just-created project loads")
                .1;
            tracing::info!(dir = %project.dir.display(), "fresh tape");
            (project, template, manifest.created_at_unix)
        }
    };
    // The boot graph resolves against an EMPTY slot map — honest silence
    // until the device orchestrator opens what the project wants and its
    // first slot map triggers the real compile.
    let compiled = compile(
        &initial,
        sample_rate,
        settings.audio.block_size,
        &trib_engine::InputSlots::default(),
    );
    let (engine_handle, engine) = engine_pair(compiled.graph);

    // Audio failure degrades (state edits still work); it never kills the
    // daemon. The command ring then has no consumer, which the control task
    // reports loudly on every dropped write.
    let audio = backend();
    let stream = match audio.start(
        &StreamConfig {
            sample_rate,
            block_size: settings.audio.block_size,
        },
        engine,
    ) {
        Ok(stream) => Some(stream),
        Err(e) => {
            tracing::error!(%e, "audio backend failed to start; running without audio");
            None
        }
    };
    // The orchestrator owns the stream handle from here: input devices
    // open on demand (when patched) and close when unpatched. Audio stops
    // when its channel closes at shutdown and the thread drops the handle.
    let (device_tx, device_rx) = std::sync::mpsc::channel();
    let devices = device_host::DeviceHandle::new(device_tx);

    let (meter_keys_tx, meter_keys_rx) = tokio::sync::watch::channel(compiled.meter_keys);
    // The control task owns the project and republishes it on a
    // destination swap; the API reads whatever is current.
    let (project_watch_tx, project_watch_rx) =
        tokio::sync::watch::channel(Arc::new(project.clone()));
    let monitor_generation = Arc::new(std::sync::atomic::AtomicU32::new(0));
    let control = engine_host::spawn(
        hub.clone(),
        engine_handle.cmd_tx,
        initial,
        compiled.params,
        engine_host::EngineConfig {
            sample_rate,
            block_size: settings.audio.block_size,
            monitor_generation: monitor_generation.clone(),
        },
        meter_keys_tx,
        project,
        project_created_at,
        engine_host::RecordingHost {
            prefs,
            prefs_path,
            default_root,
            default_sample_rate: settings.audio.sample_rate,
            active_root: projects_root,
            project_watch: project_watch_tx,
        },
        Some(devices.clone()),
    );
    device_host::spawn(device_rx, audio, stream, control.clone());
    meter_pump::spawn(
        hub.clone(),
        registry.clone(),
        engine_handle.meter_rx,
        engine_handle.retire_rx,
        meter_keys_rx,
    );
    // Small backlog: a lagged monitor listener should skip, not savor.
    let (monitor_tx, _) = tokio::sync::broadcast::channel(16);
    monitor_pump::spawn(
        engine_handle.monitor_rx,
        monitor_generation,
        monitor_tx.clone(),
    );

    // Drives appear on their own: the kernel wakes this on every mount
    // table change, so nothing anywhere polls for storage.
    mount_watch::spawn(hub.clone(), registry.clone());

    let state = api::AppState {
        hub,
        registry,
        cors_origins: Arc::new(settings.server.cors_origins.clone()),
        control,
        project: project_watch_rx,
        can_format: format::available(),
        monitor_tx,
        devices,
    };

    let listener = tokio::net::TcpListener::bind(&settings.server.bind).await?;
    tracing::info!(bind = %settings.server.bind, "tribd listening");
    axum::serve(listener, api::build(state))
        .with_graceful_shutdown(shutdown_signal())
        .await?;
    Ok(())
}

async fn shutdown_signal() {
    tokio::signal::ctrl_c()
        .await
        .expect("ctrl-c handler installs");
    tracing::info!("shutting down");
}
