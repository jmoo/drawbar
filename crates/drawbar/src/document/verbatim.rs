//! The document for a body no registry describes.
//!
//! The container is read — format, version, checksum, where the body starts and how long
//! it is — and the body itself is carried untouched. There is nothing to draw as a
//! control and nothing that could be written differently, so the page says which of those
//! two it is, states what the container does say, and shows the bytes.

use eframe::egui;
use nord_format::accept::Family;
use nord_format::cbin::Generation;
use nord_format::Entity;

use super::capability::{facts, Fact};
use super::{controls, sample};
use crate::app;
use crate::browser::Kind;
use crate::icon::Glyph;
use crate::strings::kind_word;
use crate::workspace::LocalEntity;
use crate::{fields, room};

/// Whether this is a body the app can only keep as it found it.
///
/// ⚠️ Not a catch-all. A piano library is not one of these: its name and variant are
/// edited on the header, so a page saying nothing is editable would be false. The
/// instrument, the project and the set list have editors of their own.
pub fn is_verbatim(entity: &Entity) -> bool {
    !(fields::has_registry(entity)
        || fields::is_set_list(entity)
        || sample::is_sample(entity)
        || super::project::is_project(entity)
        || Kind::of(Some(entity)) == Kind::Piano)
}

/// How many bytes of the body the page shows, which is enough to recognise a header and
/// no more. The whole of it is on the Advanced face.
const SHOWN: usize = 192;
const PER_ROW: usize = 16;

const PAD: f32 = 12.0;
const HEX_TEXT: f32 = 10.5;
const HEX_ROW: f32 = 17.0;
const OFFSET_W: f32 = 44.0;
const BYTES_W: f32 = 350.0;
const SENTENCE: f32 = 11.0;
const COLUMN_MIN: f32 = 300.0;

/// The one sentence this page exists to say.
const WHY: &str = "No registry declares this model's fields yet, so there is nothing to draw \
                   and nothing to write differently. The file can still be sent, copied, \
                   tagged and placed; every byte goes up exactly as it came down.";

/// Where the body sits in the file: the container's own answer, so nothing is copied to
/// show it.
fn body(entity: &LocalEntity) -> &[u8] {
    let Some(container) = &entity.container else {
        return &entity.bytes;
    };
    let start = container.header.generation.body_start() as usize;
    let end = start.saturating_add(container.body_len as usize);
    entity.bytes.get(start..end).unwrap_or(&entity.bytes)
}

fn generation(generation: Generation) -> &'static str {
    match generation {
        Generation::V0 => "cbin v0",
        Generation::V1 => "cbin v1",
    }
}

/// What the container says about itself, which is the whole of what is known.
fn stated(entity: &LocalEntity) -> Vec<Fact> {
    let kind = Kind::of(entity.entity.as_ref());
    let tag = entity.tag();
    let mut rows = vec![
        Fact {
            key: "Format",
            value: tag.clone(),
            note: "the four-character tag the container carries",
        },
        Fact {
            key: "Model",
            value: kind_word(kind, Family::of_tag(&tag)),
            note: "what the tag is accepted as",
        },
    ];
    let Some(container) = &entity.container else {
        return rows;
    };
    rows.push(Fact {
        key: "Container",
        value: format!("{} · {}", generation(container.header.generation), tag),
        note: "read, checked and written back unchanged",
    });
    rows.push(Fact {
        key: "Body",
        value: format!("{} · verbatim", room::measure(container.body_len)),
        note: "kept byte for byte — no registry for this model",
    });
    rows.push(Fact {
        key: "Version",
        value: container.header.version.to_string(),
        note: "the schema version the file states",
    });
    rows.push(Fact {
        key: container.checksum_label.trim_end_matches(':'),
        value: match container.checksum_ok {
            true => format!("{} · ok", container.checksum),
            false => format!("{} · does not match the bytes", container.checksum),
        },
        note: "what the file stores, against what its bytes hash to",
    });
    rows.push(Fact {
        key: "Where",
        value: entity.origin.label(),
        note: "",
    });
    rows
}

/// The whole page. `true` when the one action on it was clicked.
pub fn ui(ui: &mut egui::Ui, entity: &LocalEntity) -> bool {
    controls::heading(
        ui,
        "Nothing to edit here yet",
        "drawbar reads this file's container and keeps its body byte-for-byte",
        None,
    );
    let rows = stated(entity);
    let wide = ui.available_width() >= COLUMN_MIN * 2.0;
    let saved = match wide {
        true => {
            // ⚠️ Two children with their own rects, not `allocate_ui(vec2(w, 0.0))`: a
            // zero-height allocation inside a row lets a label wrap at the row's whole
            // width and run out under the dock.
            let row = ui.available_rect_before_wrap();
            let split = row.left() + row.width() / 2.0;
            let mut left = ui.new_child(
                egui::UiBuilder::new()
                    .max_rect(egui::Rect::from_min_max(
                        row.min,
                        egui::pos2(split, row.max.y),
                    ))
                    .layout(egui::Layout::top_down(egui::Align::Min)),
            );
            facts(&mut left, &rows);
            let mut right = ui.new_child(
                egui::UiBuilder::new()
                    .max_rect(egui::Rect::from_min_max(
                        egui::pos2(split + PAD, row.min.y),
                        row.max,
                    ))
                    .layout(egui::Layout::top_down(egui::Align::Min)),
            );
            let saved = sentence(&mut right);
            ui.advance_cursor_after_rect(left.min_rect().union(right.min_rect()));
            saved
        }
        false => {
            facts(ui, &rows);
            ui.add_space(8.0);
            sentence(ui)
        }
    };

    let body = body(entity);
    let shown = body.len().min(SHOWN);
    controls::heading(
        ui,
        "Body",
        &format!(
            "{} kept verbatim — the first {shown} shown",
            room::measure(body.len() as u64)
        ),
        None,
    );
    hex(ui, body, 0..shown.div_ceil(PER_ROW));
    saved
}

