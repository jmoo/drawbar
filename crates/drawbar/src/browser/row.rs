//! The one row the tree is built from, painted rather than assembled from widgets.

use eframe::egui;

use crate::icon::{painted, Glyph};

/// What one row of the tree shows.
#[derive(Default)]
pub struct Cells<'a> {
    /// Where the row's contents start. See `tree::indent`.
    pub indent: f32,
    /// The triangle, and which way it points. A row with nothing under it has none.
    pub open: Option<bool>,
    pub glyph: Option<Glyph>,
    /// The monospace location column, `7:4`. Assets on this computer have none.
    pub at: Option<String>,
    pub name: &'a str,
    /// A faint word after the name — what kind of thing it is, or where it is owed.
    pub note: Option<&'a str>,
    /// The dot at the right end: attached, waiting, differs.
    pub dot: Option<egui::Color32>,
    /// The monospace readout at the right end — how full, or how many.
    pub count: Option<String>,
    /// The name is a stand-in rather than a real one.
    pub faint: bool,
    pub dirty: bool,
    /// The instrument's panel has this slot loaded.
    pub loaded: bool,
    /// A row inside a branch: a point shorter, and in the smaller face.
    pub child: bool,
}

/// A drawn row: what it answered, and where the parts a click can mean something on
/// ended up.
pub struct Drawn {
    pub response: egui::Response,
    /// The name's own rectangle — clicking the name of the only picked row starts a
    /// rename.
    pub name: egui::Rect,
    /// The triangle's box, where the row has one. A click there opens the branch rather
    /// than picking the row.
    pub chevron: Option<egui::Rect>,
}

/// How tall a row is, and a child row under it.
pub const ROW: f32 = 22.0;
pub const CHILD: f32 = 21.0;

/// The triangle's box and the gap after it — what a leaf skips so its glyph lines up
/// under the glyph of a branch beside it.
pub const CHEVRON: f32 = 12.0;
pub const STEP: f32 = CHEVRON + GAP;

/// The kind glyph's box, and the gap between a row's parts.
const GLYPH: f32 = 12.0;
const GAP: f32 = 6.0;

/// The dot that says attached, waiting or differs.
const DOT: f32 = 6.0;

/// The width the location column takes, so names line up under each other.
const AT_W: f32 = 34.0;

/// The faces a row paints in. Painted rather than laid out, so the sizes are here
/// rather than resolved from the named styles in [`crate::app`].
const NAME: f32 = 12.0;
const CHILD_NAME: f32 = 11.5;
const MONO: f32 = 10.0;

/// The ink for a cell that carries a colour of its own — a state word, a dependency, a
/// count, an address.
///
/// ⚠️ The signal colours measure 2.3–4.3:1 against `selection.bg_fill`. A selected row
/// gives every one of them the selection's own ink instead.
pub fn cell_ink(selected: bool, own: egui::Color32, visuals: &egui::Visuals) -> egui::Color32 {
    match selected {
        true => visuals.selection.stroke.color,
        false => own,
    }
}

