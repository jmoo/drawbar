//! Shared chrome: section and dock headers, table column widths, chips, dashed borders,
//! and flat buttons for bars.

use std::ops::Range;

use eframe::egui;

use crate::browser::cell_ink;
use crate::icon::{icon, Glyph};

/// How tall a section header is, wherever it is drawn.
pub const HEADER: f32 = 24.0;

/// How tall a dock's own header is: the tab strip's height, so the strip and every dock
/// header beside it form one line across the window.
pub const DOCK: f32 = crate::tabs::HEIGHT;

/// A bar's padding at each end, and the gap between its parts. The title bar, the
/// toolbar, the tab strip, and every header share them, so their contents line up down
/// the window.
pub(crate) const PAD: f32 = 8.0;
pub(crate) const GAP: f32 = 6.0;

/// The size of a glyph in a bar: a toolbar action, a tab's kind, a menu item's mark.
pub(crate) const GLYPH: f32 = 13.0;

/// The canvas left between two cards, and between a card and the window's edge.
pub const GUTTER: f32 = 8.0;

/// The rounding of a card on the canvas: a side panel, or the document under the tabs.
pub const CARD_RADIUS: u8 = 12;

/// The rounding of a card inside a card: an inspector section, a note, a diff.
pub const INNER_RADIUS: u8 = 10;

/// The rounding of a list row, and how far a row stands in from its card's edges.
pub const ROW_RADIUS: u8 = 7;
pub const ROW_INSET: f32 = 6.0;

/// The height of a pill: a badge, a chip, a state on a row.
pub const PILL: f32 = 18.0;

/// The size of the collapse triangle, and of the grip before a dock header's title.
const CHEVRON: f32 = 12.0;
const GRIP: f32 = 12.0;

/// The grip's opacity relative to the caption color. It is decoration, not a control.
const GRIP_ALPHA: f32 = 0.6;

/// The width a table column asks for: a fixed width, a share of what the fixed columns
/// leave, or a share that stops growing at `max` px and leaves the rest to the other
/// shares.
pub enum Track {
    Px(f32),
    Share(f32),
    Capped { share: f32, max: f32 },
}

impl Track {
    fn px(&self) -> f32 {
        match self {
            Track::Px(px) => *px,
            Track::Share(_) | Track::Capped { .. } => 0.0,
        }
    }

    fn share(&self) -> f32 {
        match self {
            Track::Px(_) => 0.0,
            Track::Share(share) | Track::Capped { share, .. } => *share,
        }
    }
}

/// The width one unit of share gets from `spare`, after every binding cap has taken its
/// maximum and left the rest to the shares still growing.
///
/// A cap binds when one unit of share would get more than `max / share`, and each binding
/// cap only raises what the rest get, so taking caps in that order settles in one pass.
fn rate(spare: f32, wanted: &[Track]) -> f32 {
    let mut caps: Vec<(f32, f32)> = wanted
        .iter()
        .filter_map(|track| match track {
            Track::Capped { share, max } => Some((*share, *max)),
            Track::Px(_) | Track::Share(_) => None,
        })
        .collect();
    caps.sort_by(|(share, max), (other, limit)| (max / share).total_cmp(&(limit / other)));

    let mut spare = spare;
    let mut pool: f32 = wanted.iter().map(Track::share).sum();
    for (share, max) in caps {
        if pool <= 0.0 || spare / pool * share <= max {
            break;
        }
        spare -= max;
        pool -= share;
    }
    match pool > 0.0 {
        true => spare / pool,
        false => 0.0,
    }
}

