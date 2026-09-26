//! Drag and drop: what is carried, where it may land, and what landing does.
//!
//! Pure rules over the two places a sound can be. Apart from [`ghost`], nothing here
//! draws or touches the instrument, so the rules are testable without a frame.

use eframe::egui;
use nord_format::accept::Family;
use nord_format::Entity;
use nord_usb::{Location, ObjectClass};

use crate::device::{read_only, DeviceState};
use crate::icon::Glyph;
use crate::strings::folder;
use crate::workspace::{LocalEntity, Workspace};

/// What an asset is, which decides the folder it belongs in.
///
/// Every decoded [`Entity`] has a kind of its own, so a decoded file is never called just
/// a file. Declaration order matches [`Kind::ALL`], the order any set of kinds is listed
/// in.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Kind {
    Program,
    SetList,
    Sample,
    Piano,
    Live,
    Settings,
    /// A synth patch, on the models that bank them separately from programs.
    Synth,
    OrganPreset,
    PianoPreset,
    /// A Lead performance: the multi-slot layer above that family's programs.
    Performance,
    /// A Lead SysEx bank: the dump itself, or the MIDI file carrying one. The content is
    /// the same in either container.
    LeadBank,
    /// An Electro 2 sample library. It holds a whole library, not one instrument, so it
    /// is not a [`Kind::Sample`].
    SampleLibrary,
    /// A C2 pipe-organ library.
    PipeLibrary,
    /// An archive of objects: a bundle, a backup, or a Drum-family bank.
    Bundle,
    /// A Nord Sample Editor project (`.nsmpproj`): a text file that generates a sample.
    /// No instrument has a folder for it.
    Project,
    /// A text note. No instrument holds one, so it has no folder and nothing sends it.
    Text,
    /// Bytes that did not decode.
    Other,
}

/// The kinds the instrument has a folder for, each with that folder's class.
///
/// [`Kind::home`] reads the table forward and [`Kind::from_class`] backward, so a class
/// and its kind cannot drift apart.
const HOMES: [(Kind, ObjectClass); 6] = [
    (Kind::Program, ObjectClass::Program),
    (Kind::SetList, ObjectClass::SetList),
    (Kind::Sample, ObjectClass::Sample),
    (Kind::Piano, ObjectClass::Piano),
    (Kind::Live, ObjectClass::Live),
    (Kind::Settings, ObjectClass::Settings),
];

impl Kind {
    /// Every kind, in the order any list of kinds uses.
    pub const ALL: [Kind; 17] = [
        Kind::Program,
        Kind::SetList,
        Kind::Sample,
        Kind::Piano,
        Kind::Live,
        Kind::Settings,
        Kind::Synth,
        Kind::OrganPreset,
        Kind::PianoPreset,
        Kind::Performance,
        Kind::LeadBank,
        Kind::SampleLibrary,
        Kind::PipeLibrary,
        Kind::Bundle,
        Kind::Project,
        Kind::Text,
        Kind::Other,
    ];

    /// What an asset is.
    ///
    /// ⚠️ Exhaustive over [`Entity`], so a family the library adds is a compile error
    /// here and never a nameless row.
    ///
    /// Bytes that did not decode are a note when
    /// [`is_text`](crate::document::text::is_text) said so on arrival, and
    /// [`Kind::Other`] otherwise.
    pub fn of(entity: &LocalEntity) -> Kind {
        let Some(decoded) = entity.entity.as_ref() else {
            return match entity.is_text {
                true => Kind::Text,
                false => Kind::Other,
            };
        };
        match decoded {
            Entity::Program(_) => Kind::Program,
            Entity::Song(_) => Kind::SetList,
            Entity::Sample(_) => Kind::Sample,
            Entity::Piano(_) | Entity::PianoLibrary(_) => Kind::Piano,
            Entity::Live(_) => Kind::Live,
            Entity::Settings(_) => Kind::Settings,
            Entity::Synth(_) => Kind::Synth,
            Entity::OrganPreset(_) => Kind::OrganPreset,
            Entity::PianoPreset(_) => Kind::PianoPreset,
            Entity::Performance(_) => Kind::Performance,
            Entity::Midi(_) | Entity::Sysex(_) => Kind::LeadBank,
            Entity::Cne3(_) => Kind::SampleLibrary,
            Entity::PipeLibrary(_) => Kind::PipeLibrary,
            Entity::Bundle(_) => Kind::Bundle,
            Entity::SampleProject(_) => Kind::Project,
        }
    }

