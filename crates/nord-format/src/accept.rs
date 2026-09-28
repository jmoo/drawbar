//! Which format tags an instrument family takes, and the class it keeps each of them in.
//!
//! A file names its model in its four-character tag, and an instrument names its own in
//! the USB product string. [`Family`] maps both to one instrument family, and
//! [`Family::accepts`] is the table: for one storage class and one tag, whether that
//! family takes it and how that is known.
//!
//! ⚠️ Schema versions are not in the table. Which revisions of a tag an instrument
//! accepts has not been measured, so a version is never a reason to refuse here.

use crate::fields::Library;
use crate::formats::{
    nc2, nc2d, nd2, nd3, ne3, ne4, ne5, ne6, ne7, ng2, nl4, nla1, no3, np, np2, np3, np4, np5,
    npip, npno, ns2, ns3, ns4, nsclassic, nsmp, nw, nw2,
};

/// One instrument family, as named by both a file's tag and an instrument's product
/// string.
///
/// Models whose files carry one set of tags are one family (the Electro 3 and 3 HP, the
/// Electro 4 and 4D, the Stage Classic and Stage EX), because nothing in a file says
/// which of the pair wrote it.
///
/// Two families are absent because their files carry no tag: the older Leads use a SysEx
/// or MIDI carrier shared across four models, and the Electro 2's sample library is its
/// own container with no CBIN tag.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Family {
    Electro3,
    Electro4,
    Electro5,
    Electro6,
    Electro7,
    StageClassic,
    Stage2,
    Stage3,
    Stage4,
    Piano,
    Piano2,
    Piano3,
    Piano4,
    Piano5,
    Grand,
    Wave,
    Wave2,
    C2,
    C2D,
    Organ3,
    Lead4,
    LeadA1,
    Drum2,
    Drum3,
}

/// A class of storage on the instrument: the four libraries a decoded body can refer
/// into, plus the two singleton buffers no reference points at.
///
/// The wire codes live on `nord-usb`'s `ObjectClass`, which takes its library codes from
/// [`Library`]. This enum names the same classes without the wire codes, so a caller
/// holding both converts between them with no second table.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Slot {
    Piano,
    Sample,
    Program,
    SetList,
    /// The live buffer: the panel's current state.
    Live,
    /// The global settings singleton.
    Settings,
}

impl From<Library> for Slot {
    fn from(library: Library) -> Slot {
        match library {
            Library::Piano => Slot::Piano,
            Library::Sample => Slot::Sample,
            Library::Program => Slot::Program,
            Library::SetList => Slot::SetList,
        }
    }
}

impl Slot {
    pub const ALL: [Slot; 6] = [
        Slot::Piano,
        Slot::Sample,
        Slot::Program,
        Slot::SetList,
        Slot::Live,
        Slot::Settings,
    ];
}

/// Whether a family takes a tag in a class, and how that is known.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Acceptance {
    /// A file under this tag has been written into this class and read back.
    Confirmed,
    /// The tag is this family's own and belongs in this class, but no such write has
    /// been made.
    Inferred,
    /// The tag names a family and this class is not where it goes: another family's
    /// tag, or this family's own in the wrong class.
    Refused,
    /// The table does not say: the tag belongs to no single family (a shared library
    /// format or a carrier), or this crate does not read it.
    Unknown,
}

