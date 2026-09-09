//! What a drag is: the thing being carried, where it may land, and what landing means.
//!
//! Pure rules over the two places a sound can live — nothing here draws a row or touches
//! the instrument, so the whole vocabulary is testable without a frame.

use eframe::egui;
use nord_format::accept::Family;
use nord_format::Entity;
use nord_usb::{Location, ObjectClass};

use crate::device::{read_only, DeviceState};
use crate::icon::Glyph;
use crate::strings::folder;
use crate::workspace::Workspace;

/// What an asset is, which is what decides the folder it belongs in.
///
/// One per family of [`Entity`], so a file that decoded is never called a file. The
/// declaration order is [`Kind::ALL`]'s, which is the order any set of kinds is listed
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
    /// A Lead performance — the multi-slot layer above that family's programs.
    Performance,
    /// A Lead SysEx bank: the dump itself, or the MIDI file carrying one. The two are
    /// one thing under two containers.
    LeadBank,
    /// An Electro 2 sample library — a whole library rather than one instrument, which
    /// is why it is not a [`Kind::Sample`].
    SampleLibrary,
    /// A C2 pipe-organ library.
    PipeLibrary,
    /// An archive of objects: a bundle, a backup, or a Drum-family bank.
    Bundle,
    /// A Nord Sample Editor project (`.nsmpproj`) — a text file that generates a sample,
    /// and the one kind with no folder on the instrument to send it to.
    Project,
    /// Bytes that did not decode.
    Other,
}

/// The kinds the instrument has a folder for, each under the class that folder is.
///
/// One table, read forwards by [`Kind::home`] and backwards by [`Kind::from_class`], so
/// a class and its kind cannot drift apart.
const HOMES: [(Kind, ObjectClass); 6] = [
    (Kind::Program, ObjectClass::Program),
    (Kind::SetList, ObjectClass::SetList),
    (Kind::Sample, ObjectClass::Sample),
    (Kind::Piano, ObjectClass::Piano),
    (Kind::Live, ObjectClass::Live),
    (Kind::Settings, ObjectClass::Settings),
];

impl Kind {
    /// Every kind, in the order anything showing a set of them shows them.
    pub const ALL: [Kind; 16] = [
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
        Kind::Other,
    ];

    /// What a decoded file is. ⚠️ Exhaustive over [`Entity`], so a family the library
    /// adds is a compile error here rather than another nameless row.
    pub fn of(entity: Option<&Entity>) -> Kind {
        match entity {
            Some(Entity::Program(_)) => Kind::Program,
            Some(Entity::Song(_)) => Kind::SetList,
            Some(Entity::Sample(_)) => Kind::Sample,
            Some(Entity::Piano(_) | Entity::PianoLibrary(_)) => Kind::Piano,
            Some(Entity::Live(_)) => Kind::Live,
            Some(Entity::Settings(_)) => Kind::Settings,
            Some(Entity::Synth(_)) => Kind::Synth,
            Some(Entity::OrganPreset(_)) => Kind::OrganPreset,
            Some(Entity::PianoPreset(_)) => Kind::PianoPreset,
            Some(Entity::Performance(_)) => Kind::Performance,
            Some(Entity::Midi(_) | Entity::Sysex(_)) => Kind::LeadBank,
            Some(Entity::Cne3(_)) => Kind::SampleLibrary,
            Some(Entity::PipeLibrary(_)) => Kind::PipeLibrary,
            Some(Entity::Bundle(_)) => Kind::Bundle,
            Some(Entity::SampleProject(_)) => Kind::Project,
            None => Kind::Other,
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
            Kind::Other => "file",
        }
    }

    /// What the tree calls a whole kind of thing.
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
                _ => "Other",
            },
        }
    }

    /// The one glyph this kind wears — in the rail, the table, a tab, the queue and the
    /// slot map alike.
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
            Kind::Other => Glyph::HardDrive,
        }
    }
}

/// The kinds that exist here: what the list on this computer holds, and what the
/// attached instrument has a folder for, in [`Kind::ALL`] order.
///
/// The union of the two places, because a kind is a way of narrowing what the library
/// shows and the library shows both. A row for a kind neither place holds narrows to
/// nothing.
pub fn kinds_present(workspace: &Workspace, device: &DeviceState) -> Vec<Kind> {
    let here: Vec<Kind> = workspace
        .listed()
        .map(|entity| Kind::of(entity.entity.as_ref()))
        .chain(device.classes().into_iter().map(Kind::from_class))
        .collect();
    Kind::ALL
        .into_iter()
        .filter(|kind| here.contains(kind))
        .collect()
}

