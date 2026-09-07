//! The browser's one tree: the places a sound can live, the kinds there are, and the
//! tags on the list on this computer.
//!
//! Every row is [`super::row::row`], and the tree computes its own indents rather than
//! nesting `Ui`s — a leaf skips the triangle's box so its glyph lines up under the glyph
//! of the branch beside it.

use eframe::egui;
use nord_usb::{Location, ObjectClass};

use super::act::{owed, Act};
use super::drag::{Held, Item, Kind, Onto};
use super::row::{row, Cells, Drawn, STEP};
use super::{Ask, Browser, Click};
use crate::device::{occupancy, read_only, Connection, Device, BROWSED};
use crate::filter::{Filter, Narrow, Place};
use crate::icon::Glyph;
use crate::panel::panel_header;
use crate::shell::Page;
use crate::strings::{folder, place, shown};
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

/// The kinds the tree offers to narrow the library to, in the order §5.1 lists them.
const KINDS: [Kind; 7] = [
    Kind::Program,
    Kind::Live,
    Kind::SetList,
    Kind::Sample,
    Kind::Piano,
    Kind::Settings,
    Kind::Project,
];

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
        filter: &Filter,
        acts: &mut Vec<Act>,
    ) {
        egui::ScrollArea::vertical()
            .id_salt("browser_tree")
            .auto_shrink([false; 2])
            .show(ui, |ui| {
                let mut sections = self.sections;
                if section(ui, "places", &mut sections.places) {
                    self.places(ui, workspace, device, acts);
                }
                if section(ui, "kinds", &mut sections.kinds) {
                    self.kinds(ui, filter, acts);
                }
                if section(ui, "tags", &mut sections.tags) {
                    self.tag_rows(ui, filter, acts);
                }
                self.sections = sections;
            });
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

    fn places(
        &mut self,
        ui: &mut egui::Ui,
        workspace: &Workspace,
        device: &Device,
        acts: &mut Vec<Act>,
    ) {
        self.computer_row(ui, workspace, device, acts);
        if self.open.contains(&Branch::Computer) {
            for id in self.folder_ids() {
                self.folder_row(ui, id, workspace, device, acts);
            }
            let loose: Vec<Item> = workspace
                .listed()
                .filter(|entity| self.folders.holding(entity.id).is_none())
                .map(|entity| Item::Local(entity.id))
                .collect();
            for entity in workspace.listed() {
                if self.folders.holding(entity.id).is_none() {
                    self.local_row(ui, entity, 1, &loose, workspace, acts);
                }
            }
            if loose.is_empty() && self.folders.all().is_empty() {
                nothing(ui, 1, "Drop Nord files here, or use Open…");
            }
        }

        match device.state.connected() {
            true => self.instrument_rows(ui, workspace, device, acts),
            false => self.connect_row(ui, device, acts),
        }

        let waiting = workspace.pending().len();
        if waiting == 0 {
            return;
        }
        let drawn = row(
            ui,
            false,
            &Cells {
                indent: indent(0, false),
                glyph: Some(Glyph::Upload),
                name: "Waiting to send",
                dot: Some(crate::app::warn(ui.visuals())),
                count: Some(waiting.to_string()),
                ..Cells::default()
            },
        );
        if drawn
            .response
            .on_hover_text("everything owed back to the instrument")
            .clicked()
        {
            acts.push(Act::ShowPage(Page::Queue));
        }
    }

    fn computer_row(
        &mut self,
        ui: &mut egui::Ui,
        workspace: &Workspace,
        device: &Device,
        acts: &mut Vec<Act>,
    ) {
        let drawn = row(
            ui,
            false,
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
                false => {
                    acts.push(Act::ShowTab(Spot::Library));
                    acts.push(Act::Narrow(Narrow::Place(Place::Computer)));
                }
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
                        let click = Click {
                            item,
                            from: &name,
                            list: &list,
                        };
                        self.clicked(ui, click, &drawn.response, drawn.name);
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
                // count distinguishes those scopes before the confirmation dialog.
                if ui
                    .add_enabled(
                        sendable > 0,
                        egui::Button::new(format!("Send folder to keyboard ({sendable})")),
                    )
                    .on_hover_text("everything in here that came off a slot, changed or not")
                    .on_disabled_hover_text(
                        "nothing in here came off a slot, so there is nowhere to send it back to",
                    )
                    .clicked()
                {
                    self.ask_send(
                        workspace,
                        device,
                        &members,
                        format!("Send everything in “{name}” to the instrument?"),
                        Act::SendFolder(id),
                    );
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
            self.local_row(ui, entity, 2, &inside, workspace, acts);
        }
    }

    fn local_row(
        &mut self,
        ui: &mut egui::Ui,
        entity: &LocalEntity,
        depth: usize,
        list: &[Item],
        workspace: &Workspace,
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

        let owed = entity.pending.then(|| destination(entity)).flatten();
        let filed = self.folders.holding(entity.id);
        let wears = self.tags.worn(entity.id).len();
        let drawn = row(
            ui,
            selected,
            &Cells {
                indent: indent(depth, false),
                glyph: Some(kind.glyph()),
                name: &entity.name,
                note: owed.as_deref().or(Some(kind.chip())),
                dirty: entity.dirty,
                dot: owed.is_some().then(|| crate::app::warn(ui.visuals())),
                count: (wears > 0).then(|| wears.to_string()),
                child: true,
                ..Cells::default()
            },
        );
        let response = drawn.response;

        if response.dragged() {
            let head = Held {
                what: item,
                kind,
                filed,
            };
            let carried = self.carrying(head, &entity.name, workspace);
            egui::DragAndDrop::set_payload(ui.ctx(), carried);
        }
        // A drop onto a row is a drop onto the list; it is taken here so the branch's
        // own zone does not act on it a second time.
        self.drop_zone(ui, &response, Onto::Computer, acts);

        if response.double_clicked() {
            acts.push(Act::Open(item));
        } else if response.clicked() {
            let click = Click {
                item,
                from: &entity.name,
                list,
            };
            self.clicked(ui, click, &response, drawn.name);
        }
        if selected && ui.input(|i| i.key_pressed(egui::Key::F2)) {
            self.start_rename(item, &entity.name);
        }

        response.context_menu(|ui| {
            self.aim(item);
            let picked = self.selection.locals();
            if ui.button("Open").clicked() {
                acts.push(Act::Open(item));
                ui.close();
            }
            if ui.button("Export…").clicked() {
                acts.push(Act::Save(entity.id));
                ui.close();
            }
            if ui.button("Rename").clicked() {
                self.start_rename(item, &entity.name);
                ui.close();
            }
            if ui.button("Duplicate").clicked() {
                acts.push(Act::DuplicateLocal(entity.id));
                ui.close();
            }
            self.filing_menu(ui, entity.id, filed, acts);
            self.tag_menu(ui, &picked, acts);
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
                acts.push(Act::Remove(entity.id));
                ui.close();
            }
        });
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
    fn tag_menu(&self, ui: &mut egui::Ui, picked: &[u64], acts: &mut Vec<Act>) {
        ui.menu_button("Tag", |ui| {
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
        });
    }

    // ---- the instrument ---------------------------------------------------------

    fn instrument_rows(
        &mut self,
        ui: &mut egui::Ui,
        workspace: &Workspace,
        device: &Device,
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
            false,
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
                false => {
                    acts.push(Act::ShowTab(Spot::Keyboard));
                    acts.push(Act::Narrow(Narrow::Place(Place::Keyboard)));
                }
            }
        }
        let reading = BROWSED
            .iter()
            .filter_map(|class| device.state.scan.progress(*class))
            .any(|progress| progress.running);
        let waiting = workspace.pending().len();
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
        for class in BROWSED {
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
                name: folder(class),
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
            if fetchable && response.dragged() {
                let head = Held {
                    what: item,
                    kind: Kind::from_class(class),
                    filed: None,
                };
                let carried = self.carrying(head, name, workspace);
                egui::DragAndDrop::set_payload(ui.ctx(), carried);
            }
        }
        self.drop_zone(ui, &response, Onto::Slot { class, at }, acts);

        if response.double_clicked() {
            if held.is_some() && fetchable {
                acts.push(Act::Open(item));
            }
        } else if response.clicked() {
            match (&held, fetchable) {
                (Some(name), true) => {
                    let click = Click {
                        item,
                        from: name,
                        list,
                    };
                    self.clicked(ui, click, &response, drawn.name);
                }
                _ => self.select(item),
            }
        }
        if let Some(name) = &held {
            if selected && fetchable && ui.input(|i| i.key_pressed(egui::Key::F2)) {
                self.start_rename(item, name);
            }
        }

        let Some(name) = held else {
            return;
        };
        let free = device.state.first_free(class);
        response.context_menu(|ui| {
            self.aim(item);
            if !fetchable {
                ui.label(
                    egui::RichText::new("Installed on the instrument; nothing to change here.")
                        .weak(),
                );
                return;
            }
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
                    act: Act::DeleteSlot { class, at },
                });
                ui.close();
            }
        });
    }

    // ---- kinds and tags ---------------------------------------------------------

    fn kinds(&mut self, ui: &mut egui::Ui, filter: &Filter, acts: &mut Vec<Act>) {
        for kind in KINDS {
            let narrow = Narrow::Kind(kind);
            let drawn = row(
                ui,
                filter.on(narrow),
                &Cells {
                    indent: indent(0, false),
                    glyph: Some(kind.glyph()),
                    name: kind.plural(),
                    ..Cells::default()
                },
            );
            if drawn.response.clicked() {
                acts.push(Act::Narrow(narrow));
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
            let narrow = Narrow::Tag(id);
            let drawn = row(
                ui,
                filter.on(narrow),
                &Cells {
                    indent: indent(0, false),
                    glyph: Some(Glyph::Tag),
                    name: &name,
                    count: Some(self.tags.count(id).to_string()),
                    ..Cells::default()
                },
            );
            if drawn.response.clicked() {
                acts.push(Act::Narrow(narrow));
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

/// Where an asset is owed, for the note that says so.
fn destination(entity: &LocalEntity) -> Option<String> {
    let (class, at) = entity.origin.slot()?;
    Some(format!("→ {}", place(class, at)))
}

#[cfg(test)]
mod tests {
    use super::*;

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