/// Where each track sits across `width`, with `gap` between neighbors.
///
/// The fixed tracks are laid out first, the shares split what is left, and a share that
/// reaches its cap passes the remainder to the others. When even the fixed tracks do not
/// fit, every track and gap shrinks by one factor: a track may reach zero, but none is
/// negative and none extends past `width`.
pub fn tracks(width: f32, wanted: &[Track], gap: f32) -> Vec<Range<f32>> {
    let gaps = gap * (wanted.len().saturating_sub(1)) as f32;
    let fixed: f32 = wanted.iter().map(Track::px).sum();
    let spare = (width - gaps - fixed).max(0.0);
    let rate = rate(spare, wanted);
    let asked: Vec<f32> = wanted
        .iter()
        .map(|track| match track {
            Track::Px(px) => *px,
            Track::Share(share) => rate * share,
            Track::Capped { share, max } => (rate * share).min(*max),
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

/// `text` on one line, truncated to `width` with an ellipsis, painted from `left` and
/// centered on `middle`. Returns the size painted, which is zero without text or room.
pub fn cut(
    painter: &egui::Painter,
    left: f32,
    middle: f32,
    width: f32,
    text: &str,
    format: egui::TextFormat,
) -> egui::Vec2 {
    if width <= 0.0 || text.is_empty() {
        return egui::Vec2::ZERO;
    }
    let mut job = egui::text::LayoutJob::single_section(text.to_owned(), format);
    job.break_on_newline = false;
    job.wrap = egui::text::TextWrapping::truncate_at_width(width);
    let galley = painter.layout_job(job);
    let size = galley.size();
    painter.galley(
        egui::pos2(left, middle - size.y / 2.0),
        galley,
        egui::Color32::PLACEHOLDER,
    );
    size
}

/// Where a row's cell sits in one of the tracks [`tracks`] laid out across it.
pub fn cell(rect: egui::Rect, track: &Range<f32>) -> egui::Rect {
    egui::Rect::from_min_max(
        egui::pos2(rect.left() + track.start, rect.top()),
        egui::pos2(rect.left() + track.end, rect.bottom()),
    )
}

/// A child `Ui` for a table, inset from the left by [`PAD`] so its heads and rows start
/// where a tree row starts and the scroll bar stays at the panel's edge.
///
/// ⚠️ Without it the first track starts at the panel's edge, and a mark's left stroke is
/// painted half outside the window.
pub fn inset(ui: &mut egui::Ui) -> egui::Ui {
    let room = ui.available_rect_before_wrap();
    ui.new_child(
        egui::UiBuilder::new()
            .max_rect(room.with_min_x(room.left() + PAD))
            .layout(*ui.layout()),
    )
}

/// The width a table's head and `rows` rows share: all that is available, less the
/// scroll bar when the rows overflow the height left under the head.
pub fn list_width(ui: &egui::Ui, rows: usize, row: f32, head: f32) -> f32 {
    let scrolls = rows as f32 * row > ui.available_height() - head;
    let bar = match scrolls {
        true => ui.spacing().scroll.bar_width,
        false => 0.0,
    };
    (ui.available_width() - bar).max(0.0)
}

/// Paint a list row's fill for its selection and hover, and return the row's text inks:
/// the body's, and the quiet one.
pub fn row_ink(
    painter: &egui::Painter,
    rect: egui::Rect,
    selected: bool,
    hovered: bool,
    visuals: &egui::Visuals,
) -> (egui::Color32, egui::Color32) {
    let fill = match (selected, hovered) {
        (true, _) => Some(visuals.selection.bg_fill),
        (false, true) => Some(visuals.widgets.hovered.weak_bg_fill),
        (false, false) => None,
    };
    if let Some(fill) = fill {
        painter.rect_filled(rect, ROW_RADIUS, fill);
    }
    (
        cell_ink(selected, visuals.text_color(), visuals),
        cell_ink(selected, visuals.weak_text_color(), visuals),
    )
}

/// A bordered glyph and a word: a filter in the library's bar, or a tag on the
/// selection.
///
/// A chip with a `fill` is solid: it is true of everything it stands for. One without is
/// hollow, and true of only some.
pub fn chip(
    ui: &mut egui::Ui,
    glyph: Glyph,
    size: f32,
    text: &str,
    tint: egui::Color32,
    fill: Option<egui::Color32>,
) -> egui::Response {
    let border = ui.visuals().widgets.noninteractive.bg_stroke.color;
    egui::Frame::new()
        .fill(fill.unwrap_or(egui::Color32::TRANSPARENT))
        .stroke(egui::Stroke::new(1.0_f32, border))
        .corner_radius(2.0)
        .inner_margin(egui::Margin::symmetric(5, 1))
        .show(ui, |ui| {
            ui.spacing_mut().item_spacing.x = 4.0;
            icon(ui, glyph, size, tint);
            ui.label(
                egui::RichText::new(text)
                    .text_style(crate::app::ui())
                    .color(tint),
            );
        })
        .response
}

/// A section label or a column head in the shell, in the caller's sentence case.
pub fn section_label(text: &str) -> egui::RichText {
    egui::RichText::new(text).text_style(crate::app::section())
}

/// A word on a rounded pill: `ink` on `fill`, in monospace. A state, a count, or where
/// something is.
pub fn pill(
    ui: &mut egui::Ui,
    text: &str,
    ink: egui::Color32,
    fill: egui::Color32,
) -> egui::Response {
    let galley = ui
        .painter()
        .layout_no_wrap(text.to_owned(), egui::FontId::monospace(10.5), ink);
    let size = egui::vec2(galley.size().x + 14.0, PILL);
    let (rect, response) = ui.allocate_exact_size(size, egui::Sense::hover());
    ui.painter().rect_filled(rect, PILL / 2.0, fill);
    ui.painter().galley(
        rect.center() - galley.size() / 2.0,
        galley,
        egui::Color32::PLACEHOLDER,
    );
    response
}

/// A pill tinted by a signal: the signal's own color on a faint wash of it.
pub fn signal_pill(ui: &mut egui::Ui, text: &str, signal: egui::Color32) -> egui::Response {
    pill(ui, text, signal, crate::app::tint(signal, 0.15))
}

/// A quiet pill, for a fact with no signal in it.
pub fn quiet_pill(ui: &mut egui::Ui, text: &str) -> egui::Response {
    let visuals = ui.visuals();
    let (ink, fill) = (visuals.text_color(), visuals.widgets.inactive.weak_bg_fill);
    pill(ui, text, ink, fill)
}

/// A button in a card's body: an optional glyph and a word on a tonal fill, with no
/// border.
///
/// It rests on `bg-inactive` and turns `bg-hovered` under the pointer, so it reads as a
/// control without the weight of a bordered button.
pub fn tonal_button(ui: &mut egui::Ui, glyph: Option<Glyph>, label: &str) -> egui::Response {
    const HEIGHT: f32 = 28.0;
    const RADIUS: u8 = 7;
    const SIDE: f32 = 10.0;
    const ICON: f32 = 12.0;
    const TEXT: f32 = 12.0;

    let galley = ui.painter().layout_no_wrap(
        label.to_owned(),
        egui::FontId::proportional(TEXT),
        egui::Color32::PLACEHOLDER,
    );
    let mark = match glyph {
        Some(_) => ICON + GAP,
        None => 0.0,
    };
    let width = SIDE + mark + galley.size().x + SIDE;
    let (rect, response) = ui.allocate_exact_size(egui::vec2(width, HEIGHT), egui::Sense::click());
    let widgets = &ui.visuals().widgets;
    let state = match response.hovered() {
        true => &widgets.hovered,
        false => &widgets.inactive,
    };
    let (fill, ink) = (state.weak_bg_fill, state.fg_stroke.color);
    let painter = ui.painter();
    painter.rect_filled(rect, RADIUS, fill);
    let middle = rect.center().y;
    let x = rect.left() + SIDE;
    if let Some(glyph) = glyph {
        crate::icon::painted(
            ui,
            glyph,
            egui::Rect::from_min_size(egui::pos2(x, middle - ICON / 2.0), egui::Vec2::splat(ICON)),
            ink,
        );
    }
    let height = galley.size().y;
    painter.galley(egui::pos2(x + mark, middle - height / 2.0), galley, ink);
    response.widget_info(|| {
        egui::WidgetInfo::labeled(egui::WidgetType::Button, ui.is_enabled(), label)
    });
    response
}

/// A button that carries the accent: the one action a sheet exists for. A faint wash
/// and border of the accent, its glyph in the accent, and its label in the active ink.
pub fn accent_button(
    ui: &mut egui::Ui,
    glyph: Glyph,
    label: &str,
    enabled: bool,
) -> egui::Response {
    let visuals = ui.visuals().clone();
    let lit = crate::app::accent(&visuals);
    let button = egui::Button::image_and_text(
        crate::icon::sized(glyph, 14.0, lit),
        egui::RichText::new(label).color(visuals.widgets.active.fg_stroke.color),
    )
    .image_tint_follows_text_color(false)
    .fill(crate::app::tint(lit, 0.16))
    .stroke(egui::Stroke::new(1.0_f32, crate::app::tint(lit, 0.55)))
    .corner_radius(8.0)
    .min_size(egui::vec2(0.0, 32.0));
    ui.scope(|ui| {
        ui.spacing_mut().button_padding = egui::vec2(12.0, 4.0);
        ui.add_enabled(enabled, button)
    })
    .inner
}

/// A header title: [`crate::app::micro`], uppercased.
///
/// Uppercasing is the only treatment: egui has no letter spacing, and faking it looks
/// worse than none.
pub fn caps(text: &str) -> egui::RichText {
    egui::RichText::new(text.to_uppercase()).text_style(crate::app::micro())
}

/// A section header: a collapse triangle and a [`caps`] title.
///
/// It takes the color of the panel it is on, changing to `faint_bg_color` only under the
/// pointer. A dock's own header is [`dock_header`], which is not a control.
pub fn panel_header(ui: &mut egui::Ui, title: &str, open: &mut bool) -> egui::Response {
    bar(ui, HEADER, egui::Color32::TRANSPARENT, |ui| {
        if chevron(ui, *open).clicked() {
            *open = !*open;
        }
        ui.label(caps(title).color(crate::app::caption(ui.visuals())));
    })
}

/// A dock's own header: a grip and a [`caps`] title, with nothing to click.
///
/// ⚠️ A dock collapses from its toolbar toggle and the View menu. A header with its own
/// triangle would look like one of the sections beneath it.
pub fn dock_header(ui: &mut egui::Ui, title: &str) -> egui::Response {
    let fill = ui.visuals().faint_bg_color;
    let response = bar(ui, DOCK, fill, |ui| {
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

/// A dock header with its own controls: [`dock_header`]'s bar, holding the bottom dock's
/// controls in place of a title.
pub fn strip<R>(ui: &mut egui::Ui, contents: impl FnOnce(&mut egui::Ui) -> R) -> egui::Response {
    let fill = ui.visuals().faint_bg_color;
    bar(ui, DOCK, fill, contents)
}

/// The bar a header is drawn into: full bleed, padded at each end, laid out left to
/// right. The response is the whole bar, so a header can be clicked as one thing.
///
/// `resting` is the bar's fill when the pointer is elsewhere; under the pointer the fill
/// is always `faint_bg_color`.
fn bar<R>(
    ui: &mut egui::Ui,
    height: f32,
    resting: egui::Color32,
    contents: impl FnOnce(&mut egui::Ui) -> R,
) -> egui::Response {
    let (rect, response) = ui.allocate_exact_size(
        egui::vec2(ui.available_width(), height),
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

/// The triangle that shows whether a section is open, with its own click response.
pub fn chevron(ui: &mut egui::Ui, open: bool) -> egui::Response {
    let glyph = match open {
        true => Glyph::ChevronDown,
        false => Glyph::ChevronRight,
    };
    let drawn = icon(ui, glyph, CHEVRON, crate::app::caption(ui.visuals()));
    ui.interact(drawn.rect, drawn.id.with("chevron"), egui::Sense::click())
}

/// A dashed rectangle: the border for something absent, or for an action with nothing
/// to act on. egui draws dashes along a line, so a rectangle is four lines.
pub fn dashed_rect(painter: &egui::Painter, rect: egui::Rect, stroke: egui::Stroke) {
    const DASH: f32 = 3.0;
    let corners = [
        rect.left_top(),
        rect.right_top(),
        rect.right_bottom(),
        rect.left_bottom(),
        rect.left_top(),
    ];
    for side in corners.windows(2) {
        painter.extend(egui::Shape::dashed_line(side, stroke, DASH, DASH));
    }
}

/// Style the buttons in a bar to blend with it: no fill and no border until the pointer
/// is on one.
///
/// Scope this to a child `Ui`: it changes the visuals every later widget reads.
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
    use crate::testing::{self, context, fills};

    fn widths(width: f32, wanted: &[Track]) -> Vec<f32> {
        tracks(width, wanted, 0.0)
            .iter()
            .map(|track| track.end - track.start)
            .collect()
    }

    #[test]
    fn a_capped_track_stops_at_its_maximum_and_hands_the_rest_to_the_other_shares() {
        let wanted = [
            Track::Capped {
                share: 1.0,
                max: 40.0,
            },
            Track::Share(1.0),
        ];
        assert_eq!(widths(60.0, &wanted), vec![30.0, 30.0]);
        assert_eq!(widths(100.0, &wanted), vec![40.0, 60.0]);
        assert_eq!(widths(1000.0, &wanted), vec![40.0, 960.0]);
    }

    /// A cap is only a maximum, not a floor.
    #[test]
    fn a_width_below_the_fixed_tracks_shrinks_a_capped_track_like_any_other() {
        let wanted = [
            Track::Px(50.0),
            Track::Capped {
                share: 1.0,
                max: 40.0,
            },
            Track::Share(1.0),
        ];
        for width in [0.0_f32, 10.0, 50.0] {
            let held = widths(width, &wanted);
            assert!(
                held.iter().all(|track| *track >= 0.0),
                "at {width}: {held:?}"
            );
            assert!(
                held.iter().sum::<f32>() <= width + 0.01,
                "at {width}: {held:?}"
            );
        }
    }

    /// A header spans the full width at its kind's height, so a dock's body always starts
    /// at the same place and a header's fill reaches both edges.
    #[test]
    fn a_header_claims_its_own_height_and_the_whole_width() {
        let mut section = egui::Rect::ZERO;
        let mut dock = egui::Rect::ZERO;
        let mut width = 0.0;
        testing::run(&context(), egui::RawInput::default(), |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                width = ui.available_width();
                section = panel_header(ui, "places", &mut true).rect;
                dock = dock_header(ui, "browser").rect;
            });
        });
        assert_eq!(section.height(), HEADER);
        assert_eq!(dock.height(), crate::tabs::HEIGHT);
        assert_eq!(section.width(), width);
        assert_eq!(dock.width(), width);
    }

    /// Run frames with the pointer over the header or away from it, and return what the
    /// header painted behind itself.
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
            let output = testing::run(ctx, input, |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| rect = header(ui).rect);
            });
            at.set(rect.center());
            fill = fills(&output, rect);
        }
        fill
    }

    /// Three permanently gray bars down a dock would read as three separate panels, not
    /// as the headings of one.
    #[test]
    fn a_section_header_wears_the_panel_until_the_pointer_is_on_it() {
        let ctx = context();
        let section = |ui: &mut egui::Ui| panel_header(ui, "places", &mut true);
        assert_eq!(
            header_fill(&ctx, false, section),
            vec![egui::Color32::TRANSPARENT],
        );
        assert_eq!(
            header_fill(&ctx, true, section),
            vec![ctx.style().visuals.faint_bg_color],
        );
    }

    /// A dock header names the dock, not a section, and has nothing to click.
    #[test]
    fn a_dock_header_keeps_its_own_color_whether_or_not_it_is_pointed_at() {
        let ctx = context();
        let faint = ctx.style().visuals.faint_bg_color;
        for pointed in [false, true] {
            assert_eq!(
                header_fill(&ctx, pointed, |ui| dock_header(ui, "browser")),
                vec![faint],
                "pointed at: {pointed}",
            );
        }
    }

    #[test]
    fn the_triangle_toggles_the_bool_it_was_handed() {
        let ctx = context();
        let mut open = true;
        let at = std::cell::Cell::new(egui::Pos2::ZERO);
        let frame = |press: bool, open: &mut bool| {
            let input = egui::RawInput {
                events: match press {
                    true => testing::click(at.get()),
                    false => Vec::new(),
                },
                ..Default::default()
            };
            testing::run(&ctx, input, |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| {
                    let rect = panel_header(ui, "browser", open).rect;
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
        assert!(open, "the next click opens it again");
    }

    /// A scroll bar is a thumb with nothing under it, and the rows it scrolls stop short
    /// of it instead of running under it.
    #[test]
    fn a_scroll_bar_paints_no_track_and_keeps_clear_of_the_rows() {
        let ctx = context();
        let mut rows = egui::Rect::NOTHING;
        let mut output = None;
        // Enough frames, a tenth of a second apart, for the bar to finish appearing.
        for frame in 0..10 {
            let input = egui::RawInput {
                time: Some(f64::from(frame) / 10.0),
                ..testing::screen(egui::vec2(300.0, 200.0), Vec::new())
            };
            output = Some(testing::run(&ctx, input, |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| {
                    let area = egui::ScrollArea::vertical().auto_shrink(false);
                    area.show(ui, |ui| {
                        rows = ui.max_rect();
                        for row in 0..40 {
                            ui.label(format!("row {row}"));
                        }
                    });
                });
            }));
        }
        let output = output.expect("a frame ran");
        let thumb_fill = ctx.style().visuals.widgets.inactive.bg_fill;
        let thumb = testing::rects(&output)
            .into_iter()
            .find(|drawn| drawn.fill == thumb_fill)
            .expect("the overflowing list shows a thumb")
            .rect;
        assert!(
            rows.right() < thumb.left(),
            "rows end at {} and the thumb starts at {}",
            rows.right(),
            thumb.left()
        );
        let under: Vec<egui::Color32> = testing::rects(&output)
            .into_iter()
            .filter(|drawn| drawn.rect.x_range() == thumb.x_range() && drawn.rect != thumb)
            .map(|drawn| drawn.fill)
            .filter(|fill| fill.a() > 0)
            .collect();
        assert!(under.is_empty(), "the bar painted a track: {under:?}");
    }
}
