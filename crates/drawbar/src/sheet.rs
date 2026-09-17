//! The shape a sheet wears: the modals drawbar opens over the whole window — the first-run
//! welcome, what changed, and About — share one masthead, one heading, one foot and two
//! buttons, so they read as one thing on every target.

use eframe::egui;

use crate::icon::{sized, Glyph};

pub(crate) const VERSION: &str = env!("CARGO_PKG_VERSION");

/// One line of what this software is.
pub(crate) const WHAT: &str =
    "Your Nord's sounds, in a window: browse what is on your computer and on your \
                               instrument, edit programs, samples and pianos, and send them back.";

/// Required of every public face of the project — see `CONTRIBUTING.md`.
pub(crate) const DISCLAIMER: &str = "Not affiliated with, authorized, or endorsed by Clavia DMI AB. \
                                     \"Nord\", \"Clavia\" and \"Electro\" are trademarks of Clavia DMI AB, \
                                     used here only to identify the hardware these formats come from.";

/// The room between two lines of a sheet.
pub(crate) const GAP: f32 = 4.0;

/// The room inside a sheet's side edges, which every section keeps and the foot does not.
pub(crate) const PAD: f32 = 20.0;

/// The least room a sheet keeps from the window's edge.
const MARGIN: f32 = 24.0;

/// The room inside a sheet's buttons, wider than the shell's own.
const PADDING: egui::Vec2 = egui::vec2(12.0, 4.0);

/// The glyph beside a sheet's title.
const MARK: f32 = 22.0;

/// A sheet is `most` wide, or the window less its margins where that is narrower.
pub(crate) fn width(ctx: &egui::Context, most: f32) -> f32 {
    (ctx.screen_rect().width() - 2.0 * MARGIN).min(most)
}

/// The most a sheet's scrolling middle may claim, so the `around` it — masthead and foot —
/// stays on screen, and never less than `fewest`.
pub(crate) fn middle(ctx: &egui::Context, around: f32, fewest: f32) -> f32 {
    (ctx.screen_rect().height() - around).max(fewest)
}

/// The frame a sheet is drawn in. Its sections pad themselves, so the foot can run edge
/// to edge.
pub(crate) fn frame(visuals: &egui::Visuals) -> egui::Frame {
    egui::Frame::new()
        .fill(visuals.panel_fill)
        .stroke(visuals.widgets.noninteractive.bg_stroke)
        .corner_radius(egui::CornerRadius::same(2))
        .shadow(egui::Shadow {
            offset: [0, 18],
            blur: 48,
            spread: 0,
            color: egui::Color32::from_black_alpha(115),
        })
}

/// A section of a sheet: its content, in from the side edges by [`PAD`].
pub(crate) fn section<R>(ui: &mut egui::Ui, add: impl FnOnce(&mut egui::Ui) -> R) -> R {
    egui::Frame::new()
        .inner_margin(egui::Margin::symmetric(PAD as i8, 0))
        .show(ui, add)
        .inner
}

/// The mark, the name, the version and, while this is alpha, the pill that says so.
/// `tagline` adds [`WHAT`] beneath.
pub(crate) fn masthead(ui: &mut egui::Ui, tagline: bool) {
    ui.horizontal_top(|ui| {
        let accent = crate::app::accent(ui.visuals());
        ui.add_space(-2.0);
        ui.add(sized(Glyph::SlidersVertical, MARK, accent));
        ui.vertical(|ui| {
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 9.0;
                ui.label(
                    egui::RichText::new("drawbar")
                        .font(egui::FontId::new(17.0, crate::app::bold())),
                );
                ui.label(
                    egui::RichText::new(VERSION)
                        .font(egui::FontId::monospace(12.0))
                        .weak(),
                );
                pill(ui, "ALPHA", crate::app::warn(ui.visuals()));
            });
            if tagline {
                ui.add_space(GAP);
                ui.add(egui::Label::new(egui::RichText::new(WHAT).weak()).wrap());
            }
        });
    });
}

/// A small outlined word in `tint`: the alpha mark.
pub(crate) fn pill(ui: &mut egui::Ui, text: &str, tint: egui::Color32) -> egui::Response {
    egui::Frame::new()
        .stroke(egui::Stroke::new(1.0_f32, tint))
        .corner_radius(egui::CornerRadius::same(2))
        .inner_margin(egui::Margin::symmetric(6, 1))
        .show(ui, |ui| {
            ui.label(
                egui::RichText::new(text)
                    .font(egui::FontId::monospace(9.5))
                    .color(tint),
            );
        })
        .response
}

/// A section heading, with a quieter aside after it where there is one.
pub(crate) fn heading(ui: &mut egui::Ui, title: &str, aside: Option<&str>) {
    ui.add_space(GAP * 3.0);
    ui.horizontal(|ui| {
        ui.label(egui::RichText::new(title).strong());
        if let Some(aside) = aside {
            ui.label(egui::RichText::new(aside).small().weak());
        }
    });
    ui.add_space(GAP);
}

/// The foot of a sheet: a hairline, then a darker strip with `left` at its left and
/// `right` laid right to left from its right edge. Both are drawn in from the edges by
/// [`PAD`].
pub(crate) fn foot(
    ui: &mut egui::Ui,
    left: impl FnOnce(&mut egui::Ui),
    right: impl FnOnce(&mut egui::Ui),
) {
    ui.add_space(GAP * 3.0);
    let fill = ui.visuals().window_fill;
    let hairline = ui.visuals().widgets.noninteractive.bg_stroke;
    egui::Frame::new()
        .fill(fill)
        .inner_margin(egui::Margin::symmetric(PAD as i8, 12))
        .show(ui, |ui| {
            let top = ui.max_rect().top() - 12.0;
            ui.painter()
                .hline(ui.max_rect().x_range().expand(PAD), top, hairline);
            ui.horizontal(|ui| {
                left(ui);
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), right);
            });
        });
}

