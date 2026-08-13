//! Where each open input device's channels sit within the engine's
//! fixed-stride input frame. Owned by the control side; `compile()`
//! resolves strip patches against it, so the audio thread only ever sees
//! flat indices.

/// Fixed engine input stride — sibling of `MAX_METERS`. The audio backend
/// always assembles this many interleaved channels; unassigned ones are
/// silence, so devices can open and close without the stride ever moving.
pub const MAX_INPUT_CHANNELS: usize = 64;

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

/// First-fit allocator over the `MAX_INPUT_CHANNELS` frame. One entry per
/// open device; entries are kept sorted by offset.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct InputSlots {
    entries: Vec<SlotEntry>,
}

impl InputSlots {
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
        let mut offset = 0u16;
        for entry in &self.entries {
            if offset + channels <= entry.offset {
                break;
            }
            offset = entry.offset + entry.channels;
        }
        if usize::from(offset + channels) > MAX_INPUT_CHANNELS {
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

    /// The pre-multi-device world: only the system default input, at
    /// offset 0. The boot state and the test fixture.
    pub fn single_default(channels: u16) -> Self {
        let mut slots = InputSlots::default();
        slots
            .allocate(None, channels)
            .expect("default device fits an empty frame");
        slots
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
}
