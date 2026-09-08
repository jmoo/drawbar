//! The header a dock wears, the geometry every dock header shares, and how a button in
//! a bar wears the bar it sits in.

use std::ops::Range;

use eframe::egui;

use crate::icon::{icon, Glyph};

/// How tall a panel header is, wherever it is drawn.
pub const HEADER: f32 = 24.0;

/// The room a header keeps at each end.
const PAD: f32 = 8.0;

/// The gap between a header's parts.
const GAP: f32 = 6.0;

/// The collapse triangle's box, and the grip a dock header wears before its title.
const CHEVRON: f32 = 12.0;
const GRIP: f32 = 12.0;

/// How much of the caption ink the grip keeps. It is decoration, not a control.
const GRIP_ALPHA: f32 = 0.6;

/// What a column of a table asks for: a fixed width, or a share of what the fixed
/// ones leave.
pub enum Track {
    Px(f32),
    Share(f32),
}

/// Where each track sits across `width`, with `gap` between two of them.
///
/// The fixed tracks are laid out first and the shares split what is left. When even the
/// fixed ones do not fit, every track and every gap shrinks by one factor — so a track
/// may reach zero, but none is ever negative and none reaches past `width`.
pub fn tracks(width: f32, wanted: &[Track], gap: f32) -> Vec<Range<f32>> {
    let gaps = gap * (wanted.len().saturating_sub(1)) as f32;
    let fixed: f32 = wanted
        .iter()
        .filter_map(|track| match track {
            Track::Px(px) => Some(*px),
            Track::Share(_) => None,
        })
        .sum();
    let shares: f32 = wanted
        .iter()
        .filter_map(|track| match track {
            Track::Share(share) => Some(*share),
            Track::Px(_) => None,
        })
        .sum();
    let spare = (width - gaps - fixed).max(0.0);
    let asked: Vec<f32> = wanted
        .iter()
        .map(|track| match track {
            Track::Px(px) => *px,
            Track::Share(share) => spare * share / shares,
        })
        .collect();

    let total: f32 = asked.iter().sum::<f32>() + gaps;
    let scale = match total > width {
        true => (width / total).max(0.0),
        false => 1.0,
    };
    let mut x = 0.0;
    asked
        .iter()
        .map(|held| {
            let track = x..x + held * scale;
            x = track.end + gap * scale;
            track
        })
        .collect()
}

/// A header title: [`crate::app::micro`], uppercased.
///
/// Uppercasing is the whole of the treatment — egui has no letter spacing, and a faked
/// one is worse than none.
pub fn caps(text: &str) -> egui::RichText {
    egui::RichText::new(text.to_uppercase()).text_style(crate::app::micro())
}

