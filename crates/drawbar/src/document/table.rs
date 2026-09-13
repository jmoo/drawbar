//! The grid the document's row lists are laid out on: what each column is worth, and
//! the strip of heads above them.
//!
//! A list states its columns as a table of widths and a table of names; the arithmetic
//! that turns those into left edges is written once.

use eframe::egui;

use crate::app;

/// The page's own side margin, which every row and heading keeps.
pub const PAD: f32 = 12.0;
/// The room between two columns.
const GAP: f32 = 10.0;
const HEAD_H: f32 = 20.0;
const HEAD_TEXT: f32 = 9.0;
/// The first column of a row, which is what the row is called.
pub const NAME_TEXT: f32 = 11.5;

/// What one column is worth: a figure the list fixes, or a share of what the fixed ones
/// leave.
#[derive(Clone, Copy)]
pub enum Width {
    Fixed(f32),
    Share(f32),
}

/// Each column's left edge and width, in order.
pub fn columns<const N: usize>(rect: egui::Rect, widths: [Width; N]) -> [(f32, f32); N] {
    let fixed: f32 = widths
        .iter()
        .map(|width| match width {
            Width::Fixed(width) => *width,
            Width::Share(_) => 0.0,
        })
        .sum();
    let shares: f32 = widths
        .iter()
        .map(|width| match width {
            Width::Share(share) => *share,
            Width::Fixed(_) => 0.0,
        })
        .sum();
    let taken = fixed + GAP * N.saturating_sub(1) as f32 + PAD * 2.0;
    let free = (rect.width() - taken).max(0.0);
    let mut left = rect.left() + PAD;
    let mut out = [(0.0, 0.0); N];
    for (cell, width) in out.iter_mut().zip(widths) {
        let width = match width {
            Width::Fixed(width) => width,
            // A table of nothing but fixed columns has no free room to divide.
            Width::Share(share) if shares > 0.0 => free * share / shares,
            Width::Share(_) => 0.0,
        };
        *cell = (left, width);
        left += width + GAP;
    }
    out
}

/// The strip above the rows: a hairline over and under, and each column's name in MICRO
/// caps. A column named with an empty string gets no head.
///
/// `right` names the columns whose figures are set right to left, so the head stands over
/// them rather than over the room beside them.
pub fn heads<const N: usize>(
    ui: &mut egui::Ui,
    widths: [Width; N],
    labels: [&str; N],
    right: &[usize],
) {
    let visuals = ui.visuals().clone();
    let quiet = app::caption(&visuals);
    let hairline = egui::Stroke::new(1.0_f32, visuals.widgets.noninteractive.bg_stroke.color);
    let (rect, _) = ui.allocate_exact_size(
        egui::vec2(ui.available_width(), HEAD_H),
        egui::Sense::hover(),
    );
    let painter = ui.painter();
    painter.hline(rect.x_range(), rect.top() + 0.5, hairline);
    painter.hline(rect.x_range(), rect.bottom() - 0.5, hairline);
    for (column, ((left, width), text)) in columns(rect, widths).into_iter().zip(labels).enumerate()
    {
        if text.is_empty() {
            continue;
        }
        let galley = painter.layout_no_wrap(
            text.to_uppercase(),
            egui::FontId::proportional(HEAD_TEXT),
            quiet,
        );
        let left = match right.contains(&column) {
            true => left + width - galley.size().x,
            false => left,
        };
        painter.galley(
            egui::pos2(left, rect.center().y - galley.size().y / 2.0),
            galley,
            quiet,
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The fixed columns take what they are given, the rest divide what is left in the
    /// proportions asked for, and every column keeps the page's margin and the gap
    /// between it and the next.
    #[test]
    fn the_shared_columns_divide_what_the_fixed_ones_leave() {
        let rect = egui::Rect::from_min_size(egui::pos2(0.0, 0.0), egui::vec2(500.0, 20.0));
        let cells = columns(
            rect,
            [
                Width::Fixed(56.0),
                Width::Share(1.1),
                Width::Share(1.5),
                Width::Fixed(74.0),
            ],
        );
        let free = 500.0 - 56.0 - 74.0 - GAP * 3.0 - PAD * 2.0;
        assert_eq!(cells[0], (PAD, 56.0));
        assert_eq!(cells[1].1, free * 1.1 / 2.6);
        assert_eq!(cells[2].1, free * 1.5 / 2.6);
        assert_eq!(cells[3].1, 74.0);
        assert_eq!(cells[3].0 + cells[3].1, 500.0 - PAD);

        // Narrower than the fixed columns: the shared ones vanish rather than going
        // negative and painting to the left of the column before them.
        let tight = egui::Rect::from_min_size(egui::pos2(0.0, 0.0), egui::vec2(40.0, 20.0));
        let cells = columns(tight, [Width::Fixed(56.0), Width::Share(1.0)]);
        assert_eq!(cells[1].1, 0.0);
    }
}
