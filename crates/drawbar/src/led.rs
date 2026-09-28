//! A lamp with a switch under it, for an on/off value as on the panel.
//!
//! The instrument has no checkboxes, only buttons that light, so this is a button that
//! lights: lit is on and dark is off. The two differ in brightness, not by a glyph, which
//! is how the panel is read from across a stage.

use eframe::egui;

const LENS: f32 = 9.0;
const PAD: egui::Vec2 = egui::vec2(8.0, 5.0);
/// The gap between the lens and its label.
const GAP: f32 = 5.0;

/// A lit button labeled `word`. Returns the state it was switched to.
///
/// Focusable, and switched by Space or Enter as well as by a click, so it can be used
/// without a mouse.
pub fn ui(ui: &mut egui::Ui, on: bool, word: &str) -> Option<bool> {
    let text = egui::WidgetText::from(egui::RichText::new(word).small());
    let galley = text.into_galley(
        ui,
        Some(egui::TextWrapMode::Extend),
        f32::INFINITY,
        egui::TextStyle::Small,
    );
    let size = egui::vec2(
        galley.size().x + LENS + PAD.x * 2.0 + GAP,
        galley.size().y.max(LENS) + PAD.y * 2.0,
    );
    let (rect, response) = ui.allocate_exact_size(size, egui::Sense::click());

    let mut switched = response.clicked();
    if response.has_focus() {
        switched |=
            ui.input(|i| i.key_pressed(egui::Key::Space) || i.key_pressed(egui::Key::Enter));
    }
    let showing = match switched {
        true => !on,
        false => on,
    };

    if ui.is_rect_visible(rect) {
        let visuals = ui.visuals();
        let widget = ui.style().interact(&response);
        let painter = ui.painter();
        painter.rect_filled(rect, 4.0, widget.bg_fill);
        let edge = match response.has_focus() {
            true => visuals.selection.stroke,
            false => widget.bg_stroke,
        };
        painter.rect_stroke(rect, 4.0, edge, egui::StrokeKind::Inside);

        let lens = egui::pos2(rect.left() + PAD.x + LENS / 2.0, rect.center().y);
        let lit = crate::app::accent(visuals);
        match showing {
            // The glow shows "on" at a glance; the lens alone is just a dot.
            true => {
                painter.circle_filled(lens, LENS / 2.0 + 2.0, lit.gamma_multiply(0.30));
                painter.circle_filled(lens, LENS / 2.0, lit);
            }
            false => {
                // On a dark panel an unlit lens takes the panel's color; on a light one
                // it must be gray, or it disappears against the button.
                let dark_lens = match visuals.dark_mode {
                    true => visuals.extreme_bg_color,
                    false => egui::Color32::from_gray(0x88),
                };
                painter.circle_filled(lens, LENS / 2.0, dark_lens);
                painter.circle_stroke(
                    lens,
                    LENS / 2.0,
                    egui::Stroke::new(1.0_f32, crate::app::unlit(visuals)),
                );
            }
        }
        painter.galley(
            egui::pos2(
                lens.x + LENS / 2.0 + GAP,
                rect.center().y - galley.size().y / 2.0,
            ),
            galley,
            widget.fg_stroke.color,
        );
    }

    switched.then_some(showing)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::{self, context};

    /// Drive one lamp by pointer or keyboard. `events` is handed the lamp's middle.
    fn press(events: impl Fn(egui::Pos2) -> Vec<egui::Event>, focus: bool) -> Option<bool> {
        let ctx = context();
        let mut answer = None;
        let mut lamp = egui::Id::NULL;
        // Two passes: the first lays the lamp out, the second delivers the input to the
        // rect the first one claimed.
        for pass in 0..2 {
            let input = egui::RawInput {
                events: match pass {
                    0 => Vec::new(),
                    _ => events(
                        ctx.read_response(lamp)
                            .expect("the lamp was drawn")
                            .rect
                            .center(),
                    ),
                },
                ..Default::default()
            };
            testing::run(&ctx, input, |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| {
                    lamp = ui.next_auto_id();
                    if focus {
                        ui.memory_mut(|m| m.request_focus(lamp));
                    }
                    if let Some(want) = super::ui(ui, false, "vibrato") {
                        answer = Some(want);
                    }
                });
            });
        }
        answer
    }

    #[test]
    fn a_click_switches_the_lamp() {
        let switched = press(
            |on| {
                let mut clicked = testing::click(on);
                clicked.push(egui::Event::PointerGone);
                clicked
            },
            false,
        );
        assert_eq!(switched, Some(true), "a click lights a dark lamp");
    }

    #[test]
    fn space_switches_the_focused_lamp() {
        let switched = press(|_| vec![testing::key(egui::Key::Space)], true);
        assert_eq!(switched, Some(true));
    }
}
