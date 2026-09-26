//! The browser dock: one tree with three sections, for the places a sound can be, the
//! kinds of sound, and the tags on the local list.
//!
//! Nothing here touches the instrument. Drawing reads the caches and returns a list of
//! [`Act`]s, which [`apply`] then runs against the workspace, the device and the tabs. A
//! row can therefore be drawn while the thing it stands for is about to change.
//!
//! This file holds the state rows share: the selection, the in-place rename, the
//! confirmation modal, and which branches are open. The tree is drawn in `tree`, the drag
//! rules are in `drag`, and a single row is drawn in `row`. Folders and tags for the
//! local list live outside the browser, in [`crate::folders`] and [`crate::tags`].

use std::collections::BTreeSet;
use std::sync::Arc;

use eframe::egui;
use nord_usb::{Location, ObjectClass};

use crate::device::{read_only, Device, DeviceState};
use crate::filter::Filter;
use crate::folders::{self, Folders};
use crate::queue::Queue;
use crate::tags::{self, Tags};
use crate::workspace::Workspace;

mod act;
#[cfg(test)]
pub(crate) mod bench;
mod drag;
mod instrument;
mod row;
mod selection;
mod tree;

pub use act::{apply, bulk, foreign_format, Act, Bulk};
pub use drag::{
    families_present, kinds_present, landing, qualifier, Carried, Held, Item, Kind, Landing, Onto,
};
pub use instrument::about;
pub use row::{cell_ink, starred, Cells};
pub use selection::Selection;
pub use tree::new_menu;

use act::{will_write, write_warnings};
use drag::ghost;
use selection::{gesture, Gesture};
use tree::{Branch, Sections};

/// An in-place rename, waiting on Enter or Esc.
struct Rename {
    what: Item,
    text: String,
    /// True on the first frame, when the field takes focus and selects its text.
    fresh: bool,
}

/// A click on a row, and the list a ⇧-click extends across.
struct Click<'a> {
    item: Item,
    /// The rows of the list this one sits in, in the order they are drawn.
    list: &'a [Item],
}

/// A confirmation asked before something is lost, and the acts a yes runs. One question
/// covers a whole checked set.
struct Ask {
    title: String,
    note: Option<String>,
    verb: &'static str,
    acts: Vec<Act>,
}

/// The new name Enter commits from an in-place rename, if any.
///
/// A blank or unchanged field returns `None`, so no operation is sent that would do
/// nothing.
pub fn renamed(original: &str, typed: &str) -> Option<String> {
    let typed = typed.trim();
    match typed.is_empty() || typed == original.trim() {
        true => None,
        false => Some(typed.to_string()),
    }
}

/// The act a verdict runs. `None` for a refusal, which [`Browser::land`] reports instead.
fn act_of(verdict: Landing) -> Option<Act> {
    Some(match verdict {
        Landing::Copy { class, at } => Act::Copy { class, at },
        Landing::Send { id, class, at } => Act::Send { id, class, at },
        Landing::Rearrange { class, from, to } => Act::Rearrange { class, from, to },
        Landing::File { id, folder } => Act::File {
            id,
            folder: Some(folder),
        },
        Landing::Unfile { id } => Act::File { id, folder: None },
        Landing::No(_) => return None,
    })
}

pub struct Browser {
    selection: Selection,
    rename: Option<Rename>,
    ask: Option<Ask>,
    folders: Folders,
    tags: Tags,
    /// Which of the three sections are showing.
    sections: Sections,
    /// The open branches of the tree. Both places start open, so a new panel shows what
    /// is in them.
    open: BTreeSet<Branch>,
    /// A slot to scroll to and select, once the branches holding it have been drawn.
    jump: Option<(ObjectClass, Location)>,
}

impl Default for Browser {
    fn default() -> Browser {
        Browser {
            selection: Selection::default(),
            rename: None,
            ask: None,
            folders: Folders::default(),
            tags: Tags::default(),
            sections: Sections::default(),
            open: BTreeSet::from([Branch::Computer, Branch::Instrument]),
            jump: None,
        }
    }
}

impl Browser {
    /// Restore the folders and tags from storage.
    pub fn restore(&mut self, storage: &dyn eframe::Storage) {
        self.folders = storage
            .get_string(folders::KEY)
            .map(|text| Folders::read(&text))
            .unwrap_or_default();
        self.tags = storage
            .get_string(tags::KEY)
            .map(|text| Tags::read(&text))
            .unwrap_or_default();
    }

    /// Drop folder and tag memberships of assets the restored list does not hold. Call
    /// once, after every store has been read.
    pub fn settle(&mut self, workspace: &Workspace) {
        self.folders.forget_missing(workspace);
        self.tags.forget_missing(workspace);
    }

