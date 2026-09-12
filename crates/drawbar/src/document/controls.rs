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
use crate::{drawbar_widget, knob, led, strings};

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
        Control::Bar(_) => 44.0,
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

/// A parameter with the performance controls that morph it underneath.
///
/// ⚠️ A morph target is the value the parameter is driven *to*, not a setting of its own —
/// drawn beside its parameter it reads as a second knob for the same thing. The
/// declaration says which parameter each slot moves; nothing here matches names.
pub fn morphed(ui: &mut egui::Ui, ctx: &Ctx, field: &Field, morphs: &[&Field], sets: &mut Sets) {
    if morphs.is_empty() {
        return cell(ui, ctx, field, sets);
    }
    let span = width(ctx.control(field)).max(MORPH_W * morphs.len() as f32);
    ui.allocate_ui(egui::vec2(span, 0.0), |ui| {
        ui.vertical_centered(|ui| {
            ui.spacing_mut().item_spacing.y = 3.0;
            if let Some(value) = control(ui, ctx, field) {
                sets.push((field.path.clone(), value));
            }
            caption(ui, &field.path);
            strip(ui, |ui| {
                for slot in morphs {
                    ui.allocate_ui(egui::vec2(MORPH_W, 0.0), |ui| {
                        ui.vertical_centered(|ui| {
                            ui.spacing_mut().item_spacing.y = 2.0;
                            if let Some(value) = control(ui, ctx, slot) {
                                sets.push((slot.path.clone(), value));
                            }
                            ui.label(egui::RichText::new(morph_label(&slot.path)).small().weak());
                        });
                    });
                }
            });
        });
    });
}

/// How wide one morph target's cell is — narrower than a control of its own, because it
/// is a reading of the parameter above it.
const MORPH_W: f32 = 56.0;

/// The performance control a morph slot belongs to, off the suffix the declaration binds
/// on. A slot spelled some other way reads as its own leaf name.
fn morph_label(path: &str) -> &str {
    let leaf = path.rsplit('.').next().unwrap_or(path);
    for (suffix, word) in [
        ("_wheel", "wheel"),
        ("_aftertouch", "after"),
        ("_ctrl_pedal", "pedal"),
    ] {
        if leaf.ends_with(suffix) {
            return word;
        }
    }
    leaf
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
        Control::Bar(rank) => bar(ui, field, rank),
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

/// The values a picker offers.
///
/// A value the library could not name is never something to choose. If the file holds one
/// it stays in the list all the same, so a change away from it can be put back.
fn choices(path: &str, legal: &[String], current: &str) -> Vec<String> {
    let mut out: Vec<String> = legal
        .iter()
        .filter(|value| offerable(path, value))
        .cloned()
        .collect();
    if !out.iter().any(|value| value == current) {
        out.push(current.to_string());
    }
    out
}

/// Whether a value is one a player would pick.
///
/// `Routing::Unknown` is a named variant rather than an unrecognised number, but it is
/// how older firmware spelled *off* and it presents as off. Two entries both meaning off
/// are not offered together.
/// Confirmed on hardware.
fn offerable(path: &str, value: &str) -> bool {
    if strings::unrecognised(value).is_some() {
        return false;
    }
    !(value == "Unknown"
        && matches!(
            path,
            "effects_panel.fx1" | "effects_panel.fx2" | "effects_panel.fx3" | "effects_panel.fx4"
        ))
}

/// A named-value picker. Shows the panel's word for each value and sets the library's.
fn choice(ui: &mut egui::Ui, ctx: &Ctx, field: &Field) -> Option<String> {
    let entry = ctx.entry(field);
    let offered = choices(&field.path, &entry.legal, &field.value);
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

/// One drawbar, for the bodies that give each bar its own field. Which rank it is the
/// field's own declaration says; a bar placed in no register draws claiming none.
fn bar(ui: &mut egui::Ui, field: &Field, rank: Option<usize>) -> Option<String> {
    let position = field.value.trim().parse().ok()?;
    let moved = drawbar_widget::ui_one(ui, rank, position, true);
    moved.map(|moved| moved.to_string())
}

/// Nine drawbars and the positions under them. No hex: the digits are the readout.
pub fn register(ui: &mut egui::Ui, field: &Field, live: bool) -> Option<String> {
    let bits = drawbar_widget::parse(&field.value)?;
    bars(
        ui,
        drawbar_widget::bars(bits),
        live,
        &drawbar_widget::ALL_RANKS,
    )
    .map(|moved| drawbar_widget::spell(drawbar_widget::bits(moved)))
}

/// The drawbars themselves, plus the digits. Returns the positions when one is pulled.
pub fn bars(
    ui: &mut egui::Ui,
    positions: [u8; drawbar_widget::BARS],
    live: bool,
    ranks: &[usize],
) -> Option<[u8; drawbar_widget::BARS]> {
    let mut moved = None;
    let count = ranks.len().min(drawbar_widget::BARS);
    ui.vertical(|ui| {
        moved = drawbar_widget::ui_ranks(ui, positions, live, ranks);
        let shown = moved.unwrap_or(positions);
        ui.label(
            egui::RichText::new(drawbar_widget::digits(&shown[..count]))
                .monospace()
                .small()
                .weak(),
        );
    });
    moved
}

#[cfg(test)]
mod tests {
    use super::*;

    /// An unrecognised value is not offered, but a file holding one keeps it reachable.
    #[test]
    fn an_unrecognised_value_is_kept_but_never_offered() {
        let legal: Vec<String> = ["B3", "B3Bass", "Pipe", "unknown (6)"]
            .iter()
            .map(|s| s.to_string())
            .collect();

        assert_eq!(
            choices("center_panel.organ_type", &legal, "B3"),
            ["B3", "B3Bass", "Pipe"]
        );
        // Holding one: it is the last entry, so changing away from it can be undone.
        assert_eq!(
            choices("center_panel.organ_type", &legal, "unknown (6)"),
            ["B3", "B3Bass", "Pipe", "unknown (6)"]
        );
    }

    /// Two spellings of off would read as two different settings.
    #[test]
    fn the_older_spelling_of_off_is_not_offered_alongside_off() {
        let legal: Vec<String> = ["Off", "Unknown", "Lower", "Upper"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        assert_eq!(
            choices("effects_panel.fx1", &legal, "Off"),
            ["Off", "Lower", "Upper"]
        );
        // A file holding it keeps it, spelled for what it is.
        assert_eq!(
            choices("effects_panel.fx1", &legal, "Unknown"),
            ["Off", "Lower", "Upper", "Unknown"]
        );
        // The same variant name elsewhere is a real choice.
        assert!(offerable("some_other_field", "Unknown"));
    }

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

        ctx.control(&fields[0]);
        ctx.control(&fields[0]);
        assert_eq!(ctx.read.borrow().len(), 1);
    }
}