/// One row of the tree: a full-width click target with its parts painted into it.
///
/// ⚠️ Nothing inside is a widget. A label allocates a hover rect of its own, which then
/// wins the hit test over the row — the highlight drops out as the pointer crosses the
/// text, and clicks land on whichever word happens to be under them. The row is the only
/// thing that senses.
pub(super) fn row(ui: &mut egui::Ui, selected: bool, cells: &Cells) -> Drawn {
    let height = match cells.child {
        true => CHILD,
        false => ROW,
    };
    let (rect, response) = ui.allocate_exact_size(
        egui::vec2(ui.available_width(), height),
        egui::Sense::click_and_drag(),
    );

    let visuals = ui.visuals().clone();
    let fill = match (selected, response.hovered()) {
        (true, _) => Some(visuals.selection.bg_fill),
        (false, true) => Some(visuals.faint_bg_color),
        (false, false) => None,
    };
    // ⚠️ Normal body text has insufficient contrast on the selection fill.
    let ink = match selected {
        true => visuals.selection.stroke.color,
        false => visuals.text_color(),
    };
    let quiet = cell_ink(selected, visuals.weak_text_color(), &visuals);
    let strong = match cells.faint {
        true => quiet,
        false => ink,
    };
    let painter = ui.painter().clone();
    if let Some(fill) = fill {
        painter.rect_filled(rect, 3.0, fill);
    }
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

    if let Some(glyph) = cells.glyph {
        let box_ =
            egui::Rect::from_min_size(egui::pos2(x, middle(GLYPH)), egui::Vec2::splat(GLYPH));
        painted(ui, glyph, box_, strong);
        x += GLYPH + GAP;
    }

    // One gutter, two marks that never meet: only a local asset is dirty, and only a
    // slot is loaded on the panel.
    if cells.dirty {
        let mark = cell_ink(selected, crate::app::warn(&visuals), &visuals);
        painter.circle_filled(egui::pos2(x + 3.5, rect.center().y), 3.5, mark);
        x += 10.0;
    } else if cells.loaded {
        let mark = cell_ink(selected, crate::app::good(&visuals), &visuals);
        painter.circle_stroke(
            egui::pos2(x + 3.5, rect.center().y),
            3.0,
            egui::Stroke::new(1.5_f32, mark),
        );
        x += 10.0;
    }

    if let Some(at) = &cells.at {
        let galley = painter.layout_no_wrap(at.clone(), egui::FontId::monospace(MONO), quiet);
        painter.galley(
            egui::pos2(x, middle(galley.size().y)),
            galley,
            egui::Color32::PLACEHOLDER,
        );
        x += AT_W;
    }

    // The right end is claimed first: the name takes whatever is left, and is cut to it.
    let mut right = rect.right() - GAP;
    if let Some(count) = &cells.count {
        let galley = painter.layout_no_wrap(count.clone(), egui::FontId::monospace(MONO), quiet);
        right -= galley.size().x;
        painter.galley(
            egui::pos2(right, middle(galley.size().y)),
            galley,
            egui::Color32::PLACEHOLDER,
        );
        right -= GAP;
    }
    if let Some(dot) = cells.dot {
        right -= DOT;
        painter.circle_filled(
            egui::pos2(right + DOT / 2.0, rect.center().y),
            DOT / 2.0,
            cell_ink(selected, dot, &visuals),
        );
        right -= GAP;
    }

    let size = match cells.child {
        true => CHILD_NAME,
        false => NAME,
    };
    let mut job = egui::text::LayoutJob::simple_singleline(
        cells.name.to_string(),
        egui::FontId::proportional(size),
        strong,
    );
    job.wrap = egui::text::TextWrapping::truncate_at_width((right - x).max(0.0));
    let galley = painter.layout_job(job);
    let elided = galley.elided;
    let at = egui::pos2(x, middle(galley.size().y));
    let name = egui::Rect::from_min_size(at, galley.size());
    x += galley.size().x + GAP;
    painter.galley(at, galley, egui::Color32::PLACEHOLDER);

    if let Some(note) = cells.note {
        let mut job = egui::text::LayoutJob::simple_singleline(
            note.to_string(),
            egui::FontId::proportional(MONO),
            quiet,
        );
        job.wrap = egui::text::TextWrapping::truncate_at_width((right - x).max(0.0));
        let galley = painter.layout_job(job);
        painter.galley(
            egui::pos2(x, middle(galley.size().y)),
            galley,
            egui::Color32::PLACEHOLDER,
        );
    }

    // A name the row had to cut is a name nothing else in this panel would show.
    let response = match elided {
        true => response.on_hover_text(cells.name),
        false => response,
    };
    Drawn {
        response,
        name,
        chevron,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_coloured_cell_keeps_its_colour_until_its_row_is_selected() {
        for visuals in [egui::Visuals::dark(), egui::Visuals::light()] {
            for own in [
                crate::app::good(&visuals),
                crate::app::warn(&visuals),
                crate::app::bad(&visuals),
                crate::app::accent(&visuals),
                crate::app::unlit(&visuals),
            ] {
                assert_eq!(cell_ink(false, own, &visuals), own);
                assert_eq!(
                    cell_ink(true, own, &visuals),
                    visuals.selection.stroke.color
                );
            }
        }
    }

    /// ⚠️ A name too long for the panel is cut with an ellipsis rather than painted over
    /// the count beside it, and the whole of it is one hover away.
    #[test]
    fn a_name_too_long_for_its_row_is_cut_and_offered_on_hover() {
        let ctx = egui::Context::default();
        let long = "Africa Split, the one with the long tail and the second manual";
        let mut cut = egui::Rect::NOTHING;
        let mut whole = egui::Rect::NOTHING;
        let _ = ctx.run(egui::RawInput::default(), |ctx| {
            egui::SidePanel::left("places")
                .exact_width(232.0)
                .show(ctx, |ui| {
                    cut = row(
                        ui,
                        false,
                        &Cells {
                            name: long,
                            count: Some("128/400".into()),
                            ..Cells::default()
                        },
                    )
                    .name;
                    whole = row(
                        ui,
                        false,
                        &Cells {
                            name: "Africa Split",
                            ..Cells::default()
                        },
                    )
                    .name;
                });
        });
        assert!(cut.width() > 0.0, "something was painted");
        assert!(
            cut.right() <= 232.0,
            "it stayed inside the panel: {}",
            cut.right()
        );
        assert!(whole.width() < cut.width(), "a short name is not cut");
    }
}
