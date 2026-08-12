//! The sidebar: the two places a sound can live, and the moving of sounds between them.
//!
//! Nothing here touches the instrument. Rendering reads the caches and answers with a
//! list of [`Act`]s, which [`apply`] then runs against the workspace, the device and the
//! tabs — so a row can be drawn while the thing it stands for is about to change.

use std::collections::HashMap;
use std::sync::Arc;

use eframe::egui;
use nord_format::Entity;
use nord_usb::{Location, ObjectClass};

use crate::app::dot;
use crate::device::{
    occupancy, put_refusal, read_only, Connection, Device, DeviceCmd, Outgoing, BROWSED,
};
use crate::log::Log;
use crate::strings::{folder, place, shown};
use crate::tabs::Tabs;
use crate::workspace::{ExportWhat, Fresh, LocalEntity, Workspace};

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
            } else if put_refusal(class).is_some() {
                Landing::No("this folder cannot be written to over USB")
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

/// What the browser asks the rest of the app to do.
pub enum Act {
    Connect,
    Disconnect,
    OpenFiles,
    New(Fresh),
    /// Read the whole instrument again — every class, its geometry and its focus.
    Resync,
    ReadAgain(ObjectClass),
    Open(Item),
    /// A view of a slot becomes an asset on this computer.
    Keep(u64),
    NewFolder,
    RemoveFolder(u64),
    /// Put an asset in a folder, or out of the one it is in.
    File {
        id: u64,
        folder: Option<u64>,
    },
    /// Write every sendable asset in a folder back where it came from. Already agreed to.
    SendFolder(u64),
    Copy {
        class: ObjectClass,
        at: Location,
    },
    LoadOnInstrument {
        class: ObjectClass,
        at: Location,
    },
    /// Put a local asset into a slot, asking first if something is already there.
    Send {
        id: u64,
        class: ObjectClass,
        at: Location,
    },
    /// Write everything waiting, grouped by folder. Already agreed to.
    SendAll,
    /// The same as a Send, already agreed to. Nothing asks twice.
    Replace {
        id: u64,
        class: ObjectClass,
        at: Location,
    },
    Rearrange {
        class: ObjectClass,
        from: Location,
        to: Location,
    },
    RenameLocal {
        id: u64,
        name: String,
    },
    RenameFolder {
        id: u64,
        name: String,
    },
    RenameSlot {
        class: ObjectClass,
        at: Location,
        name: String,
    },
    DuplicateLocal(u64),
    DuplicateSlot {
        class: ObjectClass,
        from: Location,
        to: Location,
    },
    DeleteSlot {
        class: ObjectClass,
        at: Location,
    },
    Remove(u64),
    Save(u64),
    /// Nothing happened, and this is why.
    Refused(String),
}

/// An in-place rename, waiting on Enter or Esc.
struct Rename {
    what: Item,
    text: String,
    /// The first frame, in which the field takes focus and selects what is in it.
    fresh: bool,
}

/// A question that has to be answered before something is lost.
struct Ask {
    title: String,
    note: Option<String>,
    verb: &'static str,
    act: Act,
}

/// Whether a plain click starts a rename rather than moving the selection.
///
/// Both halves are needed. Selecting is the whole row's job, so a row that answers a
/// click anywhere would otherwise arm the editor on every second click.
pub fn arms_rename(selected: bool, on_name: bool) -> bool {
    selected && on_name
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

/// One folder on this computer.
pub struct Folder {
    pub id: u64,
    pub name: String,
}

/// How the local list is grouped.
///
/// ⚠️ A folder is a **view of the list**, not a place bytes live: an asset in one is an
/// asset like any other, and nothing here is a directory, an archive or anything the
/// instrument has ever heard of. Membership is kept beside the divider rather than in
/// the workspace for that reason.
#[derive(Default)]
pub struct Folders {
    list: Vec<Folder>,
    /// Which folder an asset is in, by its workspace id. Absent is loose.
    of: HashMap<u64, u64>,
}

impl Folders {
    pub fn all(&self) -> &[Folder] {
        &self.list
    }

    fn name_of(&self, id: u64) -> Option<&str> {
        self.list
            .iter()
            .find(|folder| folder.id == id)
            .map(|folder| folder.name.as_str())
    }

    /// A new folder, under a name nothing else in the list is using.
    fn make(&mut self) -> u64 {
        let id = self.list.iter().map(|folder| folder.id).max().unwrap_or(0) + 1;
        let taken = |name: &str| self.list.iter().any(|folder| folder.name == name);
        let mut name = "New folder".to_string();
        for nth in 2.. {
            if !taken(&name) {
                break;
            }
            name = format!("New folder {nth}");
        }
        self.list.push(Folder { id, name });
        id
    }

    fn rename(&mut self, id: u64, name: String) {
        if let Some(folder) = self.list.iter_mut().find(|folder| folder.id == id) {
            folder.name = name;
        }
    }

    /// Drop a folder. What was in it goes back to the loose part of the list — a folder
    /// holds nothing, so removing one cannot take anything with it.
    fn remove(&mut self, id: u64) {
        self.list.retain(|folder| folder.id != id);
        self.of.retain(|_, held| *held != id);
    }

    fn file(&mut self, entity: u64, folder: Option<u64>) {
        match folder.filter(|id| self.name_of(*id).is_some()) {
            Some(id) => self.of.insert(entity, id),
            None => self.of.remove(&entity),
        };
    }

    fn forget(&mut self, entity: u64) {
        self.of.remove(&entity);
    }

    /// Drop the memberships of assets the list does not hold.
    ///
    /// The store keeps the folders and the assets in two files that are read back
    /// separately, and only the asset file decides what survived — anything too big to
    /// keep, or dropped for want of room, leaves its membership behind. Left alone they
    /// accumulate for as long as the app is installed.
    fn forget_missing(&mut self, workspace: &Workspace) {
        self.of.retain(|entity, _| workspace.get(*entity).is_some());
    }

    /// Which folder an asset is in.
    pub fn holding(&self, entity: u64) -> Option<u64> {
        self.of.get(&entity).copied()
    }

    /// What this folder holds, in the order the list holds it.
    fn members<'a>(&self, id: u64, workspace: &'a Workspace) -> Vec<&'a LocalEntity> {
        workspace
            .listed()
            .filter(|entity| self.holding(entity.id) == Some(id))
            .collect()
    }
}

/// The folders and their membership as one string, for the store.
///
/// `f` lines are the folders and `m` lines are what is in them, so a folder with nothing
/// in it survives a session like any other.
fn written(folders: &Folders) -> String {
    let mut out = format!("{FOLDERS_VERSION}\n");
    for folder in &folders.list {
        out.push_str(&format!("f\t{}\t{}\n", folder.id, escaped(&folder.name)));
    }
    for (entity, folder) in &folders.of {
        out.push_str(&format!("m\t{entity}\t{folder}\n"));
    }
    out
}

/// Read back what [`written`] wrote. Anything unaccounted for is no folders at all —
/// half a grouping is worse than none, because a folder nobody made is one nobody can
/// explain.
fn read(text: &str) -> Folders {
    let mut lines = text.lines();
    if lines.next() != Some(FOLDERS_VERSION) {
        return Folders::default();
    }
    let mut folders = Folders::default();
    for line in lines {
        let mut parts = line.split('\t');
        match (parts.next(), parts.next(), parts.next()) {
            (Some("f"), Some(id), Some(name)) => {
                if let Ok(id) = id.parse() {
                    folders.list.push(Folder {
                        id,
                        name: unescaped(name),
                    });
                }
            }
            (Some("m"), Some(entity), Some(folder)) => {
                if let (Ok(entity), Ok(folder)) = (entity.parse(), folder.parse()) {
                    folders.of.insert(entity, folder);
                }
            }
            _ => {}
        }
    }
    // A membership naming a folder that is not in the file would be an asset nothing
    // shows and nothing can get back.
    let known: Vec<u64> = folders.list.iter().map(|folder| folder.id).collect();
    folders.of.retain(|_, folder| known.contains(folder));
    folders
}

/// Tabs separate the fields and newlines separate the lines, so a name holding either
/// would be a name that ate the rest of the store.
fn escaped(text: &str) -> String {
    text.replace('\\', "\\\\")
        .replace('\t', "\\t")
        .replace('\n', "\\n")
}

fn unescaped(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars();
    while let Some(c) = chars.next() {
        match (c, chars.clone().next()) {
            ('\\', Some('t')) => {
                chars.next();
                out.push('\t');
            }
            ('\\', Some('n')) => {
                chars.next();
                out.push('\n');
            }
            ('\\', Some('\\')) => {
                chars.next();
                out.push('\\');
            }
            _ => out.push(c),
        }
    }
    out
}

pub struct Browser {
    selection: Option<Item>,
    rename: Option<Rename>,
    ask: Option<Ask>,
    /// Where the divider sits between the two columns, as a share of the dock.
    split: f32,
    folders: Folders,
    /// A slot to scroll to and select, once the list holding it has been drawn.
    jump: Option<(ObjectClass, Location)>,
}

impl Default for Browser {
    fn default() -> Browser {
        Browser {
            selection: None,
            rename: None,
            ask: None,
            split: EVEN,
            folders: Folders::default(),
            jump: None,
        }
    }
}

/// The divider's home: half the dock each.
const EVEN: f32 = 0.5;

/// Neither column may be dragged out of existence.
const LEAST: f32 = 0.15;

/// The strip the divider answers on.
const HANDLE: f32 = 7.0;

const FOLDERS_VERSION: &str = "drawbar folders 1";

impl Browser {
    /// Where the divider is kept between sessions.
    pub const SPLIT: &'static str = "drawbar.dock_split";

    /// Where the folders and their membership are kept between sessions.
    ///
    /// ⚠️ Membership is by workspace id, which is the same id the local list is stored
    /// under — the two files are read back into one list, so they have to agree about
    /// what an id means.
    pub const FOLDERS: &'static str = "drawbar.folders";

