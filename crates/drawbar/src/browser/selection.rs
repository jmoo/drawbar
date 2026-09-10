//! What the browser has picked out, and the three gestures that change it.

use std::collections::BTreeSet;

use eframe::egui;

use super::drag::Item;

/// Which of the three things a click on a row means.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Gesture {
    /// This row and nothing else, or nothing at all when it is the row already picked.
    Plain,
    /// ⌘: this row in or out of what is picked.
    Toggle,
    /// ⇧: the run from the anchor to this row.
    Extend,
}

/// What the modifiers held during a click make it.
///
/// ⚠️ ⌘ wins over ⇧. A run *and* a toggle has no meaning, and the toggle is the one
/// about the row the pointer is actually over.
pub fn gesture(how: &egui::Modifiers) -> Gesture {
    if how.command {
        return Gesture::Toggle;
    }
    match how.shift {
        true => Gesture::Extend,
        false => Gesture::Plain,
    }
}

/// The rows the browser has picked, and the one a ⇧-click measures from.
///
/// ⚠️ Not [`crate::workspace::Workspace::selected`], which is the single asset the
/// document editor is on. This is the browser's own, and a tag, an export or a drag
/// acts on all of it.
#[derive(Default)]
pub struct Selection {
    anchor: Option<Item>,
    set: BTreeSet<Item>,
}

impl Selection {
    /// The one row picked, or `None` when none or several are. What F2 renames.
    pub fn sole(&self) -> Option<Item> {
        match self.set.len() {
            1 => self.set.iter().copied().next(),
            _ => None,
        }
    }

    pub fn holds(&self, item: Item) -> bool {
        self.set.contains(&item)
    }

