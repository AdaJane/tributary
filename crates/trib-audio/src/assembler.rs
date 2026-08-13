//! The input assembler: merges per-device rings into the engine's fixed
//! `MAX_INPUT_CHANNELS`-wide interleaved frame inside the audio callback.
//! Devices attach and detach through rings (the EngineCommand doctrine);
//! detached inputs RETIRE back to the stream thread for dropping — the
//! callback never allocates or deallocates.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use rtrb::{Consumer, Producer};
use trib_engine::MAX_INPUT_CHANNELS;

/// Simultaneously attached input devices. Far above any real desk.
pub const MAX_INPUT_DEVICES: usize = 16;

/// Attach/detach and retire ring depth: every device attaching and
/// detaching in one tick still fits.
pub const ASSEMBLER_RING_CAPACITY: usize = MAX_INPUT_DEVICES * 2;

/// Health/telemetry shared between a device's stream and the control side.
#[derive(Debug, Default)]
pub struct InputShared {
    pub underruns: AtomicU64,
    /// Whole frames the producer dropped because the ring was full. Kept
    /// apart from `underruns` (the drained-ring count) because they blame
    /// opposite ends: overruns mean the engine isn't draining fast enough.
    pub overruns: AtomicU64,
    /// Set by the cpal error callback; the orchestrator reopens on Refresh.
    pub failed: AtomicBool,
}

/// Push whole device frames into `tx`, dropping any frame that does not fit
/// ENTIRELY. Returns the number of frames dropped.
///
/// This is the only sanctioned way to feed an input ring, because
/// [`InputAssembler::fill`] de-interleaves *positionally* — there is no
/// frame marker in the ring and no resynchronisation anywhere. A ring left
/// holding a partial frame therefore rotates every one of that device's
/// channels by the remainder, silently, for the rest of the stream's life.
/// Dropping a whole frame costs one frame; tearing one costs the take.
///
/// Allocation-free: safe to call from an audio callback.
pub fn push_frames(tx: &mut Producer<f32>, samples: &[f32], channels: usize) -> u64 {
    debug_assert!(channels > 0, "a device with no channels cannot produce");
    debug_assert!(
        samples.len().is_multiple_of(channels),
        "producer handed a partial frame: {} samples at {channels} channels",
        samples.len()
    );
    samples
        .chunks_exact(channels)
        .fold(0, |dropped, frame| match tx.write_chunk_uninit(channels) {
            Ok(chunk) => {
                chunk.fill_from_iter(frame.iter().copied());
                dropped
            }
            Err(_) => dropped + 1,
        })
}

/// One attached device: its ring consumer and where its channels land in
/// the engine frame.
pub struct Attached {
    pub consumer: Consumer<f32>,
    pub offset: u16,
    pub channels: u16,
    pub shared: Arc<InputShared>,
}

pub enum AssemblerCmd {
    Attach(Box<Attached>),
    /// Keyed by offset — unique while a device is open, and no String ever
    /// crosses onto the audio thread.
    Detach {
        offset: u16,
    },
}

pub struct InputAssembler {
    /// Fixed-capacity slot table; index is arbitrary, offset is identity.
    slots: Vec<Option<Box<Attached>>>,
    /// Retirees waiting for ring space — preallocated, never grows. The
    /// boxes are deliberate: parking moves a pointer, never an `Attached`,
    /// on the audio thread.
    #[allow(clippy::vec_box)]
    parked: Vec<Box<Attached>>,
    cmd_rx: Consumer<AssemblerCmd>,
    retire_tx: Producer<Box<Attached>>,
}

impl InputAssembler {
    pub fn new(cmd_rx: Consumer<AssemblerCmd>, retire_tx: Producer<Box<Attached>>) -> Self {
        InputAssembler {
            slots: (0..MAX_INPUT_DEVICES).map(|_| None).collect(),
            parked: Vec::with_capacity(MAX_INPUT_DEVICES),
            cmd_rx,
            retire_tx,
        }
    }