    pub fn keep(&self, storage: &mut dyn eframe::Storage) {
        storage.set_string(folders::KEY, self.folders.written());
        storage.set_string(tags::KEY, self.tags.written());
    }

    /// Draw the tree and collect what the user asked for.
    pub fn ui(
        &mut self,
        ui: &mut egui::Ui,
        workspace: &Workspace,
        device: &Device,
        queue: &Queue,
        filter: &Filter,
    ) -> Vec<Act> {
        let mut acts = Vec::new();
        self.dialog(ui.ctx(), &mut acts);
        self.tree(ui, workspace, device, queue, filter, &mut acts);
        ghost(ui.ctx());
        acts
    }

    /// The selection, which the library table and the tree share.
    pub fn picked(&self) -> &Selection {
        &self.selection
    }

    /// The tags on this computer's list, for the views that count them.
    pub fn tags(&self) -> &Tags {
        &self.tags
    }

    /// A click on a row drawn outside the tree, with the same gestures on the same
    /// selection.
    pub fn pick(&mut self, ui: &egui::Ui, item: Item, list: &[Item]) {
        self.clicked(ui, Click { item, list });
    }

    /// Escape clears the selection, wherever its rows were drawn.
    ///
    /// ⚠️ Called whether or not the browser dock is open: the library's table shows the
    /// same selection, and a selection nothing draws could never be cleared.
    ///
    /// During a rename, Escape cancels the rename and the row stays selected.
    pub fn let_go(&mut self, ctx: &egui::Context) {
        if self.rename.is_none() && ctx.input(|input| input.key_pressed(egui::Key::Escape)) {
            self.selection.clear();
        }
    }

    /// Clear the selection, because a click landed below the last row.
    pub fn unpick(&mut self) {
        self.selection.clear();
    }

    /// A click on a row's checkbox, which toggles that row.
    ///
    /// ⚠️ A plain click on the checkbox acts as the ⌘ gesture. A click on the row itself
    /// follows [`gesture`].
    pub fn check(&mut self, item: Item) {
        self.rename = None;
        self.selection.toggle(item);
    }

    fn select(&mut self, item: Item) {
        self.rename = None;
        self.selection.only(item);
    }

    /// Whether this row is the only one selected, which is what F2 renames.
    fn sole_is(&self, item: Item) -> bool {
        self.selection.sole() == Some(item)
    }

    /// Cancel an open rename of `what` and drop it from the selection, because its row is
    /// about to go away and will not be drawn to close the editor.
    fn forget_rename(&mut self, what: Item) {
        if self.rename.as_ref().is_some_and(|r| r.what == what) {
            self.rename = None;
        }
        self.selection.forget(what);
    }

    fn start_rename(&mut self, what: Item, from: &str) {
        self.selection.only(what);
        self.rename = Some(Rename {
            what,
            text: from.to_string(),
            fresh: true,
        });
    }

    /// Apply a click on a row to the selection.
    ///
    /// ⚠️ No click opens the rename editor. An editor opened by a second click on a
    /// selected row would wait with the whole name selected, and the next keystroke, even
    /// one meant for the document, would replace it. Renaming is F2 and the row's menu.
    fn clicked(&mut self, ui: &egui::Ui, click: Click) {
        self.rename = None;
        match gesture(&ui.input(|input| input.modifiers)) {
            Gesture::Plain => self.selection.plain(click.item),
            Gesture::Toggle => self.selection.toggle(click.item),
            Gesture::Extend => self.selection.extend(click.item, click.list),
        }
    }

    /// What one drag carries: the pressed row, and the rest of the selection when the
    /// pressed row is in it.
    pub(crate) fn carrying(
        &self,
        head: Held,
        name: &str,
        workspace: &Workspace,
        device: &DeviceState,
    ) -> Carried {
        let rest: Vec<Held> = match self.selection.holds(head.what) {
            false => Vec::new(),
            true => self
                .selection
                .items()
                .filter(|item| *item != head.what)
                .filter_map(|item| self.held(item, workspace, device))
                .collect(),
        };
        Carried {
            head,
            name: match rest.len() {
                0 => name.to_string(),
                more => format!("{name}  +{more}"),
            },
            rest,
        }
    }

    /// What the drag rules need to know about a row, drawn in the tree or the library's
    /// table. `None` for a row that is never dragged.
    ///
    /// ⚠️ A drag cannot carry a slot of a partition this app cannot name, or a slot the
    /// scan found empty. Neither holds anything this app could ask the instrument for.
    pub(crate) fn held(
        &self,
        item: Item,
        workspace: &Workspace,
        device: &DeviceState,
    ) -> Option<Held> {
        match item {
            Item::Local(id) => {
                let entity = workspace.get(id)?;
                Some(Held {
                    what: item,
                    kind: Kind::of(entity),
                    filed: self.folders.holding(id),
                    fits: crate::device::fit(device, entity).allowed(),
                })
            }
            Item::Folder(_) | Item::Tag(_) => None,
            // What is already on the instrument fits it.
            Item::Slot { class, at } => {
                (!read_only(class) && device.slot(class, at).flatten().is_some()).then_some(Held {
                    what: item,
                    kind: Kind::from_class(class),
                    filed: None,
                    fits: true,
                })
            }
        }
    }