/// The [`DISCLAIMER`] at the left of a foot, wrapping in the room the buttons after it
/// leave, which is everything but `keep`.
pub(crate) fn disclaimer(ui: &mut egui::Ui, keep: f32) {
    let room = egui::vec2((ui.available_width() - keep).max(0.0), 0.0);
    ui.allocate_ui(room, |ui| {
        ui.add(egui::Label::new(egui::RichText::new(DISCLAIMER).small().weak()).wrap());
    });
}

/// The one button a sheet is dismissed with: the accent around it, on the active fill.
pub(crate) fn primary(ui: &mut egui::Ui, glyph: Option<Glyph>, label: &str) -> egui::Response {
    let accent = crate::app::accent(ui.visuals());
    let fill = ui.visuals().widgets.active.bg_fill;
    button(
        ui,
        glyph,
        accent,
        label,
        fill,
        egui::Stroke::new(1.0_f32, accent),
    )
}

/// A button beside [`primary`] that does something other than dismiss the sheet.
pub(crate) fn secondary(ui: &mut egui::Ui, glyph: Option<Glyph>, label: &str) -> egui::Response {
    let ink = ui.visuals().text_color();
    let stroke = ui.visuals().widgets.noninteractive.bg_stroke;
    button(ui, glyph, ink, label, egui::Color32::TRANSPARENT, stroke)
}

fn button(
    ui: &mut egui::Ui,
    glyph: Option<Glyph>,
    tint: egui::Color32,
    label: &str,
    fill: egui::Color32,
    stroke: egui::Stroke,
) -> egui::Response {
    let text = egui::RichText::new(label).color(ui.visuals().strong_text_color());
    let button = match glyph {
        Some(glyph) => egui::Button::image_and_text(sized(glyph, 13.0, tint), text),
        None => egui::Button::new(text),
    };
    ui.scope(|ui| {
        ui.spacing_mut().button_padding = PADDING;
        ui.add(button.fill(fill).stroke(stroke))
    })
    .inner
}

/// ⚠️ Always a new tab: in a browser the app *is* the page, and following a link in
/// place ends the session and everything unsaved in it.
pub(crate) fn link(ui: &mut egui::Ui, label: &str, url: &str) -> egui::Response {
    ui.add(egui::Hyperlink::from_label_and_url(label, url).open_in_new_tab(true))
}

/// A [`link`] with a glyph before its label, both in the link colour.
pub(crate) fn glyph_link(
    ui: &mut egui::Ui,
    glyph: Glyph,
    label: &str,
    url: &str,
) -> egui::Response {
    let tint = ui.visuals().hyperlink_color;
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 5.0;
        ui.add(sized(glyph, 12.0, tint));
        link(ui, label, url)
    })
    .inner
}

#[cfg(test)]
mod tests {
    use super::*;

    fn headless() -> egui::Context {
        let ctx = egui::Context::default();
        egui_extras::install_image_loaders(&ctx);
        ctx.set_fonts(crate::app::fonts());
        ctx
    }

    fn drawn_at(ctx: &egui::Context, size: egui::Vec2, add: impl FnOnce(&mut egui::Ui)) {
        let input = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, size)),
            ..Default::default()
        };
        let mut add = Some(add);
        let _ = ctx.run(input, |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                if let Some(add) = add.take() {
                    add(ui);
                }
            });
        });
    }

    #[test]
    fn a_sheet_never_reaches_the_window_edge() {
        let ctx = egui::Context::default();
        drawn_at(&ctx, egui::vec2(660.0, 430.0), |_| {});
        assert_eq!(width(&ctx, 940.0), 660.0 - 2.0 * MARGIN);
        drawn_at(&ctx, egui::vec2(1400.0, 980.0), |_| {});
        assert_eq!(width(&ctx, 940.0), 940.0);
    }

    #[test]
    fn the_middle_keeps_the_room_around_it_and_never_shrinks_past_fewest() {
        let ctx = egui::Context::default();
        drawn_at(&ctx, egui::vec2(1400.0, 980.0), |_| {});
        assert_eq!(middle(&ctx, 200.0, 120.0), 780.0);
        drawn_at(&ctx, egui::vec2(660.0, 430.0), |_| {});
        assert_eq!(middle(&ctx, 380.0, 120.0), 120.0);
    }

    #[test]
    fn the_masthead_names_the_version_that_is_running() {
        assert_eq!(VERSION, env!("CARGO_PKG_VERSION"));
        assert!(!VERSION.is_empty());
    }

    /// Every helper lays out in a headless frame: a `Glyph` the image loader lacks, or a
    /// layout that asks for more than it is given, panics here rather than in a window.
    #[test]
    fn every_helper_draws() {
        let ctx = headless();
        drawn_at(&ctx, egui::vec2(660.0, 430.0), |ui| {
            masthead(ui, true);
            heading(ui, "This build", Some("an aside"));
            section(ui, |ui| {
                ui.label("a section");
            });
            foot(
                ui,
                |ui| {
                    disclaimer(ui, 90.0);
                    glyph_link(ui, Glyph::HardDrive, "a link", "https://drawbar.app/");
                },
                |ui| {
                    primary(ui, Some(Glyph::Check), "Continue");
                    secondary(ui, None, "Close");
                },
            );
        });
    }
}
