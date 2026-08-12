//! Everything the engineering build showed: the field table, and the record beside it.
//!
//! Nothing here is a control but the table. The rest is the record: what the container
//! says, what the bytes did, and — for something read off the instrument — what the
//! instrument says about the slot it came from.

use eframe::egui;
use nord_format::fields::Field;
use nord_usb::{Location, ObjectClass};

use super::controls::{self, Sets};
use crate::device::{Device, DeviceCmd};
use crate::fields::{byte_diff, DiffRow};
use crate::strings;
use crate::workspace::LocalEntity;

/// A read the Meta face asked the instrument for.
pub struct SlotDetails {
    pub class: ObjectClass,
    pub at: Location,
}

/// Column widths for the table. Wide enough for the longest of each in an ne5 body.
const NAME_W: f32 = 230.0;
const BITS_W: f32 = 70.0;
const STORED_W: f32 = 220.0;

/// A cell being typed into, and what the library said about it last.
#[derive(Default)]
struct Cell {
    path: String,
    text: String,
    /// The first frame the cell is open, so focus is taken once.
    fresh: bool,
    /// The library's refusal. While it is set the cell stays in edit.
    error: Option<String>,
}

#[derive(Default)]
pub struct Advanced {
    /// Narrows the table by path or label. A body has ninety fields.
    filter: String,
    cell: Cell,
    /// The entity the cached dump belongs to.
    ///
    /// ⚠️ `{:#?}` over an undecoded body prints every byte, and a piano library is
    /// hundreds of megabytes — it is rendered once and kept, never per frame.
    dump_for: Option<u64>,
    dump: String,
}

impl Advanced {
    /// The whole body as a table: every field the library declares, engineering-only
    /// ones included, each value editable by the spelling `set_field` takes.
    ///
    /// This is the engineer's view, so nothing is hidden and nothing is prettied up: an
    /// unrecognised value is spelled `unknown (9)` here and that spelling is accepted
    /// back, which the friendly view deliberately will not do.
    pub fn table(&mut self, ui: &mut egui::Ui, fields: &[Field], sets: &mut Sets) {
        ui.horizontal(|ui| {
            ui.label("Filter");
            ui.add(
                egui::TextEdit::singleline(&mut self.filter)
                    .desired_width(200.0)
                    .hint_text("path or name"),
            );
            let shown = fields.iter().filter(|f| self.matches(f)).count();
            ui.label(
                egui::RichText::new(format!("{shown} of {} fields", fields.len()))
                    .small()
                    .weak(),
            );
        });
        ui.separator();

        egui::Grid::new("registry_table")
            .num_columns(4)
            .striped(true)
            .show(ui, |ui| {
                ui.label(egui::RichText::new("field").small().weak());
                ui.label(egui::RichText::new("bits").small().weak());
                ui.label(egui::RichText::new("stored").small().weak());
                ui.label(egui::RichText::new("value").small().weak());
                ui.end_row();

                // Declaration order: it is the order the body is laid out in, which is
                // what an engineer reading a dump alongside this is following.
                let rows: Vec<&Field> = fields.iter().filter(|f| self.matches(f)).collect();
                for field in rows {
                    self.cell_row(ui, field, sets);
                    ui.end_row();
                }
            });
    }

    fn matches(&self, field: &Field) -> bool {
        let wanted = self.filter.trim().to_ascii_lowercase();
        if wanted.is_empty() {
            return true;
        }
        field.path.to_ascii_lowercase().contains(&wanted)
            || strings::label(&field.path)
                .to_ascii_lowercase()
                .contains(&wanted)
    }

