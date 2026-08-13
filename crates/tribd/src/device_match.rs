//! Device name reconciliation: a project's stored device names vs what the
//! OS enumerates TODAY. Names drift across reboots ("… USB" grows a
//! " #2", ALSA appends "Audio"), so wanted-but-missing names get one
//! careful matching attempt. The rule that matters: a match is adopted
//! ONLY when unique — ambiguity degrades to absent, never to the wrong mic.

use std::collections::{BTreeSet, HashMap, HashSet};

use trib_audio::InputDeviceInfo;

/// Map each wanted-but-not-present name to the present device it almost
/// certainly is. Present devices already wanted by exact name are never
/// offered as candidates, and no candidate is claimed twice.
pub fn reconcile_names(wanted: &[String], present: &[InputDeviceInfo]) -> HashMap<String, String> {
    let present_names: HashSet<&str> = present.iter().map(|d| d.name.as_str()).collect();
    let wanted_names: HashSet<&str> = wanted.iter().map(String::as_str).collect();

    let missing: Vec<&String> = wanted
        .iter()
        .filter(|w| !present_names.contains(w.as_str()))
        .collect();
    let candidates: Vec<&InputDeviceInfo> = present
        .iter()
        .filter(|d| !wanted_names.contains(d.name.as_str()))
        .collect();

    // Score every (missing, candidate) pair, best first.
    let mut scored: Vec<(u8, &String, &str)> = Vec::new();
    for name in &missing {
        for candidate in &candidates {
            if let Some(score) = match_score(name, &candidate.name) {
                scored.push((score, name, candidate.name.as_str()));
            }
        }
    }
    scored.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| a.1.cmp(b.1)));

    let mut aliases = HashMap::new();
    let mut claimed: HashSet<&str> = HashSet::new();
    for name in &missing {
        let mut best: Option<(u8, &str)> = None;
        let mut tied = false;
        for &(score, scored_name, candidate) in &scored {
            if scored_name != *name || claimed.contains(candidate) {
                continue;
            }
            match best {
                None => best = Some((score, candidate)),
                Some((best_score, best_candidate)) if score == best_score => {
                    tied |= candidate != best_candidate;
                }
                Some(_) => {}
            }
        }
        if let Some((_, candidate)) = best
            && !tied
        {
            claimed.insert(candidate);
            aliases.insert((*name).clone(), candidate.to_owned());
        }
    }
    aliases
}

/// How alike two device names are. `None` = not a plausible match.
fn match_score(stored: &str, current: &str) -> Option<u8> {
    let a = normalize(stored);
    let b = normalize(current);
    if a == b {
        return Some(3);
    }
    let ta = tokens(&a);
    let tb = tokens(&b);
    if !ta.is_empty() && !tb.is_empty() && (ta.is_subset(&tb) || tb.is_subset(&ta)) {
        return Some(2);
    }
    None
}

/// Lowercase, collapse whitespace, and strip a trailing enumeration index
/// (" #2", " (3)") — the classic replug renames.
fn normalize(name: &str) -> String {
    let lowered = name.to_lowercase();
    let collapsed = lowered.split_whitespace().collect::<Vec<_>>().join(" ");
    if let Some((head, tail)) = collapsed.rsplit_once(' ') {
        let is_index = tail
            .strip_prefix('#')
            .is_some_and(|n| n.parse::<u32>().is_ok())
            || tail
                .strip_prefix('(')
                .and_then(|t| t.strip_suffix(')'))
                .is_some_and(|n| n.parse::<u32>().is_ok());
        if is_index {
            return head.to_owned();
        }
    }
    collapsed
}

fn tokens(normalized: &str) -> BTreeSet<&str> {
    normalized.split(' ').filter(|t| !t.is_empty()).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dev(name: &str) -> InputDeviceInfo {
        InputDeviceInfo {
            name: name.into(),
            description: None,
            channels: 2,
            active: false,
            pulse: false,
            card: None,
            channel_map: None,
        }
    }

    #[test]
    fn an_enumeration_index_rename_reconciles() {
        let aliases = reconcile_names(
            &["ThinkPad Thunderbolt 4 Dock USB".into()],
            &[
                dev("sof-hda-dsp"),
                dev("ThinkPad Thunderbolt 4 Dock USB #2"),
            ],
        );
        assert_eq!(
            aliases["ThinkPad Thunderbolt 4 Dock USB"],
            "ThinkPad Thunderbolt 4 Dock USB #2"
        );
    }

    #[test]
    fn a_token_superset_rename_reconciles() {
        let aliases = reconcile_names(
            &["ThinkPad Thunderbolt 4 Dock USB".into()],
            &[dev("ThinkPad Thunderbolt 4 Dock USB Audio")],
        );
        assert_eq!(aliases.len(), 1);
    }

    #[test]
    fn an_exactly_present_name_is_never_remapped() {
        let aliases = reconcile_names(
            &["sof-hda-dsp".into()],
            &[dev("sof-hda-dsp"), dev("sof-hda-dsp #2")],
        );
        assert!(aliases.is_empty());
    }

    #[test]
    fn ambiguous_twins_stay_absent_rather_than_guessing() {
        let aliases = reconcile_names(
            &["USB Microphone".into()],
            &[dev("USB Microphone #2"), dev("USB Microphone #3")],
        );
        assert!(aliases.is_empty(), "two equal candidates: no match");
    }

    #[test]
    fn a_candidate_is_never_claimed_twice() {
        // Both stored names normalize toward the same present device; the
        // first wanted name claims it and the other stays absent.
        let aliases = reconcile_names(&["Mic (2)".into(), "Mic #3".into()], &[dev("Mic")]);
        assert_eq!(aliases.len(), 1);
        assert_eq!(aliases["Mic (2)"], "Mic");
    }

    #[test]
    fn unrelated_devices_never_match() {
        let aliases = reconcile_names(
            &["Scarlett 18i20 USB".into()],
            &[dev("Webcam C922"), dev("sof-hda-dsp")],
        );
        assert!(aliases.is_empty());
    }
}
