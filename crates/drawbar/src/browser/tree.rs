//! The browser's tree: the places a sound can be, the kinds of sound, and the tags on
//! this computer's list.
//!
//! Every row is drawn by [`super::row::row`]. The tree computes its own indents instead
//! of nesting `Ui`s, so a leaf can skip the triangle's box and line its glyph up under
//! the glyph of the branch beside it.

use eframe::egui;
use nord_format::accept::Family;
use nord_usb::{Location, ObjectClass};

use super::act::{will_write, Act, Bulk};
use super::drag::{kinds_present, qualifier, Item, Kind, Onto};
use super::row::{row, Cells, Drawn, STEP};
use super::{Ask, Browser, Click};
use crate::device::{occupancy, read_only, Connection, Device, DeviceState};
use crate::filter::{Filter, Narrow, Place, State};
use crate::icon::Glyph;
use crate::newproject::Making;
use crate::panel::panel_header;
use crate::queue::{Queue, Queued};
use crate::shell::marked;
use crate::strings::{place, shown};
use crate::tabs::Spot;
use crate::workspace::{Fresh, LocalEntity, Workspace};

/// The New menu. Above the separator are files an instrument holds: each family's
/// defaults, and the two instrument files built from audio. Below it are files only this
/// computer keeps: a note, a Sample Editor project, and a folder.
///
/// ⚠️ One menu, used everywhere. The tree's context menu, the File menu, the toolbar and
/// the tab strip all offer "New", and four different menus of one name would be four
/// things to learn. Connecting an instrument makes nothing on this computer, so it is on
/// the tree's instrument row instead.
pub fn new_menu(ui: &mut egui::Ui, acts: &mut Vec<Act>) {
    for family in &Fresh::FAMILIES {
        ui.menu_button(family.label, |ui| {
            for kind in family.kinds {
                entry(ui, *kind, acts);
            }
        });
    }
    for making in Making::FROM_WAVS.iter().filter(|it| it.instrument_file()) {
        from_wavs(ui, *making, acts);
    }
    ui.separator();
    for kind in Fresh::LOOSE {
        entry(ui, kind, acts);
    }
    for making in Making::FROM_WAVS.iter().filter(|it| !it.instrument_file()) {
        from_wavs(ui, *making, acts);
    }
    if ui
        .button("New folder")
        .on_hover_text("groups the list on this computer; the instrument never sees it")
        .clicked()
    {
        acts.push(Act::NewFolder);
        ui.close();
    }
}

/// One kind this app creates from a default.
fn entry(ui: &mut egui::Ui, kind: Fresh, acts: &mut Vec<Act>) {
    let mut button = ui.button(kind.label());
    if let Some(note) = kind.note() {
        button = button.on_hover_text(note);
    }
    if button.clicked() {
        acts.push(Act::New(kind));
        ui.close();
    }
}

/// One kind built from audio files, which asks for the files before it exists.
fn from_wavs(ui: &mut egui::Ui, making: Making, acts: &mut Vec<Act>) {
    let (item, hint) = making.item();
    if ui.button(item).on_hover_text(hint).clicked() {
        acts.push(Act::NewFromWavs(making));
        ui.close();
    }
}

/// A row of the tree with something under it.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub(super) enum Branch {
    Computer,
    Folder(u64),
    Instrument,
    Class(u32),
    /// ⚠️ The bank as the panel numbers it, which keys the scan cache. Never a
    /// [`Location`]'s zero-based bank.
    Bank(u32, u64),
}

/// Which of the three sections are showing.
#[derive(Clone, Copy)]
pub(super) struct Sections {
    places: bool,
    kinds: bool,
    tags: bool,
}

impl Default for Sections {
    fn default() -> Sections {
        Sections {
            places: true,
            kinds: true,
            tags: true,
        }
    }
}

/// Where a row's contents start.
///
/// A top-level branch starts 8 px in, a leaf beside it at 26, and a leaf one level down
/// at 40. A leaf skips the triangle's box, which lines its glyph up under the glyph of a
/// branch at the same depth.
fn indent(depth: usize, branch: bool) -> f32 {
    const FIRST: f32 = 8.0;
    const DOWN: f32 = 14.0;
    let past_the_triangle = match branch {
        true => 0.0,
        false => STEP,
    };
    FIRST + DOWN * depth as f32 + past_the_triangle
}

/// A section's header, and whether its body should be drawn.
fn section(ui: &mut egui::Ui, title: &str, open: &mut bool) -> bool {
    panel_header(ui, title, Some(open), None);
    *open
}

/// Whether a click landed on the triangle, which opens the branch instead of selecting
/// the row.
fn on_triangle(drawn: &Drawn) -> bool {
    let (Some(box_), Some(at)) = (drawn.chevron, drawn.response.interact_pointer_pos()) else {
        return false;
    };
    box_.expand(3.0).contains(at)
}

/// Turn one of the library's filters on or off, and bring the library forward to show the
/// result. A filter applied out of sight would surprise whoever finds it later.
fn narrow(acts: &mut Vec<Act>, narrow: Narrow) {
    acts.push(Act::ShowTab(Spot::Library));
    acts.push(Act::Narrow(narrow));
}

/// A line where a branch has nothing to show.
fn nothing(ui: &mut egui::Ui, depth: usize, said: &str) {
    row(
        ui,
        false,
        &Cells {
            indent: indent(depth, false),
            name: said,
            faint: true,
            child: true,
            ..Cells::default()
        },
    );
}

/// Whether the kinds section is worth showing. With one kind in both places there is
/// nothing to choose between, and the row would only narrow the library to what it
/// already shows.
fn worth_choosing(kinds: &[Kind]) -> bool {
    kinds.len() > 1
}

/// The open-set key for one bank's rows.
pub(super) fn bank_branch(class: ObjectClass, bank: u64) -> Branch {
    Branch::Bank(class.to_raw(), bank)
}

/// What a local row's kind word needs from outside the row: the families on this
/// computer's list, and the attached instrument's family. Read once a frame, because
/// every row asks the same question of the whole list.
struct Naming {
    kept: Vec<Family>,
    instrument: Option<Family>,
}