/// Which family's files carry a tag, and the class that family keeps it in.
///
/// Each class is the one its family's format module implies. `None` marks a tag this
/// table places in no class, so every class refuses it.
///
/// ⚠️ The shared library formats are absent: several families read the same sample
/// instrument or piano library file, so a tag missing here is no evidence that an
/// instrument refuses it. Only a tag listed here can refuse another family's instrument.
const CARRIES: &[(&str, Family, Option<Slot>)] = &[
    (ne3::program::FORMAT, Family::Electro3, Some(Slot::Program)),
    (ne3::organ_preset::FORMAT, Family::Electro3, None),
    (ne4::program::FORMAT, Family::Electro4, Some(Slot::Program)),
    (ne4::live::FORMAT, Family::Electro4, Some(Slot::Live)),
    (
        ne4::settings::FORMAT,
        Family::Electro4,
        Some(Slot::Settings),
    ),
    (ne5::program::FORMAT, Family::Electro5, Some(Slot::Program)),
    (ne5::live::FORMAT, Family::Electro5, Some(Slot::Live)),
    (ne5::song::FORMAT, Family::Electro5, Some(Slot::SetList)),
    (
        ne5::settings::FORMAT,
        Family::Electro5,
        Some(Slot::Settings),
    ),
    (ne6::program::FORMAT, Family::Electro6, Some(Slot::Program)),
    (ne6::live::FORMAT, Family::Electro6, Some(Slot::Live)),
    (
        ne6::settings::FORMAT,
        Family::Electro6,
        Some(Slot::Settings),
    ),
    (ne7::program::FORMAT, Family::Electro7, Some(Slot::Program)),
    (ne7::live::FORMAT, Family::Electro7, Some(Slot::Live)),
    (
        ne7::settings::FORMAT,
        Family::Electro7,
        Some(Slot::Settings),
    ),
    (
        nsclassic::program::FORMAT,
        Family::StageClassic,
        Some(Slot::Program),
    ),
    (nsclassic::synth::FORMAT, Family::StageClassic, None),
    (
        nsclassic::piano_library::FORMAT,
        Family::StageClassic,
        Some(Slot::Piano),
    ),
    (ns2::program::FORMAT, Family::Stage2, Some(Slot::Program)),
    (ns2::live::FORMAT, Family::Stage2, Some(Slot::Live)),
    (ns2::synth::FORMAT, Family::Stage2, None),
    (ns2::settings::FORMAT, Family::Stage2, Some(Slot::Settings)),
    (ns3::program::FORMAT, Family::Stage3, Some(Slot::Program)),
    (ns3::live::FORMAT, Family::Stage3, Some(Slot::Live)),
    (ns3::song::FORMAT, Family::Stage3, Some(Slot::SetList)),
    (ns3::synth::FORMAT, Family::Stage3, None),
    (ns3::settings::FORMAT, Family::Stage3, Some(Slot::Settings)),
    (ns4::program::FORMAT, Family::Stage4, Some(Slot::Program)),
    (ns4::live::FORMAT, Family::Stage4, Some(Slot::Live)),
    (ns4::synth::FORMAT, Family::Stage4, None),
    (ns4::piano_preset::FORMAT, Family::Stage4, None),
    (ns4::organ_preset::FORMAT, Family::Stage4, None),
    (ns4::settings::FORMAT, Family::Stage4, Some(Slot::Settings)),
    (np::program::FORMAT, Family::Piano, Some(Slot::Program)),
    (np::live::FORMAT, Family::Piano, Some(Slot::Live)),
    (np::settings::FORMAT, Family::Piano, Some(Slot::Settings)),
    (np2::program::FORMAT, Family::Piano2, Some(Slot::Program)),
    (np2::live::FORMAT, Family::Piano2, Some(Slot::Live)),
    (np2::settings::FORMAT, Family::Piano2, Some(Slot::Settings)),
    (np3::program::FORMAT, Family::Piano3, Some(Slot::Program)),
    (np3::live::FORMAT, Family::Piano3, Some(Slot::Live)),
    (np3::settings::FORMAT, Family::Piano3, Some(Slot::Settings)),
    (np4::program::FORMAT, Family::Piano4, Some(Slot::Program)),
    (np4::live::FORMAT, Family::Piano4, Some(Slot::Live)),
    (np4::settings::FORMAT, Family::Piano4, Some(Slot::Settings)),
    (np5::program::FORMAT, Family::Piano5, Some(Slot::Program)),
    (np5::live::FORMAT, Family::Piano5, Some(Slot::Live)),
    (np5::settings::FORMAT, Family::Piano5, Some(Slot::Settings)),
    (ng2::program::FORMAT, Family::Grand, Some(Slot::Program)),
    (ng2::live::FORMAT, Family::Grand, Some(Slot::Live)),
    (ng2::settings::FORMAT, Family::Grand, Some(Slot::Settings)),
    (nw::program::FORMAT, Family::Wave, Some(Slot::Program)),
    (nw::settings::FORMAT, Family::Wave, Some(Slot::Settings)),
    (nw2::program::FORMAT, Family::Wave2, Some(Slot::Program)),
    (nw2::live::FORMAT, Family::Wave2, Some(Slot::Live)),
    (nw2::settings::FORMAT, Family::Wave2, Some(Slot::Settings)),
    (nc2::program::FORMAT, Family::C2, Some(Slot::Program)),
    (nc2::settings::FORMAT, Family::C2, Some(Slot::Settings)),
    (npip::pipe_library::FORMAT, Family::C2, None),
    (nc2d::program::FORMAT, Family::C2D, Some(Slot::Program)),
    (nc2d::settings::FORMAT, Family::C2D, Some(Slot::Settings)),
    (no3::program::FORMAT, Family::Organ3, Some(Slot::Program)),
    (no3::settings::FORMAT, Family::Organ3, Some(Slot::Settings)),
    (nl4::program::FORMAT, Family::Lead4, Some(Slot::Program)),
    (nl4::performance::FORMAT, Family::Lead4, None),
    (nl4::settings::FORMAT, Family::Lead4, Some(Slot::Settings)),
    (nla1::program::FORMAT, Family::LeadA1, Some(Slot::Program)),
    (nla1::performance::FORMAT, Family::LeadA1, None),
    (nla1::settings::FORMAT, Family::LeadA1, Some(Slot::Settings)),
    (nd2::program::FORMAT, Family::Drum2, Some(Slot::Program)),
    (nd3::kit::FORMAT, Family::Drum3, Some(Slot::Program)),
];

