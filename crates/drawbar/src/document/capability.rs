//! What a format holds, as the Advanced face states it: one row per capability the
//! instrument editors know, each in the state this format puts it in.
//!
//! A field the format lacks is absent from the Edit face; this table is the one place
//! that absence is written down, so the reader can tell a missing knob from a bug.

use eframe::egui;

use super::controls;
use crate::app;
use crate::icon::{painted, Glyph};

/// How one capability stands in one format.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum State {
    Editable,
    ReadOnly,
    /// The format has no such field.
    Absent,
    /// The bytes can only be made by encoding audio, which this app cannot yet do here.
    NeedsEncode,
    /// A round trip the test suite proves byte for byte.
    Verified,
}

impl State {
    fn glyph(self) -> Glyph {
        match self {
            State::Editable => Glyph::Check,
            State::ReadOnly => Glyph::Eye,
            State::Absent => Glyph::Minus,
            State::NeedsEncode => Glyph::CircleAlert,
            State::Verified => Glyph::CircleCheck,
        }
    }

    fn ink(self, visuals: &egui::Visuals) -> egui::Color32 {
        match self {
            State::Editable | State::Verified => app::good(visuals),
            State::ReadOnly | State::Absent => app::caption(visuals),
            State::NeedsEncode => app::warn(visuals),
        }
    }

    pub fn word(self) -> &'static str {
        match self {
            State::Editable => "editable",
            State::ReadOnly => "read-only",
            State::Absent => "not in this format",
            State::NeedsEncode => "needs an encode",
            State::Verified => "verified",
        }
    }
}

/// One capability as one format holds it. `note` is the format's own detail — an
/// offset, a limit, a bank — and may be empty.
pub struct Row {
    pub name: &'static str,
    pub state: State,
    pub note: &'static str,
}

/// Where an edited field lands in the file: the offset, what is there, and a note.
pub struct Offset {
    pub at: String,
    pub holds: String,
    pub note: &'static str,
}

const ROW_H: f32 = 22.0;
const PAD: f32 = 12.0;
const COLUMN_MIN: f32 = 280.0;
const GLYPH: f32 = 12.0;
const NAME: f32 = 11.0;
const NOTE: f32 = 10.0;

/// The capability table under its heading: `live · read · absent` at the right, the
/// absent ones named in the note, the rows in as many columns as the width allows.
pub fn table(ui: &mut egui::Ui, rows: &[Row]) {
    let count = |state: State| rows.iter().filter(|row| row.state == state).count();
    let live = count(State::Editable) + count(State::Verified);
    let absent: Vec<&str> = rows
        .iter()
        .filter(|row| row.state == State::Absent)
        .map(|row| row.name)
        .collect();
    let badge = format!(
        "{live} live · {} read · {} absent",
        count(State::ReadOnly),
        absent.len()
    );
    let note = match absent.len() {
        0 => "every capability this editor knows is live on this format".to_string(),
        n => format!(
            "absent here because it is absent there: {}{}",
            absent
                .iter()
                .take(3)
                .copied()
                .collect::<Vec<_>>()
                .join(", "),
            match n > 3 {
                true => format!(" +{}", n - 3),
                false => String::new(),
            }
        ),
    };
    let quiet = app::caption(ui.visuals());
    controls::heading(ui, "What this format holds", &note, Some((&badge, quiet)));

    let width = (ui.available_width() - PAD * 2.0).max(1.0);
    let columns = ((width / COLUMN_MIN).floor() as usize).max(1);
    let per_column = rows.len().div_ceil(columns).max(1);
    let column_w = width / columns as f32;
    let (rect, _) = ui.allocate_exact_size(
        egui::vec2(ui.available_width(), per_column as f32 * ROW_H),
        egui::Sense::hover(),
    );
    for (index, row) in rows.iter().enumerate() {
        let column = index / per_column;
        let line = index % per_column;
        let cell = egui::Rect::from_min_size(
            egui::pos2(
                rect.left() + PAD + column as f32 * column_w,
                rect.top() + line as f32 * ROW_H,
            ),
            egui::vec2(column_w - PAD, ROW_H),
        );
        capability(ui, cell, row);
    }
    ui.add_space(6.0);
}

