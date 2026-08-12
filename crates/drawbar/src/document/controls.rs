//! The pieces a panel is built out of: a titled section, a run of controls across it,
//! and one control with its name printed underneath.
//!
//! A control reads its value out of the field list and hands back a `path = value` set
//! when the user moves it. Nothing here writes: the document collects every set the frame
//! produced and applies them together, so a control that owns two fields moves both or
//! neither.
//!
//! The arrangement is the instrument's, not a form's. Names sit **under** what they name,
//! controls run left to right in strips and wrap when the window is narrow, and a value
//! is turned or lit rather than typed — the panel has no text boxes, so neither does
//! this, until you ask one for a number.

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use eframe::egui;
use nord_format::fields::Field;

use crate::fields::Control;
use crate::{drawbar_widget, knob, led, strings, visibility};

/// What every row needs and none of them should compute twice.
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
    read: RefCell<HashMap<String, Rc<Entry>>>,
}

/// A field's legal values and the control they picked.
struct Entry {
    control: Control,
    legal: Vec<String>,
}

impl Ctx {
    fn entry(&self, field: &Field) -> Rc<Entry> {
        if let Some(entry) = self.read.borrow().get(&field.path) {
            return Rc::clone(entry);
        }
        let legal = (field.spec.legal)();
        let entry = Rc::new(Entry {
            control: Control::of(field, &legal),
            legal,
        });
        self.read
            .borrow_mut()
            .insert(field.path.clone(), Rc::clone(&entry));
        entry
    }

    pub fn control(&self, field: &Field) -> Control {
        self.entry(field).control
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

/// A run of controls across the panel, wrapping when it runs out of width.
///
/// ⚠️ Aligned to the **top** of the row rather than egui's default centre. A cell asks
/// for its width and lets its height follow its contents, and a centred row hands each
/// one the whole remaining height to be centred in — which leaves the controls staggered
/// down the row and the rows themselves hundreds of points tall.
pub fn strip(ui: &mut egui::Ui, body: impl FnOnce(&mut egui::Ui)) {
    let row = egui::Layout::left_to_right(egui::Align::TOP).with_main_wrap(true);
    ui.with_layout(row, |ui| {
        ui.spacing_mut().item_spacing = egui::vec2(10.0, 10.0);
        body(ui);
    });
}

/// How wide a cell is, which is as wide as what stands in it.
fn width(control: Control) -> f32 {
    match control {
        Control::Choice => 156.0,
        Control::Stored => 140.0,
        Control::Register => 220.0,
        Control::Bar => 44.0,
        _ => 78.0,
    }
}

/// One field as a cell: the control, and the panel's name for it underneath.
pub fn cell(ui: &mut egui::Ui, ctx: &Ctx, field: &Field, sets: &mut Sets) {
    named_cell(ui, &field.path, width(ctx.control(field)), |ui| {
        if let Some(value) = control(ui, ctx, field) {
            sets.push((field.path.clone(), value));
        }
    });
}

/// A cell whose control is the caller's — for the two that are not one field each.
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
            caption(ui, path);
        });
    })
    .response
}

/// The name under a control, in the panel's own words.
fn caption(ui: &mut egui::Ui, path: &str) {
    let rough = !strings::known(path);
    let mut text = egui::RichText::new(strings::label(path)).small();
    if rough {
        text = text.italics();
    }
    let response = ui.add(egui::Label::new(text.color(ui.visuals().weak_text_color())));
    if rough {
        response.on_hover_text(format!("{path} — this app has no name for it yet"));
    }
}