/// The rows written to the instrument and read back byte-exact: programs and set lists
/// into slots, a sample instrument and a trimmed piano library into their partitions,
/// and the live and settings singletons written in place. Confirmed on hardware.
///
/// The shared library rows are here and not in [`CARRIES`], so they name no family.
const CONFIRMED: &[(Family, Slot, &str)] = &[
    (Family::Electro5, Slot::Program, ne5::program::FORMAT),
    (Family::Electro5, Slot::SetList, ne5::song::FORMAT),
    (Family::Electro5, Slot::Live, ne5::live::FORMAT),
    (Family::Electro5, Slot::Settings, ne5::settings::FORMAT),
    (Family::Electro5, Slot::Sample, nsmp::FORMAT),
    (Family::Electro5, Slot::Piano, npno::FORMAT),
];

// Every hand-written CBIN `FORMAT` reaches one of these two tables, and the stub macro
// asserts its own, so a tag of the wrong length fails the build.
const _: () = {
    let mut i = 0;
    while i < CONFIRMED.len() {
        assert!(CONFIRMED[i].2.len() == 4, "a CBIN tag is four bytes");
        i += 1;
    }
    let mut i = 0;
    while i < CARRIES.len() {
        assert!(CARRIES[i].0.len() == 4, "a CBIN tag is four bytes");
        i += 1;
    }
};

impl Family {
    pub const ALL: [Family; 24] = [
        Family::Electro3,
        Family::Electro4,
        Family::Electro5,
        Family::Electro6,
        Family::Electro7,
        Family::StageClassic,
        Family::Stage2,
        Family::Stage3,
        Family::Stage4,
        Family::Piano,
        Family::Piano2,
        Family::Piano3,
        Family::Piano4,
        Family::Piano5,
        Family::Grand,
        Family::Wave,
        Family::Wave2,
        Family::C2,
        Family::C2D,
        Family::Organ3,
        Family::Lead4,
        Family::LeadA1,
        Family::Drum2,
        Family::Drum3,
    ];

