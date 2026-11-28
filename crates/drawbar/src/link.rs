//! A file's slot on a keyboard, and how the two compare.
//!
//! A [`Link`] is stored with the file, one per keyboard. Everything shown about a file and
//! its slot is [`standing`] over the link, the file's saved body and what a walk reported
//! for the slot: nothing is matched by equal bytes or remembered from the session.

use nord_usb::{Location, ObjectClass};
use serde::{Deserialize, Serialize};

use crate::store::keys::{class_code, class_of};

/// How many earlier checksums a link keeps.
pub const PAST: usize = 8;

/// A keyboard as a library knows one: its model and its serial number. One that reports
/// no serial is every keyboard of its model.
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Debug, Serialize, Deserialize)]
pub struct Keyboard {
    pub model: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub serial: Option<String>,
}

/// A wire body as a slot reports it: its CRC-32 and its length.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug, Serialize, Deserialize)]
pub struct Print {
    pub crc: u32,
    pub len: u32,
}

/// Which slot of its class a file is linked to.
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Debug, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Spot {
    /// A bank and a slot from zero, as the wire numbers them.
    At { bank: u32, slot: u32 },
    /// The name the keyboard keeps a piano or a sample under. A library holds one object
    /// of each name, wherever it sits.
    Named(String),
}

impl Spot {
    pub fn at(at: Location) -> Spot {
        Spot::At {
            bank: at.bank,
            slot: at.slot,
        }
    }

    /// How `class` keys a slot at `at` that the keyboard calls `name`.
    pub fn of(class: ObjectClass, at: Location, name: &str) -> Spot {
        match class.is_library() {
            true => Spot::Named(name.trim().to_string()),
            false => Spot::at(at),
        }
    }
}

/// The slot a file is linked to, and the versions of it that slot has held.
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Debug, Serialize, Deserialize)]
pub struct Held {
    /// The class's [`class_code`].
    class: u32,
    pub spot: Spot,
    /// The name an addressed slot had when the file and the slot last agreed. Empty for a
    /// slot kept by its name, and in a link stored without one.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    name: String,
    /// What the file and the slot both held when they last agreed.
    pub base: Print,
    /// The checksums the slot held before `base`, newest first, at most [`PAST`].
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub past: Vec<u32>,
}

impl Held {
    /// A link to the slot `at` of `class` the keyboard calls `name`, agreed on `base`.
    /// `None` for a class the library keeps no code for, and for a slot kept by its name
    /// with a blank one.
    pub fn new(class: ObjectClass, at: Location, name: &str, base: Print) -> Option<Held> {
        let spot = Spot::of(class, at, name);
        if matches!(&spot, Spot::Named(name) if name.is_empty()) {
            return None;
        }
        Some(Held {
            class: class_code(class)?,
            name: recorded(&spot, name),
            spot,
            base,
            past: Vec::new(),
        })
    }

    pub fn class(&self) -> ObjectClass {
        class_of(self.class)
    }

    /// The address, for a class linked by one.
    pub fn location(&self) -> Option<Location> {
        match self.spot {
            Spot::At { bank, slot } => Some(Location { bank, slot }),
            Spot::Named(_) => None,
        }
    }

    /// The name the slot had when the file and the slot last agreed, where the link
    /// knows it.
    pub fn name(&self) -> Option<&str> {
        match &self.spot {
            Spot::Named(name) => Some(name),
            Spot::At { .. } => Some(self.name.as_str()).filter(|name| !name.is_empty()),
        }
    }

    /// Whether the slot is known to have held a body with this checksum.
    pub fn knew(&self, crc: u32) -> bool {
        self.age(crc).is_some()
    }

    /// How many agreed versions ago the two held a body with this checksum: none for
    /// the base, and `None` for a body they never agreed on.
    fn age(&self, crc: u32) -> Option<usize> {
        match self.base.crc == crc {
            true => Some(0),
            false => self
                .past
                .iter()
                .position(|past| *past == crc)
                .map(|at| at + 1),
        }
    }

    /// This link once the file and the slot, called `name`, agree on `print`.
    pub fn agreed(&self, print: Print, name: &str) -> Held {
        let mut past = self.past.clone();
        if self.base.crc != print.crc {
            past.insert(0, self.base.crc);
        }
        past.retain(|crc| *crc != print.crc);
        past.dedup();
        past.truncate(PAST);
        Held {
            name: recorded(&self.spot, name),
            base: print,
            past,
            ..self.clone()
        }
    }

    /// This link still knowing `crc`, as the oldest checksum it keeps, where it had
    /// forgotten it.
    pub fn remembering(mut self, crc: u32) -> Held {
        if !self.knew(crc) {
            self.past.truncate(PAST - 1);
            self.past.push(crc);
        }
        self
    }

    /// This link at another slot of its class, under `name` where drawbar renamed the
    /// slot. Otherwise the name it had travels with it.
    pub fn moved(&self, spot: Spot, name: Option<&str>) -> Held {
        Held {
            name: name.map_or_else(|| self.name.clone(), |name| recorded(&spot, name)),
            spot,
            ..self.clone()
        }
    }
}