    /// The in-place editor, prefilled and selected.
    ///
    /// ⚠️ Only Enter renames; clicking away cancels. Committing on blur would turn a
    /// stray keystroke into an unwanted rename, and the name is the only record of what
    /// an object is, because the files do not store their own names.
    fn rename_row(&mut self, ui: &mut egui::Ui, indent: f32, original: &str) -> Option<String> {
        let rename = self.rename.as_mut()?;
        let output = ui
            .horizontal(|ui| {
                ui.add_space(indent);
                egui::TextEdit::singleline(&mut rename.text)
                    .desired_width(ui.available_width())
                    .show(ui)
            })
            .inner;
        if rename.fresh {
            rename.fresh = false;
            output.response.request_focus();
            let all = egui::text::CCursorRange::two(
                egui::text::CCursor::new(0),
                egui::text::CCursor::new(rename.text.chars().count()),
            );
            if let Some(mut state) = egui::TextEdit::load_state(ui.ctx(), output.response.id) {
                state.cursor.set_char_range(Some(all));
                state.store(ui.ctx(), output.response.id);
            }
            return None;
        }
        // ⚠️ Global Enter may belong to another editor. Commit only when this field lost
        // focus on the same keypress.
        let lost = output.response.lost_focus();
        let entered = lost && ui.input(|i| i.key_pressed(egui::Key::Enter));
        if !lost {
            return None;
        }
        let typed = std::mem::take(&mut rename.text);
        self.rename = None;
        entered.then(|| renamed(original, &typed)).flatten()
    }

    /// Take a drop, if the dragged row can land here.
    ///
    /// A target that would refuse is not highlighted. Dropping on it anyway reports why
    /// in the status strip.
    pub(crate) fn drop_zone(
        &mut self,
        ui: &egui::Ui,
        response: &egui::Response,
        onto: Onto,
        acts: &mut Vec<Act>,
    ) {
        if let Some(carried) = response.dnd_hover_payload::<Carried>() {
            if landing(&carried.head, onto).allowed() {
                ui.painter().rect_stroke(
                    response.rect,
                    3.0,
                    egui::Stroke::new(1.0_f32, ui.visuals().selection.stroke.color),
                    egui::StrokeKind::Inside,
                );
            }
        }
        let Some(carried) = response.dnd_release_payload::<Carried>() else {
            return;
        };
        self.land(&carried, onto, acts);
    }

    /// Run the drop for the pressed row, and for the rest of what it carries when the
    /// verdict [`Landing::repeats`].
    fn land(&mut self, carried: &Arc<Carried>, onto: Onto, acts: &mut Vec<Act>) {
        let verdict = landing(&carried.head, onto);
        if let Landing::No(why) = verdict {
            return acts.push(Act::Refused(format!(
                "“{}” cannot go there: {why}.",
                carried.name
            )));
        }
        if !verdict.repeats() {
            return acts.extend(act_of(verdict));
        }
        for held in carried.all() {
            let each = landing(&held, onto);
            if each.same(verdict) {
                acts.extend(act_of(each));
            }
        }
    }

    /// Ask before a slot is replaced or emptied.
    fn dialog(&mut self, ctx: &egui::Context, acts: &mut Vec<Act>) {
        let Some(ask) = &self.ask else {
            return;
        };
        let mut decision = None;
        egui::Modal::new(egui::Id::new("browser_ask")).show(ctx, |ui| {
            ui.set_width(400.0);
            ui.heading(&ask.title);
            if let Some(note) = &ask.note {
                ui.add_space(4.0);
                ui.label(note);
            }
            ui.add_space(8.0);
            ui.separator();
            ui.horizontal(|ui| {
                if ui.button("Cancel").clicked() {
                    decision = Some(false);
                }
                if ui
                    .add(egui::Button::new(egui::RichText::new(ask.verb).strong()))
                    .clicked()
                {
                    decision = Some(true);
                }
            });
        });
        match decision {
            Some(true) => {
                if let Some(ask) = self.ask.take() {
                    acts.extend(ask.acts);
                }
            }
            Some(false) => self.ask = None,
            None => {}
        }
    }

