//! Everything the engineering build showed: the field table, and the record beside it.
//!
//! Nothing here is a control but the table. The rest is the record: what the container
//! says, what the bytes did, and — for something read off the instrument — what the
//! instrument says about the slot it came from.

use eframe::egui;
use nord_format::fields::Field;
use nord_usb::{Location, ObjectClass};

use super::controls::{self, Sets};
use super::field;
use crate::app;
use crate::device::{Device, DeviceCmd};
use crate::fields::{byte_diff, DiffRow};
use crate::icon::{icon, Glyph};
use crate::strings;
use crate::workspace::LocalEntity;

/// A read the Meta face asked the instrument for.
pub struct SlotDetails {
    pub class: ObjectClass,
    pub at: Location,
}

/// The table's columns, left to right. Wide enough for the longest of each in an ne5
/// body, and fixed rather than reflowing: a path is long, a body has hundreds of them,
/// and a column that moves per row cannot be read down.
const COLUMNS: [(&str, f32); 6] = [
    ("Path", 250.0),
    ("Bits", 74.0),
    ("Control", 110.0),
    ("Raw", 150.0),
    ("Writes · editable", 140.0),
    ("", 20.0),
];

/// One row of it, and the page's own left margin.
const ROW: f32 = 22.0;
const PAD: f32 = 12.0;
const MONO: f32 = 10.5;
const HEAD: f32 = 9.0;

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
    /// What the file says about itself: the same facts the document was built from,
    /// read here and never written differently.
    pub fn about(ui: &mut egui::Ui, rows: &[(&'static str, String, String)]) {
        let quiet = app::caption(ui.visuals());
        controls::heading(
            ui,
            "About this file",
            "what the file says about itself",
            None,
        );
        for (label, value, note) in rows {
            ui.horizontal(|ui| {
                ui.add_space(PAD);
                ui.spacing_mut().item_spacing.x = 10.0;
                ui.add_sized(
                    [110.0, ROW],
                    egui::Label::new(
                        egui::RichText::new(*label)
                            .font(egui::FontId::proportional(11.0))
                            .color(ui.visuals().weak_text_color()),
                    )
                    .halign(egui::Align::LEFT),
                );
                ui.label(
                    egui::RichText::new(value)
                        .font(egui::FontId::monospace(11.0))
                        .color(ui.visuals().text_color()),
                );
                if !note.is_empty() {
                    ui.add(
                        egui::Label::new(
                            egui::RichText::new(note)
                                .font(egui::FontId::proportional(10.0))
                                .color(quiet),
                        )
                        .truncate(),
                    );
                }
            });
        }
    }

    /// The whole body as a table: every field the library declares, engineering-only
    /// ones included, each value editable by the spelling `set_field` takes.
    ///
    /// This is the engineer's view, so nothing is hidden and nothing is prettied up: an
    /// unrecognised value is spelled `unknown (9)` here and that spelling is accepted
    /// back, and a field the Edit face does not draw is a row like any other, flagged
    /// for what it is.
    pub fn table(&mut self, ui: &mut egui::Ui, table: &Table<'_>, sets: &mut Sets) {
        let quiet = app::caption(ui.visuals());
        let rows: Vec<&Field> = table
            .fields
            .iter()
            .filter(|field| self.matches(field))
            .collect();
        let unseen = rows
            .iter()
            .filter(|field| !table.shows(&field.path))
            .count();
        controls::heading(
            ui,
            "Every field",
            "registry order · raw is what was read; type in Writes to change it — the value \
             is taken as spelled, refused if the field cannot hold it",
            Some((
                &format!(
                    "{} of {} rows · {unseen} hidden from Edit",
                    rows.len(),
                    table.fields.len()
                ),
                quiet,
            )),
        );
        ui.horizontal(|ui| {
            ui.add_space(PAD);
            ui.label(egui::RichText::new("Filter").small().color(quiet));
            ui.add(
                egui::TextEdit::singleline(&mut self.filter)
                    .desired_width(200.0)
                    .hint_text("path or name"),
            );
        });
        ui.add_space(4.0);

        ui.horizontal(|ui| {
            ui.add_space(PAD);
            ui.spacing_mut().item_spacing.x = 10.0;
            for (head, width) in COLUMNS {
                ui.add_sized(
                    [width, 14.0],
                    egui::Label::new(
                        egui::RichText::new(head.to_uppercase())
                            .font(egui::FontId::proportional(HEAD))
                            .color(quiet),
                    )
                    .halign(egui::Align::LEFT),
                );
            }
        });
        ui.separator();

        // Declaration order: it is the order the body is laid out in, which is what an
        // engineer reading a dump alongside this is following.
        for field in rows {
            self.row(ui, field, table, sets);
        }
    }

    fn row(&mut self, ui: &mut egui::Ui, field: &Field, table: &Table<'_>, sets: &mut Sets) {
        let visuals = ui.visuals().clone();
        let changed = table.changed.contains(&field.path);
        let hidden = !table.shows(&field.path);
        let labelled = strings::known(&field.path);
        let (rect, response) =
            ui.allocate_exact_size(egui::vec2(ui.available_width(), ROW), egui::Sense::hover());
        if changed {
            ui.painter()
                .rect_filled(rect, 0.0, visuals.selection.bg_fill);
        }
        let mut row = ui.new_child(
            egui::UiBuilder::new()
                .max_rect(rect)
                .layout(egui::Layout::left_to_right(egui::Align::Center)),
        );
        row.spacing_mut().item_spacing.x = 10.0;
        row.add_space(PAD);
        let ink = match hidden {
            true => app::caption(&visuals),
            false => visuals.weak_text_color(),
        };
        cell(&mut row, &field.path, COLUMNS[0].1, ink);
        cell(
            &mut row,
            field.spec.placement,
            COLUMNS[1].1,
            app::caption(&visuals),
        );
        row.add_sized(
            [COLUMNS[2].1, ROW],
            egui::Label::new(
                egui::RichText::new(field::kind_word(field))
                    .font(egui::FontId::proportional(MONO))
                    .color(app::caption(&visuals)),
            )
            .truncate()
            .halign(egui::Align::LEFT),
        );
        cell(&mut row, table.raw(&field.path), COLUMNS[3].1, ink);
        self.writes(&mut row, field, sets);
        if let Some((glyph, tint)) = flag(changed, hidden, labelled, &visuals) {
            icon(&mut row, glyph, 11.0, tint);
        }

        // ⚠️ Asked only of the row under the pointer. Enumerating a field walks every
        // bit pattern it can hold, and a body has hundreds of fields in one table.
        if !response.hovered() {
            return;
        }
        let accepts = match (field.spec.legal)() {
            legal if legal.is_empty() => "its stored bits, as spelled".to_string(),
            legal if legal.len() > 12 => format!("{} .. {}", legal[0], legal[legal.len() - 1]),
            legal => legal.join(", "),
        };
        response.on_hover_text(format!(
            "{} · accepts {accepts}",
            match (hidden, labelled) {
                (true, _) => "not relevant: the instrument is not using this for the state the \
                              file holds — stored, valid, writable"
                    .to_string(),
                (false, true) => strings::label(&field.path),
                (false, false) => "no label in this app's table yet".to_string(),
            }
        ));
    }

    /// The one editable column. A box opens where the value is clicked, commits on Enter
    /// or on losing focus, and stays open holding what was typed while the library is
    /// refusing it.
    fn writes(&mut self, ui: &mut egui::Ui, field: &Field, sets: &mut Sets) {
        let width = COLUMNS[4].1;
        if self.cell.path != field.path {
            let drawn = ui.add_sized(
                [width, ROW - 4.0],
                egui::Button::new(
                    egui::RichText::new(&field.value)
                        .font(egui::FontId::monospace(MONO))
                        .color(ui.visuals().text_color()),
                )
                .fill(egui::Color32::TRANSPARENT)
                .stroke(egui::Stroke::new(
                    1.0_f32,
                    ui.visuals().widgets.noninteractive.bg_stroke.color,
                )),
            );
            if drawn.on_hover_text("click to type a value").clicked() {
                self.cell = Cell {
                    path: field.path.clone(),
                    text: field.value.clone(),
                    fresh: true,
                    error: None,
                };
            }
            return;
        }

        let response = ui.add_sized(
            [width, ROW - 4.0],
            egui::TextEdit::singleline(&mut self.cell.text).font(egui::FontId::monospace(MONO)),
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
        // The refusal sits beside the cell it is about: a message at the foot of eight
        // hundred rows is a message about nothing in particular.
        if let Some(why) = &self.cell.error {
            ui.label(
                egui::RichText::new(why)
                    .small()
                    .color(crate::app::bad(ui.visuals())),
            );
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
    /// have done since it was last saved, what the instrument says about its slot, and
    /// the decode in full.
    pub fn meta(
        &mut self,
        ui: &mut egui::Ui,
        entity: &LocalEntity,
        device: &Device,
    ) -> Option<SlotDetails> {
        let mut asked = None;
        controls::section(ui, "Container", |ui| {
            verify(ui, entity);
            container(ui, entity);
        });
        let saved = &entity.saved.bytes;
        let rows = byte_diff(saved, &entity.bytes);
        let title = match rows.len() {
            0 => "Changes".to_string(),
            n => format!("Changes ({n} bytes)"),
        };
        controls::section(ui, &title, |ui| diff(ui, entity, saved, rows));
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
        // ⚠️ Formatting is synchronous; keep large library bodies folded until requested.
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

/// What the Advanced table reads besides the working fields: the decode of the bytes
/// this document was last saved as, the paths the two spell differently, and which
/// fields the Edit face draws at all.
pub struct Table<'a> {
    pub fields: &'a [Field],
    pub saved: &'a [Field],
    pub changed: &'a [String],
    pub doc: Option<&'a field::Doc<'a>>,
}

impl Table<'_> {
    /// The value this path held in the bytes the document was last saved as. A field the
    /// saved decode does not carry reads as what is in front of the operator.
    fn raw(&self, path: &str) -> &str {
        self.saved
            .iter()
            .chain(self.fields)
            .find(|field| field.path == path)
            .map_or("", |field| field.value.as_str())
    }

    fn shows(&self, path: &str) -> bool {
        self.doc.is_none_or(|doc| doc.shows(path))
    }
}

/// One mono column of a row.
fn cell(ui: &mut egui::Ui, text: &str, width: f32, ink: egui::Color32) {
    ui.add_sized(
        [width, ROW],
        egui::Label::new(
            egui::RichText::new(text)
                .font(egui::FontId::monospace(MONO))
                .color(ink),
        )
        .truncate()
        .halign(egui::Align::LEFT),
    );
}

/// The one mark at the end of a row, in the order that decides which it wears: what the
/// operator changed, then what the Edit face does not draw, then what this app has no
/// name for.
fn flag(
    changed: bool,
    hidden: bool,
    labelled: bool,
    visuals: &egui::Visuals,
) -> Option<(Glyph, egui::Color32)> {
    match (changed, hidden, labelled) {
        (true, _, _) => Some((Glyph::Pencil, app::warn(visuals))),
        (false, true, _) => Some((Glyph::EyeOff, app::caption(visuals))),
        (false, false, false) => Some((Glyph::Tag, app::caption(visuals))),
        (false, false, true) => None,
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

fn diff(ui: &mut egui::Ui, entity: &LocalEntity, saved: &[u8], rows: Vec<DiffRow>) {
    if rows.is_empty() {
        ui.label(
            egui::RichText::new(match saved.len() == entity.bytes.len() {
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
