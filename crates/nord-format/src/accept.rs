//! Which format tags an instrument family takes, and the class it keeps each of them in.
//!
//! A file names its model in its four-character tag; an instrument names its own in the
//! USB product string. [`Family`] is the vocabulary the two meet in, and
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

/// One instrument family: the unit both a file's tag and an instrument's product string
/// name.
///
/// Models whose files carry one set of tags are one family — the Electro 3 and 3 HP, the
/// Electro 4 and 4D, the Stage Classic and Stage EX — because nothing in a file says
/// which of the pair wrote it.
///
/// Two families are absent, because their files carry no tag to name them by: the older
/// Leads ride a SysEx or MIDI carrier shared across four models, and the Electro 2's
/// sample library is its own container under no CBIN tag at all.
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
/// [`Library`]. This is that same vocabulary without the wire, so a caller holding both
/// converts rather than keeping a second table.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Slot {
    Piano,
    Sample,
    Program,
    SetList,
    /// The live buffer — the panel as it stands.
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
    /// The tag is another family's.
    Refused,
    /// Nothing here says either way: a tag no family's own files carry — the shared
    /// library formats and the carriers — or one this crate does not read.
    Unknown,
}

/// How a row of [`TAKES`] is known.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Evidence {
    /// Confirmed on hardware.
    Hardware,
    /// Inferred from specimens; not confirmed on hardware.
    Specimens,
}

impl Evidence {
    fn acceptance(self) -> Acceptance {
        match self {
            Evidence::Hardware => Acceptance::Confirmed,
            Evidence::Specimens => Acceptance::Inferred,
        }
    }
}