    /// The confirmation for a batch write: every entry it would write, and what each
    /// would replace. Entries the instrument has already refused are left out.
    fn ask_send(
        &mut self,
        workspace: &Workspace,
        device: &Device,
        queue: &Queue,
        title: String,
        act: Act,
    ) {
        let mut lines = Vec::new();
        let mut warnings: Vec<String> = Vec::new();
        for held in will_write(queue) {
            let Some(entity) = workspace.get(held.id) else {
                continue;
            };
            let (class, at) = (held.class, held.at);
            for warning in write_warnings(&device.state, class, entity) {
                if !warnings.contains(&warning) {
                    warnings.push(warning);
                }
            }
            // What is known about the slot; for an unread bank, that it has not been
            // read.
            lines.push(format!(
                "“{}” → {}",
                entity.name,
                held.replaces.said(class, at)
            ));
        }
        if lines.is_empty() {
            return;
        }
        // The warnings first: they are the reason to say no.
        let mut note = warnings;
        if !note.is_empty() {
            note.push(String::new());
        }
        note.extend(lines);
        self.ask = Some(Ask {
            title,
            note: Some(note.join("\n")),
            verb: "Send",
            acts: vec![act],
        });
    }

    /// Ask before a write back to one slot, showing the note that write carries.
    fn ask_write(&mut self, name: &str, at: String, note: String, act: Act) {
        self.ask = Some(Ask {
            title: format!("Save “{name}” to {at}?"),
            note: Some(note),
            verb: "Save",
            acts: vec![act],
        });
    }

    /// Ask, as Finder does, before a drop replaces what is in the destination.
    fn ask_replace(
        &mut self,
        occupant: &str,
        incoming: &str,
        at: String,
        warning: Option<String>,
        act: Act,
    ) {
        let note =
            format!("“{occupant}” is read back first and put where it was if anything goes wrong.");
        self.ask = Some(Ask {
            title: format!("Replace “{occupant}” in {at} with “{incoming}”?"),
            note: Some(match warning {
                Some(warning) => format!("{warning}\n\n{note}"),
                None => note,
            }),
            verb: "Replace",
            acts: vec![act],
        });
    }

    /// One bulk action on the checked set, drawn the same in the library's footer and in
    /// a checked row's menu.
    ///
    /// The control is disabled when it has nothing to act on, and its hover says why:
    /// nothing of the right kind is checked, or the attached instrument refuses all of
    /// it. Delete asks first, once for the whole set.
    pub(crate) fn bulk_item(
        &mut self,
        ui: &mut egui::Ui,
        action: Bulk,
        checked: &[Item],
        workspace: &Workspace,
        state: &DeviceState,
        acts: &mut Vec<Act>,
    ) {
        if action == Bulk::Tag {
            let locals: Vec<u64> = checked.iter().copied().filter_map(Item::local).collect();
            ui.add_enabled_ui(!locals.is_empty(), |ui| {
                ui.menu_button(action.label(), |ui| self.tag_items(ui, &locals, acts))
                    .response
                    .on_disabled_hover_text(action.nothing());
            });
            return;
        }
        let wanted = bulk(action, checked, state);
        // ⚠️ Only Queue checks what the instrument accepts. Everything else happens on
        // this computer, where another instrument's file is still a file.
        let fits = (action == Bulk::Queue).then(|| act::fits(checked, workspace, state));
        let label = match &fits {
            Some(fits) => fits.label(),
            None => action.label().to_string(),
        };
        let live = !wanted.is_empty() && fits.as_ref().is_none_or(|fits| fits.takes > 0);
        let dead = fits
            .and_then(|fits| fits.why)
            .unwrap_or_else(|| action.nothing().to_string());
        let mut button = ui
            .add_enabled(live, egui::Button::new(label))
            .on_disabled_hover_text(dead);
        if action == Bulk::Queue {
            button = button
                .on_hover_text("to the slot it is linked to, or the first free one in its folder");
        }
        if !button.clicked() {
            return;
        }
        match action {
            Bulk::Delete => self.ask_discard(checked, wanted),
            _ => acts.extend(wanted),
        }
        ui.close();
    }