    fn cell_row(&mut self, ui: &mut egui::Ui, field: &Field, sets: &mut Sets) {
        // Widths are set rather than left to the text: a path is long and a body has
        // ninety of them, and a column that reflows per row cannot be read down.
        ui.vertical(|ui| {
            ui.set_min_width(NAME_W);
            ui.add(egui::Label::new(strings::label(&field.path)).truncate());
            ui.add(
                egui::Label::new(egui::RichText::new(&field.path).monospace().small().weak())
                    .truncate(),
            );
        });
        ui.add_sized(
            [BITS_W, ui.spacing().interact_size.y],
            egui::Label::new(
                egui::RichText::new(field.spec.placement)
                    .monospace()
                    .small()
                    .weak(),
            )
            .truncate()
            .halign(egui::Align::LEFT),
        );
        ui.add_sized(
            [STORED_W, ui.spacing().interact_size.y],
            egui::Label::new(egui::RichText::new(&field.display).monospace().small())
                .truncate()
                .halign(egui::Align::LEFT),
        );

        let editing = self.cell.path == field.path;
        if !editing {
            // Not a `selectable_label`: clicking the value is what opens it.
            if ui
                .add_sized(
                    [180.0, ui.spacing().interact_size.y],
                    egui::Label::new(egui::RichText::new(&field.value).monospace())
                        .truncate()
                        .halign(egui::Align::LEFT)
                        .sense(egui::Sense::click()),
                )
                .on_hover_text("click to edit")
                .clicked()
            {
                self.cell = Cell {
                    path: field.path.clone(),
                    text: field.value.clone(),
                    fresh: true,
                    error: None,
                };
            }
            return;
        }

        let response = ui.add(
            egui::TextEdit::singleline(&mut self.cell.text)
                .desired_width(160.0)
                .font(egui::TextStyle::Monospace),
        );
        // ⚠️ Taken once. Asking for focus every frame would mean the cell could never be
        // left by clicking anything else.
        if self.cell.fresh {
            self.cell.fresh = false;
            response.request_focus();
            // Selected, so typing replaces the value rather than growing it.
            let all = egui::text::CCursorRange::two(
                egui::text::CCursor::new(0),
                egui::text::CCursor::new(self.cell.text.chars().count()),
            );
            if let Some(mut state) = egui::TextEdit::load_state(ui.ctx(), response.id) {
                state.cursor.set_char_range(Some(all));
                state.store(ui.ctx(), response.id);
            }
        }
        // The refusal sits beside the cell it is about: a message at the foot of ninety
        // rows is a message about nothing in particular.
        if let Some(why) = &self.cell.error {
            let bad = crate::app::bad(ui.visuals());
            ui.label(egui::RichText::new(why).small().color(bad));
        }
        if ui.input(|i| i.key_pressed(egui::Key::Escape)) {
            self.cell = Cell::default();
            return;
        }
        let entered = ui.input(|i| i.key_pressed(egui::Key::Enter));
        // Losing focus while a refusal is showing keeps the cell open: the typed value
        // is the only copy of what the operator meant.
        let settled = entered || (response.lost_focus() && self.cell.error.is_none());
        if !settled {
            return;
        }
        let typed = self.cell.text.trim().to_string();
        if typed == field.value {
            self.cell = Cell::default();
            return;
        }
        sets.push((field.path.clone(), typed));
    }

    /// Forget the cell being typed into.
    ///
    /// ⚠️ One table serves every tab, and a cell is remembered by the **path** it sits on
    /// — which two documents of the same format both declare. Left standing, a half-typed
    /// value follows the operator into the next document and lands there on Enter.
    pub(super) fn leave(&mut self) {
        self.cell = Cell::default();
    }

    /// The path of the cell being typed into, if any.
    #[cfg(test)]
    pub(super) fn editing(&self) -> Option<&str> {
        (!self.cell.path.is_empty()).then_some(self.cell.path.as_str())
    }

    /// Open a cell as a click would, for a test that cannot click.
    #[cfg(test)]
    pub(super) fn pretend_editing(&mut self, path: &str, typed: &str) {
        self.cell = Cell {
            path: path.to_string(),
            text: typed.to_string(),
            fresh: true,
            error: None,
        };
    }

