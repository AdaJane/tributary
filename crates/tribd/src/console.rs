//! The console a session starts from.
//!
//! Product policy, not mixer algebra — which is why it lives here and not
//! in `trib-core`: the pure crate should not carry an opinion about what a
//! good default desk looks like.

use trib_core::{
    BusId, BusKind, BusState, FxId, FxParams, FxState, InputAssign, MixerState, StripId, StripState,
};

/// What a new session's desk starts from, ordered by how much of the
/// current rig it keeps.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize, utoipa::ToSchema,
)]
#[serde(rename_all = "snake_case")]
pub enum SessionSeed {
    /// The appliance's known-good desk: one strip on input 0, AUX 1/2 into
    /// reverb and delay. A genuinely new rig.
    Template,
    /// Same stage, fresh mix. Carries strip identity, input patching and
    /// record-arm; resets everything that shapes sound.
    Mapping,
    /// An exact copy of the desk as it stands. Same band, next song.
    Console,
}

/// The shipped default desk: one strip patched to input 0 and the classic
/// AUX 1/2 → reverb/delay loop pre-wired.
pub fn fresh_console() -> MixerState {
    let mut first = StripState::new(StripId(0), "Ch 1".into());
    first.input = Some(InputAssign {
        device: None,
        device_channel: 0,
    });
    MixerState {
        strips: vec![first],
        buses: vec![
            BusState::new(BusId(0), BusKind::Aux, "FX 1".into()),
            BusState::new(BusId(1), BusKind::Aux, "FX 2".into()),
        ],
        fx: vec![
            FxState {
                id: FxId(0),
                name: "Verb".into(),
                input: BusId(0),
                params: FxParams::default_reverb(),
                return_level_db: -10.0,
            },
            FxState {
                id: FxId(1),
                name: "Echo".into(),
                input: BusId(1),
                params: FxParams::default_delay(),
                return_level_db: -10.0,
            },
        ],
        ..MixerState::default()
    }
}

/// Build the console a new session starts from.
///
/// `Mapping` is the interesting one: it keeps what the *stage* decided —
/// which strips exist, what they are called, which input feeds each, and
/// what is armed — and resets everything the *mix* decided: gain, EQ,
/// sends, pan, fader, routing, buses and FX. Arming counts as wiring
/// rather than mix because re-arming eight channels for every song of the
/// same set is exactly the tedium this option exists to remove.
///
/// Faders land at `StripState::new`'s default, which is fully down. That
/// is the same rule a freshly patched line follows: level check rides PFL,
/// then the fader comes up — a carried-over mix must never blast a room
/// whose gain staging has not been rechecked.
pub fn seed_console(seed: SessionSeed, current: &MixerState) -> MixerState {
    match seed {
        SessionSeed::Template => fresh_console(),
        SessionSeed::Console => current.clone(),
        SessionSeed::Mapping => {
            let strips = current
                .strips
                .iter()
                .map(|strip| {
                    let mut fresh = StripState::new(strip.id, strip.name.clone());
                    fresh.input = strip.input.clone();
                    fresh.record_arm = strip.record_arm;
                    fresh
                })
                .collect();
            let template = fresh_console();
            MixerState {
                strips,
                master: trib_core::MasterState {
                    record_arm: current.master.record_arm,
                    ..template.master
                },
                ..template
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use trib_core::RouteTarget;

    /// A wired desk: two named strips on named inputs, armed, with a mix
    /// on top of them.
    fn wired() -> MixerState {
        let mut kick = StripState::new(StripId(0), "Kick".into());
        kick.input = Some(InputAssign {
            device: Some("UMC1820".into()),
            device_channel: 3,
        });
        kick.record_arm = true;
        kick.gain_db = 12.0;
        kick.fader_db = -6.0;
        kick.pan = -0.5;
        kick.eq.low.gain_db = 4.0;
        kick.route_to = RouteTarget::Bus { id: BusId(0) };

        let mut vox = StripState::new(StripId(1), "Vox".into());
        vox.input = Some(InputAssign {
            device: None,
            device_channel: 7,
        });
        vox.fader_db = -3.0;

        MixerState {
            strips: vec![kick, vox],
            master: trib_core::MasterState {
                record_arm: true,
                fader_db: -2.0,
            },
            ..fresh_console()
        }
    }

    #[test]
    fn template_ignores_the_running_desk_entirely() {
        assert_eq!(
            seed_console(SessionSeed::Template, &wired()),
            fresh_console()
        );
    }

    #[test]
    fn console_is_an_exact_copy() {
        let current = wired();
        assert_eq!(seed_console(SessionSeed::Console, &current), current);
    }

    #[test]
    fn mapping_keeps_the_wiring_and_resets_the_mix() {
        let current = wired();
        let seeded = seed_console(SessionSeed::Mapping, &current);

        // Kept: which strips exist, their names, their inputs, their arming.
        assert_eq!(seeded.strips.len(), 2);
        assert_eq!(seeded.strips[0].name, "Kick");
        assert_eq!(seeded.strips[0].input, current.strips[0].input);
        assert_eq!(seeded.strips[1].input, current.strips[1].input);
        assert!(seeded.strips[0].record_arm);
        assert!(seeded.master.record_arm);

        // Reset: everything that shapes sound. The fader in particular
        // lands down, so a new room gets a level check before any level.
        let fresh = StripState::new(StripId(0), "Kick".into());
        assert_eq!(seeded.strips[0].gain_db, fresh.gain_db);
        assert_eq!(seeded.strips[0].fader_db, fresh.fader_db);
        assert_eq!(seeded.strips[0].pan, fresh.pan);
        assert_eq!(seeded.strips[0].eq, fresh.eq);
        assert_eq!(seeded.strips[0].route_to, RouteTarget::Master);
        assert_eq!(seeded.master.fader_db, fresh_console().master.fader_db);
        // Buses and FX come back as the template's.
        assert_eq!(seeded.buses, fresh_console().buses);
        assert_eq!(seeded.fx, fresh_console().fx);
    }

    /// The implicit seed at an empty destination copies the running desk.
    /// Naming that behaviour `Console` is what lets both creates share one
    /// code path instead of drifting apart.
    #[test]
    fn console_matches_the_implicit_seed_at_an_empty_destination() {
        let current = wired();
        assert_eq!(
            seed_console(SessionSeed::Console, &current),
            current.clone()
        );
    }
}