/// What each family keeps in each of its classes.
///
/// The Electro 5's rows are `Hardware` because this project has written each of them to
/// the instrument and read the body back byte-exact: programs and set lists into slots,
/// a sample instrument and a trimmed piano library into their partitions, and the live
/// and settings singletons written in place. Every other row is the tag its family's
/// format module declares, in the class the module's own name gives it.
const TAKES: &[(Family, Slot, &str, Evidence)] = &[
    (
        Family::Electro3,
        Slot::Program,
        ne3::program::FORMAT,
        Evidence::Specimens,
    ),
    (
        Family::Electro4,
        Slot::Program,
        ne4::program::FORMAT,
        Evidence::Specimens,
    ),
    (
        Family::Electro4,
        Slot::Live,
        ne4::live::FORMAT,
        Evidence::Specimens,
    ),
    (
        Family::Electro4,
        Slot::Settings,
        ne4::settings::FORMAT,
        Evidence::Specimens,
    ),
    (
        Family::Electro5,
        Slot::Program,
        ne5::program::FORMAT,
        Evidence::Hardware,
    ),
    (
        Family::Electro5,
        Slot::SetList,
        ne5::song::FORMAT,
        Evidence::Hardware,
    ),
    (
        Family::Electro5,
        Slot::Live,
        ne5::live::FORMAT,
        Evidence::Hardware,
    ),
    (
        Family::Electro5,
        Slot::Settings,
        ne5::settings::FORMAT,
        Evidence::Hardware,
    ),
    (
        Family::Electro5,
        Slot::Sample,
        nsmp::FORMAT,
        Evidence::Hardware,
    ),
    (
        Family::Electro5,
        Slot::Piano,
        npno::FORMAT,
        Evidence::Hardware,
    ),
    (
        Family::Electro6,
        Slot::Program,
        ne6::program::FORMAT,
        Evidence::Specimens,
    ),
    (
        Family::Electro6,
        Slot::Live,
        ne6::live::FORMAT,
        Evidence::Specimens,
    ),
    (
        Family::Electro6,
        Slot::Settings,
        ne6::settings::FORMAT,
        Evidence::Specimens,
    ),
    (
        Family::Electro7,
        Slot::Program,
        ne7::program::FORMAT,
        Evidence::Specimens,
    ),
    (
        Family::Electro7,
        Slot::Live,
        ne7::live::FORMAT,
        Evidence::Specimens,
    ),
    (
        Family::Electro7,
        Slot::Settings,
        ne7::settings::FORMAT,
        Evidence::Specimens,
    ),
    (
        Family::StageClassic,
        Slot::Program,
        nsclassic::program::FORMAT,
        Evidence::Specimens,
    ),
    (
        Family::StageClassic,
        Slot::Piano,
        nsclassic::piano_library::FORMAT,
        Evidence::Specimens,
    ),
    (
        Family::Stage2,
        Slot::Program,
        ns2::program::FORMAT,
        Evidence::Specimens,
    ),
    (
        Family::Stage2,
        Slot::Live,
        ns2::live::FORMAT,
        Evidence::Specimens,
    ),
    (
        Family::Stage2,
        Slot::Settings,
        ns2::settings::FORMAT,
        Evidence::Specimens,
    ),
    (
        Family::Stage3,
        Slot::Program,
        ns3::program::FORMAT,
        Evidence::Specimens,
    ),
    (
        Family::Stage3,
        Slot::Live,
        ns3::live::FORMAT,
        Evidence::Specimens,
    ),
    (
        Family::Stage3,
        Slot::SetList,
        ns3::song::FORMAT,
        Evidence::Specimens,
    ),
    (
        Family::Stage3,
        Slot::Settings,
        ns3::settings::FORMAT,
        Evidence::Specimens,
    ),
    (
        Family::Stage4,
        Slot::Program,
        ns4::program::FORMAT,
        Evidence::Specimens,
    ),
    (
        Family::Stage4,
        Slot::Live,
        ns4::live::FORMAT,
        Evidence::Specimens,
    ),
    (
        Family::Stage4,
        Slot::Settings,
        ns4::settings::FORMAT,
        Evidence::Specimens,
    ),
    (
        Family::Piano,
        Slot::Program,
        np::program::FORMAT,
        Evidence::Specimens,
    ),
    (
        Family::Piano,
        Slot::Live,
        np::live::FORMAT,
        Evidence::Specimens,
    ),
    (
        Family::Piano,
        Slot::Settings,
        np::settings::FORMAT,
        Evidence::Specimens,
    ),
    (
        Family::Piano2,
        Slot::Program,
        np2::program::FORMAT,
        Evidence::Specimens,
    ),
    (
        Family::Piano2,
        Slot::Live,
        np2::live::FORMAT,
        Evidence::Specimens,
    ),
    (
        Family::Piano2,
        Slot::Settings,
        np2::settings::FORMAT,
        Evidence::Specimens,
    ),
    (
        Family::Piano3,
        Slot::Program,
        np3::program::FORMAT,
        Evidence::Specimens,
    ),
    (
        Family::Piano3,
        Slot::Live,
        np3::live::FORMAT,
        Evidence::Specimens,
    ),
    (
        Family::Piano3,
        Slot::Settings,
        np3::settings::FORMAT,
        Evidence::Specimens,
    ),
    (
        Family::Piano4,
        Slot::Program,
        np4::program::FORMAT,
        Evidence::Specimens,
    ),
    (
        Family::Piano4,
        Slot::Live,
        np4::live::FORMAT,
        Evidence::Specimens,
    ),
    (
        Family::Piano4,
        Slot::Settings,
        np4::settings::FORMAT,
        Evidence::Specimens,
    ),
    (
        Family::Piano5,
        Slot::Program,
        np5::program::FORMAT,
        Evidence::Specimens,
    ),
    (
        Family::Piano5,
        Slot::Live,
        np5::live::FORMAT,
        Evidence::Specimens,
    ),
    (
        Family::Piano5,
        Slot::Settings,
        np5::settings::FORMAT,
        Evidence::Specimens,
    ),
    (
        Family::Grand,
        Slot::Program,
        ng2::program::FORMAT,
        Evidence::Specimens,
    ),
    (
        Family::Grand,
        Slot::Live,
        ng2::live::FORMAT,
        Evidence::Specimens,
    ),
    (
        Family::Grand,
        Slot::Settings,
        ng2::settings::FORMAT,
        Evidence::Specimens,
    ),
    (
        Family::Wave,
        Slot::Program,
        nw::program::FORMAT,
        Evidence::Specimens,
    ),
    (
        Family::Wave,
        Slot::Settings,
        nw::settings::FORMAT,
        Evidence::Specimens,
    ),
    (
        Family::Wave2,
        Slot::Program,
        nw2::program::FORMAT,
        Evidence::Specimens,
    ),
    (
        Family::Wave2,
        Slot::Live,
        nw2::live::FORMAT,
        Evidence::Specimens,
    ),
    (
        Family::Wave2,
        Slot::Settings,
        nw2::settings::FORMAT,
        Evidence::Specimens,
    ),
    (
        Family::C2,
        Slot::Program,
        nc2::program::FORMAT,
        Evidence::Specimens,
    ),
    (
        Family::C2,
        Slot::Settings,
        nc2::settings::FORMAT,
        Evidence::Specimens,
    ),
    (
        Family::C2D,
        Slot::Program,
        nc2d::program::FORMAT,
        Evidence::Specimens,
    ),
    (
        Family::C2D,
        Slot::Settings,
        nc2d::settings::FORMAT,
        Evidence::Specimens,
    ),
    (
        Family::Organ3,
        Slot::Program,
        no3::program::FORMAT,
        Evidence::Specimens,
    ),
    (
        Family::Organ3,
        Slot::Settings,
        no3::settings::FORMAT,
        Evidence::Specimens,
    ),
    (
        Family::Lead4,
        Slot::Program,
        nl4::program::FORMAT,
        Evidence::Specimens,
    ),
    (
        Family::Lead4,
        Slot::Settings,
        nl4::settings::FORMAT,
        Evidence::Specimens,
    ),
    (
        Family::LeadA1,
        Slot::Program,
        nla1::program::FORMAT,
        Evidence::Specimens,
    ),
    (
        Family::LeadA1,
        Slot::Settings,
        nla1::settings::FORMAT,
        Evidence::Specimens,
    ),
    (
        Family::Drum2,
        Slot::Program,
        nd2::program::FORMAT,
        Evidence::Specimens,
    ),
    (
        Family::Drum3,
        Slot::Program,
        nd3::kit::FORMAT,
        Evidence::Specimens,
    ),
];