    /// Report what the library said about the last cell edit.
    ///
    /// `Ok` closes the cell; a refusal leaves it open with the message under the table.
    pub fn settled(&mut self, outcome: Result<(), String>) {
        match outcome {
            Ok(()) => self.cell = Cell::default(),
            Err(why) => {
                self.cell.error = Some(why);
                // Back into the cell: what was typed is the only copy of what was meant.
                self.cell.fresh = true;
            }
        }
    }

    /// The record, section by section: where it came from and what it is, what the bytes
    /// have done since the tab opened, what the instrument says about its slot, and the
    /// decode in full.
    pub fn meta(
        &mut self,
        ui: &mut egui::Ui,
        entity: &LocalEntity,
        opened: &[u8],
        device: &Device,
    ) -> Option<SlotDetails> {
        let mut asked = None;
        controls::section(ui, "Container", |ui| {
            verify(ui, entity);
            container(ui, entity);
        });
        let rows = byte_diff(opened, &entity.bytes);
        let title = match rows.len() {
            0 => "Changes".to_string(),
            n => format!("Changes ({n} bytes)"),
        };
        controls::section(ui, &title, |ui| diff(ui, entity, opened, rows));
        if entity.origin.slot().is_some() {
            controls::section(ui, "On the instrument", |ui| {
                asked = slot(ui, entity, device);
            });
        }
        if entity.entity.is_some() {
            controls::section(ui, "Raw", |ui| self.dump(ui, entity));
        }
        asked
    }

    fn dump(&mut self, ui: &mut egui::Ui, entity: &LocalEntity) {
        let Some(decoded) = &entity.entity else {
            return;
        };
        // Folded away: a library body prints hundreds of megabytes, and the operator who
        // wants it will say so.
        egui::CollapsingHeader::new("Show the decode")
            .id_salt("raw_debug")
            .show(ui, |ui| {
                if self.dump_for != Some(entity.id) {
                    self.dump = format!("{decoded:#?}");
                    self.dump_for = Some(entity.id);
                }
                egui::ScrollArea::both()
                    .max_height(360.0)
                    .auto_shrink([false, true])
                    .show(ui, |ui| {
                        ui.label(egui::RichText::new(&self.dump).monospace().small());
                    });
            });
    }
}

fn verify(ui: &mut egui::Ui, entity: &LocalEntity) {
    ui.horizontal_wrapped(|ui| {
        ui.label(egui::RichText::new("verify").weak());
        ui.label(
            egui::RichText::new(entity.verify.badge())
                .strong()
                .color(entity.verify.color(ui.visuals())),
        );
        ui.label(egui::RichText::new(entity.verify.detail()).weak());
    });
    if let Some(e) = &entity.parse_error {
        ui.label(egui::RichText::new(e).color(crate::app::bad(ui.visuals())));
    }
}

fn row(ui: &mut egui::Ui, label: &str, value: impl Into<String>) {
    ui.label(egui::RichText::new(label).weak());
    ui.label(egui::RichText::new(value.into()).monospace());
    ui.end_row();
}

fn container(ui: &mut egui::Ui, entity: &LocalEntity) {
    let Some(container) = &entity.container else {
        ui.label(
            egui::RichText::new("these bytes carry no CBIN header, so there is nothing to read")
                .weak()
                .small(),
        );
        return;
    };
    egui::Grid::new("cbin_grid").num_columns(2).show(ui, |ui| {
        row(
            ui,
            "generation",
            format!("{:?}", container.header.generation),
        );
        row(ui, "format", container.tag());
        row(ui, "version", container.header.version.to_string());
        row(ui, "slot", stored_slot(&container.header));
        row(ui, "body", format!("{} bytes", container.body_len));
        row(ui, "file", format!("{} bytes", entity.bytes.len()));
        row(
            ui,
            container.checksum_label.trim_end_matches(':'),
            match container.checksum_ok {
                true => container.checksum.clone(),
                false => format!("{} (does not match the bytes)", container.checksum),
            },
        );
    });
}

