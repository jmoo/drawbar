//! The left column: what is on this computer, loose or in a folder.

use eframe::egui;

use super::act::{owed, Act};
use super::drag::{Carried, Item, Kind, Onto};
use super::row::{row, Cells};
use super::Browser;
use crate::device::{Connection, Device};
use crate::strings::place;
use crate::workspace::{Fresh, Workspace};

impl Browser {
    pub(super) fn computer(
        &mut self,
        ui: &mut egui::Ui,
        workspace: &Workspace,
        device: &Device,
        acts: &mut Vec<Act>,
    ) {
        let mut open_files = false;
        let mut fresh = None;
        let mut new_project = false;
        let mut new_folder = false;
        let mut connect = false;
        let attached = device.state.connected();
        let connecting = matches!(device.state.connection, Connection::Connecting);
        let head = self.heading(ui, "This computer", |ui| {
            open_files = ui.small_button("Open…").clicked();
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
                ui.separator();
                // Not a family: a project is laid out from audio files rather than
                // started from a default, so it asks for them before it exists.
                if ui
                    .button("Sample Editor project…")
                    .on_hover_text(
                        "pick the WAVs it plays; the project stores their names and the \
                         editor looks for them beside it",
                    )
                    .clicked()
                {
                    new_project = true;
                    ui.close();
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
        if new_project {
            acts.push(Act::NewProject);
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
            // ⚠️ Unlike "Send all", this includes unchanged slot-backed items. The count
            // distinguishes those scopes before the confirmation dialog.
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
}

/// Where an asset is owed, for the badge that says so.
fn destination(entity: &crate::workspace::LocalEntity) -> Option<String> {
    let (class, at) = entity.origin.slot()?;
    Some(format!("will be sent to {}", place(class, at)))
}
