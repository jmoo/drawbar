//! The header a dock wears, and the geometry every dock header shares.

use eframe::egui;

use crate::icon::{icon, Glyph};

/// How tall a panel header is, wherever it is drawn.
pub const HEADER: f32 = 24.0;

/// The room a header keeps at each end.
const PAD: f32 = 8.0;

/// The gap between a header's parts.
const GAP: f32 = 6.0;

/// The collapse triangle's box.
const CHEVRON: f32 = 12.0;

/// A header title: [`crate::app::micro`], uppercased.
///
/// Uppercasing is the whole of the treatment — egui has no letter spacing, and a faked
/// one is worse than none.
pub fn caps(text: &str) -> egui::RichText {
    egui::RichText::new(text.to_uppercase()).text_style(crate::app::micro())
}

/// 24 px, faint_bg, an optional collapse triangle, a MICRO-caps title, an optional badge.
pub fn panel_header(
    ui: &mut egui::Ui,
    title: &str,
    open: Option<&mut bool>,
    badge: Option<(&str, egui::Color32)>,
) -> egui::Response {
    strip(ui, |ui| {
        if let Some(open) = open {
            if chevron(ui, *open).clicked() {
                *open = !*open;
            }
        }
        let ink = ui.visuals().widgets.noninteractive.fg_stroke.color;
        ui.label(caps(title).color(ink));
        let Some((badge, tint)) = badge else {
            return;
        };
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            ui.label(
                egui::RichText::new(badge)
                    .text_style(crate::app::micro())
                    .color(tint),
            );
        });
    })
}

/// The bar a header is drawn into: full bleed, padded at each end, laid out left to
/// right. The response is the whole bar, so a header can be clicked as one thing.
pub fn strip<R>(ui: &mut egui::Ui, contents: impl FnOnce(&mut egui::Ui) -> R) -> egui::Response {
    let (rect, response) = ui.allocate_exact_size(
        egui::vec2(ui.available_width(), HEADER),
        egui::Sense::click(),
    );
    ui.painter()
        .rect_filled(rect, 0.0, ui.visuals().faint_bg_color);
    let mut inner = ui.new_child(
        egui::UiBuilder::new()
            .max_rect(rect.shrink2(egui::vec2(PAD, 0.0)))
            .layout(egui::Layout::left_to_right(egui::Align::Center)),
    );
    inner.spacing_mut().item_spacing.x = GAP;
    contents(&mut inner);
    response
}

/// The triangle that says which way a dock will go, and answers a click of its own.
pub fn chevron(ui: &mut egui::Ui, open: bool) -> egui::Response {
    let glyph = match open {
        true => Glyph::ChevronDown,
        false => Glyph::ChevronRight,
    };
    let drawn = icon(
        ui,
        glyph,
        CHEVRON,
        ui.visuals().widgets.noninteractive.fg_stroke.color,
    );
    ui.interact(drawn.rect, drawn.id.with("chevron"), egui::Sense::click())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_title_is_uppercased_whatever_it_arrives_as() {
        assert_eq!(caps("send queue").text(), "SEND QUEUE");
        assert_eq!(caps("Browser").text(), "BROWSER");
    }

    /// A header is full bleed and exactly 24 px, so a dock's body always starts at the
    /// same place and a header's fill reaches both edges of the panel it heads.
    #[test]
    fn a_header_claims_its_own_height_and_the_whole_width() {
        let ctx = egui::Context::default();
        let mut drawn = egui::Rect::ZERO;
        let mut width = 0.0;
        let _ = ctx.run(egui::RawInput::default(), |ctx| {
            ctx.style_mut(crate::app::metrics);
            egui::CentralPanel::default().show(ctx, |ui| {
                width = ui.available_width();
                drawn = panel_header(ui, "browser", None, None).rect;
            });
        });
        assert_eq!(drawn.height(), HEADER);
        assert_eq!(drawn.width(), width);
    }

    /// The triangle is the collapse control, so a click on it is what moves the dock —
    /// not a second bool somewhere else.
    #[test]
    fn the_triangle_toggles_the_bool_it_was_handed() {
        let ctx = egui::Context::default();
        let mut open = true;
        let at = std::cell::Cell::new(egui::Pos2::ZERO);
        let frame = |press: bool, open: &mut bool| {
            let input = egui::RawInput {
                events: match press {
                    true => vec![
                        egui::Event::PointerMoved(at.get()),
                        egui::Event::PointerButton {
                            pos: at.get(),
                            button: egui::PointerButton::Primary,
                            pressed: true,
                            modifiers: egui::Modifiers::NONE,
                        },
                        egui::Event::PointerButton {
                            pos: at.get(),
                            button: egui::PointerButton::Primary,
                            pressed: false,
                            modifiers: egui::Modifiers::NONE,
                        },
                    ],
                    false => Vec::new(),
                },
                ..Default::default()
            };
            let _ = ctx.run(input, |ctx| {
                ctx.style_mut(crate::app::metrics);
                egui::CentralPanel::default().show(ctx, |ui| {
                    let rect = panel_header(ui, "browser", Some(open), None).rect;
                    at.set(egui::pos2(
                        rect.left() + PAD + CHEVRON / 2.0,
                        rect.center().y,
                    ));
                });
            });
        };
        // The first frame only learns where the triangle is; the second presses it.
        frame(false, &mut open);
        frame(true, &mut open);
        assert!(!open, "a click on the triangle shuts the dock");
        frame(true, &mut open);
        assert!(open, "and the next one opens it again");
    }
}
