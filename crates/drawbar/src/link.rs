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
    /// What the file and the slot both held when they last agreed.
    pub base: Print,
    /// The checksums the slot held before `base`, newest first, at most [`PAST`].
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub past: Vec<u32>,
}

impl Held {
    /// `None` for a class the library keeps no code for.
    pub fn new(class: ObjectClass, spot: Spot, base: Print) -> Option<Held> {
        Some(Held {
            class: class_code(class)?,
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

    /// Whether the slot is known to have held a body with this checksum.
    pub fn knew(&self, crc: u32) -> bool {
        self.base.crc == crc || self.past.contains(&crc)
    }

    /// This link once the file and the slot agree on `print`.
    pub fn agreed(&self, print: Print) -> Held {
        let mut past = self.past.clone();
        if self.base.crc != print.crc {
            past.insert(0, self.base.crc);
        }
        past.retain(|crc| *crc != print.crc);
        past.dedup();
        past.truncate(PAST);
        Held {
            base: print,
            past,
            ..self.clone()
        }
    }

    /// This link once the user chose the file over what the slot holds: the slot's
    /// version counts as an earlier one, so the file is [`Standing::Ahead`] of it.
    pub fn superseding(&self, there: There) -> Held {
        let mut held = self.clone();
        match there {
            There::Holds { crc: Some(crc), .. } if !held.knew(crc) => {
                held.past.insert(0, crc);
                held.past.truncate(PAST);
            }
            There::Holds { crc: None, len } => held.base.len = len,
            There::Holds { .. } | There::Unread | There::Vacant => {}
        }
        held
    }

    /// This link at another slot of its class.
    pub fn moved(&self, spot: Spot) -> Held {
        Held {
            spot,
            ..self.clone()
        }
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
    /// A piano, a sample and the settings report no checksum.
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
    /// The file is newer: the slot holds an earlier version, or nothing. A send settles
    /// it.
    Ahead,
    /// The slot holds a version the file never had, and the file has not changed.
    Theirs,
    /// The slot holds a version the file never had, and the file has changed too.
    Both,
}

impl Standing {
    /// Whether only the user can settle it.
    pub fn needs_attention(self) -> bool {
        matches!(self, Standing::Theirs | Standing::Both)
    }
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
        There::Vacant => Standing::Ahead,
        There::Holds { crc: Some(crc), .. } if crc == here.crc => Standing::Same,
        There::Holds { crc: Some(crc), .. } if held.knew(crc) => Standing::Ahead,
        There::Holds { crc: Some(_), .. } => changed_there(unchanged_here),
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

    fn program(base: Print) -> Held {
        let at = Spot::At { bank: 0, slot: 6 };
        Held::new(ObjectClass::Program, at, base).unwrap()
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
        let sent_twice = program(V1).agreed(V2).agreed(V3);
        assert_eq!(standing(&sent_twice, Some(V3), holds(V1)), Standing::Ahead);
    }

    #[test]
    fn an_empty_slot_is_behind_its_file() {
        assert_eq!(
            standing(&program(V1), Some(V1), There::Vacant),
            Standing::Ahead
        );
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
        assert!(both.needs_attention());
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
        let piano =
            |base| Held::new(ObjectClass::Piano, Spot::Named("Grand".into()), base).unwrap();
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
    fn choosing_the_file_over_a_changed_slot_leaves_the_file_ahead() {
        for here in [V1, V2] {
            let held = program(V1);
            assert!(standing(&held, Some(here), holds(V3)).needs_attention());
            let mine = held.superseding(holds(V3));
            assert_eq!(standing(&mine, Some(here), holds(V3)), Standing::Ahead);
        }
        let piano = Held::new(ObjectClass::Piano, Spot::Named("Grand".into()), V1).unwrap();
        let replaced = There::Holds {
            crc: None,
            len: 200,
        };
        for here in [V1, V2] {
            let mine = piano.superseding(replaced);
            assert_eq!(standing(&mine, Some(here), replaced), Standing::Ahead);
        }
    }

    #[test]
    fn agreeing_keeps_the_earlier_checksums_newest_first_and_bounded() {
        let mut held = program(Print { crc: 0, len: 1 });
        for crc in 1..=20 {
            held = held.agreed(Print { crc, len: 1 });
        }
        assert_eq!(held.base.crc, 20);
        assert_eq!(held.past, [19, 18, 17, 16, 15, 14, 13, 12]);
        let back = held.agreed(Print { crc: 15, len: 1 });
        assert_eq!(back.past, [20, 19, 18, 17, 16, 14, 13, 12]);
        assert_eq!(held.agreed(held.base), held);
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
            held: Some(program(V1).agreed(V2)),
        };
        let json = serde_json::to_string(&link).unwrap();
        assert_eq!(
            json,
            r#"{"keyboard":{"model":"Nord Electro 5"},"held":{"class":4,"spot":{"at":{"bank":0,"slot":6}},"base":{"crc":2,"len":165},"past":[1]}}"#
        );
        let newer = json.replace(r#""past":[1]"#, r#""past":[1],"seen":7"#);
        assert_eq!(serde_json::from_str::<Link>(&newer).unwrap(), link);
        let off = r#"{"keyboard":{"model":"Nord Electro 5","serial":"A1"},"held":null}"#;
        assert_eq!(serde_json::from_str::<Link>(off).unwrap().held, None);
    }
}
