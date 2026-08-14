//! Where each open device's channels sit within one of the engine's
//! fixed-stride frames. Owned by the control side; `compile()` resolves
//! patches against these, so the audio thread only ever sees flat indices.
//!
//! One allocator serves both directions. The two frames are the same width
//! today, which is exactly why [`Slots`] is tagged with a [`Plane`] marker
//! rather than being a plain `Slots<const WIDTH: usize>`: at equal widths
//! the two aliases would be the *same type*, and `compile()` — which takes
//! one of each — would silently accept them swapped.

use std::marker::PhantomData;

/// Fixed engine input stride — sibling of `MAX_METERS`. The audio backend
/// always assembles this many interleaved channels; unassigned ones are
/// silence, so devices can open and close without the stride ever moving.
pub const MAX_INPUT_CHANNELS: usize = 64;

/// Fixed engine output stride, and deliberately its own constant rather
/// than an alias of the input one. The two happen to agree; coupling them
/// would mean widening the input frame silently resizes every backend's
/// output buffer, with no compile error anywhere.
pub const MAX_OUTPUT_CHANNELS: usize = 64;

/// The monitor's reserved pair in the output plane.
///
/// Not a patch, and not allocatable. It is the one output that must exist
/// before any document does — the boot graph compiles against an empty
/// slot map and still has to make sound — and it is the only part of the
/// plane that PFL and the tape return are allowed to take over. A direct
/// out patched from the master must keep carrying the mix while someone
/// solos a channel; that difference is the whole point of a direct out,
/// and reserving the pair is what makes it structural.
pub const MONITOR_OUT: [u16; 2] = [0, 1];

/// Width of [`MONITOR_OUT`].
pub const MONITOR_CHANNELS: u16 = 2;

/// The engine's output frame: [`MAX_OUTPUT_CHANNELS`] interleaved channels
/// at `block_size` frames.
///
/// Every backend allocates through here, the fake one included, so none of
/// them can accidentally render a narrower plane. That is not a
/// hypothetical: the instrument rack was nearly shipped rendering into a
/// buffer the fake backend sized at one channel, which would have made
/// instruments silently produce nothing under `--no-default-features` and
/// in most of the test suite.
pub fn output_buffer(block_size: usize) -> Vec<f32> {
    vec![0.0; block_size * MAX_OUTPUT_CHANNELS]
}

/// Which frame a slot map allocates over.
///
/// A type-level tag carrying no data. Its only job is to keep
/// [`InputSlots`] and [`OutputSlots`] distinct types while the two frames
/// are the same width, so a swapped argument is a compile error instead of
/// audio routed against the wrong plane:
///
/// ```compile_fail
/// use trib_engine::{InputSlots, OutputSlots};
/// fn resolves_a_strip_patch(_slots: &InputSlots) {}
/// // `compile()` takes one map of each. At equal widths a bare
/// // `Slots<const WIDTH: usize>` would make this call type-check.
/// resolves_a_strip_patch(&OutputSlots::with_monitor());
/// ```
pub trait Plane {
    /// Channels in this frame.
    const WIDTH: usize;
}

/// The device input frame.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Input;

/// The engine output plane.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Output;

impl Plane for Input {
    const WIDTH: usize = MAX_INPUT_CHANNELS;
}

impl Plane for Output {
    const WIDTH: usize = MAX_OUTPUT_CHANNELS;
}

/// Slot map over the input frame.
pub type InputSlots = Slots<Input>;

/// Slot map over the output plane.
pub type OutputSlots = Slots<Output>;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SlotError {
    /// Not enough contiguous space left in the frame.
    Exhausted { needed: u16 },
    /// The device already holds a slot of a different width — a card
    /// profile switch is the usual cause. Its stream is still feeding the
    /// old range, so the caller must close and release it before the slot
    /// can be resized; quietly keeping the old width would leave the new
    /// channels resolving to silence.
    WidthChanged { held: u16, wanted: u16 },
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct SlotEntry {
    /// `None` = the system default input.
    device: Option<String>,
    offset: u16,
    channels: u16,
}

/// First-fit allocator over a [`Plane`]'s frame. One entry per open
/// device; entries are kept sorted by offset.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Slots<P> {
    entries: Vec<SlotEntry>,
    /// Channels at the head of the frame that no device may be allocated
    /// over. Zero on the input side; [`MONITOR_CHANNELS`] on the output
    /// plane, where the control-room feed lives at a fixed location that
    /// exists before any device does.
    reserved: u16,
    plane: PhantomData<P>,
}

