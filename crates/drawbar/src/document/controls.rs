//! The pieces every face of a document is built out of: the per-field cache a cell reads
//! through, the sets a frame collects, and the two containers a page is divided by.
//!
//! Nothing here writes: a control hands back a `path = value` set, the document collects
//! every set the frame produced and applies them together, so a control that owns two
//! fields moves both or neither.

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use eframe::egui;
use nord_format::fields::Field;

use crate::strings;

/// What every cell needs and none of them should compute twice.
///
/// ⚠️ Asking a field for its legal values walks every bit pattern it can hold — four
/// thousand at the enumerable ceiling — and a Stage body declares hundreds of fields, so
/// a field is asked the first time something draws it and never again. A section nobody
/// has opened costs nothing.
///
/// ⚠️ Keyed by path, so one belongs to one document: two formats declare paths that
/// collide, and a shared cache would hand one body's control the other's values.
#[derive(Default)]
pub struct Ctx {
    read: RefCell<HashMap<String, Rc<Vec<String>>>>,
}

impl Ctx {
    /// Every value the field accepts, spelled the way `set_field` takes them. Empty above
    /// the enumerable ceiling, where the stored bits are the only spelling there is.
    pub fn legal(&self, field: &Field) -> Rc<Vec<String>> {
        if let Some(legal) = self.read.borrow().get(&field.path) {
            return Rc::clone(legal);
        }
        let legal = Rc::new((field.spec.legal)());
        self.read
            .borrow_mut()
            .insert(field.path.clone(), Rc::clone(&legal));
        legal
    }
}

/// Collected `path = value` sets, applied together once the frame is painted.
pub type Sets = Vec<(String, String)>;

/// A titled panel.
///
/// The instrument's front panel does not fold its sections away, and neither does this:
/// a control you cannot see is a control you do not know you have.
pub fn section(ui: &mut egui::Ui, title: &str, body: impl FnOnce(&mut egui::Ui)) {
    egui::Frame::group(ui.style()).show(ui, |ui| {
        ui.set_width(ui.available_width());
        ui.label(egui::RichText::new(title).strong());
        ui.separator();
        body(ui);
    });
    ui.add_space(2.0);
}

/// A section of a page: a sentence-case title, a note beside it, and an optional reading
/// at the right.
///
/// ⚠️ Not [`crate::panel::panel_header`], which is the shell's: MICRO caps on
/// `faint_bg_color`, with a bar across the dock. A document's sections are part of the
/// page they are on, and a row of grey bars down a page reads as a stack of panels.
pub fn heading(ui: &mut egui::Ui, title: &str, note: &str, right: Option<(&str, egui::Color32)>) {
    const ROW: f32 = 18.0;
    const PAD: f32 = 12.0;
    const GAP: f32 = 8.0;
    const TITLE: f32 = 12.0;
    const NOTE: f32 = 10.5;
    const READING: f32 = 10.0;

    // The page's own margin stands above the first heading on it; the rest carry their
    // own room from the section before them.
    let above = match ui.min_rect().height() > 0.0 {
        true => 12.0,
        false => 8.0,
    };
    ui.add_space(above);
    let (rect, _) =
        ui.allocate_exact_size(egui::vec2(ui.available_width(), ROW), egui::Sense::hover());
    let mut row = ui.new_child(
        egui::UiBuilder::new()
            .max_rect(rect.shrink2(egui::vec2(PAD, 0.0)))
            .layout(egui::Layout::left_to_right(egui::Align::Center)),
    );
    row.spacing_mut().item_spacing.x = GAP;
    let ink = row.visuals().text_color();
    row.label(
        egui::RichText::new(title)
            .font(egui::FontId::new(TITLE, crate::app::bold()))
            .color(ink),
    );
    let caption = crate::app::caption(row.visuals());
    row.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
        if let Some((reading, tint)) = right {
            ui.label(
                egui::RichText::new(reading)
                    .font(egui::FontId::monospace(READING))
                    .color(tint),
            );
        }
        ui.with_layout(egui::Layout::left_to_right(egui::Align::Center), |ui| {
            ui.add(
                egui::Label::new(
                    egui::RichText::new(note)
                        .font(egui::FontId::proportional(NOTE))
                        .color(caption),
                )
                .truncate(),
            );
        });
    });
    ui.add_space(4.0);
}

