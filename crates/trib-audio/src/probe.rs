//! Which card the exclusive layer takes when the config names none.
//!
//! The decision is a fold over "try each, keep the first that opens", and
//! it is kept apart from the ALSA calls so the ordering rule and the
//! refusal text can be tested without a sound card. Selection is by the
//! open attempt alone — no name denylist. On the appliance the onboard
//! HDMI and headphone cards have no capture PCM at all, so they fail the
//! duplex open and are skipped for the true reason rather than a guessed
//! one; a denylist would have to be kept current with every SoC.

use std::fmt::Display;

/// Take the first candidate that opens.
///
/// Every failure is kept so the refusal names each card tried and why —
/// a boot line reading "hw:0: No such file; hw:1: Device or resource
/// busy" is a diagnosis, "no card" is not. Candidates after the first
/// success are never touched: opening a `hw:` card takes it, and this
/// backend owns exactly one.
#[cfg_attr(not(feature = "alsa-backend"), allow(dead_code))]
pub fn choose_card<T, E: Display>(
    candidates: impl IntoIterator<Item = String>,
    mut open: impl FnMut(&str) -> Result<T, E>,
) -> Result<(String, T), String> {
    let mut refusals = Vec::new();
    for name in candidates {
        match open(&name) {
            Ok(opened) => return Ok((name, opened)),
            Err(e) => refusals.push(format!("{name}: {e}")),
        }
    }
    if refusals.is_empty() {
        return Err("no ALSA cards present".to_owned());
    }
    Err(format!(
        "no ALSA card opened duplex at the engine rate: {}",
        refusals.join("; ")
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn names(list: &[&str]) -> Vec<String> {
        list.iter().map(|s| (*s).to_owned()).collect()
    }

    #[test]
    fn the_first_card_that_opens_wins_and_later_ones_are_never_tried() {
        let mut tried = Vec::new();
        let result = choose_card(names(&["hw:0", "hw:1", "hw:2"]), |name| {
            tried.push(name.to_owned());
            if name == "hw:1" {
                Ok(10u16)
            } else {
                Err("No such file or directory")
            }
        });
        assert_eq!(result, Ok(("hw:1".to_owned(), 10)));
        assert_eq!(
            tried,
            names(&["hw:0", "hw:1"]),
            "opening takes the card; a later candidate must not be touched"
        );
    }

    #[test]
    fn every_refusal_is_named_in_order_when_nothing_opens() {
        let result = choose_card::<(), _>(names(&["hw:0", "hw:1"]), |name| {
            Err(if name == "hw:0" {
                "No such file or directory"
            } else {
                "Device or resource busy"
            })
        });
        let text = result.unwrap_err();
        assert!(text.starts_with("no ALSA card opened duplex"), "{text}");
        let first = text
            .find("hw:0: No such file or directory")
            .expect("names hw:0");
        let second = text
            .find("hw:1: Device or resource busy")
            .expect("names hw:1");
        assert!(first < second, "attempts read in the order they were made");
    }

    #[test]
    fn an_empty_candidate_list_says_so_rather_than_listing_nothing() {
        let result = choose_card::<(), String>(Vec::new(), |_| unreachable!());
        assert_eq!(result.unwrap_err(), "no ALSA cards present");
    }
}
