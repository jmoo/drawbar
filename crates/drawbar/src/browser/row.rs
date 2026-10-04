//! The row the whole tree is built from, painted without child widgets.

use eframe::egui;

use crate::icon::{painted, Glyph};

/// What one row of the tree shows.
#[derive(Default)]
pub struct Cells<'a> {
    /// Where the row's contents start. See `tree::indent`.
    pub indent: f32,
    /// The triangle, and whether it points open. `None` for a row with nothing under it.
    pub open: Option<bool>,
    pub glyph: Option<Glyph>,
    /// The monospace location column, `7:4`. Assets on this computer have none.
    pub at: Option<String>,
    pub name: &'a str,
    /// A faint word after the name: its kind, or the slot it is queued for.
    pub note: Option<&'a str>,
    /// The dot at the right end, and its hover text. Nothing else in the row explains the
    /// dot.
    pub dot: Option<(egui::Color32, &'a str)>,
    /// The monospace readout at the right end: how full, or how many.
    pub count: Option<String>,
    /// How many tags it has, painted at the right end as the tag glyph and a number.
    /// Nothing is drawn for zero.
    pub tags: usize,
    /// The name is a placeholder, drawn faint.
    pub faint: bool,
    /// It differs from what was last saved, which the name shows with a star.
    pub unsaved: bool,
    /// The instrument's panel has this slot loaded.
    pub loaded: bool,
    /// A row inside a branch: shorter, with a smaller glyph and font.
    pub child: bool,
    /// The name keeps its extension, for a file with no kind glyph to say what it is.
    pub whole: bool,
}

/// A drawn row: its response, and where its clickable parts ended up.
pub struct Drawn {
    pub response: egui::Response,
    /// The triangle's box, if the row has one. A click there opens the branch instead of
    /// selecting the row.
    pub chevron: Option<egui::Rect>,
}

/// The height of a row, and of a child row.
pub const ROW: f32 = 28.0;
pub const CHILD: f32 = 26.0;

/// The triangle's box, and the box plus the gap after it: what a leaf skips so its glyph
/// lines up under the glyph of a branch beside it.
pub const CHEVRON: f32 = 12.0;
pub const STEP: f32 = CHEVRON + GAP;

/// The gap between a row's parts, and the room left after the last one at the right end.
const GAP: f32 = 8.0;
const END: f32 = 8.0;

/// The kind glyph's box, in a row and in a child row, and its opacity relative to the
/// name's ink.
const GLYPH: f32 = 14.0;
const CHILD_GLYPH: f32 = 13.0;
const GLYPH_ALPHA: f32 = 0.85;

/// The diameter of the status dot.
const DOT: f32 = 6.0;

/// The size of the tag glyph beside the tag count.
const SMALL: f32 = 11.0;

/// The width the location column takes, so names line up under each other.
const AT_W: f32 = 34.0;

/// The text sizes a child row paints in, and the monospace size of a row's readouts. A
/// top-level row's name is [`crate::app::ui`].
const CHILD_NAME: f32 = 12.0;
const MONO: f32 = 10.5;
const CHILD_MONO: f32 = 10.0;

/// The text color for a cell with a color of its own: a state word, a dependency, a
/// count, an address.
///
/// ⚠️ The signal colors measure 2.3–4.3:1 contrast against `selection.bg_fill`, so a
/// selected row draws all of them in the selection's text color.
pub fn cell_ink(selected: bool, own: egui::Color32, visuals: &egui::Visuals) -> egui::Color32 {
    match selected {
        true => visuals.selection.stroke.color,
        false => own,
    }
}

/// The name as laid out: italic and starred while the row differs from what was last
/// saved.
///
/// ⚠️ The star is part of the text, so a name too long for its row loses the star first.
/// The hover then shows the full name, without the star.
pub(super) fn name_job(
    cells: &Cells,
    font: egui::FontId,
    tint: egui::Color32,
) -> egui::text::LayoutJob {
    let mut job = egui::text::LayoutJob::default();
    let name = match cells.whole {
        true => cells.name.to_string(),
        false => starred(cells.name, cells.unsaved),
    };
    job.append(
        &name,
        0.0,
        egui::TextFormat {
            font_id: font,
            color: tint,
            italics: cells.unsaved,
            ..egui::TextFormat::default()
        },
    );
    job
}

/// The name a row shows: without the format tag, and with a star while it differs from
/// what was last saved.
pub fn starred(name: &str, unsaved: bool) -> String {
    let shown = crate::strings::display_name(name);
    match unsaved {
        true => format!("{shown}*"),
        false => shown.to_string(),
    }
}