    pub fn from_class(class: ObjectClass) -> Kind {
        HOMES
            .iter()
            .find(|(_, held)| *held == class)
            .map_or(Kind::Other, |(kind, _)| *kind)
    }

    /// The folder on the instrument this kind belongs in.
    pub fn home(self) -> Option<ObjectClass> {
        HOMES
            .iter()
            .find(|(kind, _)| *kind == self)
            .map(|(_, class)| *class)
    }

    /// The small word next to a row's name.
    pub fn chip(self) -> &'static str {
        match self {
            Kind::Program => "program",
            Kind::SetList => "set list",
            Kind::Sample => "sample",
            Kind::Piano => "piano",
            Kind::Live => "live",
            Kind::Settings => "settings",
            Kind::Synth => "synth preset",
            Kind::OrganPreset => "organ preset",
            Kind::PianoPreset => "piano preset",
            Kind::Performance => "performance",
            Kind::LeadBank => "lead bank",
            Kind::SampleLibrary => "sample library",
            Kind::PipeLibrary => "pipe library",
            Kind::Bundle => "bundle",
            Kind::Project => "project",
            Kind::Text => "note",
            Kind::Other => "file",
        }
    }

    /// The plural name the tree shows for this kind.
    pub fn plural(self) -> &'static str {
        match self.home() {
            Some(class) => folder(class),
            None => match self {
                Kind::Synth => "Synth presets",
                Kind::OrganPreset => "Organ presets",
                Kind::PianoPreset => "Piano presets",
                Kind::Performance => "Performances",
                Kind::LeadBank => "Lead banks",
                Kind::SampleLibrary => "Sample libraries",
                Kind::PipeLibrary => "Pipe organ libraries",
                Kind::Bundle => "Bundles",
                Kind::Project => "Sample Editor projects",
                Kind::Text => "Notes",
                _ => "Other",
            },
        }
    }

    /// This kind's glyph, used in the rail, the table, a tab, the queue and the slot map.
    pub fn glyph(self) -> Glyph {
        match self {
            Kind::Program => Glyph::Disc3,
            Kind::SetList => Glyph::ListMusic,
            Kind::Sample => Glyph::AudioWaveform,
            Kind::Piano => Glyph::Piano,
            Kind::Live => Glyph::AudioLines,
            Kind::Settings => Glyph::SlidersHorizontal,
            Kind::Synth => Glyph::Waves,
            Kind::OrganPreset => Glyph::Columns2,
            Kind::PianoPreset => Glyph::CircleDot,
            Kind::Performance => Glyph::Keyboard,
            Kind::LeadBank => Glyph::Save,
            Kind::SampleLibrary => Glyph::LibraryBig,
            Kind::PipeLibrary => Glyph::SlidersVertical,
            Kind::Bundle => Glyph::Folder,
            Kind::Project => Glyph::FolderGit2,
            Kind::Text => Glyph::FileText,
            Kind::Other => Glyph::HardDrive,
        }
    }
}

/// The kinds present: what the list on this computer holds and what the attached
/// instrument has a folder for, in [`Kind::ALL`] order.
///
/// The union of both places, because a kind narrows what the library shows, and the
/// library shows both.
pub fn kinds_present(workspace: &Workspace, device: &DeviceState) -> Vec<Kind> {
    let here: Vec<Kind> = workspace
        .listed()
        .map(Kind::of)
        .chain(device.classes().into_iter().map(Kind::from_class))
        .collect();
    Kind::ALL
        .into_iter()
        .filter(|kind| here.contains(kind))
        .collect()
}

/// The family to put before an asset's kind word, or `None` when the word alone is
/// clear. Shared by the tree and the library's table, which draw the same word.
pub fn qualifier(
    entity: &LocalEntity,
    kept: &[Family],
    instrument: Option<Family>,
) -> Option<Family> {
    let family = Family::of_tag(&entity.tag());
    qualified(kept, family, instrument)
        .then_some(family)
        .flatten()
}