    /// Apply pending attach/detach commands. Bounded per callback.
    pub fn drain_commands(&mut self) {
        for _ in 0..ASSEMBLER_RING_CAPACITY {
            match self.cmd_rx.pop() {
                Ok(AssemblerCmd::Attach(attached)) => {
                    if let Some(slot) = self.slots.iter_mut().find(|s| s.is_none()) {
                        *slot = Some(attached);
                    } else {
                        // Table full (control-side bug): send it straight
                        // back rather than drop on the audio thread.
                        self.retire(attached);
                    }
                }
                Ok(AssemblerCmd::Detach { offset }) => {
                    if let Some(slot) = self
                        .slots
                        .iter_mut()
                        .find(|s| s.as_ref().is_some_and(|a| a.offset == offset))
                        && let Some(attached) = slot.take()
                    {
                        self.retire(attached);
                    }
                }
                Err(_) => break,
            }
        }
        self.flush_parked();
    }

    /// Fill `frame` (interleaved `MAX_INPUT_CHANNELS` wide) for `frames`
    /// frames: silence everywhere, then each attached device's samples at
    /// its offset. A starved ring yields zeros and counts.
    pub fn fill(&mut self, frame: &mut [f32], frames: usize) {
        debug_assert!(frame.len() >= frames * MAX_INPUT_CHANNELS);
        frame[..frames * MAX_INPUT_CHANNELS].fill(0.0);
        for slot in self.slots.iter_mut().flatten() {
            let channels = usize::from(slot.channels);
            let offset = usize::from(slot.offset);
            let mut underruns = 0u64;
            for f in 0..frames {
                let base = f * MAX_INPUT_CHANNELS + offset;
                for c in 0..channels {
                    frame[base + c] = match slot.consumer.pop() {
                        Ok(sample) => sample,
                        Err(_) => {
                            underruns += 1;
                            0.0
                        }
                    };
                }
            }
            if underruns > 0 {
                slot.shared
                    .underruns
                    .fetch_add(underruns, Ordering::Relaxed);
            }
        }
    }

    /// Never drop an Attached here — its consumer's Drop may free the ring
    /// allocation. The stream thread drops retirees.
    fn retire(&mut self, attached: Box<Attached>) {
        if let Err(rtrb::PushError::Full(attached)) = self.retire_tx.push(attached) {
            if self.parked.len() < MAX_INPUT_DEVICES {
                self.parked.push(attached);
            } else {
                debug_assert!(false, "assembler retire ring and parking both full");
            }
        }
    }

