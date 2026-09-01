//! The right column: the instrument's own folders, banks and slots, as it reported them.

use eframe::egui;
use nord_usb::{Location, ObjectClass};

use super::act::Act;
use super::drag::{Carried, Item, Kind, Onto};
use super::row::{row, Cells};
use super::{Ask, Browser};
use crate::app::dot;
use crate::device::{occupancy, read_only, Device, BROWSED};
use crate::strings::{folder, place, shown};
use crate::workspace::Workspace;

impl Browser {
    pub(super) fn instrument(
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
            // The live buffer and the settings singleton divide into one bank.
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
        // Open where the panel is, closed everywhere else.
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
        // A jump can scroll only after its parent headings have exposed this row.
        if self.jump == Some((class, at)) {
            self.jump = None;
            self.selection = Some(item);
            response.scroll_to_me(Some(egui::Align::Center));
        }

        // ⚠️ Pianos are large libraries fetched whole, so this browser only lists them.
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::browser::drag::Item;

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
}
