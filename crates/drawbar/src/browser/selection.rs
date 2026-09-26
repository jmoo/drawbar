//! The browser's selection, and the three click gestures that change it.

use std::collections::BTreeSet;

use eframe::egui;

use super::drag::Item;

/// What a click on a row does.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Gesture {
    /// Select only this row, or deselect it when it is already selected.
    Plain,
    /// ⌘: add this row to the selection, or remove it.
    Toggle,
    /// ⇧: select the run from the anchor to this row.
    Extend,
}

/// The gesture the modifiers held during a click make.
///
/// ⚠️ ⌘ wins over ⇧. A run and a toggle together mean nothing, and the toggle acts on the
/// row under the pointer.
pub fn gesture(how: &egui::Modifiers) -> Gesture {
    if how.command {
        return Gesture::Toggle;
    }
    match how.shift {
        true => Gesture::Extend,
        false => Gesture::Plain,
    }
}

/// The rows selected in the browser, and the anchor a ⇧-click measures from.
///
/// ⚠️ Not [`crate::tabs::Tabs::active`], which is the one document the center shows. This
/// is the browser's own, and a tag, an export or a drag acts on all of it.
#[derive(Default)]
pub struct Selection {
    anchor: Option<Item>,
    set: BTreeSet<Item>,
}

impl Selection {
    /// The only selected row, or `None` when none or several are. F2 renames it.
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

    /// The selected assets on this computer, in id order.
    pub fn locals(&self) -> Vec<u64> {
        self.items().filter_map(Item::local).collect()
    }

    /// A plain click: select only this row, or deselect it when it is already selected.
    ///
    /// ⚠️ The only gesture that removes a row without a modifier key. Without it, a
    /// single selected row could not be deselected by a plain click.
    pub fn plain(&mut self, item: Item) {
        match self.holds(item) {
            true => self.toggle(item),
            false => self.only(item),
        }
    }

    /// Select only this row, whatever was selected before.
    pub fn only(&mut self, item: Item) {
        self.anchor = Some(item);
        self.set.clear();
        self.set.insert(item);
    }

    /// Clear the selection and its anchor.
    pub fn clear(&mut self) {
        self.set.clear();
        self.anchor = None;
    }

    /// ⌘-click: add or remove the row, and move the anchor to it.
    pub fn toggle(&mut self, item: Item) {
        if !self.set.remove(&item) {
            self.set.insert(item);
        }
        self.anchor = Some(item);
    }

    /// ⇧-click: select the run between the anchor and this row, within one list.
    ///
    /// When the anchor is not in `list` there is no run, so the click selects only this
    /// row and anchors there.
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

    /// Remove a row that is about to go away.
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

    #[test]
    fn a_plain_click_picks_one_row_and_drops_the_rest() {
        let mut selection = Selection::default();
        selection.plain(Item::Local(1));
        selection.toggle(Item::Local(2));
        assert_eq!(selection.items().count(), 2);

        selection.plain(Item::Local(3));
        assert_eq!(selection.sole(), Some(Item::Local(3)));
    }

    /// ⚠️ A plain click on a selected row deselects it. It is the only way to empty a
    /// selection without a modifier; a click that always re-selected its row would leave
    /// no way to clear a set of one.
    #[test]
    fn a_plain_click_on_a_picked_row_lets_go_of_it() {
        let mut selection = Selection::default();
        selection.plain(Item::Local(1));
        selection.plain(Item::Local(1));
        assert_eq!(selection.items().count(), 0, "the only row is deselected");

        selection.plain(Item::Local(1));
        selection.toggle(Item::Local(2));
        selection.plain(Item::Local(2));
        assert_eq!(
            selection.items().collect::<Vec<_>>(),
            vec![Item::Local(1)],
            "and deselecting one of a set keeps the rest"
        );
    }

    /// After a clear, the next ⇧-click selects only the row it lands on, because the
    /// anchor was cleared too.
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

    /// The same ⌘-click twice leaves the selection as it was.
    #[test]
    fn a_command_click_puts_a_row_in_or_takes_it_out() {
        let mut selection = Selection::default();
        selection.only(Item::Local(1));
        selection.toggle(Item::Local(2));
        assert!(selection.holds(Item::Local(1)) && selection.holds(Item::Local(2)));
        assert_eq!(selection.sole(), None, "two rows have no sole row");

        selection.toggle(Item::Local(2));
        assert_eq!(selection.sole(), Some(Item::Local(1)));
        selection.toggle(Item::Local(1));
        assert_eq!(
            selection.items().count(),
            0,
            "removing the only row leaves nothing selected"
        );
    }

    /// ⇧-click fills the run between the anchor and the clicked row in either direction,
    /// and the anchor stays put for the next one.
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

        selection.extend(rows[5], &rows);
        assert_eq!(
            selection.items().collect::<Vec<_>>(),
            vec![rows[3], rows[4], rows[5]],
            "still anchored at 3, so this run goes the other way"
        );
    }

    /// ⚠️ A run needs both ends in one list. Nothing lies between a folder's rows and a
    /// bank's rows, so a ⇧-click across the two selects only the clicked row, not
    /// everything the tree draws in between.
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

        selection.extend(rows[0], &rows);
        assert_eq!(
            selection.items().count(),
            3,
            "it re-anchored there, so a second one spans"
        );
    }

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
