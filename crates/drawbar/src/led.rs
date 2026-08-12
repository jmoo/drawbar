//! A lamp with a switch under it: what the panel puts a yes/no on.
//!
//! The instrument has no tick boxes. It has buttons that light, so this is a button that
//! lights — lit is on, dark is off, and the two are told apart by brightness rather than
//! by a glyph, which is how the panel is read from a stage away.

use eframe::egui;

const LENS: f32 = 9.0;
const PAD: egui::Vec2 = egui::vec2(8.0, 5.0);

/// A lit button carrying `word`. Returns the state it was switched to.
///
/// Focusable and switched by Space or Enter as well as by a click, because a panel
/// control that only answers the mouse is a control half the operators cannot reach.
pub fn ui(ui: &mut egui::Ui, on: bool, word: &str) -> Option<bool> {
    let text = egui::WidgetText::from(egui::RichText::new(word).small());
    let galley = text.into_galley(
        ui,
        Some(egui::TextWrapMode::Extend),
        f32::INFINITY,
        egui::TextStyle::Small,
    );
    let size = egui::vec2(
        galley.size().x + LENS + PAD.x * 2.0 + 5.0,
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
            // The glow is what carries "on" at a glance; the lens alone is a dot.
            true => {
                painter.circle_filled(lens, LENS / 2.0 + 2.0, lit.gamma_multiply(0.30));
                painter.circle_filled(lens, LENS / 2.0, lit);
            }
            false => {
                // A dark lens on a dark panel is the panel's own colour; on a light one
                // it has to be a grey, or an unlit lamp is invisible against the button.
                let dark_lens = match visuals.dark_mode {
                    true => visuals.extreme_bg_color,
                    false => egui::Color32::from_gray(0x88),
                };
                painter.circle_filled(lens, LENS / 2.0, dark_lens);
                painter.circle_stroke(
                    lens,
                    LENS / 2.0,
                    egui::Stroke::new(1.0, crate::app::unlit(visuals)),
                );
            }
        }
        painter.galley(
            egui::pos2(
                lens.x + LENS / 2.0 + 5.0,
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

    /// Driving one lamp through a click and through the keyboard, the way both an
    /// operator and a screen reader reach it.
    fn press(events: Vec<egui::Event>, focus: bool) -> Option<bool> {
        let ctx = egui::Context::default();
        let mut answer = None;
        // Two passes: the first lays the lamp out, the second delivers the input to the
        // rect the first one claimed.
        for pass in 0..2 {
            let input = egui::RawInput {
                events: match pass {
                    0 => Vec::new(),
                    _ => events.clone(),
                },
                ..Default::default()
            };
            let _ = ctx.run(input, |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| {
                    if focus {
                        let id = ui.next_auto_id();
                        ui.memory_mut(|m| m.request_focus(id));
                    }
                    if let Some(want) = super::ui(ui, false, "vibrato") {
                        answer = Some(want);
                    }
                });
            });
        }
        answer
    }

    fn at(pos: egui::Pos2, pressed: bool) -> egui::Event {
        egui::Event::PointerButton {
            pos,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: egui::Modifiers::default(),
        }
    }

    #[test]
    fn a_click_switches_the_lamp() {
        let on = egui::pos2(20.0, 18.0);
        let switched = press(
            vec![
                egui::Event::PointerMoved(on),
                at(on, true),
                at(on, false),
                egui::Event::PointerGone,
            ],
            false,
        );
        assert_eq!(switched, Some(true), "a click lights a dark lamp");
    }

    /// A panel control that only answers the mouse is one half the operators cannot
    /// reach, so the focused lamp switches on Space.
    #[test]
    fn space_switches_the_focused_lamp() {
        let switched = press(
            vec![egui::Event::Key {
                key: egui::Key::Space,
                physical_key: None,
                pressed: true,
                repeat: false,
                modifiers: egui::Modifiers::default(),
            }],
            true,
        );
        assert_eq!(switched, Some(true));
    }
}