/// Whether a kind's word needs the family before it.
///
/// True when the kept assets span more than one family, or the asset is not from the
/// attached instrument's family. With one family on this computer and that instrument
/// attached, `program` can mean only one thing.
fn qualified(kept: &[Family], asset: Option<Family>, instrument: Option<Family>) -> bool {
    if kept.len() > 1 {
        return true;
    }
    matches!((asset, instrument), (Some(asset), Some(held)) if asset != held)
}

/// The families of the assets on this computer, in [`Family::ALL`] order.
///
/// Files that name no family (the shared library formats, the carriers, bytes that did
/// not decode) add none, so a list of samples spans no families.
pub fn families_present(workspace: &Workspace) -> Vec<Family> {
    let here: Vec<Family> = workspace
        .listed()
        .filter_map(|entity| Family::of_tag(&entity.tag()))
        .collect();
    Family::ALL
        .into_iter()
        .filter(|family| here.contains(family))
        .collect()
}

/// One row of the tree.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Item {
    Local(u64),
    /// A folder of local assets. It can be renamed and selected, but it is never dragged
    /// or sent.
    Folder(u64),
    Slot {
        class: ObjectClass,
        at: Location,
    },
    /// A label on the local list. Like a folder, it is renamed and never dragged; unlike
    /// folders, an asset can have any number of tags.
    Tag(u64),
}

impl Item {
    /// The asset on this computer this row stands for, if it is one.
    pub fn local(self) -> Option<u64> {
        match self {
            Item::Local(id) => Some(id),
            _ => None,
        }
    }

    /// Locals, then folders, then slots by class and address, then tags: the order a
    /// selection iterates in.
    fn key(self) -> (u8, u32, u32, u64) {
        match self {
            Item::Local(id) => (0, 0, 0, id),
            Item::Folder(id) => (1, 0, 0, id),
            Item::Slot { class, at } => (2, class.to_raw(), at.bank, u64::from(at.slot)),
            Item::Tag(id) => (3, 0, 0, id),
        }
    }
}

impl Ord for Item {
    fn cmp(&self, other: &Item) -> std::cmp::Ordering {
        self.key().cmp(&other.key())
    }
}

impl PartialOrd for Item {
    fn partial_cmp(&self, other: &Item) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

/// One row a drag is carrying, and what the rules need to know about it.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Held {
    pub what: Item,
    pub kind: Kind,
    /// The folder a local asset is in, which lets a drag take it out of the folder.
    pub filed: Option<u64>,
    /// Whether the attached instrument takes this asset's format, from
    /// [`crate::device::fit`]. Anything already on the instrument fits it.
    pub fits: bool,
}

/// What a drag in progress carries.
///
/// ⚠️ `head` is the row the pointer was pressed on, and `rest` is the selection it
/// brought along. The verdict is [`landing`] on the head alone; `rest` follows only when
/// that verdict [`Landing::repeats`].
#[derive(Clone)]
pub struct Carried {
    pub head: Held,
    /// The ghost's text: the pressed row's name, and how many came with it.
    pub name: String,
    pub rest: Vec<Held>,
}

impl Carried {
    /// The pressed row and everything it brought, in that order.
    pub fn all(&self) -> impl Iterator<Item = Held> + '_ {
        std::iter::once(self.head).chain(self.rest.iter().copied())
    }
}

/// Where a drop would land.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Onto {
    Computer,
    /// One of this computer's own folders.
    Group(u64),
    Slot {
        class: ObjectClass,
        at: Location,
    },
}

/// What a drop would do, with everything needed to run it, or the reason it would do
/// nothing. Nothing downstream re-derives the verdict from the drag.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Landing {
    /// Instrument to this computer: a copy comes back.
    Copy {
        class: ObjectClass,
        at: Location,
    },
    /// This computer to a slot.
    Send {
        id: u64,
        class: ObjectClass,
        at: Location,
    },
    /// Slot to slot inside one folder. The instrument swaps them.
    Rearrange {
        class: ObjectClass,
        from: Location,
        to: Location,
    },
    /// Into one of this computer's folders. Nothing leaves this computer.
    File {
        id: u64,
        folder: u64,
    },
    /// Out of the folder it is in, back to the loose part of the list.
    Unfile {
        id: u64,
    },
    No(&'static str),
}

