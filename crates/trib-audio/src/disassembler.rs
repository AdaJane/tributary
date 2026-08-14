//! The output disassembler: splits the engine's fixed
//! `MAX_OUTPUT_CHANNELS`-wide interleaved plane into per-device rings
//! inside the audio callback. The exact inverse of [`crate::assembler`],
//! down to the attach/detach rings and the retire path — detached outputs
//! RETIRE back to the stream thread for dropping, so the callback never
//! allocates or deallocates.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use rtrb::{Consumer, Producer};
use trib_engine::MAX_OUTPUT_CHANNELS;

/// Simultaneously attached output devices. Fewer than the input side: a
/// desk has many sources and few destinations.
pub const MAX_OUTPUT_DEVICES: usize = 8;

/// Attach/detach and retire ring depth: every device attaching and
/// detaching in one tick still fits.
pub const DISASSEMBLER_RING_CAPACITY: usize = MAX_OUTPUT_DEVICES * 2;

/// Health/telemetry shared between a device's stream and the control side.
#[derive(Debug, Default)]
pub struct OutputShared {
    /// Frames the device asked for that the engine had not produced —
    /// the device is draining faster than we fill.
    pub underruns: AtomicU64,
    /// Whole frames the engine produced that the ring had no room for —
    /// the device is not draining fast enough. Kept apart from
    /// `underruns` because they blame opposite ends.
    pub overruns: AtomicU64,
    /// Set by the stream's error callback; the orchestrator reopens on
    /// Refresh.
    pub failed: AtomicBool,
}

/// One attached device: its ring producer and which slice of the engine
/// plane it carries.
pub struct AttachedOutput {
    pub producer: Producer<f32>,
    pub offset: u16,
    pub channels: u16,
    pub shared: Arc<OutputShared>,
}

pub enum DisassemblerCmd {
    Attach(Box<AttachedOutput>),
    /// Keyed by offset — unique while a device is open, and no String ever
    /// crosses onto the audio thread.
    Detach {
        offset: u16,
    },
}

pub struct OutputDisassembler {
    /// Fixed-capacity slot table; index is arbitrary, offset is identity.
    slots: Vec<Option<Box<AttachedOutput>>>,
    /// Retirees waiting for ring space — preallocated, never grows.
    #[allow(clippy::vec_box)]
    parked: Vec<Box<AttachedOutput>>,
    cmd_rx: Consumer<DisassemblerCmd>,
    retire_tx: Producer<Box<AttachedOutput>>,
}

impl OutputDisassembler {
    pub fn new(
        cmd_rx: Consumer<DisassemblerCmd>,
        retire_tx: Producer<Box<AttachedOutput>>,
    ) -> Self {
        OutputDisassembler {
            slots: (0..MAX_OUTPUT_DEVICES).map(|_| None).collect(),
            parked: Vec::with_capacity(MAX_OUTPUT_DEVICES),
            cmd_rx,
            retire_tx,
        }
    }