/// Why the page is empty, and the one thing to do about it.
fn sentence(ui: &mut egui::Ui) -> bool {
    let quiet = ui.visuals().weak_text_color();
    ui.vertical(|ui| {
        ui.spacing_mut().item_spacing.y = 8.0;
        ui.add(egui::Label::new(
            egui::RichText::new(WHY).size(SENTENCE).color(quiet),
        ));
        sample::action(ui, "Save a copy…", Glyph::Save, false)
    })
    .inner
}

/// The Advanced face: every byte of the body, a screenful at a time.
pub fn bytes(ui: &mut egui::Ui, entity: &LocalEntity) {
    let body = body(entity);
    let rows = body.len().div_ceil(PER_ROW);
    controls::heading(
        ui,
        "Body bytes",
        &format!("{}, kept verbatim", room::measure(body.len() as u64)),
        Some((&format!("{rows} rows"), app::caption(ui.visuals()))),
    );
    // ⚠️ A piano library is hundreds of megabytes; laying out one text row per sixteen
    // bytes of it is millions of galleys per frame. Only the rows on screen are drawn.
    egui::ScrollArea::vertical()
        .id_salt("verbatim_body")
        .max_height(HEX_ROW * 24.0)
        .auto_shrink([false, true])
        .show_rows(ui, HEX_ROW, rows, |ui, range| hex(ui, body, range));
}

/// `offset  bytes  ascii`, one row per sixteen bytes of `body`.
fn hex(ui: &mut egui::Ui, body: &[u8], rows: std::ops::Range<usize>) {
    let visuals = ui.visuals().clone();
    let quiet = app::caption(&visuals);
    let ink = visuals.weak_text_color();
    for row in rows {
        let at = row * PER_ROW;
        let Some(held) = body.get(at..(at + PER_ROW).min(body.len())) else {
            return;
        };
        let (rect, _) = ui.allocate_exact_size(
            egui::vec2(ui.available_width(), HEX_ROW),
            egui::Sense::hover(),
        );
        let painter = ui.painter();
        let mono = egui::FontId::monospace(HEX_TEXT);
        let cells = [
            (format!("{at:04x}"), quiet, rect.left() + PAD),
            (
                held.iter()
                    .map(|byte| format!("{byte:02x}"))
                    .collect::<Vec<_>>()
                    .join(" "),
                ink,
                rect.left() + PAD + OFFSET_W,
            ),
            (
                held.iter().map(|byte| readable(*byte)).collect(),
                quiet,
                rect.left() + PAD + OFFSET_W + BYTES_W,
            ),
        ];
        for (text, tint, left) in cells {
            let galley = painter.layout_no_wrap(text, mono.clone(), tint);
            painter.galley(
                egui::pos2(left, rect.center().y - galley.size().y / 2.0),
                galley,
                tint,
            );
        }
    }
}

/// A byte as the dump prints it: itself where it is printable ASCII, a stop otherwise.
fn readable(byte: u8) -> char {
    match byte {
        0x20..=0x7e => byte as char,
        _ => '.',
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The kinds with an editor of their own are not swept up by the catch-all, and the
    /// ones with nothing else are.
    #[test]
    fn only_a_body_with_no_editor_of_its_own_is_verbatim() {
        let decode = |bytes: Vec<u8>| {
            nord_format::from_stream(&mut std::io::Cursor::new(&bytes)).expect("it decodes")
        };
        assert!(
            is_verbatim(&decode(fields::blank::stage3_song())),
            "a song that decodes no further than its container"
        );
        assert!(
            !is_verbatim(&decode(fields::blank::electro5_song())),
            "a set list has its own four rows"
        );
        assert!(
            !is_verbatim(&decode(fields::blank::stage4_program())),
            "a registry body has its panel"
        );
    }

    /// ⚠️ The dump lays out only the rows it was asked for. A piano library is
    /// hundreds of megabytes, and one galley per sixteen bytes of it is a frame that
    /// never finishes.
    #[test]
    fn the_dump_lays_out_only_the_rows_it_was_asked_for() {
        let body: Vec<u8> = (0..64 * 1024).map(|byte| byte as u8).collect();
        let ctx = egui::Context::default();
        ctx.set_fonts(crate::app::fonts());
        let output = ctx.run(egui::RawInput::default(), |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| hex(ui, &body, 100..103));
        });
        fn walk(shape: &egui::Shape, into: &mut Vec<String>) {
            match shape {
                egui::Shape::Text(text) => into.push(text.galley.text().to_string()),
                egui::Shape::Vec(shapes) => shapes.iter().for_each(|shape| walk(shape, into)),
                _ => {}
            }
        }
        let mut painted = Vec::new();
        for clipped in &output.shapes {
            walk(&clipped.shape, &mut painted);
        }
        let offsets: Vec<&String> = painted
            .iter()
            .filter(|word| word.len() == 4 && word.chars().all(|c| c.is_ascii_hexdigit()))
            .collect();
        assert_eq!(offsets, ["0640", "0650", "0660"], "{painted:?}");
    }

    /// Non-printable bytes never reach the dump as characters of their own.
    #[test]
    fn the_ascii_column_prints_only_printable_bytes() {
        assert_eq!(readable(b'N'), 'N');
        assert_eq!(readable(b' '), ' ');
        assert_eq!(readable(0x7e), '~');
        for byte in [0x00, 0x1f, 0x7f, 0x80, 0xff] {
            assert_eq!(readable(byte), '.', "{byte:#04x}");
        }
    }
}
