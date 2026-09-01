//! The one row every list is built from, painted rather than assembled from widgets.

use eframe::egui;

/// What one row of a list shows.
#[derive(Default)]
pub struct Cells<'a> {
    /// The monospace location column, `7:4`. Assets on this computer have none.
    pub at: Option<String>,
    pub name: &'a str,
    /// A faint word after the name — what kind of thing it is, or where it is owed.
    pub note: Option<&'a str>,
    /// The note is a destination rather than a kind, so it is worth noticing.
    pub waiting: bool,
    /// The name is a stand-in rather than a real one.
    pub faint: bool,
    pub dirty: bool,
    /// The instrument's panel has this slot loaded.
    pub loaded: bool,
}

/// A drawn row: what it answered, and where its name ended up.
pub struct Drawn {
    pub response: egui::Response,
    /// The name's own rectangle — the only sub-area of a row that means anything, and
    /// only because clicking the name of a selected row starts a rename.
    pub name: egui::Rect,
}

/// The width the location column takes, so names line up under each other.
const AT_W: f32 = 42.0;

/// One row of a list: a full-width click target with its text painted into it.
///
/// ⚠️ Nothing inside is a widget. A label allocates a hover rect of its own, which then
/// wins the hit test over the row — the highlight drops out as the pointer crosses the
/// text, and clicks land on whichever word happens to be under them. The row is the only
/// thing that senses.
pub(super) fn row(ui: &mut egui::Ui, selected: bool, cells: &Cells) -> Drawn {
    let height = ui.text_style_height(&egui::TextStyle::Body) + 4.0;
    let (rect, response) = ui.allocate_exact_size(
        egui::vec2(ui.available_width(), height),
        egui::Sense::click_and_drag(),
    );

    let visuals = ui.visuals();
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
    let weak = match selected {
        true => ink.gamma_multiply(visuals.weak_text_alpha),
        false => visuals.weak_text_color(),
    };
    let strong = match cells.faint {
        true => weak,
        false => ink,
    };
    let painter = ui.painter().clone();
    if let Some(fill) = fill {
        painter.rect_filled(rect, 3.0, fill);
    }

    let mut x = rect.left() + 4.0;
    let gutter = egui::pos2(x + 3.5, rect.center().y);
    // One gutter, two marks that never meet: only a local asset is dirty, and only a slot
    // is loaded on the panel.
    if cells.dirty {
        painter.circle_filled(gutter, 3.5, crate::app::warn(ui.visuals()));
    } else if cells.loaded {
        painter.circle_stroke(
            gutter,
            3.0,
            egui::Stroke::new(1.5_f32, crate::app::good(ui.visuals())),
        );
    }
    x += 10.0;

    if let Some(at) = &cells.at {
        let galley = painter.layout_no_wrap(at.clone(), egui::FontId::monospace(11.0), weak);
        painter.galley(
            egui::pos2(x, rect.center().y - galley.size().y / 2.0),
            galley,
            egui::Color32::PLACEHOLDER,
        );
        x += AT_W;
    }

    let font = egui::FontId::proportional(13.0);
    let galley = painter.layout_no_wrap(cells.name.to_string(), font.clone(), strong);
    let at = egui::pos2(x, rect.center().y - galley.size().y / 2.0);
    let name = egui::Rect::from_min_size(at, galley.size());
    x += galley.size().x + 8.0;
    painter.galley(at, galley, egui::Color32::PLACEHOLDER);

    if let Some(note) = cells.note {
        let galley =
            painter.layout_no_wrap(note.to_string(), egui::FontId::proportional(10.0), weak);
        painter.galley(
            egui::pos2(x, rect.center().y - galley.size().y / 2.0),
            galley,
            egui::Color32::PLACEHOLDER,
        );
    }
    Drawn { response, name }
}