/// A cell whose control is the caller's, with the panel's name for it underneath.
///
/// `path` names the caption; an unmapped one still gets the prettified fallback, so a
/// field the strings table has not caught up with reads as a rough name rather than as a
/// nameless knob.
pub fn named_cell(
    ui: &mut egui::Ui,
    path: &str,
    width: f32,
    body: impl FnOnce(&mut egui::Ui),
) -> egui::Response {
    ui.allocate_ui(egui::vec2(width, 0.0), |ui| {
        ui.vertical_centered(|ui| {
            ui.spacing_mut().item_spacing.y = 3.0;
            body(ui);
            let rough = !strings::known(path);
            let mut text = egui::RichText::new(strings::label(path)).small();
            if rough {
                text = text.italics();
            }
            let response = ui.add(egui::Label::new(text.color(ui.visuals().weak_text_color())));
            if rough {
                response.on_hover_text(format!("{path} — this app has no name for it yet"));
            }
        });
    })
    .response
}

#[cfg(test)]
mod tests {
    use super::*;

    /// ⚠️ A document's section heading is part of the page, not a bar across it: it says
    /// its three parts in sentence case and paints no ground of its own. The shell's
    /// [`crate::panel::panel_header`] is the one that wears `faint_bg_color`.
    #[test]
    fn a_section_heading_says_its_parts_and_paints_no_bar() {
        fn walk(shape: &egui::Shape, into: &mut (Vec<String>, Vec<egui::Color32>)) {
            match shape {
                egui::Shape::Text(text) => into.0.push(text.galley.text().to_string()),
                egui::Shape::Rect(drawn) => into.1.push(drawn.fill),
                egui::Shape::Vec(shapes) => shapes.iter().for_each(|shape| walk(shape, into)),
                _ => {}
            }
        }

        let ctx = egui::Context::default();
        ctx.set_fonts(crate::app::fonts());
        let warn = crate::app::warn(&ctx.style().visuals);
        let output = ctx.run(egui::RawInput::default(), |ctx| {
            ctx.style_mut(crate::app::metrics);
            egui::CentralPanel::default().show(ctx, |ui| {
                heading(ui, "Key map", "drag a top", Some(("1 silent range", warn)));
            });
        });
        let mut painted = (Vec::new(), Vec::new());
        for clipped in &output.shapes {
            walk(&clipped.shape, &mut painted);
        }
        assert_eq!(painted.0, ["Key map", "1 silent range", "drag a top"]);
        assert!(
            painted
                .1
                .iter()
                .all(|fill| *fill == ctx.style().visuals.panel_fill),
            "the heading painted a ground of its own: {:?}",
            painted.1,
        );
    }

    /// ⚠️ A field is asked for its values when something draws it and not before, and
    /// then never again. A Stage body declares hundreds of fields, and walking every one
    /// of them on open is a stall the operator spends watching an empty document.
    #[test]
    fn a_field_is_read_as_it_is_drawn_and_only_once() {
        let bytes = crate::fields::blank::stage4_program();
        let (fields, _) = crate::fields::apply(&bytes, &[]).unwrap();
        let ctx = Ctx::default();
        assert!(fields.len() > 800);
        assert_eq!(ctx.read.borrow().len(), 0, "nothing drawn, nothing asked");

        ctx.legal(&fields[0]);
        ctx.legal(&fields[0]);
        assert_eq!(ctx.read.borrow().len(), 1);
    }
}