/// Hand-written rather than derived: `derive(Default)` would bound the
/// marker on `Default`, which the generic constructors below cannot
/// satisfy. An empty map is empty whatever plane it is over.
impl<P> Default for Slots<P> {
    fn default() -> Self {
        Slots {
            entries: Vec::new(),
            reserved: 0,
            plane: PhantomData,
        }
    }
}

impl<P: Plane> Slots<P> {
    /// Reserve a contiguous range for `device`. Re-allocating a device that
    /// already holds a slot of the SAME width returns its existing offset
    /// (the open path is idempotent); a different width is refused, because
    /// resizing under a live stream is the caller's job to sequence.
    pub fn allocate(&mut self, device: Option<&str>, channels: u16) -> Result<u16, SlotError> {
        if let Some((offset, held)) = self.get(device) {
            return if held == channels {
                Ok(offset)
            } else {
                Err(SlotError::WidthChanged {
                    held,
                    wanted: channels,
                })
            };
        }
        let mut offset = self.reserved;
        for entry in &self.entries {
            if offset + channels <= entry.offset {
                break;
            }
            offset = entry.offset + entry.channels;
        }
        if usize::from(offset + channels) > P::WIDTH {
            return Err(SlotError::Exhausted { needed: channels });
        }
        let index = self
            .entries
            .iter()
            .position(|e| e.offset > offset)
            .unwrap_or(self.entries.len());
        self.entries.insert(
            index,
            SlotEntry {
                device: device.map(str::to_owned),
                offset,
                channels,
            },
        );
        Ok(offset)
    }

    /// Free a device's range. Returns what it held, if anything.
    pub fn release(&mut self, device: Option<&str>) -> Option<(u16, u16)> {
        let index = self
            .entries
            .iter()
            .position(|e| e.device.as_deref() == device)?;
        let entry = self.entries.remove(index);
        Some((entry.offset, entry.channels))
    }

    /// `(offset, channels)` of a device's slot.
    pub fn get(&self, device: Option<&str>) -> Option<(u16, u16)> {
        self.entries
            .iter()
            .find(|e| e.device.as_deref() == device)
            .map(|e| (e.offset, e.channels))
    }

    /// A patch's flat index into the engine frame — `None` for an unknown
    /// device or a channel past its count (the compile-time silence path).
    pub fn resolve(&self, device: Option<&str>, channel: u16) -> Option<u16> {
        let (offset, channels) = self.get(device)?;
        (channel < channels).then_some(offset + channel)
    }

    pub fn devices(&self) -> impl Iterator<Item = (Option<&str>, u16, u16)> {
        self.entries
            .iter()
            .map(|e| (e.device.as_deref(), e.offset, e.channels))
    }

    /// The pre-multi-device world: only the system default device, at
    /// offset 0. The boot state and the test fixture.
    pub fn single_default(channels: u16) -> Self {
        let mut slots = Slots::default();
        slots
            .allocate(None, channels)
            .expect("default device fits an empty frame");
        slots
    }
}

