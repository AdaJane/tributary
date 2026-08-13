//! Republishes the destination list when the mount table changes.
//!
//! Event-driven, not polled: the kernel raises `POLLPRI` on
//! `/proc/self/mountinfo` every time something is mounted or unmounted, so
//! a blocked `poll(2)` costs nothing until a drive actually appears. That
//! matters for the doctrine ("the daemon never polls") and for the UX — a
//! drive mounts a beat after it is plugged in, which is exactly why a
//! manual Rescan button is a guessing game.
//!
//! Its own thread rather than a tokio task: the wait is a blocking syscall
//! and enumeration shells out to `lsblk`, matching how the device
//! orchestrator and the monitor pump already work.

use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::time::Duration;

use rustix::event::{PollFd, PollFlags, poll};

use crate::api::destinations::drive_dtos;
use crate::api::ws::{Channel, ServerMessage};
use crate::hub::Hub;
use crate::registry::ChannelRegistry;

const MOUNTINFO: &str = "/proc/self/mountinfo";

/// Inserting one drive fires several mount events in quick succession
/// (partition scan, then each filesystem), and the mount lands a moment
/// after the device does. Waiting this long before enumerating turns a
/// burst into one publish and lets the mount settle first.
const SETTLE: Duration = Duration::from_millis(400);

/// Watch the mount table and publish `destinations` whenever it changes.
/// Silent no-op on platforms without procfs — the manual Rescan path stays
/// correct either way, which is what makes this safe to degrade.
pub fn spawn(hub: Hub, registry: ChannelRegistry) {
    if let Err(e) = std::thread::Builder::new()
        .name("mount-watch".into())
        .spawn(move || watch_loop(hub, registry))
    {
        tracing::warn!(%e, "could not start the mount watcher; Rescan still works");
    }
}

/// Re-reading is not bookkeeping — it is what re-arms the kernel. The
/// procfs poll handler compares a per-open event counter against the mount
/// namespace's, so without a read after each wake `poll` returns
/// immediately forever and the thread spins.
fn rearm(file: &mut File, buf: &mut String) {
    buf.clear();
    let _ = file.seek(SeekFrom::Start(0));
    let _ = file.read_to_string(buf);
}

fn watch_loop(hub: Hub, registry: ChannelRegistry) {
    let Ok(mut file) = File::open(MOUNTINFO) else {
        tracing::warn!("{MOUNTINFO} unavailable; hotplug updates are off (Rescan still works)");
        return;
    };
    let mut buf = String::new();
    // Prime the counter, or the first poll returns before anything changed.
    rearm(&mut file, &mut buf);

    let mut last: Option<Vec<crate::api::destinations::DriveDto>> = None;
    loop {
        let mut fds = [PollFd::new(&file, PollFlags::PRI)];
        // No timeout: park until the kernel says the mount table moved.
        if let Err(e) = poll(&mut fds, None) {
            tracing::warn!(%e, "mount watch poll failed; hotplug updates are off");
            return;
        }
        // Let the burst finish and the filesystem actually mount, then
        // re-arm once — events during the wait are absorbed by this read.
        std::thread::sleep(SETTLE);
        rearm(&mut file, &mut buf);

        // Nobody watching costs nothing but the re-arm above: no lsblk, and
        // no write probes on somebody's drive.
        if !registry.is_watched(&Channel::Destinations) {
            last = None;
            continue;
        }
        let drives = drive_dtos();
        if last.as_ref() == Some(&drives) {
            continue;
        }
        tracing::info!(count = drives.len(), "mount table changed");
        last = Some(drives.clone());
        hub.publish(
            Channel::Destinations,
            &ServerMessage::Destinations { drives },
        );
    }
}