impl Landing {
    pub fn allowed(self) -> bool {
        !matches!(self, Landing::No(_))
    }

    /// Whether the rest of what the drag carries follows the pressed row.
    ///
    /// ⚠️ A send and a rearrange name one destination, and several rows sent to one slot
    /// would overwrite each other, so those take only the pressed row.
    pub(super) fn repeats(self) -> bool {
        matches!(
            self,
            Landing::Copy { .. } | Landing::File { .. } | Landing::Unfile { .. }
        )
    }

    /// Whether two verdicts are the same variant, whatever each names.
    pub(super) fn same(self, other: Landing) -> bool {
        std::mem::discriminant(&self) == std::mem::discriminant(&other)
    }
}

/// Whether a drag can end where the pointer is, and what it would mean if it did.
pub fn landing(carried: &Held, onto: Onto) -> Landing {
    match (carried.what, onto) {
        // A folder or a tag groups the list; it is not a row that moves.
        (Item::Folder(_) | Item::Tag(_), _) => Landing::No("that is a list, not a sound"),
        // The loose part of the list takes a drop only from something in a folder, which
        // takes it out of the folder.
        (Item::Local(id), Onto::Computer) => match carried.filed {
            Some(_) => Landing::Unfile { id },
            None => Landing::No("it is already on this computer"),
        },
        (Item::Local(id), Onto::Group(folder)) => match carried.filed == Some(folder) {
            true => Landing::No("it is already in that folder"),
            false => Landing::File { id, folder },
        },
        // A copy lands when the instrument answers, after the pointer is released, so
        // there is nothing yet to file.
        (Item::Slot { .. }, Onto::Group(_)) => {
            Landing::No("copy it to this computer first, then drag it into the folder")
        }
        // A folder this app cannot name is no kind's home, so the kind check also keeps
        // drops out of it.
        (Item::Local(id), Onto::Slot { class, at }) => {
            if carried.kind.home() != Some(class) {
                Landing::No("that folder holds a different kind of thing")
            } else if !carried.fits {
                Landing::No("the instrument does not take files of that format")
            } else {
                Landing::Send { id, class, at }
            }
        }
        (Item::Slot { class, at }, Onto::Computer) => Landing::Copy { class, at },
        (
            Item::Slot {
                class: from,
                at: was,
            },
            Onto::Slot { class, at },
        ) => {
            if from != class {
                Landing::No("things only move within their own folder")
            } else if read_only(class) {
                Landing::No("drawbar does not know what that folder holds")
            } else if was == at {
                Landing::No("it is already there")
            } else {
                Landing::Rearrange {
                    class,
                    from: was,
                    to: at,
                }
            }
        }
    }
}