/// A section header: a collapse triangle, a MICRO-caps title, an optional badge.
///
/// It wears whatever panel it is on and lifts to `faint_bg_color` under the pointer
/// alone. A dock's own header is [`dock_header`], which is not a control.
pub fn panel_header(
    ui: &mut egui::Ui,
    title: &str,
    open: Option<&mut bool>,
    badge: Option<(&str, egui::Color32)>,
) -> egui::Response {
    bar(ui, egui::Color32::TRANSPARENT, |ui| {
        if let Some(open) = open {
            if chevron(ui, *open).clicked() {
                *open = !*open;
            }
        }
        ui.label(caps(title).color(crate::app::caption(ui.visuals())));
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

/// A dock's own header: a grip, a MICRO-caps title, and nothing to click.
///
/// ⚠️ Collapsing a dock is its toolbar toggle and the View menu. A header carrying a
/// triangle of its own would read as one of the sections beneath it.
pub fn dock_header(ui: &mut egui::Ui, title: &str) -> egui::Response {
    let fill = ui.visuals().faint_bg_color;
    let response = bar(ui, fill, |ui| {
        let ink = crate::app::caption(ui.visuals());
        icon(
            ui,
            Glyph::GripVertical,
            GRIP,
            ink.gamma_multiply(GRIP_ALPHA),
        );
        ui.label(caps(title).color(ink));
    });
    let stroke = egui::Stroke::new(1.0_f32, ui.visuals().widgets.noninteractive.bg_stroke.color);
    let rect = response.rect;
    ui.painter()
        .hline(rect.x_range(), rect.bottom() - 0.5, stroke);
    response
}

/// The bar a header is drawn into: full bleed, padded at each end, laid out left to
/// right. The response is the whole bar, so a header can be clicked as one thing.
pub fn strip<R>(ui: &mut egui::Ui, contents: impl FnOnce(&mut egui::Ui) -> R) -> egui::Response {
    let fill = ui.visuals().faint_bg_color;
    bar(ui, fill, contents)
}

/// `resting` is what the bar wears when the pointer is elsewhere; under the pointer it
/// is `faint_bg_color` whatever it wears at rest.
fn bar<R>(
    ui: &mut egui::Ui,
    resting: egui::Color32,
    contents: impl FnOnce(&mut egui::Ui) -> R,
) -> egui::Response {
    let (rect, response) = ui.allocate_exact_size(
        egui::vec2(ui.available_width(), HEADER),
        egui::Sense::click(),
    );
    let fill = match response.hovered() {
        true => ui.visuals().faint_bg_color,
        false => resting,
    };
    ui.painter().rect_filled(rect, 0.0, fill);
    let mut inner = ui.new_child(
        egui::UiBuilder::new()
            .max_rect(rect.shrink2(egui::vec2(PAD, 0.0)))
            .layout(egui::Layout::left_to_right(egui::Align::Center)),
    );
    inner.spacing_mut().item_spacing.x = GAP;
    contents(&mut inner);
    response
}

/// The triangle that says which way a section will go, and answers a click of its own.
pub fn chevron(ui: &mut egui::Ui, open: bool) -> egui::Response {
    let glyph = match open {
        true => Glyph::ChevronDown,
        false => Glyph::ChevronRight,
    };
    let drawn = icon(ui, glyph, CHEVRON, crate::app::caption(ui.visuals()));
    ui.interact(drawn.rect, drawn.id.with("chevron"), egui::Sense::click())
}

/// Dress the buttons in a bar to wear the bar: no fill and no border until the pointer
/// is on one, which is then the only thing on the bar that is lit.
///
/// Scope this into a child `Ui` — it edits the visuals every widget after it reads.
pub fn flat(ui: &mut egui::Ui) {
    let widgets = &mut ui.visuals_mut().widgets;
    widgets.inactive.weak_bg_fill = egui::Color32::TRANSPARENT;
    widgets.inactive.bg_fill = egui::Color32::TRANSPARENT;
    for state in [
        &mut widgets.inactive,
        &mut widgets.hovered,
        &mut widgets.active,
        &mut widgets.open,
    ] {
        state.bg_stroke = egui::Stroke::NONE;
    }
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

    /// What a frame painted over `rect`, innermost last.
    fn fills(output: &egui::FullOutput, rect: egui::Rect) -> Vec<egui::Color32> {
        fn walk(shape: &egui::Shape, rect: egui::Rect, into: &mut Vec<egui::Color32>) {
            match shape {
                egui::Shape::Rect(drawn) if drawn.rect == rect => into.push(drawn.fill),
                egui::Shape::Vec(shapes) => shapes.iter().for_each(|shape| walk(shape, rect, into)),
                _ => {}
            }
        }
        let mut found = Vec::new();
        for clipped in &output.shapes {
            walk(&clipped.shape, rect, &mut found);
        }
        found
    }

    /// One frame with the pointer over the header or away from it, answering with what
    /// the header painted behind itself.
    fn header_fill(
        ctx: &egui::Context,
        under_pointer: bool,
        header: impl Fn(&mut egui::Ui) -> egui::Response,
    ) -> Vec<egui::Color32> {
        let at = std::cell::Cell::new(egui::Pos2::ZERO);
        let mut fill = Vec::new();
        // The first frame only learns where the header is; the second points at it.
        for _ in 0..2 {
            let input = egui::RawInput {
                events: match under_pointer {
                    true => vec![egui::Event::PointerMoved(at.get())],
                    false => Vec::new(),
                },
                ..Default::default()
            };
            let mut rect = egui::Rect::NOTHING;
            let output = ctx.run(input, |ctx| {
                ctx.style_mut(crate::app::metrics);
                egui::CentralPanel::default().show(ctx, |ui| rect = header(ui).rect);
            });
            at.set(rect.center());
            fill = fills(&output, rect);
        }
        fill
    }

    /// ⚠️ A section header is part of the panel it heads until the pointer is on it.
    /// Three permanently grey bars down a dock read as three separate panels rather than
    /// as the headings of one.
    #[test]
    fn a_section_header_wears_the_panel_until_the_pointer_is_on_it() {
        let ctx = egui::Context::default();
        let section = |ui: &mut egui::Ui| panel_header(ui, "places", None, None);
        assert_eq!(
            header_fill(&ctx, false, section),
            vec![egui::Color32::TRANSPARENT],
        );
        assert_eq!(
            header_fill(&ctx, true, section),
            vec![ctx.style().visuals.faint_bg_color],
        );
    }

    /// A dock's header is the one bar that keeps its own colour: it names the dock rather
    /// than a section of it, and there is nothing on it to click.
    #[test]
    fn a_dock_header_keeps_its_own_colour_whether_or_not_it_is_pointed_at() {
        let ctx = egui::Context::default();
        let faint = ctx.style().visuals.faint_bg_color;
        for pointed in [false, true] {
            assert_eq!(
                header_fill(&ctx, pointed, |ui| dock_header(ui, "browser")),
                vec![faint],
                "pointed at: {pointed}",
            );
        }
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