    /// Put the divider and the folders back where they were left.
    ///
    /// Anything the store cannot account for is the even split: a fraction outside the
    /// stops would be one column showing and the other a sliver.
    pub fn restore(&mut self, storage: &dyn eframe::Storage) {
        self.split = storage
            .get_string(Browser::SPLIT)
            .and_then(|text| text.parse::<f32>().ok())
            .filter(|share| (LEAST..=1.0 - LEAST).contains(share))
            .unwrap_or(EVEN);
        self.folders = storage
            .get_string(Browser::FOLDERS)
            .map(|text| read(&text))
            .unwrap_or_default();
    }

    /// Reconcile the folders with the list that came back beside them. Call once, after
    /// both stores have been read.
    pub fn settle(&mut self, workspace: &Workspace) {
        self.folders.forget_missing(workspace);
    }

    pub fn keep(&self, storage: &mut dyn eframe::Storage) {
        storage.set_string(Browser::SPLIT, self.split.to_string());
        storage.set_string(Browser::FOLDERS, written(&self.folders));
    }

    /// Draw the places a sound can live and collect what the user asked for.
    ///
    /// The instrument gets a column once there is an instrument. Attached, the two sit
    /// adjacent — a drag between them is then a short horizontal move, and a long device
    /// tree cannot push the local list off-screen. Unattached, the column would be half
    /// the sidebar saying nothing, so the list takes the whole width and the way to an
    /// instrument is a button beside the ones that open files.
    pub fn ui(&mut self, ui: &mut egui::Ui, workspace: &Workspace, device: &Device) -> Vec<Act> {
        let mut acts = Vec::new();
        self.dialog(ui.ctx(), &mut acts);
        match device.state.connected() {
            true => self.dock(ui, workspace, device, &mut acts),
            false => self.computer(ui, workspace, device, &mut acts),
        }
        ghost(ui.ctx());
        acts
    }

    /// The two columns and the divider between them.
    ///
    /// ⚠️ A share of the width rather than a number of points. The sidebar the two live
    /// in is itself resizable, and a column pinned to points eats the other one as the
    /// sidebar narrows — at which point the divider has nothing left to give back.
    fn dock(
        &mut self,
        ui: &mut egui::Ui,
        workspace: &Workspace,
        device: &Device,
        acts: &mut Vec<Act>,
    ) {
        let whole = ui.available_rect_before_wrap();
        let usable = (whole.width() - HANDLE).max(1.0);
        let left = usable * self.split;
        let divider = egui::Rect::from_min_size(
            egui::pos2(whole.left() + left, whole.top()),
            egui::vec2(HANDLE, whole.height()),
        );

        let dragging = ui
            .interact(
                divider,
                ui.id().with("dock_divider"),
                egui::Sense::click_and_drag(),
            )
            .on_hover_and_drag_cursor(egui::CursorIcon::ResizeHorizontal);
        if dragging.dragged() {
            self.split = ((left + dragging.drag_delta().x) / usable).clamp(LEAST, 1.0 - LEAST);
        }
        // Somewhere to put it back to, for a divider that has been dragged into a corner.
        if dragging.double_clicked() {
            self.split = EVEN;
        }

        let ends = |from: f32, to: f32| {
            egui::Rect::from_min_max(
                egui::pos2(from, whole.top()),
                egui::pos2(to, whole.bottom()),
            )
        };
        ui.scope_builder(
            egui::UiBuilder::new().max_rect(ends(whole.left(), divider.left())),
            |ui| self.computer(ui, workspace, device, acts),
        );
        ui.scope_builder(
            egui::UiBuilder::new().max_rect(ends(divider.right(), whole.right())),
            |ui| self.instrument(ui, workspace, device, acts),
        );

        let visuals = ui.visuals();
        let stroke = match dragging.hovered() || dragging.dragged() {
            true => egui::Stroke::new(2.0, visuals.selection.stroke.color),
            false => visuals.widgets.noninteractive.bg_stroke,
        };
        ui.painter()
            .vline(divider.center().x, whole.y_range(), stroke);
        ui.advance_cursor_after_rect(whole);
    }

    /// A column heading, and the strip of buttons beside it.
    ///
    /// Wrapped, because the strip is in a column the operator can drag to any width and
    /// a button pushed off the right edge is a button that is gone.
    fn heading(
        &mut self,
        ui: &mut egui::Ui,
        title: &str,
        buttons: impl FnOnce(&mut egui::Ui),
    ) -> egui::Response {
        let head = ui
            .horizontal_wrapped(|ui| {
                ui.label(egui::RichText::new(title).strong());
                buttons(ui);
            })
            .response;
        ui.separator();
        head
    }

    fn select(&mut self, item: Item) {
        let same = self.rename.as_ref().is_some_and(|r| r.what == item);
        if !same {
            self.rename = None;
        }
        self.selection = Some(item);
    }

    /// Take back an armed rename, because the row it belongs to is about to stop
    /// existing and no row will be drawn to close it.
    fn forget_rename(&mut self, what: Item) {
        if self.rename.as_ref().is_some_and(|r| r.what == what) {
            self.rename = None;
        }
        if self.selection == Some(what) {
            self.selection = None;
        }
    }

    fn start_rename(&mut self, what: Item, from: &str) {
        self.selection = Some(what);
        self.rename = Some(Rename {
            what,
            text: from.to_string(),
            fresh: true,
        });
    }

    // ---- this computer ----------------------------------------------------------

    fn computer(
        &mut self,
        ui: &mut egui::Ui,
        workspace: &Workspace,
        device: &Device,
        acts: &mut Vec<Act>,
    ) {
        let mut open_files = false;
        let mut fresh = None;
        let mut new_folder = false;
        let mut connect = false;
        let attached = device.state.connected();
        let connecting = matches!(device.state.connection, Connection::Connecting);
        let head = self.heading(ui, "This computer", |ui| {
            open_files = ui.small_button("Open…").clicked();
            // A family, then a kind inside it: one flat list of every format's every
            // object is a wall of words in which "Program" appears four times.
            ui.menu_button("New", |ui| {
                for family in &Fresh::FAMILIES {
                    ui.menu_button(family.label, |ui| {
                        for kind in family.kinds {
                            let mut entry = ui.button(kind.label());
                            if let Some(note) = kind.note() {
                                entry = entry.on_hover_text(note);
                            }
                            if entry.clicked() {
                                fresh = Some(*kind);
                                ui.close();
                            }
                        }
                    });
                }
            });
            new_folder = ui
                .small_button("New folder")
                .on_hover_text(
                    "a way of grouping the list on this computer; the instrument never sees one",
                )
                .clicked();
            if attached {
                return;
            }
            match connecting {
                true => {
                    ui.spinner();
                }
                // ⚠️ Reached inside the frame the click landed in, which is what keeps
                // the browser's transient user activation alive for `requestDevice()`.
                false => {
                    connect = ui
                        .small_button("Connect instrument")
                        .on_hover_text(
                            "Close Nord Sound Manager first — it holds the instrument on its \
                             own, and nothing else can reach it alongside.\n\nIn a browser: \
                             Chrome or Edge only.",
                        )
                        .clicked();
                }
            }
        });
        if open_files {
            acts.push(Act::OpenFiles);
        }
        if let Some(kind) = fresh {
            acts.push(Act::New(kind));
        }
        if new_folder {
            acts.push(Act::NewFolder);
        }
        if connect {
            acts.push(Act::Connect);
        }
        // The heading takes a drop, so there is one target that is never also a place a
        // drag could have started from.
        self.drop_zone(ui, &head, Onto::Computer, acts);

        egui::ScrollArea::vertical()
            .id_salt("computer_scroll")
            .auto_shrink([false; 2])
            .show(ui, |ui| {
                if workspace.listed().next().is_none() && self.folders.all().is_empty() {
                    ui.label(
                        egui::RichText::new("Drop Nord files here, or use Open…")
                            .weak()
                            .italics(),
                    );
                }
                // Folders first, then whatever is in no folder: a group has to be
                // findable without scrolling past the loose list to reach it.
                for id in self.folder_ids() {
                    self.folder_rows(ui, id, workspace, device, acts);
                }
                for entity in workspace.listed() {
                    if self.folders.holding(entity.id).is_none() {
                        self.local_row(ui, entity, acts);
                    }
                }
                if let Some(carried) = egui::DragAndDrop::payload::<Carried>(ui.ctx()) {
                    let landing = row(
                        ui,
                        false,
                        &Cells {
                            name: match carried.filed.is_some() {
                                true => "Drop here to take it out of its folder",
                                false => "Drop here to copy it to this computer",
                            },
                            faint: true,
                            ..Cells::default()
                        },
                    );
                    self.drop_zone(ui, &landing.response, Onto::Computer, acts);
                }
            });
    }

    /// The folders in the order they were made. Taken as ids so a row can change the
    /// list it is drawn from.
    fn folder_ids(&self) -> Vec<u64> {
        self.folders.all().iter().map(|folder| folder.id).collect()
    }

