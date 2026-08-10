//! Pure mixer domain for tributary: no IO, no async runtime.

mod bus;
mod command;
mod db;
mod eq;
mod fx;
mod graph;
mod id;
mod meter;
mod mix;
mod strip;

pub use bus::{BusKind, BusState};
pub use command::{
    FaderTarget, MAX_NAME_LEN, MAX_STRIPS, MixCommand, MixError, ReconcileNeed, StateDelta, apply,
};
pub use db::{FADER_MAX_DB, FADER_MIN_DB, GAIN_MAX_DB, GAIN_MIN_DB, db_to_linear, linear_to_db};
pub use eq::{
    ChannelEq, EQ_FREQ_MAX_HZ, EQ_FREQ_MIN_HZ, EQ_GAIN_RANGE_DB, EQ_Q_MAX, EQ_Q_MIN, EqBand,
    EqBandKind, MID_FREQ_MAX_HZ, MID_FREQ_MIN_HZ,
};
pub use fx::{FxParams, FxState};
pub use graph::{
    CycleError, Edge, EdgeKind, NodeId, NodeKind, SignalGraph, derive_graph, topo_order,
};
pub use id::{BusId, FxId, StripId, TakeId};
pub use meter::{CLIP_DB, LED_AMBER_DB, LED_RED_DB, LedZone, MeterKey, led_zone};
pub use mix::{MasterState, MixerState};
pub use strip::{InputAssign, RouteTarget, SendState, SendTap, StripState};

/// A wire/enum value that no variant matches.
#[derive(Debug, thiserror::Error)]
#[error("unknown {kind}: {value}")]
pub struct ParseEnumError {
    pub kind: &'static str,
    pub value: String,
}