/// What [`Held`] keeps of a slot's name: nothing for a slot kept by its name, which is
/// its spot.
fn recorded(spot: &Spot, name: &str) -> String {
    match spot {
        Spot::Named(_) => String::new(),
        Spot::At { .. } => name.trim().to_string(),
    }
}

/// A file's place on one keyboard, as the library keeps it.
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Debug, Serialize, Deserialize)]
pub struct Link {
    pub keyboard: Keyboard,
    /// `None` keeps the file off this keyboard by the user's word: nothing links it there
    /// on its own.
    pub held: Option<Held>,
}

/// What a walk of the keyboard says about a slot.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum There {
    /// No walk has reached its bank.
    Unread,
    Vacant,
    /// `crc` is `None` for a slot that reports no checksum.
    Holds {
        crc: Option<u32>,
        len: u32,
    },
}

/// How a linked file and its slot compare.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Standing {
    /// Not known yet: the slot's bank, or the file, has not been read.
    Unread,
    Same,
    /// The file is newer: the slot holds an earlier version, or the one they agreed on
    /// while the file holds one they never did. A send settles it.
    Ahead,
    /// The slot is empty: its sound was deleted on the keyboard or moved off it. Only an
    /// explicit send puts it back.
    Gone,
    /// The slot is newer: the file holds an earlier version, or the one they agreed on
    /// while the slot holds one they never did.
    Theirs,
    /// Each holds a version they never agreed on.
    Both,
}

/// How the file saved as `here` compares with what its linked slot holds. `here` is
/// `None` while the file's checksum has not been taken.
///
/// A slot that reports no checksum is compared by length against the link's base: its
/// class cannot be edited on the keyboard, so only a replacement changes it.
pub fn standing(held: &Held, here: Option<Print>, there: There) -> Standing {
    let Some(here) = here else {
        return Standing::Unread;
    };
    let unchanged_here = here == held.base;
    match there {
        There::Unread => Standing::Unread,
        There::Vacant => Standing::Gone,
        There::Holds { crc: Some(crc), .. } if crc == here.crc => Standing::Same,
        There::Holds { crc: Some(crc), .. } => match (held.age(here.crc), held.age(crc)) {
            (None, None) => Standing::Both,
            (None, Some(_)) => Standing::Ahead,
            (Some(_), None) => Standing::Theirs,
            (Some(here), Some(there)) if here < there => Standing::Ahead,
            (Some(_), Some(_)) => Standing::Theirs,
        },
        There::Holds { crc: None, len } => match (unchanged_here, len == held.base.len) {
            (true, true) => Standing::Same,
            (false, true) => Standing::Ahead,
            (unchanged_here, false) => changed_there(unchanged_here),
        },
    }
}