    /// Ask once before a checked set is deleted, saying what happens to each part: a slot
    /// is emptied on the instrument, and a local file only leaves the list.
    fn ask_discard(&mut self, checked: &[Item], acts: Vec<Act>) {
        let slots = checked
            .iter()
            .filter(|item| matches!(item, Item::Slot { .. }))
            .count();
        let locals = checked.iter().filter(|item| item.local().is_some()).count();
        let mut note = Vec::new();
        if slots > 0 {
            note.push(format!(
                "{slots} on the instrument are removed from it. There is no undo."
            ));
        }
        if locals > 0 {
            note.push(format!(
                "{locals} leave the list on this computer; the files themselves stay where \
                 they are."
            ));
        }
        self.ask = Some(Ask {
            title: format!("Delete {} checked items?", slots + locals),
            note: Some(note.join("\n\n")),
            verb: "Delete",
            acts,
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::browser::bench::{bench, context};
    use crate::shell::Shell;
    use crate::tabs::Tabs;
    use crate::workspace::Fresh;

    /// Paint the tree headlessly, to catch a layout that panics or an id that collides.
    fn paint(with_device: bool) {
        use crate::workspace::{Fresh, Origin};

        let ctx = context();
        let mut workspace = Workspace::new(ctx.clone());
        let mut device = Device::new(ctx.clone());
        let mut log = crate::log::Log::default();
        let mut tabs = Tabs::default();
        let mut browser = Browser::default();
        let mut queue = Queue::default();

        for kind in [Fresh::Program, Fresh::Live, Fresh::Settings] {
            workspace.create(kind, &mut log).unwrap();
        }
        // A folder with something in it, an empty folder, and a view of a slot: row
        // shapes the list has no other way to reach.
        let full = browser.folders.make().unwrap();
        browser.folders.make().unwrap();
        let filed = workspace.create(Fresh::Program, &mut log).unwrap();
        browser.folders.file(filed, Some(full));
        // A tag on something, and one on nothing: the two shapes the section holds.
        let sunday = browser.tags.make("Sunday").unwrap();
        browser.tags.make("Loud").unwrap();
        browser.tags.set(filed, sunday, true);
        let bytes = workspace.get(filed).unwrap().bytes.clone();
        workspace.view(
            "Africa-Split.ne5p".into(),
            Origin::Device {
                class: ObjectClass::Program,
                at: Location { bank: 6, slot: 0 },
            },
            bytes,
            &mut log,
        );
        if with_device {
            // Every row shape a list can hold: a named slot, a vacant one, the slot the
            // panel is on, and a class that was never read.
            device.pretend_scanned(ObjectClass::Program, 7, &["Africa Split", "", "Squabble B"]);
            device.pretend_scanned(ObjectClass::Program, 8, &["Bass Manual"]);
            device.pretend_scanned(ObjectClass::SetList, 1, &["Sunday"]);
            device.pretend_focused(ObjectClass::Program, Location { bank: 6, slot: 2 });
            // Named banks, which is what a piano's categories arrive as.
            device.pretend_scanned(ObjectClass::Piano, 1, &["Royal Grand 3D"]);
            device.pretend_geometry(ObjectClass::Piano, &[("Grand", 1), ("Upright", 1)]);
            device.pretend_partitions(&crate::device::ELECTRO5);
            // Every branch of the instrument open, so every row shape is painted.
            for class in device.state.classes() {
                browser.open.insert(Branch::Class(class.to_raw()));
                for bank in 0..=8 {
                    browser.open.insert(tree::bank_branch(class, bank));
                }
            }
        }

        // Twice: the second pass runs with the widget state the first left behind.
        for _ in 0..2 {
            let _ = ctx.run(egui::RawInput::default(), |ctx| {
                egui::SidePanel::left("places")
                    .exact_width(crate::shell::BROWSER)
                    .show(ctx, |ui| {
                        let acts = browser.ui(ui, &workspace, &device, &queue, &Filter::default());
                        apply(
                            &mut browser,
                            &mut Shell::default(),
                            acts,
                            &mut workspace,
                            &mut device,
                            &mut tabs,
                            &mut queue,
                            &mut log,
                        );
                    });
            });
        }
    }

    /// An in-memory store for the folders and tags the browser keeps.
    #[derive(Default)]
    struct Fake(std::collections::HashMap<String, String>);

    impl eframe::Storage for Fake {
        fn get_string(&self, key: &str) -> Option<String> {
            self.0.get(key).cloned()
        }
        fn set_string(&mut self, key: &str, value: String) {
            self.0.insert(key.to_string(), value);
        }
        fn flush(&mut self) {}
    }

    #[test]
    fn the_tree_paints_with_nothing_attached() {
        paint(false);
    }

    #[test]
    fn the_tree_paints_with_an_instrument_to_show() {
        paint(true);
    }

    /// A drag from an unselected row carries only that row.
    #[test]
    fn a_drag_from_a_picked_row_carries_the_whole_selection() {
        let (mut browser, mut workspace, device, _tabs, _queue, mut log) = bench();
        let ids: Vec<u64> = (0..3)
            .map(|_| workspace.create(Fresh::Program, &mut log).unwrap())
            .collect();
        let apart = workspace.create(Fresh::Live, &mut log).unwrap();
        for id in &ids {
            browser.selection.toggle(Item::Local(*id));
        }

        let head = browser
            .held(Item::Local(ids[0]), &workspace, &device.state)
            .expect("a local is dragged");
        let carried = browser.carrying(head, "Africa Split", &workspace, &device.state);
        assert_eq!(carried.rest.len(), 2);
        assert_eq!(carried.all().count(), 3);
        assert!(carried.name.contains("+2"), "{}", carried.name);

        let outside = browser
            .held(Item::Local(apart), &workspace, &device.state)
            .expect("a local is dragged");
        let alone = browser.carrying(outside, "Squabble B", &workspace, &device.state);
        assert!(
            alone.rest.is_empty(),
            "an unselected row carries only itself"
        );
        assert_eq!(alone.name, "Squabble B", "and says only its own name");
    }

    /// ⚠️ A slot the scan found empty holds nothing to carry. Carried with a selection,
    /// it would become a read of nothing from the instrument, a round trip that can only
    /// fail.
    #[test]
    fn an_empty_slot_is_not_something_a_drag_carries() {
        let (mut browser, workspace, mut device, _tabs, _queue, _log) = bench();
        device.pretend_scanned(ObjectClass::Program, 7, &["Africa Split", ""]);
        let slot = |slot| Item::Slot {
            class: ObjectClass::Program,
            at: Location { bank: 6, slot },
        };

        let held = browser
            .held(slot(0), &workspace, &device.state)
            .expect("7:1 holds something");
        assert!(
            browser.held(slot(1), &workspace, &device.state).is_none(),
            "7:2 was read and found empty"
        );

        browser.selection.toggle(slot(0));
        browser.selection.toggle(slot(1));
        let carried = browser.carrying(held, "Africa Split", &workspace, &device.state);
        assert!(carried.rest.is_empty(), "the empty slot stays where it is");
        assert_eq!(carried.name, "Africa Split", "and the ghost counts nothing");
    }

    /// ⚠️ The rest of the selection follows only when the drop repeats one act. Filing
    /// three assets is three filings, but sending three to one slot would overwrite each
    /// in turn, so a single destination takes only the pressed row.
    #[test]
    fn a_drop_of_many_repeats_only_where_one_destination_does_not() {
        let (mut browser, mut workspace, device, _tabs, _queue, mut log) = bench();
        let ids: Vec<u64> = (0..3)
            .map(|_| workspace.create(Fresh::Program, &mut log).unwrap())
            .collect();
        for id in &ids {
            browser.selection.toggle(Item::Local(*id));
        }
        let folder = browser.folders.make().unwrap();
        let head = browser
            .held(Item::Local(ids[0]), &workspace, &device.state)
            .unwrap();
        let carried = Arc::new(browser.carrying(head, "Africa Split", &workspace, &device.state));

        let mut filed = Vec::new();
        browser.land(&carried, Onto::Group(folder), &mut filed);
        assert_eq!(filed.len(), 3, "every selected asset goes into the folder");
        assert!(filed.iter().all(|act| matches!(
            act,
            Act::File {
                folder: Some(_),
                ..
            }
        )));

        let mut sent = Vec::new();
        browser.land(
            &carried,
            Onto::Slot {
                class: ObjectClass::Program,
                at: Location { bank: 6, slot: 0 },
            },
            &mut sent,
        );
        assert_eq!(sent.len(), 1, "one slot takes one asset");
        assert!(matches!(sent[0], Act::Send { id, .. } if id == ids[0]));
    }

    /// ⚠️ A row inside a folder is a drop target for that folder. Otherwise, releasing a
    /// filed asset where it was pressed, or on a sibling, would take it out of its
    /// folder.
    #[test]
    fn a_drop_onto_a_row_inside_a_folder_never_unfiles_it() {
        let (mut browser, mut workspace, device, _tabs, _queue, mut log) = bench();
        let folder = browser.folders.make().unwrap();
        let id = workspace.create(Fresh::Program, &mut log).unwrap();
        browser.folders.file(id, Some(folder));
        let head = browser
            .held(Item::Local(id), &workspace, &device.state)
            .expect("a local is dragged");
        let carried = Arc::new(browser.carrying(head, "Africa Split", &workspace, &device.state));

        let mut onto_sibling = Vec::new();
        browser.land(&carried, tree::onto_list(Some(folder)), &mut onto_sibling);
        assert!(
            !onto_sibling
                .iter()
                .any(|act| matches!(act, Act::File { folder: None, .. })),
            "a drop on a row inside the folder keeps the asset in the folder"
        );

        let mut onto_loose = Vec::new();
        browser.land(&carried, tree::onto_list(None), &mut onto_loose);
        assert!(
            matches!(onto_loose.as_slice(), [Act::File { folder: None, .. }]),
            "a drop on a loose row takes the asset out of the folder"
        );
    }

    /// ⚠️ F2 renames only a row selected alone. A rename typed while several are selected
    /// would look like it applies to all of them, and only one would change.
    #[test]
    fn f2_renames_only_while_its_row_is_the_only_one_picked() {
        let (mut browser, _workspace, _device, _tabs, _queue, _log) = bench();
        let row = Item::Local(1);
        browser.selection.only(row);
        assert!(browser.sole_is(row));

        browser.selection.toggle(Item::Local(2));
        assert!(!browser.sole_is(row), "two rows selected");
        browser.selection.plain(Item::Local(2));
        assert!(
            browser.sole_is(row),
            "a plain click on one of the two leaves the other sole"
        );
    }

    /// ⚠️ Escape during a rename cancels only the rename, and the selection stays.
    #[test]
    fn escape_lets_go_of_the_selection_unless_a_name_is_being_typed() {
        let ctx = context();
        let mut browser = Browser::default();
        let escape = egui::RawInput {
            events: vec![egui::Event::Key {
                key: egui::Key::Escape,
                physical_key: None,
                pressed: true,
                repeat: false,
                modifiers: egui::Modifiers::NONE,
            }],
            ..Default::default()
        };

        browser.start_rename(Item::Local(1), "Africa Split");
        let _ = ctx.run(escape.clone(), |ctx| browser.let_go(ctx));
        assert_eq!(
            browser.picked().items().count(),
            1,
            "Escape in the editor cancels only the rename"
        );

        browser.rename = None;
        let _ = ctx.run(escape, |ctx| browser.let_go(ctx));
        assert_eq!(browser.picked().items().count(), 0);
    }

    /// End to end: open the editor, type, press Enter, and the new name comes back as an
    /// act. The tests of [`renamed`] cannot see the text field, and detecting Enter on it
    /// is the part that is easy to get wrong.
    #[test]
    fn typing_a_name_and_pressing_enter_renames_the_row() {
        use crate::workspace::Fresh;

        let ctx = context();
        let mut workspace = Workspace::new(ctx.clone());
        let device = Device::new(ctx.clone());
        let mut log = crate::log::Log::default();
        let mut browser = Browser::default();
        let queue = Queue::default();
        let id = workspace.create(Fresh::Program, &mut log).unwrap();

        let key = |key| egui::Event::Key {
            key,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: egui::Modifiers::default(),
        };
        // Frame one opens the editor, focused with the name selected; frame two types
        // over the name; frame three commits.
        let frames: [Vec<egui::Event>; 3] = [
            Vec::new(),
            vec![egui::Event::Text("LA Grand".into())],
            vec![key(egui::Key::Enter)],
        ];
        browser.start_rename(Item::Local(id), "Africa Split");

        let mut named = None;
        for events in frames {
            let input = egui::RawInput {
                events,
                ..Default::default()
            };
            let _ = ctx.run(input, |ctx| {
                egui::SidePanel::left("places").show(ctx, |ui| {
                    for act in browser.ui(ui, &workspace, &device, &queue, &Filter::default()) {
                        if let Act::RenameLocal { name, .. } = act {
                            named = Some(name);
                        }
                    }
                });
            });
        }
        assert_eq!(named.as_deref(), Some("LA Grand"));
        assert!(browser.rename.is_none(), "and the editor closes");
    }

    #[test]
    fn a_rename_that_changes_nothing_is_not_a_rename() {
        assert_eq!(renamed("Africa Split", "Africa Split"), None);
        assert_eq!(renamed("Africa Split", "  Africa Split "), None);
        assert_eq!(renamed("Africa Split", "   "), None);
        assert_eq!(renamed("Africa Split", ""), None);
    }

    #[test]
    fn a_rename_takes_the_typed_name_trimmed() {
        assert_eq!(renamed("Africa Split", "LA Grand"), Some("LA Grand".into()));
        assert_eq!(
            renamed("Africa Split", "  LA Grand  "),
            Some("LA Grand".into())
        );
    }

    /// The folder store is read separately from the asset store, which decides what
    /// survived. An asset too big to keep, or dropped for lack of room, would otherwise
    /// leave its membership behind for as long as the app is installed.
    #[test]
    fn a_grouping_forgets_the_assets_the_list_came_back_without() {
        let (mut browser, mut workspace, _device, _tabs, _queue, mut log) = bench();
        let here = workspace.create(Fresh::Program, &mut log).unwrap();
        let folder = browser.folders.make().unwrap();
        browser.folders.file(here, Some(folder));
        // As a store that could not keep everything reads back: a membership for an
        // asset the list does not hold.
        browser.folders.file(here + 99, Some(folder));

        browser.settle(&workspace);
        assert_eq!(browser.folders.holding(here), Some(folder));
        assert_eq!(browser.folders.holding(here + 99), None);
        assert_eq!(browser.folders.all().len(), 1, "the folder itself stays");
    }

    /// Two assets selected and saved as a gig have that tag next session, and the third
    /// does not. An asset missing from the restored list leaves no membership behind.
    #[test]
    fn a_tag_put_on_a_multi_selection_comes_back_next_session() {
        let (mut browser, mut workspace, mut device, mut tabs, mut queue, mut log) = bench();
        let ids: Vec<u64> = (0..3)
            .map(|_| workspace.create(Fresh::Program, &mut log).unwrap())
            .collect();
        browser.selection.toggle(Item::Local(ids[0]));
        browser.selection.toggle(Item::Local(ids[1]));

        apply(
            &mut browser,
            &mut Shell::default(),
            vec![Act::SaveAsGig],
            &mut workspace,
            &mut device,
            &mut tabs,
            &mut queue,
            &mut log,
        );
        let Some(Item::Tag(tag)) = browser.rename.as_ref().map(|r| r.what) else {
            panic!("a new gig opens its editor");
        };
        assert!(browser.tags.on_all(&ids[..2], tag));
        assert!(!browser.tags.worn(ids[2]).contains(&tag));

        let mut store = Fake::default();
        browser.keep(&mut store);
        let mut after = Browser::default();
        after.restore(&store);
        after.settle(&workspace);
        assert_eq!(after.tags.name_of(tag), Some("New gig"));
        assert!(after.tags.on_all(&ids[..2], tag));

        workspace.remove(ids[0], &mut log);
        after.settle(&workspace);
        assert_eq!(after.tags.count(tag), 1, "the one still on the list");
    }

    /// ⚠️ A view is the only copy of its bytes and the store skips it, so a tag on a view
    /// would be lost with its tab. Tagging keeps it on this computer first, and the log
    /// says so.
    #[test]
    fn tagging_a_view_keeps_it_on_this_computer_first_and_says_so() {
        use crate::workspace::Origin;

        let (mut browser, mut workspace, mut device, mut tabs, mut queue, mut log) = bench();
        let bytes = {
            let id = workspace.create(Fresh::Program, &mut log).unwrap();
            let bytes = workspace.get(id).unwrap().bytes.clone();
            workspace.remove(id, &mut log);
            bytes
        };
        let id = workspace.view(
            "Africa-Split.ne5p".into(),
            Origin::Device {
                class: ObjectClass::Program,
                at: Location { bank: 6, slot: 0 },
            },
            bytes,
            &mut log,
        );
        let tag = browser.tags.make("Sunday").unwrap();
        assert!(workspace.is_view(id));

        apply(
            &mut browser,
            &mut Shell::default(),
            vec![Act::Tag { ids: vec![id], tag }],
            &mut workspace,
            &mut device,
            &mut tabs,
            &mut queue,
            &mut log,
        );
        assert!(!workspace.is_view(id), "it is kept on this computer");
        assert!(browser.tags.worn(id).contains(&tag));
        let said = log.transcript();
        assert!(said.contains("kept on this computer first"), "{said}");
    }

    /// The warning appears in the batch's modal once per format, however many items carry
    /// it, and above the list of destinations, which readers skim.
    ///
    /// The instrument reports a model the acceptance table does not know, so the only
    /// check left is against the folder's own formats. A known family would refuse these
    /// files outright.
    #[test]
    fn the_modal_says_when_a_batch_is_of_another_model() {
        use crate::workspace::Origin;

        let (mut browser, mut workspace, mut device, _tabs, mut queue, mut log) = bench();
        let class = ObjectClass::Program;
        device.pretend_scanned(class, 7, &["Africa Split", "Squabble B"]);
        device.pretend_attached_as("unnamed device");

        let mut ids = Vec::new();
        for slot in 0..2 {
            let stage = workspace.create(Fresh::Stage4Program, &mut log).unwrap();
            let bytes = workspace.get(stage).unwrap().bytes.clone();
            workspace.remove(stage, &mut log);
            let at = Location { bank: 6, slot };
            let id = workspace.ingest(
                format!("stage-{slot}.ns4p"),
                Origin::Device { class, at },
                bytes,
                &mut log,
            );
            crate::queue::enqueue(&workspace, &mut device, &mut queue, &mut log, id, class, at);
            ids.push(id);
        }

        browser.ask_send(&workspace, &device, &queue, "Send?".into(), Act::SendAll);
        let note = browser.ask.as_ref().and_then(|ask| ask.note.clone());
        let note = note.expect("the modal has a note");
        assert_eq!(note.matches("This file is ns4p").count(), 1, "{note}");
        let warned = note.find("ns4p").expect("the warning is there");
        let listed = note.find("replaces").expect("and so are the destinations");
        assert!(warned < listed, "the warning comes first:\n{note}");
    }
}