/// One row of the tree: a click target standing in [`crate::panel::ROW_INSET`] from the
/// card's edges, with its parts painted into it.
///
/// ⚠️ Nothing inside is a widget. A label allocates its own hover rect, which wins the
/// hit test over the row: the highlight drops out as the pointer crosses the text, and
/// clicks land on whichever word is under them. Only the row senses input.
pub(super) fn row(ui: &mut egui::Ui, selected: bool, cells: &Cells) -> Drawn {
    let (height, glyph, font, mono) = match cells.child {
        true => (
            CHILD,
            CHILD_GLYPH,
            egui::FontId::proportional(CHILD_NAME),
            egui::FontId::monospace(CHILD_MONO),
        ),
        false => (
            ROW,
            GLYPH,
            crate::app::ui().resolve(ui.style()),
            egui::FontId::monospace(MONO),
        ),
    };
    let (id, room) = ui.allocate_space(egui::vec2(ui.available_width(), height));
    let rect = room.shrink2(egui::vec2(crate::panel::ROW_INSET, 0.0));
    let response = ui.interact(rect, id, egui::Sense::click_and_drag());

    let visuals = ui.visuals().clone();
    let painter = ui.painter().clone();
    let (ink, quiet) =
        crate::panel::row_ink(&painter, rect, selected, response.hovered(), &visuals);
    let strong = match cells.faint {
        true => quiet,
        false => ink,
    };
    let middle = |size: f32| rect.center().y - size / 2.0;

    let mut x = rect.left() + cells.indent;
    let chevron = cells.open.map(|open| {
        let box_ =
            egui::Rect::from_min_size(egui::pos2(x, middle(CHEVRON)), egui::Vec2::splat(CHEVRON));
        let glyph = match open {
            true => Glyph::ChevronDown,
            false => Glyph::ChevronRight,
        };
        painted(ui, glyph, box_, quiet);
        x += STEP;
        box_
    });

    if let Some(kind) = cells.glyph {
        let box_ =
            egui::Rect::from_min_size(egui::pos2(x, middle(glyph)), egui::Vec2::splat(glyph));
        painted(ui, kind, box_, strong.gamma_multiply(GLYPH_ALPHA));
        x += glyph + GAP;
    }

    if cells.loaded {
        let mark = cell_ink(selected, crate::app::good(&visuals), &visuals);
        painter.circle_stroke(
            egui::pos2(x + 3.5, rect.center().y),
            3.0,
            egui::Stroke::new(1.5_f32, mark),
        );
        x += 10.0;
    }

    if let Some(at) = &cells.at {
        let galley = painter.layout_no_wrap(at.clone(), mono.clone(), quiet);
        painter.galley(
            egui::pos2(x, middle(galley.size().y)),
            galley,
            egui::Color32::PLACEHOLDER,
        );
        x += AT_W;
    }

    // The right end is laid out first; the name gets the remaining width, truncated.
    let mut right = rect.right() - END;
    if cells.tags > 0 {
        let galley = painter.layout_no_wrap(cells.tags.to_string(), mono.clone(), quiet);
        right -= galley.size().x;
        painter.galley(
            egui::pos2(right, middle(galley.size().y)),
            galley,
            egui::Color32::PLACEHOLDER,
        );
        right -= SMALL + 3.0;
        painted(
            ui,
            Glyph::Tag,
            egui::Rect::from_min_size(egui::pos2(right, middle(SMALL)), egui::Vec2::splat(SMALL)),
            quiet,
        );
        right -= GAP;
    }
    if let Some(count) = &cells.count {
        let galley = painter.layout_no_wrap(count.clone(), mono.clone(), quiet);
        right -= galley.size().x;
        painter.galley(
            egui::pos2(right, middle(galley.size().y)),
            galley,
            egui::Color32::PLACEHOLDER,
        );
        right -= GAP;
    }
    let dot = cells.dot.map(|(tint, said)| {
        right -= DOT;
        let box_ = egui::Rect::from_center_size(
            egui::pos2(right + DOT / 2.0, rect.center().y),
            egui::Vec2::splat(DOT),
        );
        painter.circle_filled(box_.center(), DOT / 2.0, cell_ink(selected, tint, &visuals));
        right -= GAP;
        (box_.expand(GAP / 2.0), said)
    });

    let mut job = name_job(cells, font, strong);
    job.wrap = egui::text::TextWrapping::truncate_at_width((right - x).max(0.0));
    let galley = painter.layout_job(job);
    let elided = galley.elided;
    let at = egui::pos2(x, middle(galley.size().y));
    x += galley.size().x + GAP;
    painter.galley(at, galley, egui::Color32::PLACEHOLDER);

    if let Some(note) = cells.note {
        crate::panel::cut(
            &painter,
            x,
            rect.center().y,
            right - x,
            note,
            egui::TextFormat::simple(egui::FontId::proportional(mono.size), quiet),
        );
    }

    // ⚠️ The dot is not a widget either: its own hover rect would take the hit test from
    // the row. The row's hover text depends on where the pointer is, as in the library's
    // cells.
    let over = |box_: egui::Rect| response.hover_pos().is_some_and(|at| box_.contains(at));
    let said = match dot {
        Some((box_, said)) if over(box_) => Some(said),
        // A truncated name is shown in full nowhere else in this panel.
        _ => elided.then_some(cells.name),
    };
    let response = match said {
        Some(said) => response.on_hover_text(said),
        None => response,
    };
    Drawn { response, chevron }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::{self, context};

    #[test]
    fn an_unsaved_row_writes_its_name_with_a_star() {
        let output = testing::run(&context(), egui::RawInput::default(), |ctx| {
            egui::SidePanel::left("places")
                .exact_width(232.0)
                .show(ctx, |ui| {
                    for unsaved in [false, true] {
                        row(
                            ui,
                            false,
                            &Cells {
                                name: "Africa Split",
                                unsaved,
                                ..Cells::default()
                            },
                        );
                    }
                });
        });
        let said = testing::words(&output);
        assert!(said.contains(&"Africa Split".to_string()), "{said:?}");
        assert!(said.contains(&"Africa Split*".to_string()), "{said:?}");
    }

    /// ⚠️ A name too long for the panel is truncated so it does not paint over the count
    /// beside it. The row's hover then shows it in full.
    #[test]
    fn a_name_too_long_for_its_row_is_cut_to_the_room_left() {
        let long = "Africa Split, the one with the long tail and the second manual";
        let mut card = egui::Rect::NOTHING;
        let output = testing::run(&context(), egui::RawInput::default(), |ctx| {
            egui::SidePanel::left("places")
                .exact_width(232.0)
                .show(ctx, |ui| {
                    card = ui.available_rect_before_wrap();
                    row(
                        ui,
                        false,
                        &Cells {
                            name: long,
                            count: Some("128/400".into()),
                            ..Cells::default()
                        },
                    );
                    row(
                        ui,
                        false,
                        &Cells {
                            name: "Africa Split",
                            ..Cells::default()
                        },
                    );
                });
        });

        let painted = testing::painted(&output);
        let cut = painted
            .iter()
            .find(|word| word.text == long)
            .expect("the long name was painted");
        assert!(cut.galley.elided, "a name without room is truncated");
        let count = testing::where_(&painted, "128/400");
        assert!(
            cut.rect.right() <= count.left(),
            "and stops before the count: {:?} against {count:?}",
            cut.rect
        );
        assert!(
            count.right() <= card.right() - crate::panel::ROW_INSET,
            "which stays inside the row: {count:?} in {card:?}"
        );
        assert!(
            painted
                .iter()
                .any(|word| word.text == "Africa Split" && !word.galley.elided),
            "a short name is painted whole"
        );
    }

    /// A row's highlight stands in from both edges of the card and is rounded, and a row
    /// inside a branch is shorter than one at the top.
    #[test]
    fn a_row_stands_in_from_the_card_at_its_own_height() {
        let ctx = context();
        let mut card = egui::Rect::NOTHING;
        let output = testing::run(&ctx, egui::RawInput::default(), |ctx| {
            egui::SidePanel::left("places")
                .exact_width(232.0)
                .show(ctx, |ui| {
                    card = ui.available_rect_before_wrap();
                    for child in [false, true] {
                        row(
                            ui,
                            true,
                            &Cells {
                                name: "Africa Split",
                                child,
                                ..Cells::default()
                            },
                        );
                    }
                });
        });

        let lit = ctx.style().visuals.selection.bg_fill;
        let highlights: Vec<egui::epaint::RectShape> = testing::rects(&output)
            .into_iter()
            .filter(|drawn| drawn.fill == lit)
            .collect();
        let heights: Vec<f32> = highlights.iter().map(|drawn| drawn.rect.height()).collect();
        assert_eq!(heights, [ROW, CHILD]);
        let inset = crate::panel::ROW_INSET;
        for drawn in &highlights {
            assert_eq!(drawn.rect.left(), card.left() + inset, "{:?}", drawn.rect);
            assert_eq!(drawn.rect.right(), card.right() - inset, "{:?}", drawn.rect);
            assert_eq!(
                drawn.corner_radius,
                egui::CornerRadius::same(crate::panel::ROW_RADIUS)
            );
        }
    }
}
