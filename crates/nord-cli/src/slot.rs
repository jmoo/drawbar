//! `BANK:SLOT`, the way the instrument labels a location.

use nord_usb::wire::Location;
use std::path::PathBuf;

/// A verb's target: a slot on the instrument, or a file standing in for one.
#[derive(Debug)]
pub enum Target {
    File(PathBuf),
    Slot(Location),
}

/// Decide whether an argument names a file or a slot.
///
/// A path that exists wins, so a file called `7:4` is still a file. Otherwise anything
/// that parses as `BANK:SLOT` is one — which is what makes `nord program edit 7:4` work
/// without a flag saying which kind of thing was meant.
pub fn target(s: &str) -> Result<Target, String> {
    let path = PathBuf::from(s);
    if path.exists() {
        return Ok(Target::File(path));
    }
    match parse(s) {
        Ok(at) => Ok(Target::Slot(at)),
        // Neither reading works, so the error has to name both.
        Err(e) => Err(format!("{s}: no such file, and not a slot ({e})")),
    }
}

/// Parse a slot: `8:14` is bank 8, slot 14.
///
/// `:` is canonical — it is what the Electro 5's display and Nord Sound Manager use, and
/// the only spelling the help documents. `-` is accepted as well.
pub fn parse(s: &str) -> Result<Location, String> {
    let (b, l) = s
        .split_once([':', '-'])
        .ok_or_else(|| format!("expected BANK:SLOT (e.g. 7:4), got {s:?}"))?;
    let bank: u32 = b.trim().parse().map_err(|_| format!("bad bank {b:?}"))?;
    let slot: u32 = l.trim().parse().map_err(|_| format!("bad slot {l:?}"))?;
    if bank == 0 || slot == 0 {
        return Err("banks and slots are numbered from 1, as shown on the instrument".into());
    }
    Ok(Location::from_user(bank, slot))
}

/// Parse a list of slots, reporting the first that does not parse.
pub fn parse_all(slots: &[String]) -> Result<Vec<Location>, String> {
    slots.iter().map(|s| parse(s)).collect()
}

/// One-indexed `bank N slot M`, matching the instrument's own labels.
pub fn shown(at: Location) -> String {
    format!("bank {} slot {}", at.bank + 1, at.slot + 1)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Both separators must land on the same zero-indexed wire location.
    #[test]
    fn both_slot_separators_parse_to_the_same_place() {
        let colon = parse("7:4").unwrap();
        let dash = parse("7-4").unwrap();
        assert_eq!(colon, dash);
        // The UI is one-indexed, the wire is zero-indexed.
        assert_eq!((colon.bank, colon.slot), (6, 3));
    }

    #[test]
    fn whitespace_around_the_numbers_is_tolerated() {
        assert_eq!(parse(" 8 : 14 ").unwrap(), parse("8:14").unwrap());
    }

    /// Zero is the giveaway that someone passed a wire index instead of a panel label.
    #[test]
    fn zero_is_rejected_because_the_panel_counts_from_one() {
        for bad in ["0:1", "1:0", "0:0"] {
            let err = parse(bad).unwrap_err();
            assert!(err.contains("numbered from 1"), "{bad}: {err}");
        }
    }

    #[test]
    fn malformed_slots_say_what_was_expected() {
        assert!(parse("74").unwrap_err().contains("BANK:SLOT"));
        assert!(parse("7:x").unwrap_err().contains("bad slot"));
        assert!(parse("x:4").unwrap_err().contains("bad bank"));
    }

    #[test]
    fn a_bad_slot_in_a_list_fails_the_whole_list() {
        let slots = ["7:1".to_string(), "nope".to_string()];
        assert!(parse_all(&slots).is_err());
        assert_eq!(parse_all(&slots[..1]).unwrap().len(), 1);
    }

    /// A path that exists is a file even if it would also parse as a slot; anything else
    /// falls through to slot parsing.
    #[test]
    fn an_existing_path_wins_over_a_slot_reading() {
        let existing = concat!(env!("CARGO_MANIFEST_DIR"), "/Cargo.toml");
        assert!(matches!(target(existing), Ok(Target::File(_))));
        assert!(matches!(target("7:4"), Ok(Target::Slot(_))));
    }

    /// When neither reading works the error must name both, or the user is left
    /// guessing which interpretation was even attempted.
    #[test]
    fn an_ambiguous_target_error_names_both_readings() {
        let err = target("no-such-file.ne5p").unwrap_err();
        assert!(err.contains("no such file"), "{err}");
        assert!(err.contains("not a slot"), "{err}");
    }
}
