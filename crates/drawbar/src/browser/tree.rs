//! The browser's one tree: the places a sound can live, the kinds there are, and the
//! tags on the list on this computer.
//!
//! Every row is [`super::row::row`], and the tree computes its own indents rather than
//! nesting `Ui`s — a leaf skips the triangle's box so its glyph lines up under the glyph
//! of the branch beside it.

use eframe::egui;
use nord_format::accept::Family;
use nord_usb::{Location, ObjectClass};

use super::act::{owed, Act, Bulk};
use super::drag::{kinds_present, Item, Kind, Onto};
use super::row::{row, Cells, Drawn, STEP};
use super::{Ask, Browser, Click};
use crate::device::{occupancy, read_only, Connection, Device};
use crate::filter::{Filter, Narrow, Place, State};
use crate::icon::Glyph;
use crate::panel::panel_header;
use crate::queue::{Queue, Queued};
use crate::strings::{place, shown};
use crate::tabs::Spot;
use crate::workspace::{Fresh, LocalEntity, Workspace};

/// The New menu: every kind this app can build from nothing, and the project that is
/// laid out from audio files rather than started from a default.
///
/// Written once, because the tree's context menu, the File menu, the toolbar and the tab
/// strip all offer it and they must offer the same thing.
pub fn new_menu(ui: &mut egui::Ui, acts: &mut Vec<Act>) {
    for family in &Fresh::FAMILIES {
        ui.menu_button(family.label, |ui| {
            for kind in family.kinds {
                let mut entry = ui.button(kind.label());
                if let Some(note) = kind.note() {
                    entry = entry.on_hover_text(note);
                }
                if entry.clicked() {
                    acts.push(Act::New(*kind));
                    ui.close();
                }
            }
        });
    }
    ui.separator();
    // Not a family: a project is laid out from audio files rather than started from a
    // default, so it asks for them before it exists.
    if ui
        .button("Sample Editor project…")
        .on_hover_text(
            "pick the WAVs it plays; the project stores their names and the editor looks \
             for them beside it",
        )
        .clicked()
    {
        acts.push(Act::NewProject);
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
    Bank(u32, u32),
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
/// A branch at the top is 8 px in, a leaf beside it 26, and a leaf one level down 40:
/// a leaf skips the triangle's box, which is what lines its glyph up under the glyph of
/// a branch at the same depth.
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

/// Whether a click landed on the triangle, which opens the branch rather than picking
/// the row.
fn on_triangle(drawn: &Drawn) -> bool {
    let (Some(box_), Some(at)) = (drawn.chevron, drawn.response.interact_pointer_pos()) else {
        return false;
    };
    box_.expand(3.0).contains(at)
}

/// Turn one of the library's filters, and bring the library forward to show what it
/// left. A narrowing nobody can see is a narrowing that will surprise whoever finds it.
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

/// Whether the kinds section is worth a section at all.
///
/// One kind is what everything in both places is, so there is nothing to choose between
/// and the rows would only narrow the library to what it already shows.
fn worth_choosing(kinds: &[Kind]) -> bool {
    kinds.len() > 1
}

/// Whether a bank's own name says anything the number beside every row does not.
///
/// Programs come back called "Bank 1", "Bank 2" — a caption repeating the number the
/// location column already carries is a line of furniture. Pianos come back called
/// "Grand" and "Upright", which is the whole reason to show a caption at all.
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
                    self.kinds(ui, &kinds, filter, acts);
                }
                if section(ui, "tags", &mut sections.tags) {
                    self.tag_rows(ui, filter, acts);
                }
                self.sections = sections;
                self.empty_below(ui);
            });
    }

    /// The room under the last row: a click there is a click on no row, which lets go of
    /// everything picked.
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

    /// Right-clicking a row nothing has picked picks it; right-clicking one that is
    /// already picked leaves the rest alone, so a menu acts on the whole selection.
    fn aim(&mut self, item: Item) {
        if !self.selection.holds(item) {
            self.select(item);
        }
    }

    // ---- places -----------------------------------------------------------------

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
            for id in self.folder_ids() {
                self.folder_row(ui, id, workspace, device, queue, acts);
            }
            let loose: Vec<Item> = workspace
                .listed()
                .filter(|entity| self.folders.holding(entity.id).is_none())
                .map(|entity| Item::Local(entity.id))
                .collect();
            for entity in workspace.listed() {
                if self.folders.holding(entity.id).is_none() {
                    self.local_row(ui, entity, 1, &loose, workspace, device, queue, acts);
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
            // A row that has gone to nothing stays while it is the one narrowing, so
            // there is always something left to click to widen the library again.
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
                    dot: Some(dot),
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
        // The head of the branch takes a drop, so there is one target that is never
        // also a place a drag could have started from.
        self.drop_zone(ui, &drawn.response, Onto::Computer, acts);
        if drawn.response.clicked() {
            match on_triangle(&drawn) {
                true => self.twist(Branch::Computer),
                false => narrow(acts, here),
            }
        }
        let attached = device.state.connected();
        drawn.response.context_menu(|ui| {
            if ui.button("Open…").clicked() {
                acts.push(Act::OpenFiles);
                ui.close();
            }
            ui.menu_button("New", |ui| new_menu(ui, acts));
            if ui
                .button("New folder")
                .on_hover_text(
                    "a way of grouping the list on this computer; the instrument never sees one",
                )
                .clicked()
            {
                acts.push(Act::NewFolder);
                ui.close();
            }
            if !attached {
                ui.separator();
                if ui.button("Connect instrument").clicked() {
                    acts.push(Act::Connect);
                    ui.close();
                }
            }
        });
    }

    /// The row that stands in for an instrument until there is one.
    ///
    /// ⚠️ The click reaches `requestDevice()` inside the frame it landed in, which is
    /// what keeps the browser's transient user activation alive for it.
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
                "Close Nord Sound Manager first — it holds the instrument on its own, and \
                 nothing else can reach it alongside.\n\nIn a browser: Chrome or Edge only.",
            )
            .clicked()
        {
            acts.push(Act::Connect);
        }
    }

    /// The folders in the order they were made. Taken as ids so a row can change the
    /// list it is drawn from.
    fn folder_ids(&self) -> Vec<u64> {
        self.folders.all().iter().map(|folder| folder.id).collect()
    }

    fn folder_row(
        &mut self,
        ui: &mut egui::Ui,
        id: u64,
        workspace: &Workspace,
        device: &Device,
        queue: &Queue,
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
            // A folder is renamed with its contents in view: what is in it is what the
            // name is about.
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
            let sendable = members
                .iter()
                .filter_map(|id| workspace.get(*id))
                .filter(|entity| owed(entity).is_some())
                .count();
            drawn.response.context_menu(|ui| {
                self.aim(item);
                // ⚠️ Unlike "Send all", this includes unchanged slot-backed items. The
                // count says which scope this is before the queue is added to.
                if ui
                    .add_enabled(
                        sendable > 0,
                        egui::Button::new(format!("Queue folder for the keyboard ({sendable})")),
                    )
                    .on_hover_text("everything in here that came off a slot, changed or not")
                    .on_disabled_hover_text(
                        "nothing in here came off a slot, so there is nowhere to send it back to",
                    )
                    .clicked()
                {
                    acts.push(Act::SendFolder(id));
                    ui.close();
                }
                ui.add_enabled(false, egui::Button::new("Export as a bundle…"))
                    .on_disabled_hover_text("bundles are not written yet");
                ui.separator();
                if ui.button("Rename").clicked() {
                    self.start_rename(item, &name);
                    ui.close();
                }
                if ui
                    .button("Remove folder")
                    .on_hover_text("what is in it goes back to the list; nothing is deleted")
                    .clicked()
                {
                    acts.push(Act::RemoveFolder(id));
                    ui.close();
                }
            });
        }

        if !open {
            return;
        }
        if members.is_empty() {
            nothing(ui, 2, "empty — drag sounds in");
        }
        for entity in members.iter().filter_map(|id| workspace.get(*id)) {
            self.local_row(ui, entity, 2, &inside, workspace, device, queue, acts);
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn local_row(
        &mut self,
        ui: &mut egui::Ui,
        entity: &LocalEntity,
        depth: usize,
        list: &[Item],
        workspace: &Workspace,
        device: &Device,
        queue: &Queue,
        acts: &mut Vec<Act>,
    ) {
        let item = Item::Local(entity.id);
        let kind = Kind::of(entity.entity.as_ref());
        let selected = self.selection.holds(item);

        // While a name is being typed the row stops sensing anything: a drag sense over
        // the field would take the clicks that place the cursor in it.
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
        let word = crate::strings::kind_word(kind, qualifier(entity, workspace, device));
        let drawn = row(
            ui,
            selected,
            &Cells {
                indent: indent(depth, false),
                glyph: Some(kind.glyph()),
                name: &entity.name,
                note: owed.as_deref().or(Some(word.as_str())),
                unsaved: entity.is_unsaved(),
                dot: crate::library::keyboard_mark(entity, &device.state, queue, ui.visuals()),
                tags: wears,
                child: true,
                ..Cells::default()
            },
        );
        let response = drawn.response;

        if response.dragged() {
            if let Some(head) = self.held(item, workspace, &device.state) {
                let carried = self.carrying(head, &entity.name, workspace, &device.state);
                egui::DragAndDrop::set_payload(ui.ctx(), carried);
            }
        }
        // A drop onto a row is a drop onto the list; it is taken here so the branch's
        // own zone does not act on it a second time.
        self.drop_zone(ui, &response, Onto::Computer, acts);

        if response.double_clicked() {
            acts.push(Act::Open(item));
        } else if response.clicked() {
            self.clicked(ui, Click { item, list });
        }
        if self.sole_is(item) && ui.input(|i| i.key_pressed(egui::Key::F2)) {
            self.start_rename(item, &entity.name);
        }

        response.context_menu(|ui| self.menu(ui, item, workspace, device, acts));
    }

    /// The menu a row offers, wherever it is drawn — the tree, or the library table.
    ///
    /// A row inside a checked set of several offers what the library's footer offers,
    /// because the menu is about the set rather than about the row under the pointer.
    ///
    /// A folder and a tag are rows of the tree alone; their menus stay with the rows
    /// that draw them.
    pub fn menu(
        &mut self,
        ui: &mut egui::Ui,
        item: Item,
        workspace: &Workspace,
        device: &Device,
        acts: &mut Vec<Act>,
    ) {
        self.aim(item);
        let checked: Vec<Item> = self.selection.items().collect();
        if checked.len() > 1 && self.selection.holds(item) {
            for action in Bulk::ALL {
                self.bulk_item(ui, action, &checked, acts);
            }
            return;
        }
        match item {
            Item::Local(id) => self.local_menu(ui, id, workspace, acts),
            Item::Slot { class, at } => self.slot_menu(ui, class, at, device, acts),
            Item::Folder(_) | Item::Tag(_) => {}
        }
    }

    fn local_menu(
        &mut self,
        ui: &mut egui::Ui,
        id: u64,
        workspace: &Workspace,
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
        if ui
            .button(Bulk::Queue.label())
            .on_hover_text("to the slot it is linked to, or the first free one in its folder")
            .clicked()
        {
            acts.push(Act::SendChecked(vec![id]));
            ui.close();
        }
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
            .on_hover_text("what is picked, under a tag of its own")
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

    /// Where an asset can be put, for the operators who would rather pick than drag.
    fn filing_menu(&self, ui: &mut egui::Ui, id: u64, filed: Option<u64>, acts: &mut Vec<Act>) {
        if self.folders.all().is_empty() {
            return;
        }
        ui.menu_button("Move to folder", |ui| {
            for folder in self.folders.all() {
                if ui
                    .selectable_label(filed == Some(folder.id), &folder.name)
                    .clicked()
                {
                    acts.push(Act::File {
                        id,
                        folder: Some(folder.id),
                    });
                    ui.close();
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

    /// Every tag, checked where it is on everything picked, and one more.
    ///
    /// The items alone, so the row's own menu and the library's footer can each put
    /// their own label over them.
    pub fn tag_items(&self, ui: &mut egui::Ui, picked: &[u64], acts: &mut Vec<Act>) {
        for tag in self.tags.all() {
            let on = self.tags.on_all(picked, tag.id);
            if ui.selectable_label(on, &tag.name).clicked() {
                let ids = picked.to_vec();
                acts.push(match on {
                    true => Act::Untag { ids, tag: tag.id },
                    false => Act::Tag { ids, tag: tag.id },
                });
                ui.close();
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

    // ---- the instrument ---------------------------------------------------------

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
                dot: Some(crate::app::good(ui.visuals())),
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
        let waiting = queue.len();
        drawn.response.context_menu(|ui| {
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

        if !self.open.contains(&Branch::Instrument) {
            return;
        }
        // The slots something is looking at right now, which the list on this computer
        // deliberately does not show.
        let viewed: Vec<(ObjectClass, Location)> = workspace
            .entities()
            .iter()
            .filter(|entity| !entity.kept)
            .filter_map(|entity| entity.origin.slot())
            .collect();
        for class in device.state.classes() {
            self.class_row(ui, device, class, &viewed, workspace, acts);
        }
    }

    fn class_row(
        &mut self,
        ui: &mut egui::Ui,
        device: &Device,
        class: ObjectClass,
        viewed: &[(ObjectClass, Location)],
        workspace: &Workspace,
        acts: &mut Vec<Act>,
    ) {
        // A jump wins over whatever the branch was left in: the point of it is to reach
        // a slot that is inside something closed.
        if let Some((held, at)) = self.jump.filter(|(held, _)| *held == class) {
            self.open.insert(Branch::Class(class.to_raw()));
            self.open.insert(Branch::Bank(held.to_raw(), at.bank + 1));
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
        drawn.response.context_menu(|ui| {
            if ui
                .button("Read this folder again")
                .on_hover_text("Read everything reads the whole instrument; this reads one folder")
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
                    self.jump = Some((class, at));
                    ui.close();
                }
            }
        });

        // ⚠️ A jump at a slot the walk has never reached would hold the branch open for
        // as long as the instrument stays attached: nothing draws the row that clears it.
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
        // The live buffer and the settings singleton divide into one bank.
        let cut = banks.len() > 1;
        for bank in banks {
            self.bank_rows(ui, device, class, bank, cut, viewed, workspace, acts);
        }
    }

    /// One bank, as a branch over its own slots.
    ///
    /// ⚠️ A container in the browser and a numbering everywhere else. The instrument has
    /// no folders inside a class — a location is a bank and a slot and that is all — but
    /// four hundred rows in one run is a tree nobody can navigate, so the numbering is
    /// what the list is cut on. A bank the device named says so in its row; for pianos
    /// those names are the panel's categories.
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
        acts: &mut Vec<Act>,
    ) {
        let Some(slots) = device.state.bank(class, bank) else {
            return;
        };
        let count = slots.len();
        let held = slots.iter().filter(|slot| slot.is_some()).count();
        let list: Vec<Item> = (0..count)
            .map(|index| Item::Slot {
                class,
                at: Location::from_user(bank, index as u32 + 1),
            })
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
                    open: Some(self.open.contains(&Branch::Bank(class.to_raw(), bank))),
                    glyph: Some(Glyph::Folder),
                    name: &name,
                    count: Some(format!("{held}/{count}")),
                    child: true,
                    ..Cells::default()
                },
            );
            if drawn.response.clicked() {
                self.twist(Branch::Bank(class.to_raw(), bank));
            }
            if !self.open.contains(&Branch::Bank(class.to_raw(), bank)) {
                return;
            }
        }
        for item in &list {
            let Item::Slot { at, .. } = item else {
                continue;
            };
            self.slot_row(
                ui, device, class, *at, depth, &list, viewed, workspace, acts,
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
        acts: &mut Vec<Act>,
    ) {
        let held = device
            .state
            .slot(class, at)
            .flatten()
            .map(|info| info.name.trim().to_string());
        let item = Item::Slot { class, at };
        let selected = self.selection.holds(item);

        // While a name is being typed the row stops sensing anything: a drag sense over
        // the field would take the clicks that place the cursor in it.
        if self.rename.as_ref().is_some_and(|r| r.what == item) {
            let was = held.clone().unwrap_or_default();
            if let Some(name) = self.rename_row(ui, indent(depth, false), &was) {
                acts.push(Act::RenameSlot { class, at, name });
            }
            return;
        }

        let loaded = device.state.focused(class) == Some(at);
        // A slot open as a view says so here, because the tab strip cannot: what a tab
        // shows is the document's name, and a view's name is the slot's own.
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
            response = response.on_hover_text("on the instrument's panel now");
        }
        if viewing {
            response = response
                .on_hover_text("open in a tab as a view of this slot — it is not on this computer");
        }
        // A jump can scroll only after the branches holding this row have opened.
        if self.jump == Some((class, at)) {
            self.jump = None;
            self.selection.only(item);
            response.scroll_to_me(Some(egui::Align::Center));
        }

        // ⚠️ Pianos are large libraries fetched whole, so this browser only lists them.
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
        response.context_menu(|ui| self.menu(ui, item, workspace, device, acts));
    }

    /// What a slot offers. A vacant one offers nothing, so nothing is drawn for it.
    fn slot_menu(
        &mut self,
        ui: &mut egui::Ui,
        class: ObjectClass,
        at: Location,
        device: &Device,
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
        // ⚠️ Pianos are large libraries fetched whole, so this browser only lists them.
        if read_only(class) {
            ui.label(
                egui::RichText::new("Installed on the instrument; nothing to change here.").weak(),
            );
            return;
        }
        let item = Item::Slot { class, at };
        let free = device.state.first_free(class);
        if ui
            .button("Open")
            .on_hover_text("a view of this slot; nothing joins the list on this computer")
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
            .on_disabled_hover_text("every slot read so far is taken")
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

    // ---- kinds and tags ---------------------------------------------------------

    fn kinds(&mut self, ui: &mut egui::Ui, kinds: &[Kind], filter: &Filter, acts: &mut Vec<Act>) {
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
        }
    }

    fn tag_rows(&mut self, ui: &mut egui::Ui, filter: &Filter, acts: &mut Vec<Act>) {
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
            drawn.response.context_menu(|ui| {
                if ui.button("Rename").clicked() {
                    self.start_rename(item, &name);
                    ui.close();
                }
                if ui
                    .button("Remove tag")
                    .on_hover_text("it comes off everything wearing it; nothing is deleted")
                    .clicked()
                {
                    acts.push(Act::RemoveTag(id));
                    ui.close();
                }
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

/// The family to put in front of an asset's kind word, where the word alone would not
/// say whose files these are.
fn qualifier(entity: &LocalEntity, workspace: &Workspace, device: &Device) -> Option<Family> {
    let family = Family::of_tag(&entity.tag());
    let instrument = device.state.product().and_then(Family::from_product);
    super::qualified(&super::families_present(workspace), family, instrument)
        .then_some(family)
        .flatten()
}

/// Where a queued asset is going, for the note that says so.
fn destination(held: &Queued) -> String {
    format!("→ {}", place(held.class, held.at))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::browser::act::apply;
    use crate::browser::bench::{bench, context};
    use crate::shell::Shell;

    /// ⚠️ A filter turned while a document is in front narrows a table nobody is looking
    /// at. Every row of the tree that narrows brings the library forward with it.
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

    /// ⚠️ A place row narrows the library like a kind or a tag row, so it reads as on
    /// like one. Without it nothing in the window says where the narrowing came from.
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
        assert_eq!(on(&filter), 1, "This computer is the one row lit");
    }

    /// The kinds row list is what the two places actually hold, in one order. A row for
    /// a kind neither place holds narrows the library to nothing.
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

    /// ⚠️ The row that turns a narrowing off goes with the kind. A filter left pointing
    /// at a kind that is nowhere shows an empty table with nothing to click to refill it.
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

    /// A caption earns its line by saying something the location column does not. The
    /// piano categories do; "Bank 1" over the rows already labelled `1:…` does not.
    #[test]
    fn a_bank_caption_only_shows_what_the_number_does_not_say() {
        assert!(worth_captioning(1, "Grand"));
        assert!(worth_captioning(2, "Upright"));
        for furniture in ["Bank 1", "bank 1", "BANK 1", "1", " ", ""] {
            assert!(!worth_captioning(1, furniture), "{furniture:?}");
        }
        // The number has to match to be redundant — "Bank 2" over bank 1 is worth saying,
        // because one of the two is wrong and hiding it would hide that.
        assert!(worth_captioning(1, "Bank 2"));
    }

    /// ⚠️ A leaf's glyph lines up under the glyph of the branch beside it, because a
    /// leaf's indent already carries the triangle's box that a branch's does not.
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