/// Which family's files carry a tag.
///
/// ⚠️ The shared library formats are deliberately absent: a sample instrument and a
/// piano library are one file several families read, so a tag missing here is *not*
/// evidence that an instrument refuses it. Only a tag listed here can refuse another
/// family's instrument.
const CARRIES: &[(&str, Family)] = &[
    (ne3::program::FORMAT, Family::Electro3),
    (ne3::organ_preset::FORMAT, Family::Electro3),
    (ne4::program::FORMAT, Family::Electro4),
    (ne4::live::FORMAT, Family::Electro4),
    (ne4::settings::FORMAT, Family::Electro4),
    (ne5::program::FORMAT, Family::Electro5),
    (ne5::live::FORMAT, Family::Electro5),
    (ne5::song::FORMAT, Family::Electro5),
    (ne5::settings::FORMAT, Family::Electro5),
    (ne6::program::FORMAT, Family::Electro6),
    (ne6::live::FORMAT, Family::Electro6),
    (ne6::settings::FORMAT, Family::Electro6),
    (ne7::program::FORMAT, Family::Electro7),
    (ne7::live::FORMAT, Family::Electro7),
    (ne7::settings::FORMAT, Family::Electro7),
    (nsclassic::program::FORMAT, Family::StageClassic),
    (nsclassic::synth::FORMAT, Family::StageClassic),
    (nsclassic::piano_library::FORMAT, Family::StageClassic),
    (ns2::program::FORMAT, Family::Stage2),
    (ns2::live::FORMAT, Family::Stage2),
    (ns2::synth::FORMAT, Family::Stage2),
    (ns2::settings::FORMAT, Family::Stage2),
    (ns3::program::FORMAT, Family::Stage3),
    (ns3::live::FORMAT, Family::Stage3),
    (ns3::song::FORMAT, Family::Stage3),
    (ns3::synth::FORMAT, Family::Stage3),
    (ns3::settings::FORMAT, Family::Stage3),
    (ns4::program::FORMAT, Family::Stage4),
    (ns4::live::FORMAT, Family::Stage4),
    (ns4::synth::FORMAT, Family::Stage4),
    (ns4::piano_preset::FORMAT, Family::Stage4),
    (ns4::organ_preset::FORMAT, Family::Stage4),
    (ns4::settings::FORMAT, Family::Stage4),
    (np::program::FORMAT, Family::Piano),
    (np::live::FORMAT, Family::Piano),
    (np::settings::FORMAT, Family::Piano),
    (np2::program::FORMAT, Family::Piano2),
    (np2::live::FORMAT, Family::Piano2),
    (np2::settings::FORMAT, Family::Piano2),
    (np3::program::FORMAT, Family::Piano3),
    (np3::live::FORMAT, Family::Piano3),
    (np3::settings::FORMAT, Family::Piano3),
    (np4::program::FORMAT, Family::Piano4),
    (np4::live::FORMAT, Family::Piano4),
    (np4::settings::FORMAT, Family::Piano4),
    (np5::program::FORMAT, Family::Piano5),
    (np5::live::FORMAT, Family::Piano5),
    (np5::settings::FORMAT, Family::Piano5),
    (ng2::program::FORMAT, Family::Grand),
    (ng2::live::FORMAT, Family::Grand),
    (ng2::settings::FORMAT, Family::Grand),
    (nw::program::FORMAT, Family::Wave),
    (nw::settings::FORMAT, Family::Wave),
    (nw2::program::FORMAT, Family::Wave2),
    (nw2::live::FORMAT, Family::Wave2),
    (nw2::settings::FORMAT, Family::Wave2),
    (nc2::program::FORMAT, Family::C2),
    (nc2::settings::FORMAT, Family::C2),
    (npip::pipe_library::FORMAT, Family::C2),
    (nc2d::program::FORMAT, Family::C2D),
    (nc2d::settings::FORMAT, Family::C2D),
    (no3::program::FORMAT, Family::Organ3),
    (no3::settings::FORMAT, Family::Organ3),
    (nl4::program::FORMAT, Family::Lead4),
    (nl4::performance::FORMAT, Family::Lead4),
    (nl4::settings::FORMAT, Family::Lead4),
    (nla1::program::FORMAT, Family::LeadA1),
    (nla1::performance::FORMAT, Family::LeadA1),
    (nla1::settings::FORMAT, Family::LeadA1),
    (nd2::program::FORMAT, Family::Drum2),
    (nd3::kit::FORMAT, Family::Drum3),
];

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

    /// The family a USB product string names.
    ///
    /// The string is the model and then the keybed — an Electro 5 reads
    /// `Nord Electro 5`, and a 73-key 5D reads `Nord Electro 5D 73` — so the family is
    /// the **longest** [`label`](Self::label) the string contains. Longest because
    /// `Piano` sits inside `Piano 5` and `C2` inside `C2D`.
    ///
    /// Confirmed on hardware for the Electro 5: `Nord Electro 5` is the descriptor
    /// string the recorded exchanges in `nord-usb`'s replay scripts carry.
    pub fn from_product(product: &str) -> Option<Family> {
        Family::ALL
            .into_iter()
            .filter(|family| product.contains(family.label()))
            .max_by_key(|family| family.label().len())
    }

    /// The family whose files carry `tag`, where one family's do. `None` for the shared
    /// library formats, the carriers, and anything this crate does not read.
    pub fn of_tag(tag: &str) -> Option<Family> {
        CARRIES
            .iter()
            .find(|(held, _)| *held == tag)
            .map(|(_, family)| *family)
    }

    /// Whether this family keeps files under `tag` in `slot`.
    pub fn accepts(self, slot: Slot, tag: &str) -> Acceptance {
        if let Some((_, _, _, evidence)) = TAKES
            .iter()
            .find(|(family, held, format, _)| (*family, *held, *format) == (self, slot, tag))
        {
            return evidence.acceptance();
        }
        match Family::of_tag(tag) {
            Some(other) if other != self => Acceptance::Refused,
            _ => Acceptance::Unknown,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The four outcomes, on the one family whose rows are hardware.
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
        for family in Family::ALL {
            let mine: Vec<&str> = TAKES
                .iter()
                .filter(|(held, _, _, _)| *held == family)
                .map(|(_, _, tag, _)| *tag)
                .collect();
            for tag in &mine {
                assert_eq!(
                    mine.iter().filter(|held| *held == tag).count(),
                    1,
                    "{} lists {tag} in more than one class",
                    family.label()
                );
                assert!(
                    Family::of_tag(tag).is_none_or(|held| held == family),
                    "{} takes {tag}, which is another family's",
                    family.label()
                );
            }
        }
    }

    /// A tag another family's files carry is refused in every class.
    #[test]
    fn a_foreign_tag_is_refused_in_every_class() {
        for family in Family::ALL {
            for (tag, owner) in CARRIES.iter().filter(|(_, owner)| *owner != family) {
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
        }
    }
}