/// Where a duplicate of a slot lands: the first slot of its folder that a scan found
/// empty and nothing in the queue is waiting for.
///
/// ⚠️ The same exclusion that places a queued asset. Two writes to one address would
/// leave only one.
fn spare_slot(device: &DeviceState, class: ObjectClass, queue: &Queue) -> Option<Location> {
    device.first_free(class, &queue.waiting_in(class))
}

/// Where a drop onto a row of the local list lands: the folder the row is drawn under, or
/// the loose part of the list.
pub(super) fn onto_list(folder: Option<u64>) -> Onto {
    match folder {
        Some(id) => Onto::Group(id),
        None => Onto::Computer,
    }
}

/// Whether a bank's name says anything the number beside every row does not.
///
/// Program banks come back named "Bank 1", "Bank 2", which only repeats the location
/// column. Piano banks come back named "Grand" and "Upright", which is why a caption is
/// shown at all.
fn worth_captioning(bank: u32, name: &str) -> bool {
    let name = name.trim();
    !name.is_empty()
        && name != bank.to_string()
        && !name.eq_ignore_ascii_case(&format!("bank {bank}"))
}

impl Browser {
    pub(super) fn tree(
        &mut self,
        ui: &mut egui::Ui,
        workspace: &Workspace,
        device: &Device,
        queue: &Queue,
        filter: &Filter,
        acts: &mut Vec<Act>,
    ) {
        egui::ScrollArea::vertical()
            .id_salt("browser_tree")
            .auto_shrink([false; 2])
            .show(ui, |ui| {
                let mut sections = self.sections;
                if section(ui, "places", &mut sections.places) {
                    self.places(ui, workspace, device, queue, filter, acts);
                }
                let kinds = kinds_present(workspace, &device.state);
                if worth_choosing(&kinds) && section(ui, "kinds", &mut sections.kinds) {
                    self.kinds(ui, &kinds, workspace, device, filter, acts);
                }
                if section(ui, "tags", &mut sections.tags) {
                    self.tag_rows(ui, workspace, device, filter, acts);
                }
                self.sections = sections;
                self.empty_below(ui);
            });
    }

    /// The space below the last row. A click there clears the selection.
    fn empty_below(&mut self, ui: &mut egui::Ui) {
        let rest = ui.available_rect_before_wrap();
        if rest.height() <= 0.0 {
            return;
        }
        if ui
            .interact(rest, ui.id().with("tree_empty"), egui::Sense::click())
            .clicked()
        {
            self.selection.clear();
        }
    }

    fn twist(&mut self, branch: Branch) {
        if !self.open.remove(&branch) {
            self.open.insert(branch);
        }
    }

