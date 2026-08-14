//! What the machine actually granted the audio thread, probed once.
//!
//! Kept apart from the backend that uses it because the decision is a
//! table, not a cascade, and a table is worth testing without opening a
//! sound card.

use crate::backend::{RealtimeStatus, Scheduling};

/// Priority the audio thread asks for.
///
/// Chosen against what else is on the box, not in a vacuum. On the
/// appliance PipeWire runs as the SAME user in the SAME session, so the
/// `rtprio` limit granted for us is necessarily granted to it too — and
/// its data thread defaults to 88. The image lowers PipeWire below this
/// rather than raising us above the kernel's USB IRQ threads, which we
/// depend on being serviced first.
pub const RT_PRIORITY: u32 = 10;

/// PipeWire's stock `rt.prio`, read out of
/// `/usr/share/pipewire/pipewire.conf`.
const PIPEWIRE_RT_PRIO: u32 = 88;

/// A compile-time guard rather than a test, because it must hold for every
/// build and not only under `cargo test`.
///
/// The appliance runs PipeWire as the SAME user in the SAME session, so
/// the `rtprio` limit granted for the audio thread necessarily reaches
/// PipeWire too. The image lowers PipeWire beneath us; if [`RT_PRIORITY`]
/// ever climbs past its default without that drop-in, PipeWire preempts
/// the audio thread and the entire real-time exercise is undone.
const _: () = assert!(RT_PRIORITY < PIPEWIRE_RT_PRIO);

/// Memory we want pinned before we ask to lock any.
///
/// Locking is skipped rather than attempted when the limit is smaller than
/// this: a partial lock buys nothing and the failure is noise.
pub const MEMLOCK_FLOOR: u64 = 8 * 1024 * 1024;

/// What the limits allow, before anything is attempted.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Limits {
    /// `RLIMIT_RTPRIO`, soft.
    pub rtprio: u64,
    /// `RLIMIT_MEMLOCK`, soft. `None` = unlimited.
    pub memlock: Option<u64>,
}

/// Decide the posture from the limits. Pure; the caller does the syscalls.
///
/// Three rows, and the third is the one that runs on every untuned system:
/// `ulimit -r` is 0 out of the box, so SCHED_FIFO returns EPERM and the
/// honest answer is to say so rather than claim real-time scheduling we
/// did not get and be blamed for xruns nobody can explain.
pub fn posture(limits: Limits) -> RealtimeStatus {
    if u64::from(RT_PRIORITY) > limits.rtprio {
        return RealtimeStatus {
            scheduling: Scheduling::Other,
            priority: None,
            memory_locked: false,
            reason: Some(format!(
                "no real-time scheduling: RLIMIT_RTPRIO is {} and this needs {RT_PRIORITY}. \
                 Grant it with a limits.d drop-in or LimitRTPRIO= on the unit. \
                 Audio still runs; xruns under load are expected.",
                limits.rtprio
            )),
        };
    }
    let memory_locked = limits.memlock.is_none_or(|bytes| bytes >= MEMLOCK_FLOOR);
    RealtimeStatus {
        scheduling: Scheduling::Fifo,
        priority: Some(RT_PRIORITY),
        memory_locked,
        reason: (!memory_locked).then(|| {
            format!(
                "real-time scheduling granted, but memory is not locked: RLIMIT_MEMLOCK is {} \
                 and this needs {MEMLOCK_FLOOR}. Raise it with LimitMEMLOCK= on the unit. \
                 A page fault in the render loop shows up as an xrun.",
                limits.memlock.unwrap_or(0)
            )
        }),
    }
}

/// Read the two limits that decide the posture.
#[cfg(feature = "alsa-backend")]
pub fn limits() -> Limits {
    use rustix::process::{Resource, getrlimit};
    Limits {
        rtprio: getrlimit(Resource::Rtprio).current.unwrap_or(0),
        memlock: getrlimit(Resource::Memlock).current,
    }
}

/// Apply the posture to the CURRENT thread. Returns what actually stuck.
///
/// Never fails the boot: a console that will not start because it could
/// not get real-time scheduling is worse than one that says it did not.
#[cfg(feature = "alsa-backend")]
pub fn apply(status: RealtimeStatus) -> RealtimeStatus {
    use thread_priority::{
        RealtimeThreadSchedulePolicy, ThreadPriority, ThreadPriorityValue, ThreadSchedulePolicy,
        set_thread_priority_and_policy, thread_native_id,
    };

    let mut status = status;
    if status.scheduling == Scheduling::Fifo {
        let value = ThreadPriorityValue::try_from(RT_PRIORITY as u8);
        let applied = value.ok().is_some_and(|priority| {
            set_thread_priority_and_policy(
                thread_native_id(),
                ThreadPriority::Crossplatform(priority),
                ThreadSchedulePolicy::Realtime(RealtimeThreadSchedulePolicy::Fifo),
            )
            .is_ok()
        });
        if !applied {
            status.scheduling = Scheduling::Other;
            status.priority = None;
            status.reason = Some(
                "the kernel refused SCHED_FIFO even though the limit allows it. \
                 Audio still runs; xruns under load are expected."
                    .to_owned(),
            );
        }
    }
    if status.memory_locked {
        // MCL_CURRENT ONLY. With MCL_FUTURE a 64 MB soundfont against a
        // typical 8 MB LimitMEMLOCK makes every LATER allocation fail —
        // the allocator returns null and the process aborts. Locking what
        // is already resident is the whole benefit anyway: the render
        // loop's buffers are allocated before this runs.
        let locked = rustix::mm::mlockall(rustix::mm::MlockAllFlags::CURRENT).is_ok();
        if !locked {
            status.memory_locked = false;
            status.reason.get_or_insert_with(|| {
                "memory could not be locked; a page fault in the render loop shows up as an xrun"
                    .to_owned()
            });
        }
    }
    status
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn no_rtprio_degrades_to_other_scheduling_and_says_how_to_fix_it() {
        // The arm every untuned machine takes, including this development
        // box — `ulimit -r` is 0 out of the box.
        let status = posture(Limits {
            rtprio: 0,
            memlock: None,
        });
        assert_eq!(status.scheduling, Scheduling::Other);
        assert!(status.priority.is_none());
        assert!(!status.memory_locked, "no point locking without RT anyway");
        let reason = status.reason.unwrap();
        assert!(reason.contains("RLIMIT_RTPRIO"));
        assert!(
            reason.contains("LimitRTPRIO"),
            "name the fix, not the fault"
        );
    }

    #[test]
    fn a_small_memlock_limit_skips_locking_rather_than_failing_to_boot() {
        let status = posture(Limits {
            rtprio: 95,
            memlock: Some(64 * 1024),
        });
        assert_eq!(status.scheduling, Scheduling::Fifo);
        assert_eq!(status.priority, Some(RT_PRIORITY));
        assert!(!status.memory_locked);
        assert!(status.reason.unwrap().contains("LimitMEMLOCK"));
    }

    #[test]
    fn a_fully_granted_machine_says_nothing_at_all() {
        // Silence on success: a reason exists to explain a shortfall, and
        // there is no shortfall here.
        let status = posture(Limits {
            rtprio: 95,
            memlock: None,
        });
        assert_eq!(status.scheduling, Scheduling::Fifo);
        assert!(status.memory_locked);
        assert!(status.reason.is_none());
    }
}