    /// Apply pending attach/detach commands. Bounded per callback.
    pub fn drain_commands(&mut self) {
        for _ in 0..DISASSEMBLER_RING_CAPACITY {
            match self.cmd_rx.pop() {
                Ok(DisassemblerCmd::Attach(attached)) => {
                    if let Some(slot) = self.slots.iter_mut().find(|s| s.is_none()) {
                        *slot = Some(attached);
                    } else {
                        // Table full (control-side bug): send it straight
                        // back rather than drop on the audio thread.
                        self.retire(attached);
                    }
                }
                Ok(DisassemblerCmd::Detach { offset }) => {
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

    /// Scatter `frame` (interleaved `MAX_OUTPUT_CHANNELS` wide) into every
    /// attached device's ring.
    ///
    /// Whole frames only, for the same reason [`crate::push_frames`]
    /// exists on the way in: the consumer at the far end de-interleaves
    /// positionally with no frame marker and no resynchronisation, so a
    /// ring left holding a partial frame rotates that device's channels
    /// for the rest of the stream's life. Dropping a whole frame costs one
    /// frame; tearing one costs the show.
    ///
    /// Allocation-free: safe to call from an audio callback.
    pub fn scatter(&mut self, frame: &[f32], frames: usize) {
        debug_assert!(frame.len() >= frames * MAX_OUTPUT_CHANNELS);
        for slot in self.slots.iter_mut().flatten() {
            let channels = usize::from(slot.channels);
            let offset = usize::from(slot.offset);
            let mut overruns = 0u64;
            for f in 0..frames {
                let base = f * MAX_OUTPUT_CHANNELS + offset;
                match slot.producer.write_chunk_uninit(channels) {
                    Ok(chunk) => {
                        chunk.fill_from_iter(frame[base..base + channels].iter().copied());
                    }
                    Err(_) => overruns += 1,
                }
            }
            if overruns > 0 {
                slot.shared.overruns.fetch_add(overruns, Ordering::Relaxed);
            }
        }
    }

    /// Never drop an `AttachedOutput` here — its producer's Drop may free
    /// the ring allocation. The stream thread drops retirees.
    fn retire(&mut self, attached: Box<AttachedOutput>) {
        if let Err(rtrb::PushError::Full(attached)) = self.retire_tx.push(attached) {
            if self.parked.len() < MAX_OUTPUT_DEVICES {
                self.parked.push(attached);
            } else {
                debug_assert!(false, "disassembler retire ring and parking both full");
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

    // The audio-thread paths must not allocate. The guard itself lives in
    // `assembler`'s test module — a crate gets one `#[global_allocator]`,
    // and it covers this test binary too, so `assert_no_alloc` below is
    // armed for the mirror-image code as well.

    fn plane(frames: usize, fill: impl Fn(usize, usize) -> f32) -> Vec<f32> {
        let mut buf = trib_engine::output_buffer(frames);
        for f in 0..frames {
            for c in 0..MAX_OUTPUT_CHANNELS {
                buf[f * MAX_OUTPUT_CHANNELS + c] = fill(f, c);
            }
        }
        buf
    }

    fn attach(
        tx: &mut Producer<DisassemblerCmd>,
        offset: u16,
        channels: u16,
        capacity: usize,
    ) -> (Consumer<f32>, Arc<OutputShared>) {
        let (producer, consumer) = RingBuffer::new(capacity);
        let shared = Arc::new(OutputShared::default());
        // `is_ok` rather than `expect`: the error carries the command
        // back, and `DisassemblerCmd` is deliberately not `Debug` — no
        // formatting machinery on a type that crosses to the audio thread.
        assert!(
            tx.push(DisassemblerCmd::Attach(Box::new(AttachedOutput {
                producer,
                offset,
                channels,
                shared: shared.clone(),
            })))
            .is_ok(),
            "attach ring has room"
        );
        (consumer, shared)
    }

    #[test]
    fn attached_devices_take_their_own_slice_of_the_plane() {
        let (mut cmd_tx, cmd_rx) = RingBuffer::new(DISASSEMBLER_RING_CAPACITY);
        let (retire_tx, _retire_rx) = RingBuffer::new(DISASSEMBLER_RING_CAPACITY);
        let mut disassembler = OutputDisassembler::new(cmd_rx, retire_tx);
        let (mut left, _) = attach(&mut cmd_tx, 2, 2, 64);
        let (mut right, _) = attach(&mut cmd_tx, 8, 1, 64);

        // Channel number in the sample, so a misrouted slice is obvious.
        let frame = plane(4, |_, c| c as f32);
        disassembler.drain_commands();
        disassembler.scatter(&frame, 4);

        for _ in 0..4 {
            assert_eq!(left.pop().unwrap(), 2.0);
            assert_eq!(left.pop().unwrap(), 3.0);
            assert_eq!(right.pop().unwrap(), 8.0);
        }
    }

    #[test]
    fn a_full_ring_drops_whole_frames_never_partial_ones() {
        // The invariant the whole design turns on: a partial frame in the
        // ring rotates that device's channels permanently.
        let (mut cmd_tx, cmd_rx) = RingBuffer::new(DISASSEMBLER_RING_CAPACITY);
        let (retire_tx, _retire_rx) = RingBuffer::new(DISASSEMBLER_RING_CAPACITY);
        let mut disassembler = OutputDisassembler::new(cmd_rx, retire_tx);
        // Room for two and a half frames of a 4-channel device.
        let (mut consumer, shared) = attach(&mut cmd_tx, 0, 4, 10);

        let frame = plane(4, |f, _| f as f32);
        disassembler.drain_commands();
        disassembler.scatter(&frame, 4);

        assert_eq!(
            consumer.slots() % 4,
            0,
            "the ring never holds a partial frame"
        );
        assert_eq!(shared.overruns.load(Ordering::Relaxed), 2);
        for expected in [0.0, 1.0] {
            for _ in 0..4 {
                assert_eq!(consumer.pop().unwrap(), expected);
            }
        }
        assert!(consumer.pop().is_err(), "two whole frames, then nothing");
    }

    #[test]
    fn detach_returns_the_device_through_the_retire_ring() {
        let (mut cmd_tx, cmd_rx) = RingBuffer::new(DISASSEMBLER_RING_CAPACITY);
        let (retire_tx, mut retire_rx) = RingBuffer::new(DISASSEMBLER_RING_CAPACITY);
        let mut disassembler = OutputDisassembler::new(cmd_rx, retire_tx);
        let (_consumer, _) = attach(&mut cmd_tx, 4, 2, 64);
        disassembler.drain_commands();

        cmd_tx.push(DisassemblerCmd::Detach { offset: 4 }).ok();
        disassembler.drain_commands();
        let Ok(retired) = retire_rx.pop() else {
            panic!("the device came back through the retire ring");
        };
        assert_eq!(retired.offset, 4);

        // And nothing is written for it any more.
        let frame = plane(2, |_, c| c as f32);
        disassembler.scatter(&frame, 2);
    }

    #[test]
    fn the_scatter_path_never_allocates() {
        let (mut cmd_tx, cmd_rx) = RingBuffer::new(DISASSEMBLER_RING_CAPACITY);
        let (retire_tx, _retire_rx) = RingBuffer::new(DISASSEMBLER_RING_CAPACITY);
        let mut disassembler = OutputDisassembler::new(cmd_rx, retire_tx);
        let (_consumer, _) = attach(&mut cmd_tx, 2, 2, 8);
        let frame = plane(8, |_, c| c as f32);
        assert_no_alloc::assert_no_alloc(|| {
            disassembler.drain_commands();
            // Deliberately more than the ring holds, so the overrun path
            // is inside the guard too.
            disassembler.scatter(&frame, 8);
        });
    }
}