/// The stored slot, one-indexed as `BANK:SLOT`.
///
/// Library files carry `0xffff:0xffff` where slot files keep a bank/slot pair — a
/// library object has no slot until an instrument gives it one.
fn stored_slot(header: &nord_format::cbin::Header) -> String {
    match header.slot() {
        (0xffff, 0xffff) => "none (a library file, not a slot save)".into(),
        (bank, slot) => format!("{}:{}", bank + 1, slot + 1),
    }
}

fn diff(ui: &mut egui::Ui, entity: &LocalEntity, opened: &[u8], rows: Vec<DiffRow>) {
    if rows.is_empty() {
        ui.label(
            egui::RichText::new(match opened.len() == entity.bytes.len() {
                true => "nothing moved",
                // Nothing here can pair the bytes up across a length change.
                false => "the length changed, so there is nothing to line up",
            })
            .weak()
            .small(),
        );
        return;
    }
    egui::ScrollArea::vertical()
        .id_salt("bytediff")
        .max_height(220.0)
        .auto_shrink([false, true])
        .show(ui, |ui| {
            for row in rows {
                ui.label(
                    egui::RichText::new(format!(
                        "byte {:#06x}  {:#04x} -> {:#04x}{}",
                        row.at, row.before, row.after, row.note,
                    ))
                    .monospace()
                    .small()
                    .weak(),
                );
            }
        });
}

/// What the instrument says about the slot this came off.
fn slot(ui: &mut egui::Ui, entity: &LocalEntity, device: &Device) -> Option<SlotDetails> {
    let (class, at) = entity.origin.slot()?;
    let mut asked = None;
    ui.label(
        egui::RichText::new(strings::place(class, at))
            .monospace()
            .small(),
    );
    let busy = device.state.in_flight.is_some();
    if ui
        .add_enabled(
            device.state.connected() && !busy,
            egui::Button::new("Read slot details"),
        )
        .on_disabled_hover_text("needs the instrument attached and idle")
        .clicked()
    {
        asked = Some(SlotDetails { class, at });
    }
    if device.state.detail.at != Some(at) {
        return asked;
    }
    match (&device.state.detail.info, device.state.detail.asked) {
        (Some(info), _) => {
            egui::Grid::new("slot_detail")
                .num_columns(2)
                .show(ui, |ui| {
                    row(ui, "name", format!("{:?}", info.name));
                    row(ui, "format", info.format.clone());
                    row(ui, "version", info.version.to_string());
                    row(ui, "body", format!("{} bytes", info.body_len));
                    row(
                        ui,
                        "crc32",
                        match info.crc32 {
                            Some(crc) => format!("{crc:#010x}"),
                            // Library content reports 0xffffffff: no checksum is kept for
                            // objects this large.
                            None => "none (not checksummed for this class)".into(),
                        },
                    );
                });
        }
        (None, true) => {
            ui.label(egui::RichText::new("the slot is empty").weak());
        }
        (None, false) => {}
    }
    if let Some(deps) = &device.state.detail.deps {
        ui.separator();
        if deps.is_empty() {
            ui.label(egui::RichText::new("no dependencies").weak());
        }
        egui::Grid::new("deps").num_columns(3).show(ui, |ui| {
            for dep in deps {
                ui.label(egui::RichText::new(dep.class.label()).small().weak());
                ui.label(egui::RichText::new(format!("{:08x}", dep.id)).monospace());
                // The names come from the device; a file stores ids only.
                ui.label(dep.name.trim());
                ui.end_row();
            }
        });
    }
    asked
}

/// The two reads the Meta face asks for, in the order the CLI asks them.
pub fn commands(details: SlotDetails) -> [DeviceCmd; 2] {
    let SlotDetails { class, at } = details;
    [
        DeviceCmd::SlotInfo { class, at },
        DeviceCmd::Deps { class, at },
    ]
}
