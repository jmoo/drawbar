//! The Advanced face: the field table and the record beside it.
//!
//! The table is the only control here. The record lists what the container says, which
//! bytes changed, and, for something read from the instrument, what the instrument says
//! about the slot it came from.

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

/// A read the Advanced face asked the instrument for.
pub struct SlotDetails {
    pub class: ObjectClass,
    pub at: Location,
}

/// The table's columns, left to right, each wide enough for its longest value in an ne5
/// body. The widths are fixed so that a column lines up down hundreds of rows.
const COLUMNS: [(&str, f32); 6] = [
    ("Path", 250.0),
    ("Bits", 74.0),
    ("Control", 110.0),
    ("Raw", 150.0),
    ("Writes · editable", 140.0),
    ("", 20.0),
];

const ROW: f32 = 22.0;
/// The label column of a record block.
const LABEL: f32 = 110.0;
/// The page's left margin.
const PAD: f32 = 12.0;
const MONO: f32 = 10.5;
const HEAD: f32 = 9.0;
const HEAD_ROW: f32 = 14.0;

/// The horizontal inset a `TextEdit` gives its text.
const BOX_PAD: f32 = 4.0;

/// How much of the page the byte diff takes before it scrolls inside itself.
const DIFF_HEIGHT: f32 = 220.0;

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
    /// Narrows the table by path or label.
    filter: String,
    cell: Cell,
    /// The asset id and the two byte stamps the cached diff compares.
    ///
    /// ⚠️ `byte_diff` walks both bodies. The Advanced face asks for it every frame it is
    /// shown, and a piano library is hundreds of megabytes, so it is walked once per pair
    /// of bodies.
    diff_for: Option<(u64, u64, u64)>,
    diff: Vec<DiffRow>,
}

impl Advanced {
    /// What the file says about itself: the facts the document was built from, read here
    /// and written back unchanged.
    pub fn about(ui: &mut egui::Ui, rows: &[(&'static str, String, String)]) {
        controls::heading(
            ui,
            "About this file",
            "what the file says about itself, read here and written back unchanged",
            None,
        );
        facts(ui, rows);
    }

    /// The whole body as a table: every field the library declares, engineering-only
    /// ones included, each value editable by the spelling `set_field` takes.
    ///
    /// Nothing is hidden or prettified: an unrecognized value is spelled `unknown (9)` and
    /// that spelling is accepted back, and a field the Basic face does not draw is an
    /// ordinary row with a flag.
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
            "registry order · Raw is what was read; type in Writes to change it. A value is \
             taken as spelled and refused if the field cannot hold it",
            Some((
                &format!(
                    "{} of {} rows · {unseen} hidden from Basic",
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
                cell(
                    ui,
                    &head.to_uppercase(),
                    egui::vec2(width, HEAD_ROW),
                    egui::FontId::proportional(HEAD),
                    quiet,
                );
            }
        });
        ui.separator();

        // Declaration order, which is the order the body is laid out in.
        for field in rows {
            self.row(ui, field, table, sets);
        }
    }

    fn row(&mut self, ui: &mut egui::Ui, field: &Field, table: &Table<'_>, sets: &mut Sets) {
        let visuals = ui.visuals().clone();
        let changed = table.changed.contains(&field.path);
        let hidden = !table.shows(&field.path);
        let labeled = strings::known(&field.path);
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
        let mono = egui::FontId::monospace(MONO);
        cell(
            &mut row,
            &field.path,
            egui::vec2(COLUMNS[0].1, ROW),
            mono.clone(),
            ink,
        );
        cell(
            &mut row,
            field.spec.placement,
            egui::vec2(COLUMNS[1].1, ROW),
            mono.clone(),
            app::caption(&visuals),
        );
        cell(
            &mut row,
            &field::kind_word(field),
            egui::vec2(COLUMNS[2].1, ROW),
            egui::FontId::proportional(MONO),
            app::caption(&visuals),
        );
        cell(
            &mut row,
            table.raw(&field.path),
            egui::vec2(COLUMNS[3].1, ROW),
            mono,
            ink,
        );
        self.writes(&mut row, field, sets);
        if let Some((glyph, tint)) = flag(changed, hidden, labeled, &visuals) {
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
            match (hidden, labeled) {
                (true, _) => "not relevant: the instrument ignores this in the state the file \
                              holds, though it is stored, valid and writable"
                    .to_string(),
                (false, true) => strings::label(&field.path),
                (false, false) => "no label in this app's table yet".to_string(),
            }
        ));
    }

