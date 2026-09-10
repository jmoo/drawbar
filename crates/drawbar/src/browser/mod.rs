//! The browser dock: one tree over the places a sound can live, the kinds there are,
//! and the tags on the list.
//!
//! Nothing here touches the instrument. Rendering reads the caches and answers with a
//! list of [`Act`]s, which [`apply`] then runs against the workspace, the device and the
//! tabs — so a row can be drawn while the thing it stands for is about to change.
//!
//! This file holds the state every row shares — what is picked, the in-place rename, the
//! one modal, which branches are open. The tree itself is `tree`, the drag vocabulary is
//! `drag`, the row it is painted from is `row`, and the grouping and labelling of the
//! local list live outside the browser entirely, in [`crate::folders`] and
//! [`crate::tags`].

use std::collections::BTreeSet;
use std::sync::Arc;

use eframe::egui;
use nord_usb::{Location, ObjectClass};

use crate::device::{read_only, Device, DeviceState};
use crate::filter::Filter;
use crate::folders::{self, Folders};
use crate::queue::Queue;
use crate::strings::place;
use crate::tags::{self, Tags};
use crate::workspace::Workspace;

mod act;
#[cfg(test)]
mod bench;
mod drag;
mod instrument;
mod row;
mod selection;
mod tree;

pub use act::{apply, bulk, foreign_format, Act, Bulk};
pub use drag::{
    families_present, kinds_present, landing, qualified, Carried, Held, Item, Kind, Landing, Onto,
};
pub use instrument::about;
pub use row::{cell_ink, starred, Cells, Drawn};
pub use selection::Selection;
pub use tree::new_menu;

use act::write_warnings;
use drag::ghost;
use selection::{gesture, Gesture};
use tree::{Branch, Sections};

/// An in-place rename, waiting on Enter or Esc.
struct Rename {
    what: Item,
    text: String,
    /// The first frame, in which the field takes focus and selects what is in it.
    fresh: bool,
}

/// A click on a row: which row, and the run a ⇧-click may fill.
struct Click<'a> {
    item: Item,
    /// The rows of the list this one sits in, in the order they are drawn.
    list: &'a [Item],
}

/// A question that has to be answered before something is lost, and everything the
/// answer commits to. One question covers a whole checked set.
struct Ask {
    title: String,
    note: Option<String>,
    verb: &'static str,
    acts: Vec<Act>,
}

/// What Enter does to an in-place rename: nothing, or a new name.
///
/// A blank field is not a name and an unchanged one is not a rename, so both leave the
/// asset alone rather than sending an operation that would do nothing.
pub fn renamed(original: &str, typed: &str) -> Option<String> {
    let typed = typed.trim();
    match typed.is_empty() || typed == original.trim() {
        true => None,
        false => Some(typed.to_string()),
    }
}

pub struct Browser {
    selection: Selection,
    rename: Option<Rename>,
    ask: Option<Ask>,
    folders: Folders,
    tags: Tags,
    /// Which of the three sections are showing.
    sections: Sections,
    /// The branches of the tree that are open. The two places are, so a panel that has
    /// never been touched shows what is in them.
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
    /// Put the grouping and the labelling back where they were left.
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

    /// Reconcile the grouping and the labelling with the list that came back beside
    /// them. Call once, after every store has been read.
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

    /// What is picked. One selection, so a row picked in the library table is the row
    /// the tree shows picked.
    pub fn picked(&self) -> &Selection {
        &self.selection
    }

    /// How the list on this computer is labelled, for the views that show a count of it.
    pub fn tags(&self) -> &Tags {
        &self.tags
    }

    /// A click on a row drawn somewhere other than the tree: the same three gestures
    /// over the same set.
    pub fn pick(&mut self, ui: &egui::Ui, item: Item, list: &[Item]) {
        self.clicked(ui, Click { item, list });
    }

    /// Escape lets go of everything picked, wherever its rows were drawn.
    ///
    /// ⚠️ Called whether or not the browser dock is open: the library's table shows the
    /// same selection, and a set nothing draws is one nothing can put down.
    ///
    /// An open rename keeps Escape for itself. That is how a name being typed is taken
    /// back, and the row it is being typed on stays picked.
    pub fn let_go(&mut self, ctx: &egui::Context) {
        if self.rename.is_none() && ctx.input(|input| input.key_pressed(egui::Key::Escape)) {
            self.selection.clear();
        }
    }

    /// Let go of everything picked, because a click landed past the last row.
    pub fn unpick(&mut self) {
        self.selection.clear();
    }