    pub fn items(&self) -> impl Iterator<Item = Item> + '_ {
        self.set.iter().copied()
    }

    /// The assets on this computer among what is picked, in id order.
    pub fn locals(&self) -> Vec<u64> {
        self.items().filter_map(Item::local).collect()
    }

    /// A plain click: this row and nothing else, or this row let go of when it is
    /// already picked.
    ///
    /// ⚠️ The only gesture that takes a row out without a modifier, which is what leaves
    /// a set of one reachable from the keyboard alone.
    pub fn plain(&mut self, item: Item) {
        match self.holds(item) {
            true => self.toggle(item),
            false => self.only(item),
        }
    }

    /// This row and nothing else, whatever was picked before it.
    pub fn only(&mut self, item: Item) {
        self.anchor = Some(item);
        self.set.clear();
        self.set.insert(item);
    }

    /// Let go of everything.
    pub fn clear(&mut self) {
        self.set.clear();
        self.anchor = None;
    }

    /// ⌘-click: in or out, and the anchor follows the row that was pressed.
    pub fn toggle(&mut self, item: Item) {
        if !self.set.remove(&item) {
            self.set.insert(item);
        }
        self.anchor = Some(item);
    }

    /// ⇧-click: the run between the anchor and this row, inside one list.
    ///
    /// A list the anchor is not in cannot be spanned — the rows between them are in
    /// neither — so that click selects the row it landed on and re-anchors there.
    pub fn extend(&mut self, item: Item, list: &[Item]) {
        let span = self
            .anchor
            .and_then(|anchor| Some((list.iter().position(|held| *held == anchor)?, anchor)))
            .and_then(|(from, anchor)| {
                Some((from, list.iter().position(|held| *held == item)?, anchor))
            });
        let Some((from, to, anchor)) = span else {
            return self.only(item);
        };
        let (first, last) = (from.min(to), from.max(to));
        self.set = list[first..=last].iter().copied().collect();
        self.anchor = Some(anchor);
    }

    /// Take a row out, because it is about to stop existing.
    pub fn forget(&mut self, item: Item) {
        self.set.remove(&item);
        if self.anchor == Some(item) {
            self.anchor = None;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use nord_usb::{Location, ObjectClass};

    fn list(n: u64) -> Vec<Item> {
        (0..n).map(Item::Local).collect()
    }

    /// A plain click is one row, whatever was picked before it.
    #[test]
    fn a_plain_click_picks_one_row_and_drops_the_rest() {
        let mut selection = Selection::default();
        selection.plain(Item::Local(1));
        selection.toggle(Item::Local(2));
        assert_eq!(selection.items().count(), 2);

        selection.plain(Item::Local(3));
        assert_eq!(selection.sole(), Some(Item::Local(3)));
    }

    /// ⚠️ A plain click on a row that is already picked lets go of it — the one gesture
    /// that empties a selection without a modifier. A click that only ever re-picked the
    /// row it landed on leaves no way of putting a set of one down.
    #[test]
    fn a_plain_click_on_a_picked_row_lets_go_of_it() {
        let mut selection = Selection::default();
        selection.plain(Item::Local(1));
        selection.plain(Item::Local(1));
        assert_eq!(selection.items().count(), 0, "the sole row let go of");

        selection.plain(Item::Local(1));
        selection.toggle(Item::Local(2));
        selection.plain(Item::Local(2));
        assert_eq!(
            selection.items().collect::<Vec<_>>(),
            vec![Item::Local(1)],
            "and one of a set leaves the rest picked"
        );
    }

    /// Escape lets go of everything, and the next ⇧-click measures from the row it
    /// lands on rather than from the anchor the cleared set left behind.
    #[test]
    fn clearing_lets_go_of_the_anchor_too() {
        let rows = list(4);
        let mut selection = Selection::default();
        selection.plain(rows[0]);
        selection.clear();
        assert_eq!(selection.items().count(), 0);

        selection.extend(rows[2], &rows);
        assert_eq!(selection.sole(), Some(rows[2]));
    }

    /// ⌘-click adds and removes, so the same click twice leaves what it found.
    #[test]
    fn a_command_click_puts_a_row_in_or_takes_it_out() {
        let mut selection = Selection::default();
        selection.only(Item::Local(1));
        selection.toggle(Item::Local(2));
        assert!(selection.holds(Item::Local(1)) && selection.holds(Item::Local(2)));
        assert_eq!(selection.sole(), None, "two rows have no sole row");

        selection.toggle(Item::Local(2));
        assert_eq!(selection.sole(), Some(Item::Local(1)));
        // And a row can be taken out of a set of one, leaving nothing picked.
        selection.toggle(Item::Local(1));
        assert_eq!(selection.items().count(), 0);
    }

    /// ⇧-click fills the run between the anchor and the row it landed on, either way
    /// round, and the anchor stays where it was so the next one measures from it again.
    #[test]
    fn a_shift_click_fills_the_run_from_the_anchor() {
        let rows = list(6);
        let mut selection = Selection::default();
        selection.only(rows[3]);
        selection.extend(rows[1], &rows);
        assert_eq!(
            selection.items().collect::<Vec<_>>(),
            vec![rows[1], rows[2], rows[3]]
        );

        // Still anchored at 3, so the next one runs the other way from the same place.
        selection.extend(rows[5], &rows);
        assert_eq!(
            selection.items().collect::<Vec<_>>(),
            vec![rows[3], rows[4], rows[5]]
        );
    }

    /// ⚠️ A run needs both ends in one list. The rows of a folder and the rows of a bank
    /// have nothing between them, so a ⇧-click across the two picks the row it landed on
    /// rather than everything the tree happens to hold in between.
    #[test]
    fn a_shift_click_into_another_list_picks_one_row() {
        let elsewhere = Item::Slot {
            class: ObjectClass::Program,
            at: Location { bank: 6, slot: 0 },
        };
        let rows = list(4);
        let mut selection = Selection::default();
        selection.only(elsewhere);
        selection.extend(rows[2], &rows);
        assert_eq!(selection.sole(), Some(rows[2]));

        // And it re-anchored there, so a second one does span.
        selection.extend(rows[0], &rows);
        assert_eq!(selection.items().count(), 3);
    }

    /// ⌘ wins over ⇧, and a bare click is a bare click whatever else is held.
    #[test]
    fn the_modifiers_decide_which_of_the_three_gestures_a_click_is() {
        let with = |command, shift| {
            gesture(&egui::Modifiers {
                command,
                shift,
                ..egui::Modifiers::NONE
            })
        };
        assert_eq!(with(false, false), Gesture::Plain);
        assert_eq!(with(true, false), Gesture::Toggle);
        assert_eq!(with(false, true), Gesture::Extend);
        assert_eq!(with(true, true), Gesture::Toggle);
    }

    /// Locals, then folders, then slots by class and address: the order a tag, an export
    /// or a drag walks the set in, whatever order the clicks arrived in.
    #[test]
    fn a_selection_is_walked_in_one_order_however_it_was_picked() {
        let at = |bank, slot| Item::Slot {
            class: ObjectClass::Program,
            at: Location { bank, slot },
        };
        let mut selection = Selection::default();
        for item in [at(7, 2), Item::Local(9), at(6, 3), Item::Folder(4)] {
            selection.toggle(item);
        }
        assert_eq!(
            selection.items().collect::<Vec<_>>(),
            vec![Item::Local(9), Item::Folder(4), at(6, 3), at(7, 2)]
        );
        assert_eq!(selection.locals(), vec![9]);
    }
}