impl Slots<Output> {
    /// The output plane with [`MONITOR_OUT`] held back.
    ///
    /// The monitor is not a device slot — it is a fixed location the
    /// backend reads directly, and it exists before any device is open.
    /// Reserving the head of the plane rather than allocating it under a
    /// device name is what leaves the *system default output* free to be
    /// patched like any other device, which keying the reservation on
    /// `None` would have quietly forbidden.
    pub fn with_monitor() -> Self {
        Slots {
            reserved: MONITOR_CHANNELS,
            ..Slots::default()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn allocation_is_first_fit_and_release_reopens_the_gap() {
        let mut slots = InputSlots::default();
        assert_eq!(slots.allocate(None, 2), Ok(0));
        assert_eq!(slots.allocate(Some("dock"), 2), Ok(2));
        assert_eq!(slots.allocate(Some("interface"), 8), Ok(4));
        slots.release(Some("dock"));
        assert_eq!(
            slots.allocate(Some("webcam"), 2),
            Ok(2),
            "the freed gap is reused first-fit"
        );
        assert_eq!(
            slots.allocate(Some("wide"), 4),
            Ok(12),
            "too wide for the gap: appends after the last entry"
        );
    }

    #[test]
    fn a_device_reallocating_keeps_its_slot() {
        let mut slots = InputSlots::default();
        assert_eq!(slots.allocate(Some("dock"), 2), Ok(0));
        assert_eq!(slots.allocate(Some("dock"), 2), Ok(0));
        assert_eq!(slots.devices().count(), 1);
    }

    #[test]
    fn a_device_whose_width_changed_must_be_released_first() {
        let mut slots = InputSlots::default();
        assert_eq!(slots.allocate(Some("umc"), 2), Ok(0));
        assert_eq!(
            slots.allocate(Some("umc"), 18),
            Err(SlotError::WidthChanged {
                held: 2,
                wanted: 18
            }),
            "a card profile switch widens the device; silently keeping the \
             2-wide slot would leave channels 2..18 resolving to silence \
             while the patchbay drew eighteen jacks"
        );
        slots.release(Some("umc"));
        assert_eq!(slots.allocate(Some("umc"), 18), Ok(0));
        assert_eq!(slots.resolve(Some("umc"), 17), Some(17));
    }

    #[test]
    fn the_frame_exhausts_at_sixty_four_channels() {
        let mut slots = InputSlots::default();
        assert_eq!(slots.allocate(Some("big"), 60), Ok(0));
        assert_eq!(
            slots.allocate(Some("more"), 8),
            Err(SlotError::Exhausted { needed: 8 })
        );
        assert_eq!(
            slots.allocate(Some("small"), 4),
            Ok(60),
            "exact fit still lands"
        );
    }

    #[test]
    fn resolve_maps_device_channels_and_refuses_the_unknown() {
        let mut slots = InputSlots::default();
        slots.allocate(None, 2).unwrap();
        slots.allocate(Some("dock"), 2).unwrap();
        assert_eq!(slots.resolve(None, 1), Some(1));
        assert_eq!(slots.resolve(Some("dock"), 0), Some(2));
        assert_eq!(slots.resolve(Some("dock"), 1), Some(3));
        assert_eq!(slots.resolve(Some("dock"), 2), None, "past the device");
        assert_eq!(slots.resolve(Some("ghost"), 0), None, "unknown device");
    }

    #[test]
    fn single_default_is_the_pre_multi_device_world() {
        let slots = InputSlots::single_default(8);
        assert_eq!(slots.resolve(None, 7), Some(7));
        assert_eq!(slots.resolve(None, 8), None);
        assert_eq!(slots.resolve(Some("x"), 0), None);
    }

    #[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
    struct Narrow;

    impl Plane for Narrow {
        const WIDTH: usize = 8;
    }

    #[test]
    fn a_plane_exhausts_at_its_own_width_not_the_input_frames() {
        // The whole reason the width is a property of the plane: if the
        // allocator read `MAX_INPUT_CHANNELS` directly, widening the input
        // frame would quietly widen every other frame with it.
        let mut slots = Slots::<Narrow>::default();
        assert_eq!(slots.allocate(Some("a"), 6), Ok(0));
        assert_eq!(
            slots.allocate(Some("b"), 4),
            Err(SlotError::Exhausted { needed: 4 })
        );
        assert_eq!(slots.allocate(Some("b"), 2), Ok(6), "exact fit still lands");
    }

    #[test]
    fn the_monitor_pair_is_reserved_so_no_device_slot_can_land_on_the_mix() {
        let mut plane = OutputSlots::with_monitor();
        assert_eq!(
            plane.allocate(Some("interface"), 8),
            Ok(MONITOR_CHANNELS),
            "the first real device starts after the monitor, never on it"
        );
        for channel in MONITOR_OUT {
            assert!(
                plane
                    .devices()
                    .all(|(_, offset, channels)| channel < offset || channel >= offset + channels),
                "no device slot may cover monitor channel {channel}"
            );
        }
    }

    #[test]
    fn the_system_default_output_is_patchable_like_any_other_device() {
        // The reservation holds back the head of the PLANE, not the device
        // named `None`. Keying it on `None` — the obvious first move —
        // would have made the one output every desktop has the one output
        // nobody could patch.
        let mut plane = OutputSlots::with_monitor();
        assert_eq!(plane.allocate(None, 8), Ok(MONITOR_CHANNELS));
        assert_eq!(plane.resolve(None, 0), Some(MONITOR_CHANNELS));
        assert_eq!(plane.resolve(None, 7), Some(MONITOR_CHANNELS + 7));
        assert_eq!(plane.resolve(None, 8), None, "past the device");
    }
}