/// Whether a kind's word needs the family in front of it to say what it is.
///
/// True where the word alone would not settle it: the kept assets are from more than one
/// family, or the asset is not the attached instrument's own. With one family on this
/// computer and that instrument attached, `program` can only mean one thing.
pub fn qualified(kept: &[Family], asset: Option<Family>, instrument: Option<Family>) -> bool {
    if kept.len() > 1 {
        return true;
    }
    matches!((asset, instrument), (Some(asset), Some(held)) if asset != held)
}

/// The families the assets on this computer are from, in [`Family::ALL`] order.
///
/// Files that name no family — the shared library formats, the carriers, bytes that did
/// not decode — are not one, so a list of samples spans no families at all.
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
    /// A grouping of local assets. Only a rename and a selection reach it; a folder is
    /// never dragged and never sent anywhere as a thing of its own.
    Folder(u64),
    Slot {
        class: ObjectClass,
        at: Location,
    },
    /// A label on the local list. Like a folder it is renamed rather than dragged, and
    /// unlike a folder an asset wears as many as it is given.
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

    /// Locals, then folders, then slots by class and address, then tags — the order a
    /// selection is walked in, and the order it comes back from the store in.
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
    /// The folder it is in, for a local asset. What makes dragging one out of a folder
    /// mean something.
    pub filed: Option<u64>,
    /// Whether the attached instrument takes this asset's format, from
    /// [`crate::device::fit`]. Anything already on the instrument fits it.
    pub fits: bool,
}