    /// A click on a row's own box: that row in or out of what is checked.
    ///
    /// ⚠️ A box is a checkbox, so a plain click on it is the ⌘ gesture. A click on the
    /// row itself still means what [`gesture`] says it means.
    pub fn check(&mut self, item: Item) {
        self.rename = None;
        self.selection.toggle(item);
    }

    fn select(&mut self, item: Item) {
        self.rename = None;
        self.selection.only(item);
    }

    /// Whether this row is the only one picked, which is what F2 renames.
    fn sole_is(&self, item: Item) -> bool {
        self.selection.sole() == Some(item)
    }

    /// Take back an armed rename, because the row it belongs to is about to stop
    /// existing and no row will be drawn to close it.
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

    /// What a click on a row does, and the list it can be extended across.
    ///
    /// ⚠️ No click arms the rename editor. One armed by a second click on a picked row
    /// sits there with the whole name selected, so the next keystroke — one meant for
    /// the document, or a stray one — replaces it. Renaming is F2 and the row's menu.
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

    /// What the drag rules need to know about a row, or nothing for a row that is never
    /// dragged — wherever the row was drawn, the tree or the library's table.
    ///
    /// ⚠️ Pianos are libraries the instrument installs and indexes for itself, so a slot
    /// in one is not something a drag can pick up and copy back.
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
                    kind: Kind::of(entity.entity.as_ref()),
                    filed: self.folders.holding(id),
                    fits: crate::device::fit(device, entity).allowed(),
                })
            }
            Item::Folder(_) | Item::Tag(_) => None,
            // What is already on the instrument fits it by having got there.
            Item::Slot { class, .. } => (!read_only(class)).then_some(Held {
                what: item,
                kind: Kind::from_class(class),
                filed: None,
                fits: true,
            }),
        }
    }

    // ---- shared pieces ----------------------------------------------------------

    /// The in-place editor, prefilled and selected.
    ///
    /// ⚠️ **Only Enter renames.** Clicking away cancels. An editor that commits on blur
    /// turns a stray keystroke into a rename nobody asked for, and the name is the only
    /// record of what an object is — files store no name of their own.
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

    /// Take a drop, if this is somewhere the dragged thing can land.
    ///
    /// A target that would refuse does not light up; dropping on it anyway says why in
    /// the status strip rather than silently doing nothing.
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

    /// Run the drop, for the pressed row and for everything it carried.
    ///
    /// ⚠️ The rest of the selection follows only where the verdict is one act repeated.
    /// A send and a rearrange name **one** destination, and handing several rows to one
    /// slot would write them over each other; those take the pressed row alone.
    fn land(&mut self, carried: &Arc<Carried>, onto: Onto, acts: &mut Vec<Act>) {
        let verdict = landing(&carried.head, onto);
        match verdict {
            Landing::No(why) => acts.push(Act::Refused(format!(
                "“{}” cannot go there — {why}.",
                carried.name
            ))),
            Landing::Send | Landing::Rearrange => self.one(carried.head, verdict, onto, acts),
            Landing::Copy | Landing::File | Landing::Unfile => {
                for held in carried.all() {
                    if landing(&held, onto) == verdict {
                        self.one(held, verdict, onto, acts);
                    }
                }
            }
        }
    }

    fn one(&mut self, held: Held, verdict: Landing, onto: Onto, acts: &mut Vec<Act>) {
        match (verdict, held.what, onto) {
            (Landing::Copy, Item::Slot { class, at }, _) => acts.push(Act::Copy { class, at }),
            (Landing::Rearrange, Item::Slot { at: from, .. }, Onto::Slot { class, at }) => acts
                .push(Act::Rearrange {
                    class,
                    from,
                    to: at,
                }),
            (Landing::Send, Item::Local(id), Onto::Slot { class, at }) => {
                acts.push(Act::Send { id, class, at })
            }
            (Landing::File, Item::Local(id), Onto::Group(folder)) => acts.push(Act::File {
                id,
                folder: Some(folder),
            }),
            (Landing::Unfile, Item::Local(id), Onto::Computer) => {
                acts.push(Act::File { id, folder: None })
            }
            // Every allowed pairing is spelled out above; a shape that reaches here is a
            // verdict about a drag that did not come from where it says it did.
            _ => {}
        }
    }

    /// Ask before a slot is replaced or emptied. The only dialogs left in the app.
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

    /// The one question a write asks: everything the queue is about to write, and what
    /// each of it would replace.
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
        for held in queue.entries() {
            let Some(entity) = workspace.get(held.id) else {
                continue;
            };
            let (class, at) = (held.class, held.at);
            let where_ = place(class, at);
            for warning in write_warnings(&device.state, class, entity) {
                if !warnings.contains(&warning) {
                    warnings.push(warning);
                }
            }
            lines.push(match device.state.slot(class, at).flatten() {
                Some(info) => format!(
                    "“{}” replaces “{}” in {where_}",
                    entity.name,
                    info.name.trim()
                ),
                None => format!("“{}” goes into {where_}, which is empty", entity.name),
            });
        }
        if lines.is_empty() {
            return;
        }
        // The warnings first: they are the reason to say no. Then what this send is
        // about to walk past, which is the other one.
        let mut note = warnings;
        note.extend(
            crate::queue::Behind::of(workspace, &device.state, queue)
                .said()
                .map(|said| format!("{said}, and not in this send.")),
        );
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

    /// Raise what a write back to one slot carries, where it carries anything.
    fn ask_write(&mut self, name: &str, at: String, note: String, act: Act) {
        self.ask = Some(Ask {
            title: format!("Save “{name}” to {at}?"),
            note: Some(note),
            verb: "Save",
            acts: vec![act],
        });
    }

    /// Raise the one Finder-style question a drop can need: the destination is taken.
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

    /// One of the things that can be asked of everything checked, drawn the same in the
    /// library's footer and in a checked row's own menu.
    ///
    /// A dead control is one the checked set gives nothing to do. Deleting asks first,
    /// once, for the whole set.
    pub(crate) fn bulk_item(
        &mut self,
        ui: &mut egui::Ui,
        action: Bulk,
        checked: &[Item],
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
        let wanted = bulk(action, checked);
        if !ui
            .add_enabled(!wanted.is_empty(), egui::Button::new(action.label()))
            .on_disabled_hover_text(action.nothing())
            .clicked()
        {
            return;
        }
        match action {
            Bulk::Delete => self.ask_discard(checked, wanted),
            _ => acts.extend(wanted),
        }
        ui.close();
    }

    /// Ask once before a whole checked set is deleted, naming what each half of it
    /// costs: a slot is emptied on the instrument, a local only leaves the list.
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

    /// Paint the tree headlessly. What this catches is a layout that panics or an id
    /// that collides, neither of which a unit test on the rules would see.
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
        // A folder with something in it, one with nothing, and a view of a slot: three
        // row shapes the list has no other way of reaching.
        let full = browser.folders.make();
        browser.folders.make();
        let filed = workspace.create(Fresh::Program, &mut log).unwrap();
        browser.folders.file(filed, Some(full));
        // A tag on something, and one on nothing: the two shapes the section holds.
        let sunday = browser.tags.make("Sunday");
        browser.tags.make("Loud");
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
                    browser.open.insert(Branch::Bank(class.to_raw(), bank));
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

    /// A store that answers for one key at a time, which is what the two things the
    /// browser keeps need it to be.
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

    /// A drag from a row inside the selection brings the rest of it, and one from a row
    /// outside brings only itself — the pressed row is what a drag is about.
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
        assert!(alone.rest.is_empty(), "a row nobody picked carries itself");
        assert_eq!(alone.name, "Squabble B", "and says only its own name");
    }

    /// ⚠️ The rest of the selection follows only where the drop is one act repeated.
    /// Filing three assets is three filings; sending three into one slot would write
    /// them over each other, so a single destination takes the pressed row alone.
    #[test]
    fn a_drop_of_many_repeats_only_where_one_destination_does_not() {
        let (mut browser, mut workspace, device, _tabs, _queue, mut log) = bench();
        let ids: Vec<u64> = (0..3)
            .map(|_| workspace.create(Fresh::Program, &mut log).unwrap())
            .collect();
        for id in &ids {
            browser.selection.toggle(Item::Local(*id));
        }
        let folder = browser.folders.make();
        let head = browser
            .held(Item::Local(ids[0]), &workspace, &device.state)
            .unwrap();
        let carried = Arc::new(browser.carrying(head, "Africa Split", &workspace, &device.state));

        let mut filed = Vec::new();
        browser.land(&carried, Onto::Group(folder), &mut filed);
        assert_eq!(filed.len(), 3, "every picked asset goes into the folder");
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

    /// ⚠️ F2 renames the row that is the only one picked. A rename typed while several
    /// are picked reads as a rename of all of them, and only one would take it.
    #[test]
    fn f2_renames_only_while_its_row_is_the_only_one_picked() {
        let (mut browser, _workspace, _device, _tabs, _queue, _log) = bench();
        let row = Item::Local(1);
        browser.selection.only(row);
        assert!(browser.sole_is(row));

        browser.selection.toggle(Item::Local(2));
        assert!(!browser.sole_is(row), "two rows picked");
        // And a plain click on one of the two lets go of it, leaving the other sole.
        browser.selection.plain(Item::Local(2));
        assert!(browser.sole_is(row));
    }

    /// Escape lets go of everything, whether or not the browser dock is open to show it.
    ///
    /// ⚠️ Not while a name is being typed: Escape is how a rename is taken back, and a
    /// half-typed name is not a reason to drop the selection with it.
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
            "the editor's Escape is not the selection's"
        );

        browser.rename = None;
        let _ = ctx.run(escape, |ctx| browser.let_go(ctx));
        assert_eq!(browser.picked().items().count(), 0);
    }

    /// The gesture end to end: open the editor, type, press Enter, and the new name comes
    /// back as an act.
    ///
    /// What this catches is a rename that has stopped committing at all — the unit tests
    /// on [`renamed`] cannot see the field it is fed from, and the field's own idea of
    /// "the operator pressed Enter" is the part that is easy to get wrong.
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
        // The editor opens on the first frame and takes the focus; the second types over
        // what it opened with, selected; the third commits.
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
        assert!(browser.rename.is_none(), "and the editor is done with");
    }

    /// Only Enter renames: an armed editor that commits on blur turns a stray keystroke
    /// into a rename nobody asked for.
    #[test]
    fn a_rename_needs_enter_and_a_real_change() {
        assert_eq!(renamed("Africa Split", "LA Grand"), Some("LA Grand".into()));
        // What blur hands back is nothing at all — see `rename_row`.
        assert_eq!(renamed("Africa Split", "Africa Split"), None);
    }

    /// Enter on an untouched field, or on an empty one, leaves the asset alone.
    #[test]
    fn a_rename_that_changes_nothing_is_not_a_rename() {
        assert_eq!(renamed("Africa Split", "Africa Split"), None);
        assert_eq!(renamed("Africa Split", "  Africa Split "), None);
        assert_eq!(renamed("Africa Split", "   "), None);
        assert_eq!(renamed("Africa Split", ""), None);
    }

    /// What is typed is what the asset is called, with the spaces around it dropped.
    #[test]
    fn a_rename_takes_the_typed_name_trimmed() {
        assert_eq!(
            renamed("Africa Split", "  LA Grand  "),
            Some("LA Grand".into())
        );
    }

    /// The two stores are read separately and only the asset one decides what survived —
    /// anything too big to keep, or dropped for want of room, would otherwise leave its
    /// membership behind to accumulate for as long as the app is installed.
    #[test]
    fn a_grouping_forgets_the_assets_the_list_came_back_without() {
        let (mut browser, mut workspace, _device, _tabs, _queue, mut log) = bench();
        let here = workspace.create(Fresh::Program, &mut log).unwrap();
        let folder = browser.folders.make();
        browser.folders.file(here, Some(folder));
        // As a store that could not keep everything reads back: a membership for an
        // asset the list does not hold.
        browser.folders.file(here + 99, Some(folder));

        browser.settle(&workspace);
        assert_eq!(browser.folders.holding(here), Some(folder));
        assert_eq!(browser.folders.holding(here + 99), None);
        assert_eq!(browser.folders.all().len(), 1, "the folder itself stays");
    }

    /// Two assets picked and saved as a gig wear that tag next session, and the third
    /// does not. An asset the list came back without leaves no membership behind.
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

    /// ⚠️ A view is the only copy of what it holds and the store skips it, so a tag on
    /// one would go with its tab. Tagging keeps it on this computer first, and the log
    /// says that is what happened.
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
        let tag = browser.tags.make("Sunday");
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
        assert!(!workspace.is_view(id), "it is on this computer now");
        assert!(browser.tags.worn(id).contains(&tag));
        let said = log.transcript();
        assert!(said.contains("kept on this computer first"), "{said}");
    }

    /// The warning reaches the modal a batch raises, once per format however many items
    /// carry it — and it goes above the list of destinations, which is what the eye
    /// slides past.
    ///
    /// The instrument names a model the acceptance table does not know, which is what
    /// leaves the comparison against the folder's own formats as the only thing to go
    /// on — a family it does know would refuse these outright.
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