fn changed_there(unchanged_here: bool) -> Standing {
    match unchanged_here {
        true => Standing::Theirs,
        false => Standing::Both,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const V1: Print = Print { crc: 1, len: 165 };
    const V2: Print = Print { crc: 2, len: 165 };
    const V3: Print = Print { crc: 3, len: 165 };

    const AT: Location = Location { bank: 0, slot: 6 };

    fn program(base: Print) -> Held {
        Held::new(ObjectClass::Program, AT, "Tine", base).unwrap()
    }

    fn piano(base: Print) -> Held {
        Held::new(ObjectClass::Piano, AT, "Grand", base).unwrap()
    }

    fn holds(print: Print) -> There {
        There::Holds {
            crc: Some(print.crc),
            len: print.len,
        }
    }

    #[test]
    fn a_file_and_a_slot_with_one_checksum_are_the_same_whatever_the_base() {
        assert_eq!(standing(&program(V1), Some(V2), holds(V2)), Standing::Same);
    }

    #[test]
    fn a_file_saved_since_its_slot_last_agreed_is_ahead() {
        assert_eq!(standing(&program(V1), Some(V2), holds(V1)), Standing::Ahead);
    }

    #[test]
    fn a_slot_holding_an_earlier_version_is_behind_a_file_sent_elsewhere_since() {
        let sent_twice = program(V1).agreed(V2, "Tine").agreed(V3, "Tine");
        assert_eq!(standing(&sent_twice, Some(V3), holds(V1)), Standing::Ahead);
    }

    #[test]
    fn a_file_holding_an_earlier_version_is_behind_its_slot() {
        let brought_in = program(V1).agreed(V2, "Tine");
        assert_eq!(standing(&brought_in, Some(V1), holds(V2)), Standing::Theirs);
        assert_eq!(
            standing(&brought_in, Some(V1), holds(V3)),
            Standing::Theirs,
            "and behind a version the keyboard made since"
        );
    }

    #[test]
    fn of_two_earlier_versions_the_one_agreed_later_is_the_newer() {
        let twice = program(V1).agreed(V2, "Tine").agreed(V3, "Tine");
        assert_eq!(standing(&twice, Some(V2), holds(V1)), Standing::Ahead);
        assert_eq!(standing(&twice, Some(V1), holds(V2)), Standing::Theirs);
    }

    #[test]
    fn an_emptied_slot_is_gone_whatever_the_file_holds() {
        for here in [V1, V2] {
            let gone = standing(&program(V1), Some(here), There::Vacant);
            assert_eq!(gone, Standing::Gone);
        }
    }

    #[test]
    fn a_slot_changed_on_the_keyboard_under_an_unchanged_file_is_theirs() {
        assert_eq!(
            standing(&program(V1), Some(V1), holds(V2)),
            Standing::Theirs
        );
    }

    #[test]
    fn a_slot_and_a_file_that_both_changed_need_the_user() {
        let both = standing(&program(V1), Some(V2), holds(V3));
        assert_eq!(both, Standing::Both);
    }

    #[test]
    fn nothing_is_claimed_before_the_bank_or_the_file_is_read() {
        assert_eq!(
            standing(&program(V1), Some(V1), There::Unread),
            Standing::Unread
        );
        assert_eq!(standing(&program(V1), None, holds(V1)), Standing::Unread);
    }

    #[test]
    fn a_slot_that_reports_no_checksum_is_compared_by_length_against_the_base() {
        let there = |len| There::Holds { crc: None, len };
        let longer = Print { crc: 9, len: 200 };
        assert_eq!(standing(&piano(V1), Some(V1), there(165)), Standing::Same);
        assert_eq!(
            standing(&piano(V1), Some(longer), there(165)),
            Standing::Ahead
        );
        assert_eq!(standing(&piano(V1), Some(V1), there(200)), Standing::Theirs);
        assert_eq!(standing(&piano(V1), Some(V2), there(200)), Standing::Both);
    }

    #[test]
    fn agreeing_keeps_the_earlier_checksums_newest_first_and_bounded() {
        let mut held = program(Print { crc: 0, len: 1 });
        for crc in 1..=20 {
            held = held.agreed(Print { crc, len: 1 }, "Tine");
        }
        assert_eq!(held.base.crc, 20);
        assert_eq!(held.past, [19, 18, 17, 16, 15, 14, 13, 12]);
        let back = held.agreed(Print { crc: 15, len: 1 }, "Tine");
        assert_eq!(back.past, [20, 19, 18, 17, 16, 14, 13, 12]);
        assert_eq!(held.agreed(held.base, "Tine"), held);
    }

    #[test]
    fn a_library_slot_is_keyed_by_its_name_and_any_other_by_its_address() {
        let at = Location { bank: 2, slot: 1 };
        assert_eq!(
            Spot::of(ObjectClass::Piano, at, " Grand "),
            Spot::Named("Grand".into())
        );
        assert_eq!(Spot::of(ObjectClass::Program, at, "Grand"), Spot::at(at));
    }

    #[test]
    fn a_link_is_kept_as_json_a_newer_member_does_not_break() {
        let link = Link {
            keyboard: Keyboard {
                model: "Nord Electro 5".into(),
                serial: None,
            },
            held: Some(program(V1).agreed(V2, "Tine")),
        };
        let json = serde_json::to_string(&link).unwrap();
        assert_eq!(
            json,
            r#"{"keyboard":{"model":"Nord Electro 5"},"held":{"class":4,"spot":{"at":{"bank":0,"slot":6}},"name":"Tine","base":{"crc":2,"len":165},"past":[1]}}"#
        );
        let newer = json.replace(r#""past":[1]"#, r#""past":[1],"seen":7"#);
        assert_eq!(serde_json::from_str::<Link>(&newer).unwrap(), link);
        let off = r#"{"keyboard":{"model":"Nord Electro 5","serial":"A1"},"held":null}"#;
        assert_eq!(serde_json::from_str::<Link>(off).unwrap().held, None);
    }

    #[test]
    fn a_link_stored_without_the_slots_name_knows_none_and_a_piano_is_its_name() {
        let stored = r#"{"class":4,"spot":{"at":{"bank":0,"slot":6}},"base":{"crc":1,"len":165}}"#;
        let held: Held = serde_json::from_str(stored).unwrap();
        assert_eq!(held.name(), None);
        assert_eq!(serde_json::to_string(&held).unwrap(), stored);
        assert_eq!(piano(V1).name(), Some("Grand"));
        let json = serde_json::to_string(&piano(V1)).unwrap();
        assert!(!json.contains(r#""name""#), "{json}");
    }

    #[test]
    fn agreeing_records_the_slots_name_and_a_move_carries_it() {
        let renamed = program(V1).agreed(V2, " Tine II ");
        assert_eq!(renamed.name(), Some("Tine II"));
        let elsewhere = Spot::At { bank: 1, slot: 0 };
        assert_eq!(
            renamed.moved(elsewhere.clone(), None).name(),
            Some("Tine II")
        );
        assert_eq!(renamed.moved(elsewhere, Some("Wurl")).name(), Some("Wurl"));
        let piano = piano(V1).moved(Spot::Named("Upright".into()), Some("Upright"));
        assert_eq!(piano.name(), Some("Upright"));
    }

    #[test]
    fn a_slot_kept_by_a_blank_name_cannot_be_linked() {
        assert_eq!(Held::new(ObjectClass::Sample, AT, "  ", V1), None);
        assert!(Held::new(ObjectClass::Program, AT, "", V1).is_some());
    }
}
