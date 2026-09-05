//! What a drag is: the thing being carried, where it may land, and what landing means.
//!
//! Pure rules over the two places a sound can live — nothing here draws a row or touches
//! the instrument, so the whole vocabulary is testable without a frame.

use eframe::egui;
use nord_format::Entity;
use nord_usb::{Location, ObjectClass};

use crate::device::read_only;

/// What an asset is, which is what decides the folder it belongs in.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Kind {
    Program,
    SetList,
    Sample,
    Piano,
    Live,
    Settings,
    /// Something the instrument has no folder for — a bundle, a preset of a kind no
    /// class holds, a file that did not decode at all.
    Other,
}

impl Kind {
    pub fn of(entity: Option<&Entity>) -> Kind {
        match entity {
            Some(Entity::Program(_)) => Kind::Program,
            Some(Entity::Song(_)) => Kind::SetList,
            Some(Entity::Sample(_)) => Kind::Sample,
            Some(Entity::Piano(_) | Entity::PianoLibrary(_)) => Kind::Piano,
            Some(Entity::Live(_)) => Kind::Live,
            Some(Entity::Settings(_)) => Kind::Settings,
            _ => Kind::Other,
        }
    }

    pub fn from_class(class: ObjectClass) -> Kind {
        match class {
            ObjectClass::Program => Kind::Program,
            ObjectClass::SetList => Kind::SetList,
            ObjectClass::Sample => Kind::Sample,
            ObjectClass::Piano => Kind::Piano,
            ObjectClass::Live => Kind::Live,
            ObjectClass::Settings => Kind::Settings,
            ObjectClass::Unknown(_) => Kind::Other,
        }
    }

    /// The folder on the instrument this kind belongs in.
    pub fn home(self) -> Option<ObjectClass> {
        match self {
            Kind::Program => Some(ObjectClass::Program),
            Kind::SetList => Some(ObjectClass::SetList),
            Kind::Sample => Some(ObjectClass::Sample),
            Kind::Piano => Some(ObjectClass::Piano),
            Kind::Live => Some(ObjectClass::Live),
            Kind::Settings => Some(ObjectClass::Settings),
            Kind::Other => None,
        }
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
            Kind::Other => "file",
        }
    }
}

/// One row of the tree.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Item {
    Local(u64),
    /// A grouping of local assets. Only a rename and a selection reach it; a folder is
    /// never dragged and never sent anywhere as a thing of its own.
    Folder(u64),
    Slot {
        class: ObjectClass,
        at: Location,
    },
}

/// What is under the pointer while a drag is in progress.
#[derive(Clone)]
pub struct Carried {
    pub from: Item,
    pub kind: Kind,
    pub name: String,
    /// The folder it is in, for a local asset. What makes dragging one out of a folder
    /// mean something.
    pub filed: Option<u64>,
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
pub fn landing(carried: &Carried, onto: Onto) -> Landing {
    match (carried.from, onto) {
        // A folder is a way of seeing the list, not a row that moves.
        (Item::Folder(_), _) => Landing::No("a folder is not dragged"),
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
        (Item::Local(_), Onto::Slot { class, .. }) => {
            if read_only(class) {
                Landing::No("pianos are installed on the instrument, not moved into it")
            } else if carried.kind.home() != Some(class) {
                Landing::No("that folder holds a different kind of thing")
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
                Landing::No("pianos stay where the instrument put them")
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
    use crate::device::BROWSED;
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
        let filed = |folder| Carried {
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
        let carried = Carried {
            from: Item::Folder(1),
            kind: Kind::Program,
            name: "Sunday".into(),
            filed: None,
        };
        for onto in [
            Onto::Computer,
            Onto::Group(2),
            onto(ObjectClass::Program, 6, 3),
        ] {
            assert!(!landing(&carried, onto).allowed());
        }
    }

    /// A folder holds exactly the kind named after it.
    #[test]
    fn every_kind_knows_the_folder_it_belongs_in() {
        for class in BROWSED {
            assert_eq!(Kind::from_class(class).home(), Some(class), "{class:?}");
        }
        assert_eq!(Kind::Other.home(), None);
    }
}