    /// The editable column. Clicking a value opens a box, which commits when it loses
    /// focus and stays open with the typed text while the library refuses it.
    fn writes(&mut self, ui: &mut egui::Ui, field: &Field, sets: &mut Sets) {
        let width = COLUMNS[4].1;
        if self.cell.path != field.path {
            let drawn = held(ui, &field.value, width);
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
        // ⚠️ Taken once: asking for focus every frame would keep the cell from ever losing
        // it.
        if self.cell.fresh {
            self.cell.fresh = false;
            response.request_focus();
            // Select all, so typing replaces the value.
            let all = egui::text::CCursorRange::two(
                egui::text::CCursor::new(0),
                egui::text::CCursor::new(self.cell.text.chars().count()),
            );
            if let Some(mut state) = egui::TextEdit::load_state(ui.ctx(), response.id) {
                state.cursor.set_char_range(Some(all));
                state.store(ui.ctx(), response.id);
            }
        }
        if let Some(why) = &self.cell.error {
            ui.label(
                egui::RichText::new(why)
                    .small()
                    .color(crate::app::bad(ui.visuals())),
            );
        }
        // ⚠️ Keys are read only when this cell loses focus. If they were read from the
        // window every frame, an Enter pressed anywhere would submit every open cell,
        // including a refused one, which would go back to the library and into the log on
        // every press.
        if !response.lost_focus() {
            return;
        }
        let (escaped, entered) = ui.input(|i| {
            (
                i.key_pressed(egui::Key::Escape),
                i.key_pressed(egui::Key::Enter),
            )
        });
        if escaped {
            self.cell = Cell::default();
            return;
        }
        // Losing focus while a refusal shows keeps the cell open, since the typed value is
        // the only copy of what the operator meant. Enter tries again.
        if self.cell.error.is_some() && !entered {
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
    /// ⚠️ One table serves every tab, and a cell is remembered by its path, which two
    /// documents of the same format share. Left open, a half-typed value would follow the
    /// operator into the next document and land there on Enter.
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
    /// `Ok` closes the cell; a refusal leaves it open with the message beside it.
    pub fn settled(&mut self, outcome: Result<(), String>) {
        match outcome {
            Ok(()) => self.cell = Cell::default(),
            Err(why) => {
                self.cell.error = Some(why);
                // Refocus the cell so the operator can correct what was typed.
                self.cell.fresh = true;
            }
        }
    }

    /// The record, block by block: what the container states, which bytes have moved since
    /// the asset was last saved, and what the instrument says about the slot it came off.
    /// Each block is laid out like [`Advanced::about`].
    pub fn meta(
        &mut self,
        ui: &mut egui::Ui,
        entity: &LocalEntity,
        device: &Device,
    ) -> Option<SlotDetails> {
        controls::heading(
            ui,
            "Container",
            "what the header states; read, checked and written back unchanged",
            None,
        );
        verify(ui, entity);
        facts(ui, &container(entity));

        let quiet = app::caption(ui.visuals());
        let rows = self.changes(entity);
        let moved = match rows.len() {
            0 => "none".to_string(),
            n => format!("{n} bytes"),
        };
        controls::heading(
            ui,
            "Changes",
            "the bytes that have moved since this was last saved",
            Some((&moved, quiet)),
        );
        diff(ui, entity, rows);

        let mut asked = None;
        if entity.origin.slot().is_some() {
            controls::heading(
                ui,
                "On the instrument",
                "what the slot these bytes came off reports",
                None,
            );
            asked = slot(ui, entity, device);
        }
        asked
    }

    /// The bytes that moved since the asset was last saved.
    fn changes(&mut self, entity: &LocalEntity) -> &[DiffRow] {
        let against = (entity.id, entity.stamp, entity.saved.stamp);
        if self.diff_for != Some(against) {
            self.diff = byte_diff(&entity.saved.bytes, &entity.bytes);
            self.diff_for = Some(against);
        }
        &self.diff
    }
}

/// What the Advanced table reads besides the working fields: the decode of the bytes
/// this document was last saved as, the paths the two spell differently, and which
/// fields the Basic face draws at all.
pub struct Table<'a> {
    pub fields: &'a [Field],
    pub saved: &'a [Field],
    pub changed: &'a [String],
    pub doc: Option<&'a field::Doc<'a>>,
}

impl Table<'_> {
    /// The value this path held in the bytes the document was last saved as. A field the
    /// saved decode does not carry shows its current value.
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

/// One cell, drawn at its column's width and left-aligned under its heading.
fn cell(ui: &mut egui::Ui, text: &str, size: egui::Vec2, font: egui::FontId, ink: egui::Color32) {
    let (rect, _) = ui.allocate_exact_size(size, egui::Sense::hover());
    let mut cell = ui.new_child(
        egui::UiBuilder::new()
            .max_rect(rect)
            .layout(egui::Layout::left_to_right(egui::Align::Center)),
    );
    cell.add(
        egui::Label::new(egui::RichText::new(text).font(font).color(ink))
            .truncate()
            .halign(egui::Align::LEFT),
    );
}

/// A record block: one row per fact, the label in its own column and the value with the
/// note after it.
pub fn facts(ui: &mut egui::Ui, rows: &[(&str, String, String)]) {
    let ink = ui.visuals().text_color();
    for (label, value, note) in rows {
        fact(ui, label, value, note, ink);
    }
}

/// One row of a record block, with the value in `ink`.
fn fact(ui: &mut egui::Ui, label: &str, value: &str, note: &str, ink: egui::Color32) {
    let quiet = app::caption(ui.visuals());
    ui.horizontal(|ui| {
        ui.add_space(PAD);
        ui.spacing_mut().item_spacing.x = 10.0;
        cell(
            ui,
            label,
            egui::vec2(LABEL, ROW),
            egui::FontId::proportional(11.0),
            ui.visuals().weak_text_color(),
        );
        ui.label(
            egui::RichText::new(value)
                .font(egui::FontId::monospace(11.0))
                .color(ink),
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

/// The Writes column before a click opens its box: the value, drawn where the box will
/// show it.
fn held(ui: &mut egui::Ui, value: &str, width: f32) -> egui::Response {
    let (rect, response) =
        ui.allocate_exact_size(egui::vec2(width, ROW - 4.0), egui::Sense::click());
    let visuals = ui.visuals();
    ui.painter().rect_stroke(
        rect,
        2.0,
        egui::Stroke::new(1.0_f32, visuals.widgets.noninteractive.bg_stroke.color),
        egui::StrokeKind::Inside,
    );
    let ink = visuals.text_color();
    let inner = rect.shrink2(egui::vec2(BOX_PAD, 0.0));
    let mut job = egui::text::LayoutJob::default();
    job.append(
        value,
        0.0,
        egui::TextFormat::simple(egui::FontId::monospace(MONO), ink),
    );
    job.wrap = egui::text::TextWrapping::truncate_at_width(inner.width().max(0.0));
    let galley = ui.painter().layout_job(job);
    ui.painter().galley(
        egui::pos2(inner.left(), rect.center().y - galley.size().y / 2.0),
        galley,
        ink,
    );
    response
}

/// The mark at the end of a row, by precedence: changed by the operator, then not drawn
/// on the Basic face, then unnamed in this app.
fn flag(
    changed: bool,
    hidden: bool,
    labeled: bool,
    visuals: &egui::Visuals,
) -> Option<(Glyph, egui::Color32)> {
    match (changed, hidden, labeled) {
        (true, _, _) => Some((Glyph::Pencil, app::warn(visuals))),
        (false, true, _) => Some((Glyph::EyeOff, app::caption(visuals))),
        (false, false, false) => Some((Glyph::Tag, app::caption(visuals))),
        (false, false, true) => None,
    }
}

/// Whether the bytes this app would write are the bytes it read. This is the only row
/// of the record that is a claim; the others report what was read.
fn verify(ui: &mut egui::Ui, entity: &LocalEntity) {
    let ink = entity.verify.color(ui.visuals());
    fact(
        ui,
        "Verify",
        entity.verify.badge(),
        &entity.verify.detail(),
        ink,
    );
    if let Some(e) = &entity.parse_error {
        ui.label(egui::RichText::new(e).color(crate::app::bad(ui.visuals())));
    }
}

/// What the container states about these bytes.
fn container(entity: &LocalEntity) -> Vec<(&str, String, String)> {
    let Some(container) = &entity.container else {
        return vec![(
            "Container",
            "none".to_string(),
            "these bytes carry no CBIN header, so there is nothing to read".to_string(),
        )];
    };
    vec![
        (
            "Generation",
            format!("{:?}", container.header.generation),
            String::new(),
        ),
        ("Format", container.tag(), String::new()),
        (
            "Version",
            container.header.version.to_string(),
            String::new(),
        ),
        (
            "Stored slot",
            stored_slot(container.header.slot()),
            String::new(),
        ),
        (
            "Body",
            format!("{} bytes", container.body_len()),
            String::new(),
        ),
        (
            "File",
            format!("{} bytes", entity.bytes.len()),
            String::new(),
        ),
        (
            container.checksum_label.trim_end_matches(':'),
            container.checksum.clone(),
            match container.checksum_ok {
                true => "matches the bytes".to_string(),
                false => "does not match the bytes".to_string(),
            },
        ),
    ]
}

/// The value a stored half holds when it names no position.
const NO_SLOT: u16 = 0xffff;

/// The stored slot, one-indexed as `BANK:SLOT`.
///
/// Library files hold `0xffff:0xffff` where slot files hold a bank and slot, because a
/// library object has no slot until an instrument gives it one.
fn stored_slot(slot: (u16, u16)) -> String {
    match slot {
        (NO_SLOT, NO_SLOT) => "none (a library file, not a slot save)".into(),
        (bank, slot) => format!("{}:{}", counted(bank), counted(slot)),
    }
}

/// One half of a stored slot, counted from one, or `none` for the none marker.
fn counted(half: u16) -> String {
    match half {
        NO_SLOT => "none".to_string(),
        half => (u32::from(half) + 1).to_string(),
    }
}

fn diff(ui: &mut egui::Ui, entity: &LocalEntity, rows: &[DiffRow]) {
    if rows.is_empty() {
        ui.label(
            egui::RichText::new(match entity.saved.bytes.len() == entity.bytes.len() {
                true => "nothing moved",
                false => "the length changed, so there is nothing to line up",
            })
            .weak()
            .small(),
        );
        return;
    }
    // ⚠️ A re-laid body moves thousands of bytes; only the rows on screen are drawn.
    egui::ScrollArea::vertical()
        .id_salt("bytediff")
        .max_height(DIFF_HEIGHT)
        .auto_shrink([false, true])
        .show_rows(ui, ROW, rows.len(), |ui, range| {
            for row in &rows[range] {
                fact(
                    ui,
                    &format!("byte {:#06x}", row.at),
                    &format!("{:#04x} → {:#04x}", row.before, row.after),
                    row.note.trim(),
                    ui.visuals().text_color(),
                );
            }
        });
}

/// What the instrument says about the slot this came off.
fn slot(ui: &mut egui::Ui, entity: &LocalEntity, device: &Device) -> Option<SlotDetails> {
    let (class, at) = entity.origin.slot()?;
    let mut asked = None;
    facts(ui, &[("Slot", strings::place(class, at), String::new())]);
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
    if device.state.detail.at != Some((class, at)) {
        return asked;
    }
    match &device.state.detail.info {
        Some(Some(info)) => facts(
            ui,
            &[
                ("Name", format!("{:?}", info.name), String::new()),
                ("Format", info.format.clone(), String::new()),
                ("Version", info.version.to_string(), String::new()),
                ("Body", format!("{} bytes", info.body_len), String::new()),
                (
                    "crc32",
                    match info.crc32 {
                        Some(crc) => format!("{crc:#010x}"),
                        None => "none".to_string(),
                    },
                    match info.crc32 {
                        // Library content reports 0xffffffff: no checksum is kept for
                        // objects this large.
                        Some(_) => String::new(),
                        None => "not checksummed for this class".to_string(),
                    },
                ),
            ],
        ),
        Some(None) => {
            ui.label(egui::RichText::new("the slot is empty").weak());
        }
        None => {}
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

/// The two reads the Advanced face asks for, in the order the CLI asks them.
pub fn commands(details: SlotDetails) -> [DeviceCmd; 2] {
    let SlotDetails { class, at } = details;
    [
        DeviceCmd::SlotInfo { class, at },
        DeviceCmd::Deps { class, at },
    ]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::workspace::{Fresh, Workspace};

    /// The Changes section compares the asset with its last save: an edit adds rows, and
    /// saving clears them.
    #[test]
    fn the_changes_rows_follow_the_bytes_and_the_baseline() {
        let ctx = eframe::egui::Context::default();
        let mut workspace = Workspace::new(ctx);
        let mut log = crate::log::Log::default();
        let id = workspace.create(Fresh::Program, &mut log).expect("a fresh");
        let mut advanced = Advanced::default();
        assert!(
            advanced
                .changes(workspace.get(id).expect("it is open"))
                .is_empty(),
            "nothing has moved yet"
        );

        let bytes = workspace.get(id).expect("it is open").bytes.clone();
        let (_, edited) = crate::fields::apply(
            &bytes,
            &[("center_panel.gain".to_string(), "96".to_string())],
        )
        .expect("the set is legal");
        workspace.replace_bytes(id, edited, &mut log);
        assert!(
            !advanced
                .changes(workspace.get(id).expect("it is open"))
                .is_empty(),
            "the edit is in the section"
        );

        workspace.mark_saved(id);
        assert!(
            advanced
                .changes(workspace.get(id).expect("it is open"))
                .is_empty(),
            "the baseline moved onto the bytes"
        );
    }

    #[test]
    fn a_stored_slot_counts_from_one_and_names_a_half_that_holds_no_position() {
        assert_eq!(stored_slot((0, 0)), "1:1");
        assert_eq!(stored_slot((6, 3)), "7:4");
        assert_eq!(
            stored_slot((NO_SLOT, NO_SLOT)),
            "none (a library file, not a slot save)"
        );
        assert_eq!(stored_slot((NO_SLOT, 5)), "none:6");
        assert_eq!(stored_slot((5, NO_SLOT)), "6:none");
        assert_eq!(stored_slot((0xfffe, 0xfffe)), "65535:65535");
    }
}