    fn flush_parked(&mut self) {
        while let Some(attached) = self.parked.pop() {
            if let Err(rtrb::PushError::Full(attached)) = self.retire_tx.push(attached) {
                self.parked.push(attached);
                break;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use rtrb::RingBuffer;

    use super::*;

    // The fill/drain path must stay allocation-free, like the engine.
    #[global_allocator]
    static GUARD: assert_no_alloc::AllocDisabler = assert_no_alloc::AllocDisabler;

    fn assembler() -> (
        Producer<AssemblerCmd>,
        Consumer<Box<Attached>>,
        InputAssembler,
    ) {
        let (cmd_tx, cmd_rx) = RingBuffer::new(ASSEMBLER_RING_CAPACITY);
        let (retire_tx, retire_rx) = RingBuffer::new(ASSEMBLER_RING_CAPACITY);
        (cmd_tx, retire_rx, InputAssembler::new(cmd_rx, retire_tx))
    }

    fn attached(offset: u16, channels: u16, samples: &[f32]) -> (Producer<f32>, Box<Attached>) {
        let (mut tx, rx) = RingBuffer::new(1024);
        for &s in samples {
            tx.push(s).unwrap();
        }
        (
            tx,
            Box::new(Attached {
                consumer: rx,
                offset,
                channels,
                shared: Arc::new(InputShared::default()),
            }),
        )
    }

    #[test]
    fn a_full_ring_drops_whole_frames_never_partial_ones() {
        // Capacity deliberately NOT a multiple of the channel count: the
        // ring must still never come to rest holding a torn frame.
        let (mut tx, rx) = RingBuffer::<f32>::new(5);
        let samples: Vec<f32> = (0..8).map(|i| i as f32).collect(); // 4 frames
        let dropped = assert_no_alloc::assert_no_alloc(|| push_frames(&mut tx, &samples, 2));
        assert_eq!(dropped, 2, "5 slots take 2 whole frames, so 2 are dropped");
        assert_eq!(
            rx.slots() % 2,
            0,
            "a partial frame in the ring rotates the device's channels forever"
        );
        assert_eq!(rx.slots(), 4);
    }

    #[test]
    fn surviving_frames_stay_intact_and_in_order() {
        let (mut tx, mut rx) = RingBuffer::<f32>::new(64);
        assert_eq!(push_frames(&mut tx, &[1.0, 2.0, 3.0, 4.0], 2), 0);
        let got: Vec<f32> = std::iter::from_fn(|| rx.pop().ok()).collect();
        assert_eq!(got, vec![1.0, 2.0, 3.0, 4.0]);
    }

    #[test]
    fn a_frame_that_does_not_fit_is_dropped_whole_not_truncated() {
        // One slot free, a 4-channel frame offered: all four samples must
        // be refused together.
        let (mut tx, rx) = RingBuffer::<f32>::new(4);
        push_frames(&mut tx, &[9.0, 9.0, 9.0], 3);
        assert_eq!(rx.slots(), 3, "the first frame fits");
        assert_eq!(push_frames(&mut tx, &[1.0, 2.0, 3.0], 3), 1);
        assert_eq!(rx.slots(), 3, "the second frame left nothing behind");
    }

    #[test]
    fn attached_devices_land_at_their_offsets_between_silence() {
        let (mut cmd_tx, _retire, mut asm) = assembler();
        // Mono at offset 0, stereo at offset 2.
        let (_a, mono) = attached(0, 1, &[0.1, 0.2]);
        let (_b, stereo) = attached(2, 2, &[0.5, -0.5, 0.6, -0.6]);
        cmd_tx.push(AssemblerCmd::Attach(mono)).unwrap();
        cmd_tx.push(AssemblerCmd::Attach(stereo)).unwrap();

        let mut frame = vec![0.9; 2 * MAX_INPUT_CHANNELS];
        assert_no_alloc::assert_no_alloc(|| {
            asm.drain_commands();
            asm.fill(&mut frame, 2);
        });
        assert_eq!(frame[0], 0.1);
        assert_eq!(frame[1], 0.0, "unassigned channels are silence");
        assert_eq!(frame[2], 0.5);
        assert_eq!(frame[3], -0.5);
        assert_eq!(frame[MAX_INPUT_CHANNELS], 0.2, "frame 1, mono");
        assert_eq!(frame[MAX_INPUT_CHANNELS + 2], 0.6);
        assert_eq!(frame[63], 0.0);
    }

    #[test]
    fn a_starved_ring_yields_zeros_and_counts() {
        let (mut cmd_tx, _retire, mut asm) = assembler();
        let (_tx, dev) = attached(0, 2, &[0.5, 0.5]); // one frame only
        let shared = dev.shared.clone();
        cmd_tx.push(AssemblerCmd::Attach(dev)).unwrap();

        let mut frame = vec![0.0; 3 * MAX_INPUT_CHANNELS];
        asm.drain_commands();
        asm.fill(&mut frame, 3);
        assert_eq!(frame[0], 0.5);
        assert_eq!(frame[MAX_INPUT_CHANNELS], 0.0, "starved frame is silence");
        assert_eq!(
            shared.underruns.load(Ordering::Relaxed),
            4,
            "two frames × two channels"
        );
    }

    #[test]
    fn detach_returns_the_device_through_the_retire_ring() {
        let (mut cmd_tx, mut retire_rx, mut asm) = assembler();
        let (_tx, dev) = attached(4, 1, &[]);
        cmd_tx.push(AssemblerCmd::Attach(dev)).unwrap();
        asm.drain_commands();
        cmd_tx.push(AssemblerCmd::Detach { offset: 4 }).unwrap();
        assert_no_alloc::assert_no_alloc(|| {
            asm.drain_commands();
        });
        let retired = retire_rx.pop().expect("the device came back");
        assert_eq!(retired.offset, 4);

        let mut frame = vec![0.5; MAX_INPUT_CHANNELS];
        asm.fill(&mut frame, 1);
        assert_eq!(frame[4], 0.0, "the detached range is silence again");
    }

    #[test]
    fn a_detach_for_an_unknown_offset_is_ignored() {
        let (mut cmd_tx, mut retire_rx, mut asm) = assembler();
        cmd_tx.push(AssemblerCmd::Detach { offset: 9 }).unwrap();
        asm.drain_commands();
        assert!(retire_rx.pop().is_err());
    }
}