/// What is under the pointer while a drag is in progress.
///
/// ⚠️ `head` is the row the pointer went down on and `rest` is the selection it brought
/// with it. The verdict is [`landing`] on the head alone; `rest` follows only where the
/// verdict is one act repeated, which [`crate::browser::Browser::land`] decides.
#[derive(Clone)]
pub struct Carried {
    pub head: Held,
    /// What the ghost says: the pressed row, and how many came with it.
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

/// What a drop would do, or the plain reason it would do nothing.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Landing {
    /// Device to this computer: a copy comes back.
    Copy,
    /// This computer to a slot.
    Send,
    /// Slot to slot inside one folder. The instrument swaps them.
    Rearrange,
    /// Into one of this computer's folders. Nothing leaves this computer.
    File,
    /// Out of the folder it is in, back to the loose part of the list.
    Unfile,
    No(&'static str),
}

impl Landing {
    pub fn allowed(self) -> bool {
        !matches!(self, Landing::No(_))
    }
}

/// Whether a drag can end where the pointer is, and what it would mean if it did.
pub fn landing(carried: &Held, onto: Onto) -> Landing {
    match (carried.what, onto) {
        // A folder or a tag is a way of seeing the list, not a row that moves.
        (Item::Folder(_) | Item::Tag(_), _) => Landing::No("that is a list, not a sound"),
        // The loose part of the list is a target only for something that is in a folder,
        // which is how one comes back out of one.
        (Item::Local(_), Onto::Computer) => match carried.filed {
            Some(_) => Landing::Unfile,
            None => Landing::No("it is already on this computer"),
        },
        (Item::Local(_), Onto::Group(id)) => match carried.filed == Some(id) {
            true => Landing::No("it is already in that folder"),
            false => Landing::File,
        },
        // The copy would have to land somewhere before it could be filed, and it lands
        // when the instrument answers rather than when the pointer is let go.
        (Item::Slot { .. }, Onto::Group(_)) => {
            Landing::No("copy it to this computer first, then drag it into the folder")
        }
        // The kind first: a folder this app cannot name is the home of no kind, so the
        // refusal it earns says which folder it is rather than talking about pianos.
        (Item::Local(_), Onto::Slot { class, .. }) => {
            if carried.kind.home() != Some(class) {
                Landing::No("that folder holds a different kind of thing")
            } else if !carried.fits {
                Landing::No("the instrument does not take files of that format")
            } else if read_only(class) {
                Landing::No("pianos are installed on the instrument, not moved into it")
            } else {
                Landing::Send
            }
        }
        (Item::Slot { .. }, Onto::Computer) => Landing::Copy,
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
                Landing::No("the instrument arranges this folder itself")
            } else if was == at {
                Landing::No("it is already there")
            } else {
                Landing::Rearrange
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
    use crate::browser::bench::{local, onto, slot};
    use crate::strings::folder;

    /// The two crossings the browser exists for.
    #[test]
    fn a_drag_between_the_two_places_copies_one_way_and_sends_the_other() {
        assert_eq!(
            landing(&slot(ObjectClass::Program, 6, 3), Onto::Computer),
            Landing::Copy
        );
        assert_eq!(
            landing(&local(Kind::Program), onto(ObjectClass::Program, 6, 3)),
            Landing::Send
        );
    }

    /// An empty slot is a target like any other — that is the whole reason it is a row.
    #[test]
    fn an_empty_slot_is_a_target() {
        assert_eq!(
            landing(&local(Kind::SetList), onto(ObjectClass::SetList, 0, 12)),
            Landing::Send
        );
    }

    /// A drop of something the attached instrument does not take produces no landing —
    /// the target does not light and the drop says why.
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
        assert_eq!(landing(&refused, Onto::Group(1)), Landing::File);
    }

    /// A kind's word carries the family only where the word alone would not settle whose
    /// files these are.
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

    /// A folder holds one kind of thing, and the instrument is not asked to sort it out.
    #[test]
    fn a_thing_cannot_be_dropped_into_a_folder_for_another_kind() {
        for kind in [Kind::SetList, Kind::Sample, Kind::Other] {
            assert!(!landing(&local(kind), onto(ObjectClass::Program, 0, 0)).allowed());
        }
    }

    /// A piano is a library the instrument installs and indexes for itself, and it is
    /// the only folder a drop cannot land in. The buffer classes take one.
    #[test]
    fn only_the_piano_folder_refuses_a_drop() {
        assert!(!landing(
            &local(Kind::from_class(ObjectClass::Piano)),
            onto(ObjectClass::Piano, 0, 0)
        )
        .allowed());
        for class in [ObjectClass::Live, ObjectClass::Settings] {
            let kind = Kind::from_class(class);
            assert!(
                landing(&local(kind), onto(class, 0, 0)).allowed(),
                "{}",
                folder(class)
            );
        }
    }

    /// Slot to slot is the instrument's swap, and only inside one folder.
    #[test]
    fn slots_rearrange_only_within_their_own_folder() {
        assert_eq!(
            landing(
                &slot(ObjectClass::Program, 6, 3),
                onto(ObjectClass::Program, 7, 12)
            ),
            Landing::Rearrange
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

    /// A refusal carries the words the status strip will show, so there is always
    /// something to say.
    #[test]
    fn every_refusal_explains_itself() {
        let cases = [
            landing(&local(Kind::Program), Onto::Computer),
            landing(
                &local(Kind::from_class(ObjectClass::Piano)),
                onto(ObjectClass::Piano, 0, 0),
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

    /// A folder is a way of seeing the local list. Something on this computer goes into
    /// one and comes back out of one; nothing off the instrument does either, because
    /// the copy lands when the instrument answers rather than when the pointer is let go.
    #[test]
    fn a_folder_takes_what_is_already_on_this_computer_and_nothing_else() {
        let filed = |folder| Held {
            filed: folder,
            ..local(Kind::Program)
        };
        assert_eq!(landing(&filed(None), Onto::Group(1)), Landing::File);
        assert_eq!(landing(&filed(Some(2)), Onto::Group(1)), Landing::File);
        assert_eq!(landing(&filed(Some(1)), Onto::Computer), Landing::Unfile);

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

    /// A folder is never the thing being dragged: it is where the list is cut, not a row
    /// that moves.
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

    /// A folder holds exactly the kind named after it, and a kind the instrument has no
    /// folder for belongs nowhere on it.
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

    /// One glyph per kind. Two kinds wearing the same one would read as one kind in the
    /// rail, the table, a tab, the queue and the slot map at once.
    #[test]
    fn no_two_kinds_wear_the_same_glyph() {
        let mut seen: Vec<Glyph> = Vec::new();
        for kind in Kind::ALL {
            let glyph = kind.glyph();
            assert!(!seen.contains(&glyph), "{kind:?} repeats {glyph:?}");
            seen.push(glyph);
        }
    }

    /// Every family the library decodes is a kind of its own. Only bytes that did not
    /// decode are a file.
    #[test]
    fn only_what_did_not_decode_is_called_a_file() {
        assert_eq!(Kind::of(None), Kind::Other);
        for kind in Kind::ALL.iter().filter(|kind| **kind != Kind::Other) {
            assert_ne!(kind.chip(), Kind::Other.chip(), "{kind:?}");
            assert_ne!(kind.plural(), Kind::Other.plural(), "{kind:?}");
        }
    }
}