    /// The model, spelled as [`Identity::kind`](crate::Identity::kind) spells it.
    pub fn label(self) -> &'static str {
        match self {
            Family::Electro3 => "Electro 3",
            Family::Electro4 => "Electro 4",
            Family::Electro5 => "Electro 5",
            Family::Electro6 => "Electro 6",
            Family::Electro7 => "Electro 7",
            Family::StageClassic => "Stage Classic",
            Family::Stage2 => "Stage 2",
            Family::Stage3 => "Stage 3",
            Family::Stage4 => "Stage 4",
            Family::Piano => "Piano",
            Family::Piano2 => "Piano 2",
            Family::Piano3 => "Piano 3",
            Family::Piano4 => "Piano 4",
            Family::Piano5 => "Piano 5",
            Family::Grand => "Grand",
            Family::Wave => "Wave",
            Family::Wave2 => "Wave 2",
            Family::C2 => "C2",
            Family::C2D => "C2D",
            Family::Organ3 => "no3 organ",
            Family::Lead4 => "Lead 4",
            Family::LeadA1 => "Lead A1",
            Family::Drum2 => "Drum 2",
            Family::Drum3 => "Drum 3P",
        }
    }

    /// What this family's USB product string contains, where the model's name is known.
    ///
    /// Usually the [`label`](Self::label). Two families differ: the Stage Classic calls
    /// itself `Nord Stage` ("Classic" is this project's name, to tell it from the
    /// numbered Stages), and the `no3` organ's model name is unknown, so it has none.
    fn product_name(self) -> Option<&'static str> {
        match self {
            Family::StageClassic => Some("Stage"),
            Family::Organ3 => None,
            named => Some(named.label()),
        }
    }

    /// The family a USB product string names.
    ///
    /// The string is the model followed by the keybed: an Electro 5 reads
    /// `Nord Electro 5`, and a 73-key 5D reads `Nord Electro 5D 73`. The family is the
    /// one whose model name is the longest the string contains, because `Stage` sits
    /// inside `Stage 3`, `Piano` inside `Piano 5`, and `C2` inside `C2D`.
    ///
    /// `Nord Electro 5` is the descriptor string in the recorded exchanges of
    /// `nord-usb`'s replay scripts. Confirmed on hardware.
    pub fn from_product(product: &str) -> Option<Family> {
        Family::ALL
            .into_iter()
            .filter_map(|family| Some((family, family.product_name()?)))
            .filter(|(_, name)| product.contains(name))
            .max_by_key(|(_, name)| name.len())
            .map(|(family, _)| family)
    }

    /// The family whose files carry `tag`, where one family's do. `None` for the shared
    /// library formats, the carriers, and anything this crate does not read.
    pub fn of_tag(tag: &str) -> Option<Family> {
        CARRIES
            .iter()
            .find(|(held, _, _)| *held == tag)
            .map(|(_, family, _)| *family)
    }

    /// The tag this family keeps in `slot`, which [`accepts`](Self::accepts) takes there.
    /// `None` where the table lists no tag for the pair.
    pub fn tag(self, slot: Slot) -> Option<&'static str> {
        let confirmed = CONFIRMED
            .iter()
            .find(|(family, held, _)| (*family, *held) == (self, slot))
            .map(|(_, _, tag)| *tag);
        confirmed.or_else(|| {
            CARRIES
                .iter()
                .find(|(_, family, held)| (*family, *held) == (self, Some(slot)))
                .map(|(tag, _, _)| *tag)
        })
    }

    /// Whether this family keeps files under `tag` in `slot`.
    pub fn accepts(self, slot: Slot, tag: &str) -> Acceptance {
        if CONFIRMED.contains(&(self, slot, tag)) {
            return Acceptance::Confirmed;
        }
        // A family's tag is refused outside the class the table lists it under, even by
        // its own family: the Stage 4 refuses an `ns4p` in the piano partition just as an
        // Electro 5 does.
        match CARRIES.iter().find(|(held, _, _)| *held == tag) {
            Some((_, family, Some(class))) if (*family, *class) == (self, slot) => {
                Acceptance::Inferred
            }
            Some(_) => Acceptance::Refused,
            None => Acceptance::Unknown,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The four outcomes, on the only family with hardware rows.
    #[test]
    fn an_electro_5_takes_its_own_program_and_refuses_a_stage_4s() {
        let e5 = Family::Electro5;
        assert_eq!(
            e5.accepts(Slot::Program, ne5::program::FORMAT),
            Acceptance::Confirmed
        );
        assert_eq!(
            e5.accepts(Slot::Program, ns4::program::FORMAT),
            Acceptance::Refused
        );
        assert_eq!(
            Family::Stage4.accepts(Slot::Program, ns4::program::FORMAT),
            Acceptance::Inferred
        );
        assert_eq!(e5.accepts(Slot::Program, "zzzz"), Acceptance::Unknown);
    }

    /// The Stage 4 keeps programs in the program slots, so an `ns4p` offered to the piano
    /// partition is refused, not left `Unknown`.
    #[test]
    fn a_familys_own_tag_is_refused_outside_its_class() {
        assert_eq!(
            Family::Stage4.accepts(Slot::Piano, ns4::program::FORMAT),
            Acceptance::Refused
        );
        for (tag, owner, _) in CARRIES {
            let open: Vec<Slot> = Slot::ALL
                .into_iter()
                .filter(|slot| owner.accepts(*slot, tag) != Acceptance::Refused)
                .collect();
            assert!(
                open.len() <= 1,
                "{} leaves its own {tag} unrefused in {open:?}",
                owner.label(),
            );
        }
    }

    /// A shared library format names no family, so it can never refuse an instrument
    /// this table says nothing about.
    #[test]
    fn a_shared_library_format_refuses_nobody() {
        assert_eq!(Family::of_tag(nsmp::FORMAT), None);
        assert_eq!(Family::of_tag(npno::FORMAT), None);
        assert_eq!(
            Family::Electro5.accepts(Slot::Sample, nsmp::FORMAT),
            Acceptance::Confirmed
        );
        assert_eq!(
            Family::Stage4.accepts(Slot::Sample, nsmp::FORMAT),
            Acceptance::Unknown
        );
    }

    /// Every accepted tag is its family's own or is shared, and lands in exactly one of
    /// that family's classes: two classes for one tag would let a file be sent to the
    /// wrong partition and still be called accepted.
    #[test]
    fn every_tag_a_family_takes_lands_in_one_class() {
        let tags = CARRIES
            .iter()
            .map(|(tag, _, _)| *tag)
            .chain(CONFIRMED.iter().map(|(_, _, tag)| *tag));
        for tag in tags {
            for family in Family::ALL {
                let open: Vec<Slot> = Slot::ALL
                    .into_iter()
                    .filter(|slot| {
                        matches!(
                            family.accepts(*slot, tag),
                            Acceptance::Confirmed | Acceptance::Inferred
                        )
                    })
                    .collect();
                assert!(
                    open.len() <= 1,
                    "{} takes {tag} in {open:?}",
                    family.label()
                );
                assert!(
                    open.is_empty() || Family::of_tag(tag).is_none_or(|held| held == family),
                    "{} takes {tag}, which is another family's",
                    family.label()
                );
            }
        }
    }

    #[test]
    fn the_tag_a_family_keeps_in_a_class_is_one_it_accepts_there() {
        assert_eq!(Family::Electro5.tag(Slot::Piano), Some(npno::FORMAT));
        assert_eq!(Family::Stage4.tag(Slot::Piano), None);
        for family in Family::ALL {
            for slot in Slot::ALL {
                let Some(tag) = family.tag(slot) else {
                    continue;
                };
                assert!(
                    matches!(
                        family.accepts(slot, tag),
                        Acceptance::Confirmed | Acceptance::Inferred
                    ),
                    "{} keeps {tag} in {slot:?} but does not accept it there",
                    family.label()
                );
            }
        }
    }

    #[test]
    fn a_foreign_tag_is_refused_in_every_class() {
        for family in Family::ALL {
            for (tag, owner, _) in CARRIES.iter().filter(|(_, owner, _)| *owner != family) {
                for slot in Slot::ALL {
                    assert_eq!(
                        family.accepts(slot, tag),
                        Acceptance::Refused,
                        "{} should refuse {}'s {tag}",
                        family.label(),
                        owner.label()
                    );
                }
            }
        }
    }

    /// The keybed follows the model in the product string, and a shorter model name is a
    /// substring of a longer one.
    #[test]
    fn the_product_string_names_the_model_and_then_the_keybed() {
        assert_eq!(
            Family::from_product("Nord Electro 5"),
            Some(Family::Electro5)
        );
        assert_eq!(
            Family::from_product("Nord Electro 5D 73"),
            Some(Family::Electro5)
        );
        assert_eq!(Family::from_product("Nord Piano 88"), Some(Family::Piano));
        assert_eq!(
            Family::from_product("Nord Piano 5 73"),
            Some(Family::Piano5)
        );
        assert_eq!(Family::from_product("Nord C2D"), Some(Family::C2D));
        assert_eq!(Family::from_product("Nord Wave 2"), Some(Family::Wave2));
        assert_eq!(
            Family::from_product("Nord Stage 88"),
            Some(Family::StageClassic)
        );
        assert_eq!(
            Family::from_product("Nord Stage EX 76"),
            Some(Family::StageClassic)
        );
        assert_eq!(
            Family::from_product("Nord Stage 3 88"),
            Some(Family::Stage3)
        );
        // No product string is known for the `no3` organ.
        assert_eq!(Family::from_product("Nord no3 organ"), None);
        assert_eq!(Family::from_product("Some other keyboard"), None);
    }

    /// Two families under one name would make [`Family::from_product`] pick between them
    /// arbitrarily.
    #[test]
    fn no_two_families_share_a_name() {
        for family in Family::ALL {
            assert_eq!(
                Family::ALL
                    .iter()
                    .filter(|held| held.label() == family.label())
                    .count(),
                1,
                "{} is not a unique name",
                family.label()
            );
            let Some(name) = family.product_name() else {
                continue;
            };
            assert_eq!(
                Family::ALL
                    .iter()
                    .filter(|held| held.product_name() == Some(name))
                    .count(),
                1,
                "{} is not a unique product name",
                family.label()
            );
        }
    }
}