    /// One folder: its heading, and the assets in it.
    fn folder_rows(
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

        // While the name is being typed the heading is the editor, and what is in the
        // folder is drawn under it rather than hidden for the length of the gesture.
        if self.rename.as_ref().is_some_and(|r| r.what == item) {
            if let Some(name) = self.rename_row(ui, &name) {
                acts.push(Act::RenameFolder { id, name });
            }
            ui.indent(("folder_body", id), |ui| {
                for entity in members.iter().filter_map(|id| workspace.get(*id)) {
                    self.local_row(ui, entity, acts);
                }
            });
            return;
        }

        let title = format!("{name}  ·  {}", members.len());
        let drawn = egui::CollapsingHeader::new(egui::RichText::new(title).strong())
            .id_salt(("folder", id))
            .default_open(true)
            .show(ui, |ui| {
                if members.is_empty() {
                    ui.label(egui::RichText::new("empty — drag sounds in").small().weak());
                }
                for entity in members.iter().filter_map(|id| workspace.get(*id)) {
                    self.local_row(ui, entity, acts);
                }
            });

        let head = drawn.header_response;
        self.drop_zone(ui, &head, Onto::Group(id), acts);
        if head.clicked() {
            self.select(item);
        }
        let sendable = members
            .iter()
            .filter_map(|id| workspace.get(*id))
            .filter(|entity| owed(entity).is_some())
            .count();
        head.context_menu(|ui| {
            self.select(item);
            // ⚠️ The count is the whole point of putting it there. "Send all" up in the
            // heading writes what is **waiting**; this writes everything in the folder
            // that came off a slot, waiting or not — two scopes behind one verb, and the
            // number is what tells them apart before the modal does.
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

    fn local_row(
        &mut self,
        ui: &mut egui::Ui,
        entity: &crate::workspace::LocalEntity,
        acts: &mut Vec<Act>,
    ) {
        let item = Item::Local(entity.id);
        let kind = Kind::of(entity.entity.as_ref());
        let selected = self.selection == Some(item);

        // While a name is being typed the row stops sensing anything: a drag sense over
        // the field would take the clicks that place the cursor in it.
        if self.rename.as_ref().is_some_and(|r| r.what == item) {
            if let Some(name) = self.rename_row(ui, &entity.name) {
                acts.push(Act::RenameLocal {
                    id: entity.id,
                    name,
                });
            }
            return;
        }

        let owed = entity.pending.then(|| destination(entity)).flatten();
        let filed = self.folders.holding(entity.id);
        let drawn = row(
            ui,
            selected,
            &Cells {
                name: &entity.name,
                note: owed.as_deref().or(Some(kind.chip())),
                dirty: entity.dirty,
                waiting: owed.is_some(),
                ..Cells::default()
            },
        );
        let response = drawn.response;

        if response.dragged() {
            egui::DragAndDrop::set_payload(
                ui.ctx(),
                Carried {
                    from: item,
                    kind,
                    name: entity.name.clone(),
                    filed,
                },
            );
        }
        // A drop onto a row is a drop onto the list; it is taken here so the column's
        // own zone does not act on it a second time.
        self.drop_zone(ui, &response, Onto::Computer, acts);

        if response.double_clicked() {
            acts.push(Act::Open(item));
        } else if response.clicked() {
            self.clicked(item, &response, drawn.name, &entity.name);
        }
        if selected && ui.input(|i| i.key_pressed(egui::Key::F2)) {
            self.start_rename(item, &entity.name);
        }

        response.context_menu(|ui| {
            self.select(item);
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
            ui.separator();
            if ui.button("Remove from list").clicked() {
                acts.push(Act::Remove(entity.id));
                ui.close();
            }
        });
    }

    /// Where an asset can be put, for the operators who would rather pick than drag.
    ///
    /// A submenu rather than a modal: the folders are a short list and the answer is one
    /// of them, so there is nothing to type and nothing to confirm.
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

    /// What a plain click on a row does.
    ///
    /// ⚠️ Arming the rename editor needs the click to land on the **name**, not merely
    /// on a row that was already selected. An editor armed by any second click sits
    /// there with the whole name selected, so the next keystroke — one meant for the
    /// document, or a stray one — replaces it, and the blur commits the replacement.
    /// That is how a program came to be called "0".
    fn clicked(&mut self, item: Item, response: &egui::Response, name: egui::Rect, from: &str) {
        let on_name = response
            .interact_pointer_pos()
            .is_some_and(|at| name.contains(at));
        match arms_rename(self.selection == Some(item), on_name) {
            true => self.start_rename(item, from),
            false => self.select(item),
        }
    }

    // ---- the instrument ---------------------------------------------------------

    fn instrument(
        &mut self,
        ui: &mut egui::Ui,
        workspace: &Workspace,
        device: &Device,
        acts: &mut Vec<Act>,
    ) {
        let Some(product) = device.state.product().map(str::to_string) else {
            return;
        };
        let mut disconnect = false;
        let mut send_all = false;
        let mut sync = false;
        // The slots something is looking at right now, which the list on this computer
        // deliberately does not show.
        let viewed: Vec<(ObjectClass, Location)> = workspace
            .entities()
            .iter()
            .filter(|entity| !entity.kept)
            .filter_map(|entity| entity.origin.slot())
            .collect();
        let owed = workspace.pending().len();
        let firmware = device.state.firmware();
        let reading = BROWSED
            .iter()
            .filter_map(|class| device.state.scan.progress(*class))
            .any(|progress| progress.running);
        self.heading(ui, &product, |ui| {
            dot(ui, crate::app::good(ui.visuals())).on_hover_text("attached");
            if let Some(firmware) = &firmware {
                ui.label(egui::RichText::new(firmware).small().weak())
                    .on_hover_text("the firmware version the instrument reports");
            }
            // One button for the whole column. A walk reads a class's counters, its
            // banks and the slot the panel is on all at the head of its own session, so
            // there is nothing a per-folder button could ask for that this does not.
            sync = ui
                .add_enabled(!reading, egui::Button::new("Sync").small())
                .on_hover_text("read the whole instrument again")
                .on_disabled_hover_text("already reading")
                .clicked();
            disconnect = ui.small_button("Disconnect").clicked();
            if owed > 0 {
                send_all = ui
                    .button(egui::RichText::new(format!("Send all ({owed})")).strong())
                    .on_hover_text("write every waiting sound back to the instrument")
                    .clicked();
            }
        });
        if disconnect {
            acts.push(Act::Disconnect);
        }
        if sync {
            acts.push(Act::Resync);
        }
        if send_all {
            let waiting: Vec<u64> = workspace.pending().iter().map(|e| e.id).collect();
            let title = match waiting.len() {
                1 => "Send 1 sound to the instrument?".to_string(),
                n => format!("Send {n} sounds to the instrument?"),
            };
            self.ask_send(workspace, device, &waiting, title, Act::SendAll);
        }
        egui::ScrollArea::vertical()
            .id_salt("instrument_scroll")
            .auto_shrink([false; 2])
            .show(ui, |ui| {
                self.about(ui, device);
                for class in BROWSED {
                    self.class(ui, device, class, &viewed, acts);
                }
            });
    }

    /// What the instrument said about itself, for the times that is the question.
    ///
    /// Read-only and asked for once, at connect: the descriptors, and the endpoint-0
    /// identity the desktop transport can reach. Nothing here opens a session.
    fn about(&self, ui: &mut egui::Ui, device: &Device) {
        let Some(card) = device.state.card() else {
            return;
        };
        egui::CollapsingHeader::new(egui::RichText::new("About this instrument").small())
            .id_salt("instrument_about")
            .default_open(false)
            .show(ui, |ui| {
                let mut fact = |what: &str, value: Option<String>| {
                    ui.horizontal(|ui| {
                        ui.label(egui::RichText::new(what).small().weak());
                        match value {
                            Some(value) => {
                                ui.label(egui::RichText::new(value).small().monospace());
                            }
                            // Named rather than dropped: an absent line reads as a fact
                            // nobody thought to show.
                            None => {
                                ui.label(
                                    egui::RichText::new("not asked for on this build")
                                        .small()
                                        .weak()
                                        .italics(),
                                );
                            }
                        }
                    });
                };
                fact("product", Some(card.product.clone()));
                fact("maker", card.manufacturer.clone());
                fact(
                    "usb",
                    Some(format!("{:04x}:{:04x}", card.vendor_id, card.product_id)),
                );
                fact("serial", card.serial.clone());
                fact(
                    "interface",
                    card.interface.map(|held| format!("{held} (vendor)")),
                );
                fact("firmware", device.state.firmware());
                fact("build", card.build.map(|held| held.to_string()));
                fact("kind", card.kind.map(|held| format!("{held:#06x}")));
                fact(
                    "max transfer",
                    card.max_transfer.map(|held| format!("{held} bytes")),
                );
                ui.label(
                    egui::RichText::new(
                        "The build and kind words are what the device answers at their \
                         requests; what they mean is not pinned down.",
                    )
                    .small()
                    .weak()
                    .italics(),
                );
            });
    }

    fn class(
        &mut self,
        ui: &mut egui::Ui,
        device: &Device,
        class: ObjectClass,
        viewed: &[(ObjectClass, Location)],
        acts: &mut Vec<Act>,
    ) {
        let progress = device.state.scan.progress(class);
        // The heading carries how full the folder is, so the column reads as an index of
        // the instrument rather than as six words that all have to be opened to mean
        // anything.
        let title = match progress {
            Some(p) if p.running => match p.total {
                Some(total) => format!("{}  ·  reading {} of {total}", folder(class), p.done + 1),
                None => format!("{}  ·  reading…", folder(class)),
            },
            _ => match occupancy(class, &device.state.inventory) {
                Some(held) => format!("{}  ·  {held}", folder(class)),
                None => folder(class).to_string(),
            },
        };
        let focus = device.state.focused(class);
        // A jump wins over whatever the heading was left in: the point of it is to reach
        // a slot that is inside something closed.
        let heading = egui::CollapsingHeader::new(title);
        let heading = match self.jump.is_some_and(|(held, _)| held == class) {
            true => heading.open(Some(true)),
            false => heading.default_open(matches!(class, ObjectClass::Program)),
        };
        let drawn = heading.id_salt(class.to_raw()).show(ui, |ui| {
            ui.horizontal_wrapped(|ui| {
                if read_only(class) {
                    ui.label(egui::RichText::new("read only").small().weak());
                }
                if let Some(at) = focus {
                    if ui
                        .small_button("Go to loaded")
                        .on_hover_text(format!("the panel is on {}", shown(at)))
                        .clicked()
                    {
                        self.jump = Some((class, at));
                    }
                }
            });
            let banks = device.state.banks_of(class);
            if banks.is_empty() {
                ui.label(egui::RichText::new("nothing read yet").small().weak());
            }
            // A class that divides into one bank is a numbering with nothing to number,
            // and a heading over the whole of it is a line to click through for nothing.
            // The live buffer and the settings singleton are both this.
            let cut = banks.len() > 1;
            for bank in banks {
                self.bank(ui, device, class, bank, cut, viewed, acts);
            }
        });
        // ⚠️ A jump at a slot the walk has never reached would hold the heading open for
        // as long as the instrument stays attached: nothing draws the row that clears it.
        if self
            .jump
            .is_some_and(|(held, at)| held == class && device.state.slot(class, at).is_none())
        {
            self.jump = None;
        }
        drawn.header_response.context_menu(|ui| {
            if ui
                .button("Read this folder again")
                .on_hover_text("Sync reads the whole instrument; this reads one folder")
                .clicked()
            {
                acts.push(Act::ReadAgain(class));
                ui.close();
            }
        });
    }

    /// One bank, as a heading over its own slots.
    ///
    /// ⚠️ A container in the browser and a numbering everywhere else. The instrument has
    /// no folders inside a class — a location is a bank and a slot and that is all — but
    /// four hundred rows in one run is a column nobody can navigate, so the numbering is
    /// what the list is cut on. A bank the device named says so in its heading; for
    /// pianos those names are the panel's categories.
    #[allow(clippy::too_many_arguments)]
    fn bank(
        &mut self,
        ui: &mut egui::Ui,
        device: &Device,
        class: ObjectClass,
        bank: u32,
        cut: bool,
        viewed: &[(ObjectClass, Location)],
        acts: &mut Vec<Act>,
    ) {
        let Some(slots) = device.state.bank(class, bank) else {
            return;
        };
        let count = slots.len();
        let held = slots.iter().filter(|slot| slot.is_some()).count();
        let name = device
            .state
            .bank_name(class, bank)
            .filter(|name| worth_captioning(bank, name))
            .map(str::to_string);
        let mut rows = |browser: &mut Browser, ui: &mut egui::Ui| {
            for index in 0..count {
                let at = Location::from_user(bank, index as u32 + 1);
                browser.slot_row(ui, device, class, at, viewed, acts);
            }
        };
        if !cut {
            if let Some(name) = &name {
                ui.label(egui::RichText::new(name).small().weak());
            }
            return rows(self, ui);
        }

        let title = match &name {
            Some(name) => format!("{bank} · {name}  ·  {held}/{count}"),
            None => format!("Bank {bank}  ·  {held}/{count}"),
        };
        let focused = device
            .state
            .focused(class)
            .is_some_and(|at| at.bank + 1 == bank);
        let jumping = self
            .jump
            .is_some_and(|(held, at)| held == class && at.bank + 1 == bank);
        // Open where the panel is, closed everywhere else: an operator arrives at this
        // column knowing what the instrument is playing and nothing else, and eight
        // banks of fifty in one run is a list nobody can navigate.
        let heading = egui::CollapsingHeader::new(title);
        let heading = match jumping {
            true => heading.open(Some(true)),
            false => heading.default_open(focused),
        };
        heading
            .id_salt(("bank", class.to_raw(), bank))
            .show(ui, |ui| rows(self, ui));
    }

    fn slot_row(
        &mut self,
        ui: &mut egui::Ui,
        device: &Device,
        class: ObjectClass,
        at: Location,
        viewed: &[(ObjectClass, Location)],
        acts: &mut Vec<Act>,
    ) {
        let held = device
            .state
            .slot(class, at)
            .flatten()
            .map(|info| info.name.trim().to_string());
        let item = Item::Slot { class, at };
        let selected = self.selection == Some(item);

        // While a name is being typed the row stops sensing anything: a drag sense over
        // the field would take the clicks that place the cursor in it.
        if self.rename.as_ref().is_some_and(|r| r.what == item) {
            let was = held.clone().unwrap_or_default();
            if let Some(name) = self.rename_row(ui, &was) {
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
                at: Some(shown(at)),
                name: held.as_deref().unwrap_or("empty"),
                note: viewing.then_some("open"),
                faint: held.is_none(),
                loaded,
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
        // The one place a jump lands: the row itself, once the headings above it have
        // been forced open and it has a rectangle to be scrolled to.
        if self.jump == Some((class, at)) {
            self.jump = None;
            self.selection = Some(item);
            response.scroll_to_me(Some(egui::Align::Center));
        }

        // ⚠️ A piano is a library of hundreds of megabytes, read whole into memory by
        // anything that fetches it. The folder lists what is installed and offers no way
        // to pull one down, which is also what read-only means here.
        let fetchable = !read_only(class);

        if let Some(name) = &held {
            if fetchable && response.dragged() {
                egui::DragAndDrop::set_payload(
                    ui.ctx(),
                    Carried {
                        from: item,
                        kind: Kind::from_class(class),
                        name: name.clone(),
                        filed: None,
                    },
                );
            }
        }
        self.drop_zone(ui, &response, Onto::Slot { class, at }, acts);

        if response.double_clicked() {
            if held.is_some() && fetchable {
                acts.push(Act::Open(item));
            }
        } else if response.clicked() {
            match (&held, fetchable) {
                (Some(name), true) => self.clicked(item, &response, drawn.name, name),
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
        response.context_menu(|ui| {
            self.select(item);
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
            let free = device.state.first_free(class);
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

    // ---- shared pieces ----------------------------------------------------------

    /// The in-place editor, prefilled and selected.
    ///
    /// ⚠️ **Only Enter renames.** Clicking away cancels. An editor that commits on blur
    /// turns a stray keystroke into a rename nobody asked for, and the name is the only
    /// record of what an object is — files store no name of their own.
    fn rename_row(&mut self, ui: &mut egui::Ui, original: &str) -> Option<String> {
        let rename = self.rename.as_mut()?;
        let output = ui
            .horizontal(|ui| {
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
        // ⚠️ The field's own Enter, not the frame's. `Ui::input` answers for the whole
        // app: an Enter meant for a knob's number box or a table cell reads as one here
        // too, and would close an editor the operator has not finished with. A single
        // line surrenders the focus on Enter, so the two together are the gesture.
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
    fn drop_zone(
        &mut self,
        ui: &egui::Ui,
        response: &egui::Response,
        onto: Onto,
        acts: &mut Vec<Act>,
    ) {
        if let Some(carried) = response.dnd_hover_payload::<Carried>() {
            if landing(&carried, onto).allowed() {
                ui.painter().rect_stroke(
                    response.rect,
                    3.0,
                    egui::Stroke::new(1.0, ui.visuals().selection.stroke.color),
                    egui::StrokeKind::Inside,
                );
            }
        }
        let Some(carried) = response.dnd_release_payload::<Carried>() else {
            return;
        };
        self.land(&carried, onto, acts);
    }

    fn land(&mut self, carried: &Arc<Carried>, onto: Onto, acts: &mut Vec<Act>) {
        match (landing(carried, onto), carried.from, onto) {
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
            (Landing::No(why), ..) => acts.push(Act::Refused(format!(
                "“{}” cannot go there — {why}.",
                carried.name
            ))),
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
                    acts.push(ask.act);
                }
            }
            Some(false) => self.ask = None,
            None => {}
        }
    }

    /// The one question a batch asks: everything it is about to write, and what it
    /// would replace.
    ///
    /// One question for every batch there is — the whole queue, or one folder's worth —
    /// so a folder cannot become a way of writing to the instrument without being asked.
    fn ask_send(
        &mut self,
        workspace: &Workspace,
        device: &Device,
        ids: &[u64],
        title: String,
        act: Act,
    ) {
        let mut lines = Vec::new();
        let mut warnings: Vec<String> = Vec::new();
        for entity in ids.iter().filter_map(|id| workspace.get(*id)) {
            let Some((class, at)) = owed(entity) else {
                continue;
            };
            let where_ = place(class, at);
            if let Some(warning) = foreign_format(&entity.tag(), &device.state.formats_in(class)) {
                if !warnings.contains(&warning) {
                    warnings.push(warning);
                }
            }
            // Naming the occupant is the whole point of asking: a batch is where an
            // operator stops reading each destination for themselves.
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
        // The warnings first: they are the reason to say no, and a list of destinations
        // is what the eye slides down.
        let mut note = warnings;
        if !note.is_empty() {
            note.push(String::new());
        }
        note.extend(lines);
        self.ask = Some(Ask {
            title,
            note: Some(note.join("\n")),
            verb: "Send",
            act,
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
            act,
        });
    }
}

/// What one row of a list shows.
#[derive(Default)]
pub struct Cells<'a> {
    /// The monospace location column, `7:4`. Assets on this computer have none.
    pub at: Option<String>,
    pub name: &'a str,
    /// A faint word after the name — what kind of thing it is, or where it is owed.
    pub note: Option<&'a str>,
    /// The note is a destination rather than a kind, so it is worth noticing.
    pub waiting: bool,
    /// The name is a stand-in rather than a real one.
    pub faint: bool,
    pub dirty: bool,
    /// The instrument's panel has this slot loaded.
    pub loaded: bool,
}

/// A drawn row: what it answered, and where its name ended up.
pub struct Drawn {
    pub response: egui::Response,
    /// The name's own rectangle — the only sub-area of a row that means anything, and
    /// only because clicking the name of a selected row starts a rename.
    pub name: egui::Rect,
}

/// The width the location column takes, so names line up under each other.
const AT_W: f32 = 42.0;

/// One row of a list: a full-width click target with its text painted into it.
///
/// ⚠️ Nothing inside is a widget. A label allocates a hover rect of its own, which then
/// wins the hit test over the row — the highlight drops out as the pointer crosses the
/// text, and clicks land on whichever word happens to be under them. The row is the only
/// thing that senses.
fn row(ui: &mut egui::Ui, selected: bool, cells: &Cells) -> Drawn {
    let height = ui.text_style_height(&egui::TextStyle::Body) + 4.0;
    let (rect, response) = ui.allocate_exact_size(
        egui::vec2(ui.available_width(), height),
        egui::Sense::click_and_drag(),
    );

    let visuals = ui.visuals();
    let fill = match (selected, response.hovered()) {
        (true, _) => Some(visuals.selection.bg_fill),
        (false, true) => Some(visuals.faint_bg_color),
        (false, false) => None,
    };
    // ⚠️ A selected row is painted in the instrument's red, and the body grey a row is
    // otherwise written in does not survive it. The colour that goes with the fill is the
    // one egui keeps beside it.
    let ink = match selected {
        true => visuals.selection.stroke.color,
        false => visuals.text_color(),
    };
    let weak = match selected {
        true => ink.gamma_multiply(visuals.weak_text_alpha),
        false => visuals.weak_text_color(),
    };
    let strong = match cells.faint {
        true => weak,
        false => ink,
    };
    let painter = ui.painter().clone();
    if let Some(fill) = fill {
        painter.rect_filled(rect, 3.0, fill);
    }

    let mut x = rect.left() + 4.0;
    let gutter = egui::pos2(x + 3.5, rect.center().y);
    // One gutter, two marks that never meet: only a local asset is dirty, and only a slot
    // is loaded on the panel. A ring rather than a second disc, because two dots that
    // differ in nothing but colour read as the same mark.
    if cells.dirty {
        painter.circle_filled(gutter, 3.5, crate::app::warn(ui.visuals()));
    } else if cells.loaded {
        painter.circle_stroke(
            gutter,
            3.0,
            egui::Stroke::new(1.5, crate::app::good(ui.visuals())),
        );
    }
    x += 10.0;

    if let Some(at) = &cells.at {
        let galley = painter.layout_no_wrap(at.clone(), egui::FontId::monospace(11.0), weak);
        painter.galley(
            egui::pos2(x, rect.center().y - galley.size().y / 2.0),
            galley,
            egui::Color32::PLACEHOLDER,
        );
        x += AT_W;
    }

    let font = egui::FontId::proportional(13.0);
    let galley = painter.layout_no_wrap(cells.name.to_string(), font.clone(), strong);
    let at = egui::pos2(x, rect.center().y - galley.size().y / 2.0);
    let name = egui::Rect::from_min_size(at, galley.size());
    x += galley.size().x + 8.0;
    painter.galley(at, galley, egui::Color32::PLACEHOLDER);

    if let Some(note) = cells.note {
        let galley =
            painter.layout_no_wrap(note.to_string(), egui::FontId::proportional(10.0), weak);
        painter.galley(
            egui::pos2(x, rect.center().y - galley.size().y / 2.0),
            galley,
            egui::Color32::PLACEHOLDER,
        );
    }
    Drawn { response, name }
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

/// Where an asset is owed, for the badge that says so.
fn destination(entity: &crate::workspace::LocalEntity) -> Option<String> {
    let (class, at) = entity.origin.slot()?;
    Some(format!("will be sent to {}", place(class, at)))
}

/// The name of whatever is being dragged, following the pointer.
fn ghost(ctx: &egui::Context) {
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

/// Run what the browser asked for.
pub fn apply(
    browser: &mut Browser,
    acts: Vec<Act>,
    workspace: &mut Workspace,
    device: &mut Device,
    tabs: &mut Tabs,
    log: &mut Log,
) {
    for act in acts {
        match act {
            Act::Connect => device.connect(log),
            Act::Disconnect => device.disconnect(log),
            Act::OpenFiles => workspace.open_dialog(),
            Act::New(kind) => {
                if let Some(id) = workspace.create(kind, log) {
                    tabs.open(id, workspace);
                }
            }
            Act::Resync => {
                device.resync();
                log.say("Reading the instrument again…");
            }
            Act::ReadAgain(class) => device.read_class(class),
            Act::Keep(id) => workspace.keep(id, log),
            Act::NewFolder => {
                let id = browser.folders.make();
                // ⚠️ The name `make` settled on, not the one it starts from. Prefilling
                // the editor with "New folder" beside an existing "New folder" is an
                // Enter away from two folders of one name — which is exactly what
                // `make` picked a different one to avoid.
                let name = browser.folders.name_of(id).unwrap_or_default().to_string();
                browser.start_rename(Item::Folder(id), &name);
            }
            Act::RemoveFolder(id) => {
                // ⚠️ The editor goes with it. A folder being renamed draws the editor in
                // place of its heading, and a removed folder draws nothing at all — so a
                // rename left armed here is one no row will ever close, and the next
                // folder to take this id inherits it.
                browser.forget_rename(Item::Folder(id));
                browser.folders.remove(id);
            }
            Act::File { id, folder } => browser.folders.file(id, folder),
            Act::SendFolder(id) => {
                let members: Vec<u64> = browser
                    .folders
                    .members(id, workspace)
                    .iter()
                    .map(|entity| entity.id)
                    .collect();
                send_batch(&members, workspace, device, log);
            }
            Act::Open(Item::Folder(_)) => {}
            Act::Open(Item::Local(id)) => tabs.open(id, workspace),
            // ⚠️ One view per slot. A second read of a slot already being viewed would
            // be two working copies of one place — edited apart, both owed back to it,
            // and both queued into a single batch, where the last one written wins.
            Act::Open(Item::Slot { class, at }) => match workspace.view_of(class, at) {
                Some(id) => tabs.open(id, workspace),
                None => device.send(
                    DeviceCmd::Get {
                        class,
                        at,
                        body: false,
                        open: true,
                    },
                    log,
                ),
            },
            Act::Copy { class, at } => device.send(
                DeviceCmd::Get {
                    class,
                    at,
                    body: false,
                    open: false,
                },
                log,
            ),
            Act::LoadOnInstrument { class, at } => {
                device.send(DeviceCmd::Select { class, at }, log)
            }
            Act::Send { id, class, at } => {
                send(browser, workspace, device, log, id, class, at, true)
            }
            Act::Replace { id, class, at } => {
                send(browser, workspace, device, log, id, class, at, false)
            }
            Act::SendAll => {
                let waiting: Vec<u64> = workspace.pending().iter().map(|e| e.id).collect();
                send_batch(&waiting, workspace, device, log);
            }
            Act::Rearrange { class, from, to } => {
                device.send(DeviceCmd::Move { class, from, to }, log)
            }
            Act::RenameLocal { id, name } => {
                workspace.rename(id, name.clone());
                log.say(format!("Renamed it “{name}”."));
            }
            Act::RenameFolder { id, name } => browser.folders.rename(id, name),
            Act::RenameSlot { class, at, name } => {
                device.send(DeviceCmd::Rename { class, at, name }, log)
            }
            Act::DuplicateLocal(id) => {
                workspace.duplicate(id, log);
            }
            Act::DuplicateSlot { class, from, to } => {
                device.send(DeviceCmd::Duplicate { class, from, to }, log)
            }
            Act::DeleteSlot { class, at } => device.send(DeviceCmd::Delete { class, at }, log),
            Act::Remove(id) => {
                tabs.close(id);
                browser.folders.forget(id);
                workspace.remove(id, log);
            }
            Act::Save(id) => workspace.export(id, ExportWhat::File),
            Act::Refused(why) => log.say(why),
        }
    }
}

/// Write a set of assets back, one command per folder.
///
/// The one write path a batch takes, whether the batch is everything waiting or one of
/// this computer's own folders: same refusal, same grouping, same per-item flow.
fn send_batch(ids: &[u64], workspace: &Workspace, device: &mut Device, log: &mut Log) {
    // Refused before the transport is touched: bytes that are not what they claim to be
    // must not reach a delete-then-write, and one bad item stops the whole batch rather
    // than being skipped past.
    for entity in ids.iter().filter_map(|id| workspace.get(*id)) {
        if owed(entity).is_none() {
            continue;
        }
        if let Err(e) = nord_usb::envelope::unwrap(&entity.bytes) {
            log.error(format!("{}: {e}", entity.name));
            log.trouble(format!(
                "“{}” is not a file the instrument takes, so nothing was sent.",
                entity.name
            ));
            return;
        }
    }
    for (class, items) in grouped(ids, workspace) {
        device.send(DeviceCmd::SendAll { class, items }, log);
    }
}

/// The assets named, gathered per folder in the order the list holds them.
///
/// A session belongs to a folder, so a folder is the unit a batch is cut into.
fn grouped(ids: &[u64], workspace: &Workspace) -> Vec<(ObjectClass, Vec<Outgoing>)> {
    let mut by_class: Vec<(ObjectClass, Vec<Outgoing>)> = Vec::new();
    for entity in ids.iter().filter_map(|id| workspace.get(*id)) {
        let Some((class, at)) = owed(entity) else {
            continue;
        };
        let item = Outgoing {
            id: entity.id,
            at,
            name: entity.name.clone(),
            bytes: entity.bytes.clone(),
        };
        match by_class.iter_mut().find(|(held, _)| *held == class) {
            Some((_, items)) => items.push(item),
            None => by_class.push((class, vec![item])),
        }
    }
    by_class
}

/// Whether an outgoing file is of a format the destination folder has never been seen
/// holding, and the sentence saying so.
///
/// ⚠️ **A warning, never a refusal.** A write is a delete followed by a write, so a file
/// the instrument turns out not to want costs the occupant of the slot — and the New
/// menu now makes another model's program one click away. But nothing here has watched
/// an instrument refuse one, so this reports what the scan can see and leaves the
/// decision where it belongs.
///
/// `resident` is what the walk actually read, so an unscanned folder says nothing rather
/// than guessing: not known is not the same as does not match.
pub fn foreign_format(outgoing: &str, resident: &[String]) -> Option<String> {
    let outgoing = outgoing.trim();
    // ⚠️ `?` is what this app calls bytes that told it nothing — not a format that
    // differs from every other. A file whose own tag could not be read is one there is
    // nothing true to say about, and saying it anyway is a false alarm on exactly the
    // files an operator is least sure of.
    let readable = !outgoing.is_empty() && outgoing.chars().all(|c| c.is_ascii_alphanumeric());
    if !readable || resident.is_empty() {
        return None;
    }
    if resident
        .iter()
        .any(|held| held.trim().eq_ignore_ascii_case(outgoing))
    {
        return None;
    }
    let held: Vec<&str> = resident.iter().map(|held| held.trim()).collect();
    Some(format!(
        "⚠️ This file is {outgoing}; everything read in that folder is {}. Sending it \
         deletes what is there first.",
        held.join(" or "),
    ))
}

/// Where an asset would be written back to, if anywhere: the slot it came off, and only
/// where this app will write into that class at all.
fn owed(entity: &LocalEntity) -> Option<(ObjectClass, Location)> {
    let (class, at) = entity.origin.slot()?;
    crate::device::sendable(class).then_some((class, at))
}

/// Put a local asset into a slot.
///
/// `ask` is false once the replace question has been answered, which is what keeps the
/// answer from raising the question again.
#[allow(clippy::too_many_arguments)]
fn send(
    browser: &mut Browser,
    workspace: &Workspace,
    device: &mut Device,
    log: &mut Log,
    id: u64,
    class: ObjectClass,
    at: Location,
    ask: bool,
) {
    let Some(entity) = workspace.get(id) else {
        return;
    };
    // Refused before the transport is touched: bytes that are not what they claim to be
    // must not reach a delete-then-write.
    if let Err(e) = nord_usb::envelope::unwrap(&entity.bytes) {
        log.error(format!("{}: {e}", entity.name));
        log.trouble(format!(
            "“{}” is not a file the instrument takes.",
            entity.name
        ));
        return;
    }
    let occupant = device
        .state
        .slot(class, at)
        .flatten()
        .map(|info| info.name.trim().to_string());
    match (ask, occupant) {
        (true, Some(occupant)) => browser.ask_replace(
            &occupant,
            &entity.name,
            place(class, at),
            foreign_format(&entity.tag(), &device.state.formats_in(class)),
            Act::Replace { id, class, at },
        ),
        _ => device.send(
            DeviceCmd::Put {
                id,
                class,
                at,
                name: entity.name.clone(),
                bytes: entity.bytes.clone(),
            },
            log,
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn local(kind: Kind) -> Carried {
        Carried {
            from: Item::Local(1),
            kind,
            name: "Africa Split".into(),
            filed: None,
        }
    }

    fn slot(class: ObjectClass, bank: u32, slot: u32) -> Carried {
        Carried {
            from: Item::Slot {
                class,
                at: Location { bank, slot },
            },
            kind: Kind::from_class(class),
            name: "Squabble B".into(),
            filed: None,
        }
    }

    fn onto(class: ObjectClass, bank: u32, at: u32) -> Onto {
        Onto::Slot {
            class,
            at: Location { bank, slot: at },
        }
    }

    /// Everything an act needs run against it.
    fn bench() -> (Browser, Workspace, Device, Tabs, crate::log::Log) {
        let ctx = egui::Context::default();
        (
            Browser::default(),
            Workspace::new(ctx.clone()),
            Device::new(ctx),
            Tabs::default(),
            crate::log::Log::default(),
        )
    }

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

    /// Pianos are listed and never written; live and settings have no proven write path.
    #[test]
    fn the_folders_that_cannot_be_written_refuse_a_drop() {
        for class in [ObjectClass::Piano, ObjectClass::Live, ObjectClass::Settings] {
            let kind = Kind::from_class(class);
            assert!(
                !landing(&local(kind), onto(class, 0, 0)).allowed(),
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
            landing(&local(Kind::Live), onto(ObjectClass::Live, 0, 0)),
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

    /// Paint the two columns headlessly. What this catches is a layout that panics or
    /// an id that collides, neither of which a unit test on the rules would see.
    fn paint(with_device: bool) {
        use crate::workspace::{Fresh, Origin};

        let ctx = egui::Context::default();
        let mut workspace = Workspace::new(ctx.clone());
        let mut device = Device::new(ctx.clone());
        let mut log = crate::log::Log::default();
        let mut tabs = Tabs::default();
        let mut browser = Browser::default();

        for kind in [Fresh::Program, Fresh::Live, Fresh::Settings] {
            workspace.create(kind, &mut log).unwrap();
        }
        // A folder with something in it, one with nothing, and a view of a slot: three
        // row shapes the list has no other way of reaching.
        let full = browser.folders.make();
        browser.folders.make();
        let filed = workspace.create(Fresh::Program, &mut log).unwrap();
        browser.folders.file(filed, Some(full));
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
        }

        // Twice: the second pass runs with the widget state the first left behind.
        for _ in 0..2 {
            let _ = ctx.run(egui::RawInput::default(), |ctx| {
                egui::SidePanel::left("places").show(ctx, |ui| {
                    let acts = browser.ui(ui, &workspace, &device);
                    apply(
                        &mut browser,
                        acts,
                        &mut workspace,
                        &mut device,
                        &mut tabs,
                        &mut log,
                    );
                });
            });
        }
    }

    /// A batch is one command per folder, because a session belongs to a folder — and
    /// something that cannot be written is not queued at all.
    #[test]
    fn a_batch_is_grouped_into_one_command_per_folder() {
        use crate::workspace::{Fresh, Origin};

        let ctx = egui::Context::default();
        let mut workspace = Workspace::new(ctx.clone());
        let mut log = crate::log::Log::default();
        let bytes = {
            let id = workspace.create(Fresh::Program, &mut log).unwrap();
            let bytes = workspace.get(id).unwrap().bytes.clone();
            workspace.remove(id, &mut log);
            bytes
        };
        let at = |slot| Location { bank: 6, slot };
        for (class, slot) in [
            (ObjectClass::Program, 0),
            (ObjectClass::Program, 1),
            (ObjectClass::SetList, 0),
            // Live refuses a write, so it must not reach the queue.
            (ObjectClass::Live, 0),
        ] {
            let id = workspace.ingest(
                format!("{}.ne5p", place(class, at(slot))),
                Origin::Device {
                    class,
                    at: at(slot),
                },
                bytes.clone(),
                &mut log,
            );
            workspace.mark_pending(id, true);
        }

        let waiting: Vec<u64> = workspace.pending().iter().map(|e| e.id).collect();
        let queued = grouped(&waiting, &workspace);
        assert_eq!(queued.len(), 2, "one command per folder");
        let programs = queued
            .iter()
            .find(|(class, _)| *class == ObjectClass::Program)
            .expect("programs are queued");
        assert_eq!(programs.1.len(), 2);
        assert!(queued.iter().all(|(class, _)| *class != ObjectClass::Live));
    }

    /// A lone send names the asset it is sending, so the write can pay off that one
    /// document's debt the way a batch pays off its own.
    #[test]
    fn sending_one_document_names_the_asset_it_sends() {
        use crate::workspace::{Fresh, Origin};

        let ctx = egui::Context::default();
        let mut workspace = Workspace::new(ctx.clone());
        let mut device = Device::new(ctx);
        let mut log = crate::log::Log::default();
        let mut tabs = Tabs::default();
        let mut browser = Browser::default();
        device.pretend_scanned(ObjectClass::Program, 7, &["Africa Split"]);

        let at = Location { bank: 6, slot: 1 };
        let bytes = {
            let id = workspace.create(Fresh::Program, &mut log).unwrap();
            let bytes = workspace.get(id).unwrap().bytes.clone();
            workspace.remove(id, &mut log);
            bytes
        };
        let id = workspace.ingest(
            "Africa-Split.ne5p".into(),
            Origin::Device {
                class: ObjectClass::Program,
                at,
            },
            bytes,
            &mut log,
        );
        workspace.mark_pending(id, true);

        // The slot is empty in the scan, so nothing is asked and the put goes straight out.
        apply(
            &mut browser,
            vec![Act::Send {
                id,
                class: ObjectClass::Program,
                at,
            }],
            &mut workspace,
            &mut device,
            &mut tabs,
            &mut log,
        );
        let queued = device.queued().front().expect("a put was queued");
        match queued {
            DeviceCmd::Put { id: sending, .. } => assert_eq!(*sending, id),
            other => panic!("{}", other.label()),
        }
    }

    /// The divider does what dragging it says, and stops before either column is gone.
    ///
    /// ⚠️ The share is what is kept, not a width: the sidebar the two live in is itself
    /// resizable, so the same fraction has to survive the dock changing size under it.
    #[test]
    fn the_divider_moves_and_stops_short_of_squeezing_a_column_out() {
        use crate::workspace::Fresh;

        let ctx = egui::Context::default();
        let mut workspace = Workspace::new(ctx.clone());
        let mut device = Device::new(ctx.clone());
        let mut log = crate::log::Log::default();
        let mut browser = Browser::default();
        workspace.create(Fresh::Program, &mut log).unwrap();
        device.pretend_scanned(ObjectClass::Program, 7, &["Africa Split"]);

        // Far enough left to ask for more than the stop allows.
        let travel = -400.0;
        let button = |pos, pressed| egui::Event::PointerButton {
            pos,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: egui::Modifiers::default(),
        };

        let mut divider = egui::pos2(0.0, 0.0);
        let mut frame = 0;
        while frame < 5 {
            let grip = divider;
            let moved = grip + egui::vec2(travel, 0.0);
            let input = egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(1280.0, 720.0),
                )),
                events: match frame {
                    0 => Vec::new(),
                    1 => vec![egui::Event::PointerMoved(grip)],
                    2 => vec![button(grip, true)],
                    3 => vec![egui::Event::PointerMoved(moved)],
                    _ => vec![button(moved, false)],
                },
                ..Default::default()
            };
            let _ = ctx.run(input, |ctx| {
                egui::SidePanel::left("places")
                    .exact_width(600.0)
                    .show(ctx, |ui| {
                        // The same rect the dock lays itself out in, so the test grabs
                        // the divider where the divider actually is.
                        let whole = ui.available_rect_before_wrap();
                        divider = egui::pos2(
                            whole.left() + (whole.width() - HANDLE) * browser.split + HANDLE / 2.0,
                            whole.center().y,
                        );
                        let _ = browser.ui(ui, &workspace, &device);
                    });
            });
            frame += 1;
        }

        assert!(browser.split < EVEN, "it moved: {}", browser.split);
        assert_eq!(browser.split, LEAST, "and stopped at the stop");
    }

    /// A share the store cannot account for is the even split, never one column and a
    /// sliver of the other.
    #[test]
    fn a_divider_comes_back_where_it_was_left_or_not_at_all() {
        let restored = |held: Option<&str>| {
            let mut store = Fake::default();
            if let Some(held) = held {
                eframe::Storage::set_string(&mut store, Browser::SPLIT, held.to_string());
            }
            let mut browser = Browser::default();
            browser.restore(&store);
            browser.split
        };
        assert_eq!(restored(Some("0.3")), 0.3);
        assert_eq!(restored(None), EVEN);
        for nonsense in ["0.0", "1.0", "-3", "wide", "", "NaN"] {
            assert_eq!(restored(Some(nonsense)), EVEN, "{nonsense:?}");
        }

        // And what is written comes back as itself.
        let mut store = Fake::default();
        let browser = Browser {
            split: 0.42,
            ..Browser::default()
        };
        browser.keep(&mut store);
        let mut after = Browser::default();
        after.restore(&store);
        assert_eq!(after.split, 0.42);
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
    fn the_two_columns_paint_with_nothing_attached() {
        paint(false);
    }

    #[test]
    fn the_two_columns_paint_with_a_tree_to_show() {
        paint(true);
    }

    /// ⚠️ The gesture that lost a program its name. An editor armed by any second click
    /// on a selected row sits there with everything selected, so the next keystroke
    /// replaces the name and the blur commits it.
    #[test]
    fn a_click_away_from_the_name_selects_rather_than_arming_a_rename() {
        // The row is the click target, so most of it must be safe to click.
        assert!(!arms_rename(true, false), "past the name on a selected row");
        assert!(
            !arms_rename(false, true),
            "on the name of an unselected row"
        );
        assert!(!arms_rename(false, false));
        assert!(arms_rename(true, true), "the one gesture that renames");
    }

    /// The gesture end to end: arm the editor, type, press Enter, and the new name comes
    /// back as an act.
    ///
    /// What this catches is a rename that has stopped committing at all — the unit tests
    /// on [`renamed`] cannot see the field it is fed from, and the field's own idea of
    /// "the operator pressed Enter" is the part that is easy to get wrong.
    #[test]
    fn typing_a_name_and_pressing_enter_renames_the_row() {
        use crate::workspace::Fresh;

        let ctx = egui::Context::default();
        let mut workspace = Workspace::new(ctx.clone());
        let device = Device::new(ctx.clone());
        let mut log = crate::log::Log::default();
        let mut browser = Browser::default();
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
                    for act in browser.ui(ui, &workspace, &device) {
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

    /// A new folder is one nothing else is called, so two of them are two rows rather
    /// than one row twice.
    #[test]
    fn a_new_folder_gets_a_name_no_other_folder_is_using() {
        let mut folders = Folders::default();
        let names: Vec<String> = (0..3)
            .map(|_| {
                let id = folders.make();
                folders.name_of(id).expect("it was made").to_string()
            })
            .collect();
        assert_eq!(names, ["New folder", "New folder 2", "New folder 3"]);
        // And the ids are as distinct as the names.
        let ids: Vec<u64> = folders.all().iter().map(|folder| folder.id).collect();
        assert_eq!(ids, vec![1, 2, 3]);
    }

    /// A folder holds nothing, so losing one loses nothing: what was in it is back in
    /// the loose part of the list.
    #[test]
    fn removing_a_folder_leaves_what_was_in_it_on_this_computer() {
        let mut folders = Folders::default();
        let (kept, gone) = (folders.make(), folders.make());
        folders.file(7, Some(kept));
        folders.file(8, Some(gone));
        folders.remove(gone);

        assert_eq!(folders.holding(7), Some(kept));
        assert_eq!(folders.holding(8), None, "loose, not lost");
        // And a folder that never existed is not a place anything can be put.
        folders.file(9, Some(gone));
        assert_eq!(folders.holding(9), None);
    }

    /// The grouping comes back as it was left, empty folders included — and a membership
    /// naming a folder the file does not hold is dropped rather than hiding an asset in
    /// a folder nobody can open.
    #[test]
    fn the_folders_and_what_is_in_them_survive_a_session() {
        let mut folders = Folders::default();
        let (sunday, empty) = (folders.make(), folders.make());
        folders.rename(sunday, "Sunday\tmorning".into());
        folders.file(7, Some(sunday));
        folders.file(8, Some(sunday));

        let after = read(&written(&folders));
        assert_eq!(after.all().len(), 2, "an empty folder is still a folder");
        assert_eq!(after.name_of(sunday), Some("Sunday\tmorning"));
        assert_eq!(after.name_of(empty), Some("New folder 2"));
        assert_eq!(after.holding(7), Some(sunday));
        assert_eq!(after.holding(8), Some(sunday));

        // Nothing readable is no folders at all, never half a grouping.
        assert!(read("").all().is_empty());
        assert!(read("drawbar folders 99\nf\t1\tSunday\n").all().is_empty());
        let orphaned = read(&format!("{FOLDERS_VERSION}\nm\t7\t3\n"));
        assert_eq!(orphaned.holding(7), None);
    }

    /// The two stores are read separately and only the asset one decides what survived —
    /// anything too big to keep, or dropped for want of room, would otherwise leave its
    /// membership behind to accumulate for as long as the app is installed.
    #[test]
    fn a_grouping_forgets_the_assets_the_list_came_back_without() {
        let (mut browser, mut workspace, _device, _tabs, mut log) = bench();
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

    /// Sending a folder is the batch the queue already runs: the same grouping into one
    /// command per instrument folder, and the same refusal of anything that cannot be
    /// written.
    #[test]
    fn a_folder_sends_only_what_can_go_back_to_a_slot() {
        use crate::workspace::{Fresh, Origin};

        let ctx = egui::Context::default();
        let mut workspace = Workspace::new(ctx);
        let mut log = crate::log::Log::default();
        let bytes = {
            let id = workspace.create(Fresh::Program, &mut log).unwrap();
            let bytes = workspace.get(id).unwrap().bytes.clone();
            workspace.remove(id, &mut log);
            bytes
        };
        let at = |slot| Location { bank: 6, slot };
        let mut ids = Vec::new();
        for (class, slot) in [
            (ObjectClass::Program, 0),
            (ObjectClass::SetList, 0),
            // Live refuses a write, so it must not reach the queue.
            (ObjectClass::Live, 0),
        ] {
            ids.push(workspace.ingest(
                format!("{}.ne5p", place(class, at(slot))),
                Origin::Device {
                    class,
                    at: at(slot),
                },
                bytes.clone(),
                &mut log,
            ));
        }
        // Never off an instrument, so there is nowhere to send it back to.
        ids.push(workspace.create(Fresh::Program, &mut log).unwrap());

        let queued = grouped(&ids, &workspace);
        let classes: Vec<ObjectClass> = queued.iter().map(|(class, _)| *class).collect();
        assert_eq!(classes, vec![ObjectClass::Program, ObjectClass::SetList]);
        assert!(queued.iter().all(|(_, items)| items.len() == 1));
        // A folder holding nothing sendable queues nothing at all.
        assert!(grouped(&ids[2..], &workspace).is_empty());
    }

    /// A double-click on a slot opens a view: a tab and a document, and no new row in
    /// the list. Keeping it is what puts it there.
    #[test]
    fn opening_a_slot_does_not_put_it_on_this_computer() {
        use crate::device::DeviceEvent;
        use crate::workspace::{Fresh, Origin};

        let ctx = egui::Context::default();
        let mut workspace = Workspace::new(ctx.clone());
        let mut device = Device::new(ctx);
        let mut log = crate::log::Log::default();
        let mut tabs = Tabs::default();
        let mut browser = Browser::default();
        let bytes = {
            let id = workspace.create(Fresh::Program, &mut log).unwrap();
            let bytes = workspace.get(id).unwrap().bytes.clone();
            workspace.remove(id, &mut log);
            bytes
        };
        let at = Location { bank: 6, slot: 3 };
        let origin = Origin::Device {
            class: ObjectClass::Program,
            at,
        };

        device.pretend(DeviceEvent::Got {
            name: "Africa-Split.ne5p".into(),
            origin,
            bytes,
            open: true,
        });
        device.poll(&mut log, &mut workspace, &mut tabs);

        let id = tabs.active().expect("a view opens in a tab");
        assert!(workspace.is_view(id));
        assert_eq!(workspace.listed().count(), 0, "nothing joined the list");
        // It is still a working copy in every other way: it knows the slot it came off,
        // so Send back works from it.
        assert_eq!(
            workspace.get(id).unwrap().origin.slot(),
            Some((ObjectClass::Program, at))
        );

        apply(
            &mut browser,
            vec![Act::Keep(id)],
            &mut workspace,
            &mut device,
            &mut tabs,
            &mut log,
        );
        assert!(!workspace.is_view(id));
        assert_eq!(workspace.listed().count(), 1);
    }

    /// ⚠️ The editor opens on the name the folder actually has. Prefilling it with the
    /// name `make` starts from, beside a folder already called that, is one Enter away
    /// from two folders of one name — which is what `make` picked a different one to
    /// avoid.
    #[test]
    fn a_new_folder_opens_its_editor_on_the_name_it_was_given() {
        let (mut browser, mut workspace, mut device, mut tabs, mut log) = bench();
        let mut new_folder = |browser: &mut Browser| {
            apply(
                browser,
                vec![Act::NewFolder],
                &mut workspace,
                &mut device,
                &mut tabs,
                &mut log,
            );
            let rename = browser.rename.as_ref().expect("the editor is armed");
            let Item::Folder(id) = rename.what else {
                panic!("it is armed on the folder");
            };
            (id, rename.text.clone())
        };

        let (first, typed) = new_folder(&mut browser);
        assert_eq!(typed, "New folder");
        let (second, typed) = new_folder(&mut browser);
        assert_eq!(typed, "New folder 2", "the name it actually has");
        assert_eq!(browser.folders.name_of(second), Some(typed.as_str()));
        assert_ne!(first, second);
    }

    /// A folder that goes while its name is being typed takes the editor with it: no row
    /// will be drawn to close it, and the next folder to take its id would inherit it.
    #[test]
    fn removing_a_folder_mid_rename_takes_the_editor_with_it() {
        let (mut browser, mut workspace, mut device, mut tabs, mut log) = bench();
        let mut act = |browser: &mut Browser, act| {
            apply(
                browser,
                vec![act],
                &mut workspace,
                &mut device,
                &mut tabs,
                &mut log,
            )
        };
        act(&mut browser, Act::NewFolder);
        let Some(Item::Folder(id)) = browser.rename.as_ref().map(|r| r.what) else {
            panic!("a new folder arms its editor");
        };

        act(&mut browser, Act::RemoveFolder(id));
        assert!(browser.rename.is_none(), "the editor went with it");
        assert!(browser.selection.is_none());

        // And the id `make` hands out again is a folder with no editor waiting on it.
        act(&mut browser, Act::NewFolder);
        let Some(Item::Folder(again)) = browser.rename.as_ref().map(|r| r.what) else {
            panic!("the new one arms its own");
        };
        assert_eq!(again, id, "the id came back round");
        assert_eq!(
            browser.rename.as_ref().map(|r| r.text.as_str()),
            Some("New folder")
        );
    }

    /// ⚠️ One view per slot. A second read of a slot already being viewed would be two
    /// working copies of one place — edited apart, both owed back to it, and both queued
    /// into one batch, where the last written wins.
    #[test]
    fn opening_a_slot_that_is_already_open_activates_its_tab() {
        use crate::device::DeviceEvent;
        use crate::workspace::Origin;

        let (mut browser, mut workspace, mut device, mut tabs, mut log) = bench();
        let bytes = {
            let id = workspace.create(Fresh::Program, &mut log).unwrap();
            let bytes = workspace.get(id).unwrap().bytes.clone();
            workspace.remove(id, &mut log);
            bytes
        };
        let class = ObjectClass::Program;
        let at = Location { bank: 6, slot: 3 };
        device.pretend_scanned(class, 7, &["", "", "", "Africa Split"]);

        device.pretend(DeviceEvent::Got {
            name: "Africa-Split.ne5p".into(),
            origin: Origin::Device { class, at },
            bytes,
            open: true,
        });
        device.poll(&mut log, &mut workspace, &mut tabs);
        let first = tabs.active().expect("a view opened");

        // Another double-click on the same slot.
        tabs.close(first);
        apply(
            &mut browser,
            vec![Act::Open(Item::Slot { class, at })],
            &mut workspace,
            &mut device,
            &mut tabs,
            &mut log,
        );
        assert!(device.queued().is_empty(), "nothing was read again");
        assert_eq!(tabs.active(), Some(first), "its own tab came forward");
        assert_eq!(workspace.entities().len(), 1, "and there is one copy");

        // A slot with no view open is read, as it must be.
        let elsewhere = Location { bank: 6, slot: 4 };
        apply(
            &mut browser,
            vec![Act::Open(Item::Slot {
                class,
                at: elsewhere,
            })],
            &mut workspace,
            &mut device,
            &mut tabs,
            &mut log,
        );
        assert_eq!(device.queued().len(), 1);
    }

    /// ⚠️ A write is a delete followed by a write, so a file the instrument turns out not
    /// to want costs the occupant of the slot — and the New menu makes another model's
    /// program one click away. It warns; it does not refuse, because nothing here has
    /// watched an instrument refuse one.
    #[test]
    fn a_file_of_another_model_is_warned_about_and_not_refused() {
        let held =
            |tags: &[&str]| -> Vec<String> { tags.iter().map(|tag| tag.to_string()).collect() };
        let warning = foreign_format("ns4p", &held(&["ne5p"])).expect("a Stage 4 file here");
        assert!(
            warning.contains("ns4p") && warning.contains("ne5p"),
            "{warning}"
        );
        assert!(warning.contains("deletes"), "{warning}");

        // What the folder is already holding raises nothing, whitespace and case included.
        assert_eq!(foreign_format("ne5p", &held(&["ne5p"])), None);
        assert_eq!(foreign_format(" ne5p ", &held(&["NE5P "])), None);
        assert_eq!(foreign_format("ne5p", &held(&["ne5p", "ne5l"])), None);

        // Not known is not the same as does not match: an unscanned folder says nothing,
        // and neither does a file whose own tag could not be read.
        assert_eq!(foreign_format("ns4p", &[]), None);
        assert_eq!(
            foreign_format("?", &held(&["ne5p"])),
            None,
            "no tag to judge"
        );
        assert_eq!(foreign_format("", &held(&["ne5p"])), None);
    }

    /// The warning reaches the modal a batch raises, once per format however many items
    /// carry it — and it goes above the list of destinations, which is what the eye
    /// slides past.
    #[test]
    fn the_modal_says_when_a_batch_is_of_another_model() {
        use crate::workspace::Origin;

        let (mut browser, mut workspace, mut device, _tabs, mut log) = bench();
        let class = ObjectClass::Program;
        device.pretend_scanned(class, 7, &["Africa Split", "Squabble B"]);

        let mut ids = Vec::new();
        for slot in 0..2 {
            let stage = workspace.create(Fresh::Stage4Program, &mut log).unwrap();
            let bytes = workspace.get(stage).unwrap().bytes.clone();
            workspace.remove(stage, &mut log);
            ids.push(workspace.ingest(
                format!("stage-{slot}.ns4p"),
                Origin::Device {
                    class,
                    at: Location { bank: 6, slot },
                },
                bytes,
                &mut log,
            ));
        }

        browser.ask_send(&workspace, &device, &ids, "Send?".into(), Act::SendAll);
        let note = browser.ask.as_ref().and_then(|ask| ask.note.clone());
        let note = note.expect("the modal has a note");
        assert_eq!(note.matches("This file is ns4p").count(), 1, "{note}");
        let warned = note.find("ns4p").expect("the warning is there");
        let listed = note.find("replaces").expect("and so are the destinations");
        assert!(warned < listed, "the warning comes first:\n{note}");
    }

    /// A jump opens whatever the slot is inside, selects it, and is spent. A jump at a
    /// slot no walk has reached is spent too — otherwise it would hold a heading open
    /// for as long as the instrument stayed attached.
    #[test]
    fn a_jump_lands_on_its_slot_and_is_spent_either_way() {
        let ctx = egui::Context::default();
        let workspace = Workspace::new(ctx.clone());
        let mut device = Device::new(ctx.clone());
        let mut browser = Browser::default();
        let names: Vec<&str> = (0..50).map(|_| "Africa Split").collect();
        device.pretend_scanned(ObjectClass::Program, 7, &names);
        device.pretend_scanned(ObjectClass::Program, 8, &["Bass Manual"]);
        let at = Location { bank: 7, slot: 0 };
        device.pretend_focused(ObjectClass::Program, at);

        let frame = |browser: &mut Browser| {
            let _ = ctx.run(egui::RawInput::default(), |ctx| {
                egui::SidePanel::left("places").show(ctx, |ui| {
                    let _ = browser.ui(ui, &workspace, &device);
                });
            });
        };

        browser.jump = Some((ObjectClass::Program, at));
        frame(&mut browser);
        assert!(browser.jump.is_none(), "the jump landed");
        assert!(
            browser.selection
                == Some(Item::Slot {
                    class: ObjectClass::Program,
                    at
                })
        );

        // Bank 12 was never read, so nothing will ever draw the row that clears this.
        browser.jump = Some((ObjectClass::Program, Location { bank: 11, slot: 0 }));
        frame(&mut browser);
        assert!(browser.jump.is_none(), "and a jump to nowhere is spent");
    }

    /// One button for the whole column, and it asks for every folder.
    #[test]
    fn a_sync_reads_every_folder_again() {
        let ctx = egui::Context::default();
        let mut workspace = Workspace::new(ctx.clone());
        let mut device = Device::new(ctx);
        let mut log = crate::log::Log::default();
        let mut tabs = Tabs::default();
        let mut browser = Browser::default();
        device.pretend_scanned(ObjectClass::Program, 7, &["Africa Split"]);

        apply(
            &mut browser,
            vec![Act::Resync],
            &mut workspace,
            &mut device,
            &mut tabs,
            &mut log,
        );
        for class in BROWSED {
            let progress = device.state.scan.progress(class);
            assert!(
                progress.is_some_and(|progress| progress.running),
                "{}",
                folder(class)
            );
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
