use std::fmt;

use serde::{Deserialize, Serialize};

/// Identity newtypes. Wire form is the bare number; `Display` prints the
/// tagged form ("strip#3") for logs and errors.
macro_rules! id_newtype {
    ($(#[$doc:meta])* $name:ident, $tag:literal) => {
        $(#[$doc])*
        #[derive(
            Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize,
        )]
        #[cfg_attr(feature = "openapi", derive(utoipa::ToSchema))]
        #[serde(transparent)]
        pub struct $name(pub u32);

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                write!(f, concat!($tag, "#{}"), self.0)
            }
        }
    };
}

id_newtype!(
    /// An input channel strip.
    StripId, "strip");
id_newtype!(
    /// A group or aux bus.
    BusId, "bus");
id_newtype!(
    /// A built-in effect unit.
    FxId, "fx");
id_newtype!(
    /// One recording pass within a project.
    TakeId, "take");
id_newtype!(
    /// A virtual instrument in the rack. Never reused within a session:
    /// it is the printed slot ("INST 3"), the patch identity a strip
    /// holds, and the seed for the link tape's colour.
    InstrumentId, "instrument");

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn display_prints_the_tagged_form() {
        assert_eq!(StripId(3).to_string(), "strip#3");
        assert_eq!(BusId(1).to_string(), "bus#1");
        assert_eq!(FxId(0).to_string(), "fx#0");
        assert_eq!(TakeId(7).to_string(), "take#7");
        assert_eq!(InstrumentId(2).to_string(), "instrument#2");
    }

    #[test]
    fn wire_form_is_the_bare_number() {
        assert_eq!(serde_json::to_string(&StripId(3)).unwrap(), "3");
        let id: StripId = serde_json::from_str("3").unwrap();
        assert_eq!(id, StripId(3));
    }
}