fn capability(ui: &mut egui::Ui, cell: egui::Rect, row: &Row) {
    let visuals = ui.visuals().clone();
    let painter = ui.painter().clone();
    let glyph = egui::Rect::from_center_size(
        egui::pos2(cell.left() + GLYPH / 2.0, cell.center().y),
        egui::Vec2::splat(GLYPH),
    );
    painted(ui, row.state.glyph(), glyph, row.state.ink(&visuals));
    let ink = match row.state {
        State::Absent => app::caption(&visuals),
        _ => visuals.weak_text_color(),
    };
    let name = painter.layout_no_wrap(row.name.to_string(), egui::FontId::proportional(NAME), ink);
    let left = glyph.right() + 7.0;
    painter.galley(
        egui::pos2(left, cell.center().y - name.size().y / 2.0),
        name.clone(),
        ink,
    );
    let after = left + name.size().x + 7.0;
    let room = cell.right() - after;
    let detail = match row.note.is_empty() {
        true => row.state.word(),
        false => row.note,
    };
    if room > 0.0 {
        let mut job = egui::text::LayoutJob::default();
        job.append(
            detail,
            0.0,
            egui::TextFormat::simple(egui::FontId::proportional(NOTE), app::caption(&visuals)),
        );
        job.wrap = egui::text::TextWrapping::truncate_at_width(room);
        let note = painter.layout_job(job);
        painter.galley(
            egui::pos2(after, cell.center().y - note.size().y / 2.0),
            note,
            app::caption(&visuals),
        );
    }
    let hint = match row.note.is_empty() {
        true => row.state.word().to_string(),
        false => format!("{} — {}", row.state.word(), row.note),
    };
    ui.interact(
        cell,
        ui.id().with(("capability", row.name)),
        egui::Sense::hover(),
    )
    .on_hover_text(hint);
}

/// The offsets under their heading: three columns, mono for the two that are figures.
pub fn offsets(ui: &mut egui::Ui, rows: &[Offset]) {
    controls::heading(
        ui,
        "Offsets",
        "where each edited field lands in the file",
        None,
    );
    let hairline = egui::Stroke::new(1.0_f32, ui.visuals().widgets.noninteractive.bg_stroke.color);
    for row in rows {
        let (rect, _) = ui.allocate_exact_size(
            egui::vec2(ui.available_width(), ROW_H),
            egui::Sense::hover(),
        );
        let painter = ui.painter();
        painter.hline(rect.x_range(), rect.top() + 0.5, hairline);
        let inner = rect.shrink2(egui::vec2(PAD, 0.0));
        let track = inner.width() / 3.8;
        let ink = ui.visuals().text_color();
        let weak = ui.visuals().weak_text_color();
        let quiet = app::caption(ui.visuals());
        let cells = [
            (
                row.at.as_str(),
                egui::FontId::monospace(NAME),
                weak,
                inner.left(),
            ),
            (
                row.holds.as_str(),
                egui::FontId::monospace(NAME),
                ink,
                inner.left() + track,
            ),
            (
                row.note,
                egui::FontId::proportional(NOTE),
                quiet,
                inner.left() + track * 2.0,
            ),
        ];
        for (text, font, ink, left) in cells {
            let mut job = egui::text::LayoutJob::default();
            job.append(text, 0.0, egui::TextFormat::simple(font, ink));
            job.wrap = egui::text::TextWrapping::truncate_at_width((inner.right() - left).max(0.0));
            let galley = painter.layout_job(job);
            painter.galley(
                egui::pos2(left, rect.center().y - galley.size().y / 2.0),
                galley,
                ink,
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn words(output: &egui::FullOutput) -> Vec<String> {
        fn walk(shape: &egui::Shape, into: &mut Vec<String>) {
            match shape {
                egui::Shape::Text(text) => into.push(text.galley.text().to_string()),
                egui::Shape::Vec(shapes) => shapes.iter().for_each(|shape| walk(shape, into)),
                _ => {}
            }
        }
        let mut out = Vec::new();
        for clipped in &output.shapes {
            walk(&clipped.shape, &mut out);
        }
        out
    }

    #[test]
    fn the_badge_counts_and_the_note_names_what_is_absent() {
        let rows = [
            Row {
                name: "name",
                state: State::Editable,
                note: "",
            },
            Row {
                name: "loop points",
                state: State::ReadOnly,
                note: "baked",
            },
            Row {
                name: "release samples",
                state: State::Absent,
                note: "",
            },
            Row {
                name: "round trip",
                state: State::Verified,
                note: "",
            },
        ];
        let ctx = egui::Context::default();
        ctx.set_fonts(crate::app::fonts());
        let output = ctx.run(egui::RawInput::default(), |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| table(ui, &rows));
        });
        let painted = words(&output);
        assert!(
            painted.contains(&"2 live · 1 read · 1 absent".to_string()),
            "{painted:?}"
        );
        assert!(
            painted
                .iter()
                .any(|word| word
                    .starts_with("absent here because it is absent there: release samples")),
            "{painted:?}"
        );
        for row in &rows {
            assert!(
                painted.contains(&row.name.to_string()),
                "{} not painted",
                row.name
            );
        }
        assert!(
            painted.contains(&"editable".to_string()),
            "an empty note reads as the state's word"
        );
        assert!(painted.contains(&"baked".to_string()));
    }

    #[test]
    fn every_state_has_its_own_glyph_and_word() {
        let all = [
            State::Editable,
            State::ReadOnly,
            State::Absent,
            State::NeedsEncode,
            State::Verified,
        ];
        for (i, a) in all.iter().enumerate() {
            for b in &all[i + 1..] {
                assert_ne!(a.word(), b.word());
            }
        }
        assert_eq!(State::Absent.word(), "not in this format");
    }
}