    /// Right-clicking an unselected row selects only it; right-clicking a selected row
    /// keeps the selection, so the menu acts on all of it.
    fn aim(&mut self, item: Item) {
        if !self.selection.holds(item) {
            self.select(item);
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn places(
        &mut self,
        ui: &mut egui::Ui,
        workspace: &Workspace,
        device: &Device,
        queue: &Queue,
        filter: &Filter,
        acts: &mut Vec<Act>,
    ) {
        self.computer_row(ui, workspace, device, filter, acts);
        if self.open.contains(&Branch::Computer) {
            let naming = Naming {
                kept: super::families_present(workspace),
                instrument: device.state.product().and_then(Family::from_product),
            };
            for id in self.folder_ids() {
                self.folder_row(ui, id, workspace, device, queue, &naming, acts);
            }
            let loose: Vec<Item> = workspace
                .listed()
                .filter(|entity| self.folders.holding(entity.id).is_none())
                .map(|entity| Item::Local(entity.id))
                .collect();
            for entity in workspace.listed() {
                if self.folders.holding(entity.id).is_none() {
                    self.local_row(
                        ui, entity, None, &loose, workspace, device, queue, &naming, acts,
                    );
                }
            }
            if loose.is_empty() && self.folders.all().is_empty() {
                nothing(ui, 1, "Drop Nord files here, or use Open…");
            }
        }

        match device.state.connected() {
            true => self.instrument_rows(ui, workspace, device, queue, filter, acts),
            false => self.connect_row(ui, device, acts),
        }

        let counts = [
            queue.len(),
            crate::library::differing(workspace, &device.state, queue),
        ];
        let states = [
            (
                State::Waiting,
                Glyph::Upload,
                crate::app::warn(ui.visuals()),
            ),
            (
                State::Differs,
                Glyph::CircleAlert,
                crate::app::bad(ui.visuals()),
            ),
        ];
        for ((state, glyph, dot), count) in states.into_iter().zip(counts) {
            // A row whose count drops to zero stays while its filter is on, so there is
            // always a row to click to turn the filter off.
            let asked = Narrow::State(state);
            if count == 0 && !filter.on(asked) {
                continue;
            }
            let drawn = row(
                ui,
                filter.on(asked),
                &Cells {
                    indent: indent(0, false),
                    glyph: Some(glyph),
                    name: state.title(),
                    dot: Some((dot, state.sentence())),
                    count: Some(count.to_string()),
                    ..Cells::default()
                },
            );
            if drawn.response.on_hover_text(state.sentence()).clicked() {
                narrow(acts, asked);
            }
        }
    }

    fn computer_row(
        &mut self,
        ui: &mut egui::Ui,
        workspace: &Workspace,
        device: &Device,
        filter: &Filter,
        acts: &mut Vec<Act>,
    ) {
        let here = Narrow::Place(Place::Computer);
        let drawn = row(
            ui,
            filter.on(here),
            &Cells {
                indent: indent(0, true),
                open: Some(self.open.contains(&Branch::Computer)),
                glyph: Some(Glyph::LibraryBig),
                name: "This computer",
                count: Some(workspace.listed().count().to_string()),
                ..Cells::default()
            },
        );
        // The branch's head row takes a drop, so there is always a target that no drag
        // can have started from.
        self.drop_zone(ui, &drawn.response, Onto::Computer, acts);
        if drawn.response.clicked() {
            match on_triangle(&drawn) {
                true => self.twist(Branch::Computer),
                false => narrow(acts, here),
            }
        }
        let listed = Browser::standing_for(workspace, |_| true);
        drawn.response.context_menu(|ui| {
            self.set_menu(ui, &listed, workspace, device, acts, |_, ui, acts| {
                if ui.button("Open…").clicked() {
                    acts.push(Act::OpenFiles);
                    ui.close();
                }
                ui.menu_button("New", |ui| new_menu(ui, acts));
            });
        });
    }

    /// The row shown in place of an instrument until one is connected.
    ///
    /// ⚠️ The click reaches `requestDevice()` in the frame it landed in, which keeps the
    /// browser's transient user activation alive.
    fn connect_row(&mut self, ui: &mut egui::Ui, device: &Device, acts: &mut Vec<Act>) {
        if matches!(device.state.connection, Connection::Connecting) {
            nothing(ui, 0, "Looking for an instrument…");
            return;
        }
        let drawn = row(
            ui,
            false,
            &Cells {
                indent: indent(0, false),
                glyph: Some(Glyph::Keyboard),
                name: "Connect an instrument…",
                faint: true,
                ..Cells::default()
            },
        );
        if drawn
            .response
            .on_hover_text(
                "Close Nord Sound Manager first. It keeps the USB connection to itself while \
                 it is open.\n\nIn a browser: Chrome or Edge only.",
            )
            .clicked()
        {
            acts.push(Act::Connect);
        }
    }

    /// The folder ids in creation order, copied out so a row can change the list it is
    /// drawn from.
    fn folder_ids(&self) -> Vec<u64> {
        self.folders.all().iter().map(|folder| folder.id).collect()
    }

    #[allow(clippy::too_many_arguments)]
    fn folder_row(
        &mut self,
        ui: &mut egui::Ui,
        id: u64,
        workspace: &Workspace,
        device: &Device,
        queue: &Queue,
        naming: &Naming,
        acts: &mut Vec<Act>,
    ) {
        let item = Item::Folder(id);
        let Some(name) = self.folders.name_of(id).map(str::to_string) else {
            return;
        };
        let members: Vec<u64> = self
            .folders
            .members(id, workspace)
            .iter()
            .map(|entity| entity.id)
            .collect();
        let inside: Vec<Item> = members.iter().copied().map(Item::Local).collect();
        let mut open = self.open.contains(&Branch::Folder(id));

        if self.rename.as_ref().is_some_and(|r| r.what == item) {
            if let Some(name) = self.rename_row(ui, indent(1, true), &name) {
                acts.push(Act::RenameFolder { id, name });
            }
            // A folder being renamed shows its contents, since they are what the name
            // describes.
            open = true;
        } else {
            let drawn = row(
                ui,
                self.selection.holds(item),
                &Cells {
                    indent: indent(1, true),
                    open: Some(open),
                    glyph: Some(Glyph::Folder),
                    name: &name,
                    count: Some(members.len().to_string()),
                    child: true,
                    ..Cells::default()
                },
            );
            self.drop_zone(ui, &drawn.response, Onto::Group(id), acts);
            if drawn.response.clicked() {
                match on_triangle(&drawn) {
                    true => self.twist(Branch::Folder(id)),
                    false => {
                        let list: Vec<Item> =
                            self.folder_ids().into_iter().map(Item::Folder).collect();
                        self.clicked(ui, Click { item, list: &list });
                    }
                }
            }
            drawn.response.context_menu(|ui| {
                self.aim(item);
                self.set_menu(ui, &inside, workspace, device, acts, |browser, ui, acts| {
                    if ui.button("Rename").clicked() {
                        browser.start_rename(item, &name);
                        ui.close();
                    }
                    if ui
                        .button("Remove folder")
                        .on_hover_text("its contents go back to the list; nothing is deleted")
                        .clicked()
                    {
                        acts.push(Act::RemoveFolder(id));
                        ui.close();
                    }
                });
            });
        }

        if !open {
            return;
        }
        if members.is_empty() {
            nothing(ui, 2, "empty; drag sounds here");
        }
        for entity in members.iter().filter_map(|id| workspace.get(*id)) {
            self.local_row(
                ui,
                entity,
                Some(id),
                &inside,
                workspace,
                device,
                queue,
                naming,
                acts,
            );
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn local_row(
        &mut self,
        ui: &mut egui::Ui,
        entity: &LocalEntity,
        folder: Option<u64>,
        list: &[Item],
        workspace: &Workspace,
        device: &Device,
        queue: &Queue,
        naming: &Naming,
        acts: &mut Vec<Act>,
    ) {
        let item = Item::Local(entity.id);
        let kind = Kind::of(entity);
        let selected = self.selection.holds(item);
        let depth = match folder {
            Some(_) => 2,
            None => 1,
        };

        // While a name is being typed, the row senses nothing: a drag sense over the
        // field would take the clicks that place the cursor.
        if self.rename.as_ref().is_some_and(|r| r.what == item) {
            if let Some(name) = self.rename_row(ui, indent(depth, false), &entity.name) {
                acts.push(Act::RenameLocal {
                    id: entity.id,
                    name,
                });
            }
            return;
        }

        let owed = queue.entry(entity.id).map(destination);
        let wears = self.tags.worn(entity.id).len();
        let word =
            crate::strings::kind_word(kind, qualifier(entity, &naming.kept, naming.instrument));
        let drawn = row(
            ui,
            selected,
            &Cells {
                indent: indent(depth, false),
                glyph: Some(kind.glyph()),
                name: &entity.name,
                note: owed.as_deref().or(Some(word.as_str())),
                unsaved: entity.is_unsaved(),
                dot: mark(entity, &device.state, queue, ui.visuals()),
                tags: wears,
                child: true,
                ..Cells::default()
            },
        );
        // ⚠️ The row already shows the full name on hover when it truncates it. A hover
        // here would show it twice.
        let response = drawn.response;

        if response.dragged() {
            if let Some(head) = self.held(item, workspace, &device.state) {
                let carried = self.carrying(head, &entity.name, workspace, &device.state);
                egui::DragAndDrop::set_payload(ui.ctx(), carried);
            }
        }
        // A drop onto a row lands where the row is drawn. It is taken here so the
        // branch's zone does not act on it again.
        self.drop_zone(ui, &response, onto_list(folder), acts);

        if response.double_clicked() {
            acts.push(Act::Open(item));
        } else if response.clicked() {
            self.clicked(ui, Click { item, list });
        }
        if self.sole_is(item) && ui.input(|i| i.key_pressed(egui::Key::F2)) {
            self.start_rename(item, &entity.name);
        }

        response.context_menu(|ui| self.menu(ui, item, workspace, device, queue, acts));
    }

    /// The menu of a row that stands for a set of assets: the bulk actions on the set,
    /// then the row's own items.
    ///
    /// One builder for the folder, tag, kind, place and class rows. They differ in which
    /// assets they stand for and in their own items. Otherwise they offer the same
    /// actions, disabled for the same reasons and labeled the same way.
    fn set_menu(
        &mut self,
        ui: &mut egui::Ui,
        members: &[Item],
        workspace: &Workspace,
        device: &Device,
        acts: &mut Vec<Act>,
        own: impl FnOnce(&mut Browser, &mut egui::Ui, &mut Vec<Act>),
    ) {
        for action in [Bulk::Queue, Bulk::Export] {
            self.bulk_item(ui, action, members, workspace, &device.state, acts);
        }
        ui.separator();
        own(self, ui, acts);
    }

    /// The assets on this computer a row stands for, in the order the list holds them.
    fn standing_for(workspace: &Workspace, keep: impl Fn(&LocalEntity) -> bool) -> Vec<Item> {
        workspace
            .listed()
            .filter(|entity| keep(entity))
            .map(|entity| Item::Local(entity.id))
            .collect()
    }

    /// The menu a row offers, in the tree or in the library table.
    ///
    /// A row inside a checked set of several offers what the library's footer offers,
    /// because the menu acts on the whole set.
    ///
    /// Folders and tags appear only in the tree, so the rows that draw them build their
    /// menus.
    pub fn menu(
        &mut self,
        ui: &mut egui::Ui,
        item: Item,
        workspace: &Workspace,
        device: &Device,
        queue: &Queue,
        acts: &mut Vec<Act>,
    ) {
        self.aim(item);
        let checked: Vec<Item> = self.selection.items().collect();
        if checked.len() > 1 && self.selection.holds(item) {
            for action in Bulk::ALL {
                self.bulk_item(ui, action, &checked, workspace, &device.state, acts);
            }
            return;
        }
        match item {
            Item::Local(id) => self.local_menu(ui, id, workspace, device, acts),
            Item::Slot { class, at } => self.slot_menu(ui, class, at, device, queue, acts),
            Item::Folder(_) | Item::Tag(_) => {}
        }
    }

    fn local_menu(
        &mut self,
        ui: &mut egui::Ui,
        id: u64,
        workspace: &Workspace,
        device: &Device,
        acts: &mut Vec<Act>,
    ) {
        let Some(entity) = workspace.get(id) else {
            return;
        };
        let item = Item::Local(id);
        let picked = self.selection.locals();
        if ui.button("Open").clicked() {
            acts.push(Act::Open(item));
            ui.close();
        }
        self.bulk_item(ui, Bulk::Queue, &[item], workspace, &device.state, acts);
        if ui.button("Export…").clicked() {
            acts.push(Act::Export(id));
            ui.close();
        }
        if ui.button("Rename").clicked() {
            self.start_rename(item, &entity.name);
            ui.close();
        }
        if ui.button("Duplicate").clicked() {
            acts.push(Act::DuplicateLocal(id));
            ui.close();
        }
        self.filing_menu(ui, id, self.folders.holding(id), acts);
        ui.menu_button("Tag", |ui| self.tag_items(ui, &picked, acts));
        if ui
            .button("Save as gig…")
            .on_hover_text("puts the selection under a new tag")
            .clicked()
        {
            acts.push(Act::SaveAsGig);
            ui.close();
        }
        ui.separator();
        if ui.button("Remove from list").clicked() {
            acts.push(Act::Remove(id));
            ui.close();
        }
    }

    /// The folders an asset can be moved into, for those who prefer a menu to dragging.
    fn filing_menu(&self, ui: &mut egui::Ui, id: u64, filed: Option<u64>, acts: &mut Vec<Act>) {
        if self.folders.all().is_empty() {
            return;
        }
        ui.menu_button("Move to folder", |ui| {
            for folder in self.folders.all() {
                if marked(ui, &folder.name, filed == Some(folder.id), None) {
                    acts.push(Act::File {
                        id,
                        folder: Some(folder.id),
                    });
                }
            }
            ui.separator();
            if ui
                .add_enabled(filed.is_some(), egui::Button::new("Out of any folder"))
                .clicked()
            {
                acts.push(Act::File { id, folder: None });
                ui.close();
            }
        });
    }

    /// Every tag, checked where it is on everything selected, then New tag.
    ///
    /// Only the items, so the row's menu and the library's footer can each give them
    /// their own label.
    pub fn tag_items(&self, ui: &mut egui::Ui, picked: &[u64], acts: &mut Vec<Act>) {
        for tag in self.tags.all() {
            let on = self.tags.on_all(picked, tag.id);
            if marked(ui, &tag.name, on, None) {
                let ids = picked.to_vec();
                acts.push(match on {
                    true => Act::Untag { ids, tag: tag.id },
                    false => Act::Tag { ids, tag: tag.id },
                });
            }
        }
        if !self.tags.all().is_empty() {
            ui.separator();
        }
        if ui.button("New tag…").clicked() {
            acts.push(Act::SaveAsGig);
            ui.close();
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn instrument_rows(
        &mut self,
        ui: &mut egui::Ui,
        workspace: &Workspace,
        device: &Device,
        queue: &Queue,
        filter: &Filter,
        acts: &mut Vec<Act>,
    ) {
        let Some(product) = device.state.product().map(str::to_string) else {
            return;
        };
        let held = occupancy(
            ObjectClass::Program,
            &device.state.inventory,
            device.state.allocation_unit(ObjectClass::Program),
        );
        let drawn = row(
            ui,
            filter.on(Narrow::Place(Place::Keyboard)),
            &Cells {
                indent: indent(0, true),
                open: Some(self.open.contains(&Branch::Instrument)),
                glyph: Some(Glyph::Keyboard),
                name: &product,
                dot: Some((crate::app::good(ui.visuals()), "attached")),
                count: held,
                ..Cells::default()
            },
        );
        let response = drawn
            .response
            .clone()
            .on_hover_text("attached; the count is its programs");
        if response.clicked() {
            match on_triangle(&drawn) {
                true => self.twist(Branch::Instrument),
                // ⚠️ Not [`narrow`]: the instrument's own tab is what this row opens, so
                // the narrowing it asks for is already in front.
                false => {
                    acts.push(Act::ShowTab(Spot::Keyboard));
                    acts.push(Act::Narrow(Narrow::Place(Place::Keyboard)));
                }
            }
        }
        let reading = device
            .state
            .classes()
            .into_iter()
            .filter_map(|class| device.state.scan.progress(class))
            .any(|progress| progress.running);
        // The label counts what the batch would write, which leaves out entries this
        // instrument has refused.
        let waiting = will_write(queue).count();
        // What this row stands for on this computer: every asset that stands for a slot.
        let off_it = Browser::standing_for(workspace, |entity| entity.spot().is_some());
        drawn.response.context_menu(|ui| {
            self.set_menu(ui, &off_it, workspace, device, acts, |_, ui, acts| {
                if ui
                    .add_enabled(!reading, egui::Button::new("Read everything again"))
                    .on_disabled_hover_text("already reading")
                    .clicked()
                {
                    acts.push(Act::Resync);
                    ui.close();
                }
                if waiting > 0 && ui.button(format!("Send all ({waiting})")).clicked() {
                    acts.push(Act::AskSendAll);
                    ui.close();
                }
                ui.separator();
                if ui.button("Disconnect").clicked() {
                    acts.push(Act::Disconnect);
                    ui.close();
                }
            });
        });

        if !self.open.contains(&Branch::Instrument) {
            return;
        }
        // The slots open as views, which the list on this computer does not show.
        let viewed: Vec<(ObjectClass, Location)> = workspace
            .entities()
            .iter()
            .filter(|entity| !entity.kept)
            .filter_map(|entity| entity.origin.slot())
            .collect();
        for class in device.state.classes() {
            self.class_row(ui, device, class, &viewed, workspace, queue, acts);
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn class_row(
        &mut self,
        ui: &mut egui::Ui,
        device: &Device,
        class: ObjectClass,
        viewed: &[(ObjectClass, Location)],
        workspace: &Workspace,
        queue: &Queue,
        acts: &mut Vec<Act>,
    ) {
        // A jump opens the branches it needs, since its purpose is to reach a slot inside
        // a closed one.
        if let Some((held, at)) = self.jump.filter(|(held, _)| *held == class) {
            self.open.insert(Branch::Class(class.to_raw()));
            self.open.insert(bank_branch(held, at.user_bank()));
        }
        let open = self.open.contains(&Branch::Class(class.to_raw()));
        let progress = device.state.scan.progress(class);
        let count = match progress {
            Some(p) if p.running => Some(match p.total {
                Some(total) => format!("{} of {total}", p.done + 1),
                None => "reading…".to_string(),
            }),
            _ => occupancy(
                class,
                &device.state.inventory,
                device.state.allocation_unit(class),
            ),
        };
        let drawn = row(
            ui,
            false,
            &Cells {
                indent: indent(1, true),
                open: Some(open),
                glyph: Some(Kind::from_class(class).glyph()),
                name: device.state.folder_name(class),
                note: read_only(class).then_some("read only"),
                count,
                child: true,
                ..Cells::default()
            },
        );
        if drawn.response.clicked() {
            match on_triangle(&drawn) {
                true => self.twist(Branch::Class(class.to_raw())),
                false => acts.push(Act::ShowClass(class)),
            }
        }
        let focus = device.state.focused(class);
        let off_it = Browser::standing_for(workspace, |entity| {
            entity.spot().is_some_and(|(held, _)| held == class)
        });
        drawn.response.context_menu(|ui| {
            self.set_menu(ui, &off_it, workspace, device, acts, |browser, ui, acts| {
                if ui
                    .button("Read this folder again")
                    .on_hover_text(
                        "Read everything reads the whole instrument; this reads one folder",
                    )
                    .clicked()
                {
                    acts.push(Act::ReadAgain(class));
                    ui.close();
                }
                if let Some(at) = focus {
                    if ui
                        .button("Go to loaded")
                        .on_hover_text(format!("the panel is on {}", shown(at)))
                        .clicked()
                    {
                        browser.jump = Some((class, at));
                        ui.close();
                    }
                }
            });
        });

        // ⚠️ A jump to a slot no scan has reached would hold the branch open as long as
        // the instrument stays attached, because no row is drawn to clear it.
        if self
            .jump
            .is_some_and(|(held, at)| held == class && device.state.slot(class, at).is_none())
        {
            self.jump = None;
        }
        if !open {
            return;
        }
        let banks = device.state.banks_of(class);
        if banks.is_empty() {
            nothing(ui, 2, "nothing read yet");
        }
        // The live buffer and the settings have a single bank, drawn without a bank row.
        let cut = banks.len() > 1;
        for bank in banks {
            self.bank_rows(ui, device, class, bank, cut, viewed, workspace, queue, acts);
        }
    }

    /// One bank, as a branch over its slots.
    ///
    /// ⚠️ A bank is a container only in the browser. The instrument has no folders inside
    /// a class, only a bank and slot number per location, but four hundred rows in one
    /// run cannot be navigated, so the list is split by bank. A bank the device named
    /// shows its name; for pianos, those names are the panel's categories.
    #[allow(clippy::too_many_arguments)]
    fn bank_rows(
        &mut self,
        ui: &mut egui::Ui,
        device: &Device,
        class: ObjectClass,
        bank: u32,
        cut: bool,
        viewed: &[(ObjectClass, Location)],
        workspace: &Workspace,
        queue: &Queue,
        acts: &mut Vec<Act>,
    ) {
        let Some(slots) = device.state.bank(class, bank) else {
            return;
        };
        let count = slots.len();
        let held = slots.iter().filter(|slot| slot.is_some()).count();
        let list: Vec<Item> = device
            .state
            .slots_of(class, bank)
            .map(|(at, _)| Item::Slot { class, at })
            .collect();

        let depth = match cut {
            true => 3,
            false => 2,
        };
        if cut {
            let name = device
                .state
                .bank_name(class, bank)
                .filter(|name| worth_captioning(bank, name))
                .map(|name| format!("{bank} · {name}"))
                .unwrap_or_else(|| format!("Bank {bank}"));
            let drawn = row(
                ui,
                false,
                &Cells {
                    indent: indent(2, true),
                    open: Some(self.open.contains(&bank_branch(class, u64::from(bank)))),
                    glyph: Some(Glyph::Folder),
                    name: &name,
                    count: Some(format!("{held}/{count}")),
                    child: true,
                    ..Cells::default()
                },
            );
            if drawn.response.clicked() {
                self.twist(bank_branch(class, u64::from(bank)));
            }
            if !self.open.contains(&bank_branch(class, u64::from(bank))) {
                return;
            }
        }
        for item in &list {
            let Item::Slot { at, .. } = item else {
                continue;
            };
            self.slot_row(
                ui, device, class, *at, depth, &list, viewed, workspace, queue, acts,
            );
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn slot_row(
        &mut self,
        ui: &mut egui::Ui,
        device: &Device,
        class: ObjectClass,
        at: Location,
        depth: usize,
        list: &[Item],
        viewed: &[(ObjectClass, Location)],
        workspace: &Workspace,
        queue: &Queue,
        acts: &mut Vec<Act>,
    ) {
        let held = device
            .state
            .slot(class, at)
            .flatten()
            .map(|info| info.name.trim().to_string());
        let item = Item::Slot { class, at };
        let selected = self.selection.holds(item);

        // While a name is being typed, the row senses nothing: a drag sense over the
        // field would take the clicks that place the cursor.
        if self.rename.as_ref().is_some_and(|r| r.what == item) {
            let was = held.clone().unwrap_or_default();
            if let Some(name) = self.rename_row(ui, indent(depth, false), &was) {
                acts.push(Act::RenameSlot { class, at, name });
            }
            return;
        }

        let loaded = device.state.focused(class) == Some(at);
        // A slot open as a view says so here, because the tab strip cannot: a tab shows
        // the document's name, which for a view is the slot's name.
        let viewing = viewed.contains(&(class, at));
        let drawn = row(
            ui,
            selected,
            &Cells {
                indent: indent(depth, false),
                glyph: Some(Kind::from_class(class).glyph()),
                at: Some(shown(at)),
                name: held.as_deref().unwrap_or("empty"),
                note: viewing.then_some("open"),
                faint: held.is_none(),
                loaded,
                child: true,
                ..Cells::default()
            },
        );
        let mut response = drawn.response;
        if loaded {
            response = response.on_hover_text("loaded on the instrument's panel");
        }
        if viewing {
            response = response
                .on_hover_text("open in a tab as a view of this slot; it is not on this computer");
        }
        // A jump can scroll only after the branches holding this row have opened.
        if self.jump == Some((class, at)) {
            self.jump = None;
            self.selection.only(item);
            response.scroll_to_me(Some(egui::Align::Center));
        }

        // ⚠️ A partition this app cannot name is only listed.
        let fetchable = !read_only(class);

        if let Some(name) = &held {
            if response.dragged() {
                if let Some(head) = self.held(item, workspace, &device.state) {
                    let carried = self.carrying(head, name, workspace, &device.state);
                    egui::DragAndDrop::set_payload(ui.ctx(), carried);
                }
            }
        }
        self.drop_zone(ui, &response, Onto::Slot { class, at }, acts);

        if response.double_clicked() {
            if held.is_some() && fetchable {
                acts.push(Act::Open(item));
            }
        } else if response.clicked() {
            self.clicked(ui, Click { item, list });
        }
        if let Some(name) = &held {
            if fetchable && self.sole_is(item) && ui.input(|i| i.key_pressed(egui::Key::F2)) {
                self.start_rename(item, name);
            }
        }

        if held.is_none() {
            return;
        }
        response.context_menu(|ui| self.menu(ui, item, workspace, device, queue, acts));
    }

    /// The menu of a slot. An empty slot has no menu.
    fn slot_menu(
        &mut self,
        ui: &mut egui::Ui,
        class: ObjectClass,
        at: Location,
        device: &Device,
        queue: &Queue,
        acts: &mut Vec<Act>,
    ) {
        let Some(name) = device
            .state
            .slot(class, at)
            .flatten()
            .map(|info| info.name.trim().to_string())
        else {
            return;
        };
        // ⚠️ A partition this app cannot name is only listed.
        if read_only(class) {
            ui.label(egui::RichText::new("drawbar does not know what this folder holds.").weak());
            return;
        }
        let item = Item::Slot { class, at };
        let free = spare_slot(&device.state, class, queue);
        if ui
            .button("Open")
            .on_hover_text("a view of this slot, without adding it to the list on this computer")
            .clicked()
        {
            acts.push(Act::Open(item));
            ui.close();
        }
        if ui.button("Copy to this computer").clicked() {
            acts.push(Act::Copy { class, at });
            ui.close();
        }
        if ui.button("Load on instrument").clicked() {
            acts.push(Act::LoadOnInstrument { class, at });
            ui.close();
        }
        ui.separator();
        if ui.button("Rename").clicked() {
            self.start_rename(item, &name);
            ui.close();
        }
        if ui
            .add_enabled(free.is_some(), egui::Button::new("Duplicate"))
            .on_disabled_hover_text("every slot read so far is taken or already queued")
            .clicked()
        {
            if let Some(to) = free {
                acts.push(Act::DuplicateSlot {
                    class,
                    from: at,
                    to,
                });
            }
            ui.close();
        }
        ui.separator();
        if ui.button("Delete…").clicked() {
            self.ask = Some(Ask {
                title: format!("Delete “{name}” from {}?", place(class, at)),
                note: Some("It is removed from the instrument. There is no undo.".into()),
                verb: "Delete",
                acts: vec![Act::DeleteSlot { class, at }],
            });
            ui.close();
        }
    }

    fn kinds(
        &mut self,
        ui: &mut egui::Ui,
        kinds: &[Kind],
        workspace: &Workspace,
        device: &Device,
        filter: &Filter,
        acts: &mut Vec<Act>,
    ) {
        for kind in kinds.iter().copied() {
            let asked = Narrow::Kind(kind);
            let drawn = row(
                ui,
                filter.on(asked),
                &Cells {
                    indent: indent(0, false),
                    glyph: Some(kind.glyph()),
                    name: kind.plural(),
                    ..Cells::default()
                },
            );
            if drawn.response.clicked() {
                narrow(acts, asked);
            }
            let of_it = Browser::standing_for(workspace, |entity| Kind::of(entity) == kind);
            drawn.response.context_menu(|ui| {
                self.set_menu(ui, &of_it, workspace, device, acts, |_, _, _| {});
            });
        }
    }

    fn tag_rows(
        &mut self,
        ui: &mut egui::Ui,
        workspace: &Workspace,
        device: &Device,
        filter: &Filter,
        acts: &mut Vec<Act>,
    ) {
        for id in self.tag_ids() {
            let item = Item::Tag(id);
            let Some(name) = self.tags.name_of(id).map(str::to_string) else {
                continue;
            };
            if self.rename.as_ref().is_some_and(|r| r.what == item) {
                if let Some(name) = self.rename_row(ui, indent(0, false), &name) {
                    acts.push(Act::RenameTag { id, name });
                }
                continue;
            }
            let asked = Narrow::Tag(id);
            let drawn = row(
                ui,
                filter.on(asked),
                &Cells {
                    indent: indent(0, false),
                    glyph: Some(Glyph::Tag),
                    name: &name,
                    count: Some(self.tags.count(id).to_string()),
                    ..Cells::default()
                },
            );
            if drawn.response.clicked() {
                narrow(acts, asked);
            }
            let wearing =
                Browser::standing_for(workspace, |entity| self.tags.worn(entity.id).contains(&id));
            drawn.response.context_menu(|ui| {
                self.aim(item);
                self.set_menu(
                    ui,
                    &wearing,
                    workspace,
                    device,
                    acts,
                    |browser, ui, acts| {
                        if ui.button("Rename").clicked() {
                            browser.start_rename(item, &name);
                            ui.close();
                        }
                        if ui
                            .button("Remove tag")
                            .on_hover_text("removes the tag from everything; nothing is deleted")
                            .clicked()
                        {
                            acts.push(Act::RemoveTag(id));
                            ui.close();
                        }
                    },
                );
            });
        }
        let drawn = row(
            ui,
            false,
            &Cells {
                indent: indent(0, false),
                glyph: Some(Glyph::Plus),
                name: "new tag",
                faint: true,
                ..Cells::default()
            },
        );
        if drawn.response.clicked() {
            acts.push(Act::NewTag("New tag".into()));
        }
    }

    fn tag_ids(&self) -> Vec<u64> {
        self.tags.all().iter().map(|tag| tag.id).collect()
    }
}

/// The status dot on a local row, with its color and hover text.
fn mark(
    entity: &LocalEntity,
    device: &DeviceState,
    queue: &Queue,
    visuals: &egui::Visuals,
) -> Option<(egui::Color32, &'static str)> {
    let mark = crate::library::keyboard_mark(entity, device, queue)?;
    Some((
        crate::library::mark_ink(mark, visuals),
        crate::library::mark_words(mark),
    ))
}

/// Where a queued asset is going, for the note that says so.
fn destination(held: &Queued) -> String {
    format!("→ {}", place(held.class, held.at))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::browser::act::apply;
    use crate::browser::bench::{bench, context, words};
    use crate::shell::Shell;

    /// ⚠️ Everything built from audio is on the New menu. One pick of WAVs can make any
    /// of them, and a menu offering only some would hide what the dialog does.
    #[test]
    fn the_new_menu_offers_everything_a_pick_of_wavs_makes() {
        let ctx = context();
        let output = ctx.run(egui::RawInput::default(), |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| new_menu(ui, &mut Vec::new()));
        });
        let said = words(&output);
        for making in Making::FROM_WAVS {
            let item = making.item().0;
            assert!(said.iter().any(|word| word == item), "{item} is missing");
        }
        assert!(said.iter().any(|word| word == "New folder"));
    }

    /// ⚠️ The New menu's separator splits files an instrument holds from files only this
    /// computer keeps. A kind on the wrong side would misstate where the new file can go.
    #[test]
    fn the_new_menu_parts_instrument_files_from_the_rest() {
        let ctx = context();
        let output = ctx.run(egui::RawInput::default(), |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| new_menu(ui, &mut Vec::new()));
        });
        let said = words(&output);
        let at = |word: &str| {
            said.iter()
                .position(|held| held == word)
                .unwrap_or_else(|| panic!("{word} is missing: {said:?}"))
        };
        let rule = Fresh::FAMILIES
            .iter()
            .map(|family| at(family.label))
            .chain(
                Making::FROM_WAVS
                    .iter()
                    .filter(|making| making.instrument_file())
                    .map(|making| at(making.item().0)),
            )
            .max()
            .expect("the instrument files are above it");
        let below: Vec<&str> = Fresh::LOOSE
            .iter()
            .map(|kind| kind.label())
            .chain(
                Making::FROM_WAVS
                    .iter()
                    .filter(|making| !making.instrument_file())
                    .map(|making| making.item().0),
            )
            .chain(["New folder"])
            .collect();
        for item in below {
            assert!(at(item) > rule, "{item} belongs below the rule: {said:?}");
        }
    }

    /// ⚠️ A row shows the sound's name without its format tag. The name the workspace
    /// holds keeps the tag; only the drawn text drops it.
    #[test]
    fn a_row_paints_its_name_without_the_format_tag() {
        let (mut browser, mut workspace, device, _tabs, queue, mut log) = bench();
        let id = workspace.create(Fresh::Program, &mut log).unwrap();
        workspace.rename(id, "Africa Split.ne5p".into());

        let ctx = workspace.ctx().clone();
        let output = ctx.run(egui::RawInput::default(), |ctx| {
            egui::SidePanel::left("browser")
                .exact_width(crate::shell::BROWSER)
                .show(ctx, |ui| {
                    browser.ui(ui, &workspace, &device, &queue, &Filter::default());
                });
        });

        let said = words(&output);
        assert!(said.iter().any(|word| word == "Africa Split"), "{said:?}");
        assert!(!said.iter().any(|word| word.contains(".ne5p")), "{said:?}");
        assert_eq!(workspace.get(id).unwrap().name, "Africa Split.ne5p");
    }

    /// ⚠️ A filter applied while a document is in front would narrow a table nobody is
    /// looking at, so every tree row that narrows brings the library forward.
    #[test]
    fn narrowing_the_library_brings_it_forward() {
        for asked in [
            Narrow::Kind(Kind::Program),
            Narrow::Tag(1),
            Narrow::Place(Place::Computer),
            Narrow::State(State::Waiting),
            Narrow::State(State::Differs),
        ] {
            let (mut browser, mut workspace, mut device, mut tabs, mut queue, mut log) = bench();
            let mut shell = Shell::default();
            let id = workspace.create(Fresh::Program, &mut log).unwrap();
            tabs.open(id);

            let mut acts = Vec::new();
            narrow(&mut acts, asked);
            apply(
                &mut browser,
                &mut shell,
                acts,
                &mut workspace,
                &mut device,
                &mut tabs,
                &mut queue,
                &mut log,
            );
            assert!(shell.filter.on(asked), "{asked:?}");
            assert_eq!(tabs.showing(), Some(Spot::Library), "{asked:?}");
        }
    }

    /// ⚠️ A place row narrows the library like a kind or tag row, so it is highlighted
    /// like one. Otherwise nothing in the window shows where the filter came from.
    #[test]
    fn the_place_row_reads_as_on_while_the_library_is_over_that_place() {
        fn selections(shape: &egui::Shape, want: egui::Color32) -> usize {
            match shape {
                egui::Shape::Rect(drawn) => usize::from(drawn.fill == want),
                egui::Shape::Vec(shapes) => {
                    shapes.iter().map(|shape| selections(shape, want)).sum()
                }
                _ => 0,
            }
        }

        let ctx = context();
        let (mut browser, workspace, device, _tabs, queue, _log) = bench();
        let lit = ctx.style().visuals.selection.bg_fill;
        let mut on = |filter: &Filter| -> usize {
            let output = ctx.run(egui::RawInput::default(), |ctx| {
                egui::SidePanel::left("places")
                    .exact_width(crate::shell::BROWSER)
                    .show(ctx, |ui| {
                        browser.ui(ui, &workspace, &device, &queue, filter);
                    });
            });
            output
                .shapes
                .iter()
                .map(|clipped| selections(&clipped.shape, lit))
                .sum()
        };

        assert_eq!(on(&Filter::default()), 0, "nothing is narrowed to a place");
        let mut filter = Filter::default();
        filter.narrow(Narrow::Place(Place::Computer));
        assert_eq!(on(&filter), 1, "only This computer is highlighted");
    }

    /// The kinds section lists what the two places hold, in one order. A row for a kind
    /// neither place holds would narrow the library to nothing.
    #[test]
    fn the_kinds_section_lists_the_union_of_the_two_places() {
        let (_browser, mut workspace, mut device, _tabs, _queue, mut log) = bench();
        workspace.create(Fresh::Program, &mut log).unwrap();
        let alone = kinds_present(&workspace, &device.state);
        assert_eq!(alone, [Kind::Program]);
        assert!(!worth_choosing(&alone), "nothing to choose between");

        device.pretend_partitions(&crate::device::ELECTRO5);
        workspace.create(Fresh::Stage4Synth, &mut log).unwrap();
        assert_eq!(
            kinds_present(&workspace, &device.state),
            [
                Kind::Program,
                Kind::SetList,
                Kind::Sample,
                Kind::Piano,
                Kind::Live,
                Kind::Settings,
                Kind::Synth,
            ],
            "the instrument's six folders and what is on this computer, in one order"
        );
        assert!(worth_choosing(&kinds_present(&workspace, &device.state)));
    }

    /// ⚠️ The row that turns a filter off goes away with its kind. A filter left on a
    /// kind that is nowhere would show an empty table with nothing to click to clear it.
    #[test]
    fn a_kind_that_leaves_the_union_stops_narrowing() {
        let (_browser, workspace, mut device, _tabs, _queue, _log) = bench();
        let mut filter = Filter::default();
        device.pretend_partitions(&crate::device::ELECTRO5);
        filter.narrow(Narrow::Kind(Kind::Piano));
        filter.keep_kinds(&kinds_present(&workspace, &device.state));
        assert_eq!(filter.kind, Some(Kind::Piano));

        device.pretend(crate::device::DeviceEvent::Disconnected { lost: false });
        device.poll(
            &mut crate::log::Log::default(),
            &mut Workspace::new(context()),
            &mut crate::tabs::Tabs::default(),
            &mut Queue::default(),
        );
        filter.keep_kinds(&kinds_present(&workspace, &device.state));
        assert_eq!(filter.kind, None, "the instrument took its folders with it");
    }

    /// ⚠️ A duplicate skips slots the queue is already bound for, because two writes to
    /// one address would leave only one.
    #[test]
    fn a_duplicate_lands_past_the_slot_the_queue_is_bound_for() {
        let (_browser, mut workspace, mut device, _tabs, mut queue, mut log) = bench();
        let class = ObjectClass::Program;
        device.pretend_scanned(class, 7, &["Africa Split", "", ""]);
        let id = workspace.create(Fresh::Program, &mut log).unwrap();
        assert_eq!(
            spare_slot(&device.state, class, &queue),
            Some(Location::from_user(7, 2))
        );

        crate::queue::enqueue(
            &workspace,
            &mut device,
            &mut queue,
            &mut log,
            id,
            class,
            Location::from_user(7, 2),
        );
        assert_eq!(
            spare_slot(&device.state, class, &queue),
            Some(Location::from_user(7, 3)),
            "the queue is already bound for 7:2"
        );
    }

    /// The piano categories say something the location column does not; "Bank 1" over
    /// rows already labeled `1:…` does not.
    #[test]
    fn a_bank_caption_only_shows_what_the_number_does_not_say() {
        assert!(worth_captioning(1, "Grand"));
        assert!(worth_captioning(2, "Upright"));
        for furniture in ["Bank 1", "bank 1", "BANK 1", "1", " ", ""] {
            assert!(!worth_captioning(1, furniture), "{furniture:?}");
        }
        // Only a matching number is redundant. "Bank 2" over bank 1 is shown, because one
        // of the two is wrong.
        assert!(worth_captioning(1, "Bank 2"));
    }

    /// ⚠️ A leaf's glyph lines up under the glyph of the branch beside it, because a
    /// leaf's indent includes the triangle's box that a branch draws.
    #[test]
    fn a_leaf_starts_where_the_branch_beside_it_puts_its_glyph() {
        assert_eq!(indent(0, true), 8.0);
        assert_eq!(indent(0, false), 26.0);
        assert_eq!(indent(1, false), 40.0);
        for depth in 0..4 {
            assert_eq!(
                indent(depth, false),
                indent(depth, true) + STEP,
                "depth {depth}"
            );
            assert!(indent(depth + 1, true) > indent(depth, true));
        }
    }
}