/// The control alone, without its name.
pub fn control(ui: &mut egui::Ui, ctx: &Ctx, field: &Field) -> Option<String> {
    match ctx.control(field) {
        Control::Toggle => toggle(ui, field),
        Control::Choice => choice(ui, ctx, field),
        Control::Number { min, max } => number(ui, field, min, max),
        Control::Bar => bar(ui, field),
        Control::Register => register(ui, field, true),
        // A field with no control here has only its stored value, and that is an
        // engineer's business — the Advanced table is where it is legible.
        Control::Stored => {
            ui.label(
                egui::RichText::new(&field.display)
                    .monospace()
                    .small()
                    .weak(),
            )
            .on_hover_text("stored as-is; see Advanced");
            None
        }
    }
}

/// A lamp with no word beside it: the cell's own caption names it.
fn toggle(ui: &mut egui::Ui, field: &Field) -> Option<String> {
    let on = field.value == "true";
    led::ui(ui, on, "").map(|want| want.to_string())
}

/// A named-value picker. Shows the panel's word for each value and sets the library's.
fn choice(ui: &mut egui::Ui, ctx: &Ctx, field: &Field) -> Option<String> {
    let entry = ctx.entry(field);
    let offered = visibility::choices(&field.path, &entry.legal, &field.value);
    let mut picked = None;
    egui::ComboBox::from_id_salt(&field.path)
        .selected_text(
            egui::RichText::new(strings::value_label(&field.path, &field.value))
                .text_style(egui::TextStyle::Small),
        )
        .width(ui.available_width().min(width(Control::Choice) - 12.0))
        .show_ui(ui, |ui| {
            for value in &offered {
                let label = strings::value_label(&field.path, value);
                if ui.selectable_label(*value == field.value, label).clicked() {
                    picked = Some(value.clone());
                }
            }
        });
    picked.filter(|value| *value != field.value)
}

/// A knob, because that is what the panel puts a continuous value on.
fn number(ui: &mut egui::Ui, field: &Field, min: i64, max: i64) -> Option<String> {
    let value: i64 = field.value.trim_start_matches('+').parse().ok()?;
    knob::ui(ui, &field.path, value, min, max).map(|moved| moved.to_string())
}

/// One drawbar, for the bodies that give each bar its own field. Which rank it is comes
/// off its name — see [`drawbar_widget::rank`].
fn bar(ui: &mut egui::Ui, field: &Field) -> Option<String> {
    let position = field.value.trim().parse().ok()?;
    let moved = drawbar_widget::ui_one(ui, drawbar_widget::rank(&field.path), position, true);
    moved.map(|moved| moved.to_string())
}

/// Nine drawbars and the positions under them. No hex: the digits are the readout.
pub fn register(ui: &mut egui::Ui, field: &Field, live: bool) -> Option<String> {
    let bits = drawbar_widget::parse(&field.value)?;
    bars(ui, drawbar_widget::bars(bits), live, drawbar_widget::BARS)
        .map(|moved| drawbar_widget::spell(drawbar_widget::bits(moved)))
}

/// The drawbars themselves, plus the digits. Returns the positions when one is pulled.
pub fn bars(
    ui: &mut egui::Ui,
    positions: [u8; drawbar_widget::BARS],
    live: bool,
    count: usize,
) -> Option<[u8; drawbar_widget::BARS]> {
    let mut moved = None;
    ui.vertical(|ui| {
        moved = drawbar_widget::ui_count(ui, positions, live, count);
        let shown = moved.unwrap_or(positions);
        ui.label(
            egui::RichText::new(drawbar_widget::digits(&shown[..count.min(shown.len())]))
                .monospace()
                .small()
                .weak(),
        );
    });
    moved
}

/// A lamp with its own word beside it, for the switches that sit inside a preset.
pub fn switch(ui: &mut egui::Ui, field: Option<&Field>, word: &str, sets: &mut Sets) {
    let Some(field) = field else {
        return;
    };
    let on = field.value == "true";
    if let Some(want) = led::ui(ui, on, word) {
        sets.push((field.path.clone(), want.to_string()));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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

        ctx.control(&fields[0]);
        ctx.control(&fields[0]);
        assert_eq!(ctx.read.borrow().len(), 1);
    }
}