/// The name of whatever is being dragged, following the pointer.
pub(super) fn ghost(ctx: &egui::Context) {
    let Some(carried) = egui::DragAndDrop::payload::<Carried>(ctx) else {
        return;
    };
    let Some(at) = ctx.pointer_interact_pos() else {
        return;
    };
    let painter = ctx.layer_painter(egui::LayerId::new(
        egui::Order::Tooltip,
        egui::Id::new("drag_ghost"),
    ));
    let where_ = at + egui::vec2(12.0, 6.0);
    let text = painter.layout_no_wrap(
        carried.name.clone(),
        egui::FontId::proportional(12.0),
        ctx.style().visuals.strong_text_color(),
    );
    painter.rect_filled(
        egui::Rect::from_min_size(where_, text.size()).expand(4.0),
        3.0,
        ctx.style().visuals.window_fill,
    );
    painter.galley(where_, text, egui::Color32::PLACEHOLDER);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::browser::bench::{local, onto, slot, CARRIED};
    use crate::strings::folder;

    #[test]
    fn a_drag_between_the_two_places_copies_one_way_and_sends_the_other() {
        assert_eq!(
            landing(&slot(ObjectClass::Program, 6, 3), Onto::Computer),
            Landing::Copy {
                class: ObjectClass::Program,
                at: Location { bank: 6, slot: 3 },
            }
        );
        assert_eq!(
            landing(&local(Kind::Program), onto(ObjectClass::Program, 6, 3)),
            Landing::Send {
                id: CARRIED,
                class: ObjectClass::Program,
                at: Location { bank: 6, slot: 3 },
            }
        );
    }

    /// An empty slot is a drop target, which is why it is drawn as a row.
    #[test]
    fn an_empty_slot_is_a_target() {
        assert_eq!(
            landing(&local(Kind::SetList), onto(ObjectClass::SetList, 0, 12)),
            Landing::Send {
                id: CARRIED,
                class: ObjectClass::SetList,
                at: Location { bank: 0, slot: 12 },
            }
        );
    }

    /// A drop of something the attached instrument does not take is refused: the target
    /// is not highlighted, and the drop says why.
    #[test]
    fn a_drop_of_what_the_instrument_refuses_lands_nowhere() {
        let refused = Held {
            fits: false,
            ..local(Kind::Program)
        };
        match landing(&refused, onto(ObjectClass::Program, 6, 3)) {
            Landing::No(why) => assert!(why.contains("format"), "{why}"),
            other => panic!("{other:?} should have been refused"),
        }
        // It is still a row of this computer's list, so filing it is untouched.
        assert_eq!(
            landing(&refused, Onto::Group(1)),
            Landing::File {
                id: CARRIED,
                folder: 1
            }
        );
    }

    #[test]
    fn the_family_is_named_only_where_it_says_something_the_kind_does_not() {
        let e5 = Some(Family::Electro5);
        let s4 = Some(Family::Stage4);

        assert!(
            !qualified(&[Family::Electro5], e5, e5),
            "one family, its own"
        );
        assert!(
            !qualified(&[Family::Electro5], e5, None),
            "nothing attached"
        );
        assert!(
            !qualified(&[], None, e5),
            "nothing on this computer names one"
        );
        assert!(qualified(&[Family::Electro5, Family::Stage4], e5, None));
        assert!(
            qualified(&[Family::Stage4], s4, e5),
            "not this instrument's"
        );
    }

    /// Each folder on the instrument holds one kind.
    #[test]
    fn a_thing_cannot_be_dropped_into_a_folder_for_another_kind() {
        for kind in [Kind::SetList, Kind::Sample, Kind::Other] {
            assert!(!landing(&local(kind), onto(ObjectClass::Program, 0, 0)).allowed());
        }
    }

    /// Every folder this app can name takes a drop, the two libraries and the buffer
    /// classes included. A partition it cannot name is no kind's home, so nothing drops
    /// into it.
    #[test]
    fn a_drop_lands_in_every_folder_this_app_can_name() {
        for class in [
            ObjectClass::Piano,
            ObjectClass::Sample,
            ObjectClass::Live,
            ObjectClass::Settings,
        ] {
            let kind = Kind::from_class(class);
            assert!(
                landing(&local(kind), onto(class, 0, 0)).allowed(),
                "{}",
                folder(class)
            );
        }
        assert!(!landing(
            &local(Kind::from_class(ObjectClass::Piano)),
            onto(ObjectClass::Unknown(9), 0, 0)
        )
        .allowed());
    }

    /// Slot to slot is the instrument's swap, and only inside one folder.
    #[test]
    fn slots_rearrange_only_within_their_own_folder() {
        assert_eq!(
            landing(
                &slot(ObjectClass::Program, 6, 3),
                onto(ObjectClass::Program, 7, 12)
            ),
            Landing::Rearrange {
                class: ObjectClass::Program,
                from: Location { bank: 6, slot: 3 },
                to: Location { bank: 7, slot: 12 },
            }
        );
        assert!(!landing(
            &slot(ObjectClass::Program, 6, 3),
            onto(ObjectClass::SetList, 0, 0)
        )
        .allowed());
    }

    /// Dropping something back where it came from is not a move.
    #[test]
    fn dropping_a_slot_on_itself_does_nothing() {
        assert!(!landing(
            &slot(ObjectClass::Program, 6, 3),
            onto(ObjectClass::Program, 6, 3)
        )
        .allowed());
        assert!(!landing(&local(Kind::Program), Onto::Computer).allowed());
    }

    /// Every refusal carries a reason for the status strip.
    #[test]
    fn every_refusal_explains_itself() {
        let cases = [
            landing(&local(Kind::Program), Onto::Computer),
            landing(
                &slot(ObjectClass::Unknown(9), 0, 0),
                onto(ObjectClass::Unknown(9), 1, 0),
            ),
            landing(&local(Kind::Other), onto(ObjectClass::Program, 0, 0)),
            landing(
                &slot(ObjectClass::Program, 0, 0),
                onto(ObjectClass::Sample, 0, 0),
            ),
        ];
        for case in cases {
            match case {
                Landing::No(why) => assert!(!why.is_empty()),
                other => panic!("{other:?} should have been refused"),
            }
        }
    }

    /// An asset on this computer can be filed into a folder and taken out again. A slot
    /// cannot be dropped into one, because its copy lands when the instrument answers,
    /// after the pointer is released.
    #[test]
    fn a_folder_takes_what_is_already_on_this_computer_and_nothing_else() {
        let filed = |folder| Held {
            filed: folder,
            ..local(Kind::Program)
        };
        let into = Landing::File {
            id: CARRIED,
            folder: 1,
        };
        assert_eq!(landing(&filed(None), Onto::Group(1)), into);
        assert_eq!(landing(&filed(Some(2)), Onto::Group(1)), into);
        assert_eq!(
            landing(&filed(Some(1)), Onto::Computer),
            Landing::Unfile { id: CARRIED }
        );

        for refused in [
            landing(&filed(Some(1)), Onto::Group(1)),
            landing(&filed(None), Onto::Computer),
            landing(&slot(ObjectClass::Program, 6, 3), Onto::Group(1)),
        ] {
            match refused {
                Landing::No(why) => assert!(!why.is_empty()),
                other => panic!("{other:?} should have been refused"),
            }
        }
    }

    #[test]
    fn a_folder_is_not_something_that_is_dragged() {
        let carried = Held {
            what: Item::Folder(1),
            kind: Kind::Program,
            filed: None,
            fits: true,
        };
        for onto in [
            Onto::Computer,
            Onto::Group(2),
            onto(ObjectClass::Program, 6, 3),
        ] {
            assert!(!landing(&carried, onto).allowed());
        }
    }

    /// Each instrument folder holds the kind named after it, and a kind with no folder
    /// has no home on the instrument.
    #[test]
    fn every_kind_knows_the_folder_it_belongs_in() {
        let homed: Vec<Kind> = HOMES.iter().map(|(kind, _)| *kind).collect();
        for (kind, class) in HOMES {
            assert_eq!(Kind::from_class(class), kind, "{}", folder(class));
            assert_eq!(kind.home(), Some(class), "{kind:?}");
        }
        for homeless in Kind::ALL.iter().filter(|kind| !homed.contains(kind)) {
            assert_eq!(homeless.home(), None, "{homeless:?}");
        }
        assert_eq!(Kind::from_class(ObjectClass::Unknown(9)), Kind::Other);
    }

    /// Two kinds with one glyph would read as one kind in the rail, the table, a tab, the
    /// queue and the slot map.
    #[test]
    fn no_two_kinds_wear_the_same_glyph() {
        let mut seen: Vec<Glyph> = Vec::new();
        for kind in Kind::ALL {
            let glyph = kind.glyph();
            assert!(!seen.contains(&glyph), "{kind:?} repeats {glyph:?}");
            seen.push(glyph);
        }
    }

    #[test]
    fn what_did_not_decode_is_a_note_or_a_file() {
        let mut workspace = Workspace::new(egui::Context::default());
        let mut log = crate::log::Log::default();
        let mut held = |bytes: Vec<u8>| {
            let id = workspace.ingest(
                "held".to_string(),
                crate::workspace::Origin::Fresh,
                bytes,
                &mut log,
            );
            Kind::of(workspace.get(id).expect("it is on the list"))
        };
        assert_eq!(held(b"Set 1\n".to_vec()), Kind::Text);
        assert_eq!(held(Vec::new()), Kind::Text, "a new note holds nothing yet");
        assert_eq!(held(vec![0x00, 0xff]), Kind::Other);

        for kind in Kind::ALL.iter().filter(|kind| **kind != Kind::Other) {
            assert_ne!(kind.chip(), Kind::Other.chip(), "{kind:?}");
            assert_ne!(kind.plural(), Kind::Other.plural(), "{kind:?}");
        }
    }
}
