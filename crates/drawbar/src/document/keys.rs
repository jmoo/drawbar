//! The key map: the keyboard, the lanes over it, and the geometry they share.
//!
//! ⚠️ Two geometries meet here and they do not line up. A lane gives every key in the
//! span an equal share of the width ([`Span`]); the keyboard gives every *white* key an
//! equal share and hangs the blacks between them. A band edge and the key beneath it are
//! therefore a fraction of a key apart in the middle of the span. The lanes agree with
//! each other, and the keyboard agrees with the keys.
//!
//! Nothing here scrolls, pins or holds state. A lane takes the width it is offered,
//! paints what it was handed, and answers with what the pointer asked for.

use eframe::egui;

use crate::app;
use crate::note;

/// The velocity a click on the keyboard plays at.
pub const AUDITION_VELOCITY: u8 = 90;

/// The stretch of keyboard a lane covers, inclusive at both ends.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Span {
    pub low: u8,
    pub high: u8,
}

impl Span {
    /// The ends in order, so a span handed over backwards still reads as the range
    /// between them rather than panicking a clamp.
    fn ends(self) -> (u8, u8) {
        (self.low.min(self.high), self.low.max(self.high))
    }

    /// How many keys the span holds. Never zero.
    pub fn keys(self) -> usize {
        let (low, high) = self.ends();
        (high - low) as usize + 1
    }

    pub fn contains(self, note: u8) -> bool {
        let (low, high) = self.ends();
        note >= low && note <= high
    }

    /// How wide one key's cell is.
    fn unit(self, rect: egui::Rect) -> f32 {
        rect.width() / self.keys() as f32
    }

    /// The left edge of `note`'s cell. A note past either end lands on that end.
    pub fn x_of(self, rect: egui::Rect, note: u8) -> f32 {
        let (low, high) = self.ends();
        let index = note.clamp(low, high) - low;
        rect.left() + self.unit(rect) * index as f32
    }

    /// The right edge of `note`'s cell, which is the left edge of the next one.
    pub fn x_after(self, rect: egui::Rect, note: u8) -> f32 {
        self.x_of(rect, note) + self.unit(rect)
    }

    /// The key `x` falls in, clamped to the span.
    pub fn note_at(self, rect: egui::Rect, x: f32) -> u8 {
        let (low, _) = self.ends();
        let unit = self.unit(rect);
        let index = match unit > 0.0 {
            true => ((x - rect.left()) / unit).floor(),
            false => 0.0,
        };
        low + index.clamp(0.0, (self.keys() - 1) as f32) as u8
    }
}

pub fn is_black(note: u8) -> bool {
    matches!(note % 12, 1 | 3 | 6 | 8 | 10)
}

/// How far a played key is from the root that answers it.
pub fn shifted(note: u8, root: u8) -> String {
    format!("shifted {:+} st", note as i16 - root as i16)
}

/// What the keyboard is sounding, and since when.
///
/// ⚠️ `std::time::Instant::now()` traps on `wasm32-unknown-unknown`, so the clock is
/// egui's own frame time in seconds — [`egui::InputState::time`], as [`crate::log`] uses.
pub struct Audition {
    pub note: u8,
    pub started: f64,
}

impl Audition {
    /// How long a struck key stays lit, in seconds.
    pub const HOLD: f64 = 2.2;

    pub fn new(note: u8, now: f64) -> Audition {
        Audition { note, started: now }
    }

    pub fn live(&self, now: f64) -> bool {
        now - self.started < Audition::HOLD
    }
}

// ---- shared paint -------------------------------------------------------------------

/// The dashes of an outline. egui draws dashes along a line, so a shape is its corners
/// in order.
fn dashed(painter: &egui::Painter, corners: &[egui::Pos2], stroke: egui::Stroke) {
    const DASH: f32 = 3.0;
    for side in corners.windows(2) {
        painter.extend(egui::Shape::dashed_line(side, stroke, DASH, DASH));
    }
}

/// The four sides of a dashed border.
pub fn dashed_rect(painter: &egui::Painter, rect: egui::Rect, stroke: egui::Stroke) {
    dashed(
        painter,
        &[
            rect.left_top(),
            rect.right_top(),
            rect.right_bottom(),
            rect.left_bottom(),
            rect.left_top(),
        ],
        stroke,
    );
}

/// Diagonal lines across `rect`, for a stretch of keyboard nothing answers.
///
/// Clipped to `rect`, so the lines that reach past a corner stop at the edge rather than
/// crossing whatever is drawn beside it.
pub fn hatch(painter: &egui::Painter, rect: egui::Rect, color: egui::Color32, alpha: f32) {
    const STEP: f32 = 5.0;

    let painter = painter.with_clip_rect(rect);
    let stroke = egui::Stroke::new(1.0_f32, color.gamma_multiply(alpha));
    let reach = rect.height();
    let mut lines = Vec::new();
    let mut x = rect.left() - reach;
    while x < rect.right() {
        lines.push(egui::Shape::line_segment(
            [
                egui::pos2(x, rect.bottom()),
                egui::pos2(x + reach, rect.top()),
            ],
            stroke,
        ));
        x += STEP;
    }
    painter.add(egui::Shape::Vec(lines));
}

/// `text` laid out to at most `width`, with an ellipsis where it did not fit.
fn clipped(
    painter: &egui::Painter,
    text: &str,
    font: egui::FontId,
    color: egui::Color32,
    width: f32,
) -> std::sync::Arc<egui::Galley> {
    let mut job = egui::text::LayoutJob::default();
    job.append(text, 0.0, egui::TextFormat::simple(font, color));
    job.wrap = egui::text::TextWrapping::truncate_at_width(width.max(0.0));
    painter.layout_job(job)
}

/// A galley painted left-aligned, vertically centred on `middle`.
fn write(painter: &egui::Painter, left: f32, middle: f32, galley: std::sync::Arc<egui::Galley>) {
    let top = middle - galley.size().y / 2.0;
    painter.galley(egui::pos2(left, top), galley, egui::Color32::PLACEHOLDER);
}

/// A mono chip on the window fill: what a drag reads, and what a root is called.
fn chip(
    painter: &egui::Painter,
    at: egui::Pos2,
    text: &str,
    edge: egui::Color32,
    ink: egui::Color32,
) -> egui::Rect {
    let font = egui::FontId::monospace(CHIP_TEXT);
    let galley = painter.layout_no_wrap(text.to_owned(), font, ink);
    let rect = egui::Rect::from_min_size(
        egui::pos2(at.x - (galley.size().x + CHIP_PAD * 2.0) / 2.0, at.y),
        galley.size() + egui::vec2(CHIP_PAD * 2.0, 2.0),
    );
    painter.rect_filled(rect, RADIUS, painter.ctx().style().visuals.window_fill);
    painter.rect_stroke(
        rect,
        RADIUS,
        egui::Stroke::new(1.0_f32, edge),
        egui::StrokeKind::Inside,
    );
    painter.galley(
        egui::pos2(rect.left() + CHIP_PAD, rect.top() + 1.0),
        galley,
        egui::Color32::PLACEHOLDER,
    );
    rect
}

/// The corner every rectangle here is drawn with.
const RADIUS: f32 = 2.0;

const CHIP_TEXT: f32 = 9.5;
const CHIP_PAD: f32 = 3.0;

// ---- the keyboard -------------------------------------------------------------------

/// A key worth pointing at: a zone's root, or a library's.
pub struct Mark {
    pub note: u8,
    /// The name beside the marker, where there is room for one to mean something.
    pub label: Option<String>,
}

const KEYBOARD_H: f32 = 58.0;
const BLACK_H: f32 = 35.0;
const WHITE_GAP: f32 = 1.0;
const OCTAVE_TEXT: f32 = 8.0;
/// How much of the key's own ink an octave label keeps.
const OCTAVE_ALPHA: f32 = 0.7;
const MARK_TOP: f32 = 2.0;
const MARK_H: f32 = 4.0;
const MARK_W: f32 = 6.0;

/// How many white keys the span holds.
fn white_count(span: Span) -> usize {
    let (low, high) = span.ends();
    (low..=high).filter(|note| !is_black(*note)).count()
}

/// How many white keys come before `note`.
fn whites_before(span: Span, note: u8) -> usize {
    let (low, _) = span.ends();
    (low..note).filter(|note| !is_black(*note)).count()
}

/// The key `note` occupies: a white key's share of the width, or a black key hung over
/// the two whites it sits between.
fn key_rect(rect: egui::Rect, span: Span, note: u8) -> egui::Rect {
    let unit = rect.width() / white_count(span).max(1) as f32;
    let seen = whites_before(span, note) as f32;
    match is_black(note) {
        true => egui::Rect::from_min_size(
            egui::pos2(rect.left() + unit * (seen - 0.3), rect.top()),
            egui::vec2(unit * 0.6, BLACK_H),
        ),
        false => egui::Rect::from_min_max(
            egui::pos2(rect.left() + unit * seen, rect.top()),
            egui::pos2(rect.left() + unit * (seen + 1.0) - WHITE_GAP, rect.bottom()),
        ),
    }
}

/// The key under `at`. Black keys are drawn over the whites, so they are tested first.
fn key_at(rect: egui::Rect, span: Span, at: egui::Pos2) -> Option<u8> {
    if !rect.contains(at) {
        return None;
    }
    let (low, high) = span.ends();
    let hit = |note: &u8| key_rect(rect, span, *note).contains(at);
    (low..=high)
        .filter(|note| is_black(*note))
        .find(hit)
        .or_else(|| (low..=high).filter(|note| !is_black(*note)).find(hit))
}

/// The keyboard, one clickable key per note in `span`.
///
/// `lit` is the note being auditioned: its key stays lit and wears the chip saying what
/// is playing. Returns the key that was clicked, which the caller plays at
/// [`AUDITION_VELOCITY`].
pub fn keyboard(ui: &mut egui::Ui, span: Span, lit: Option<u8>, marks: &[Mark]) -> Option<u8> {
    let width = ui.available_width().max(1.0);
    let (rect, response) =
        ui.allocate_exact_size(egui::vec2(width, KEYBOARD_H), egui::Sense::click());
    let visuals = ui.visuals().clone();
    let painter = ui.painter().clone();
    let hovered = response.hover_pos().and_then(|at| key_at(rect, span, at));

    let (low, high) = span.ends();
    for note in low..=high {
        let key = key_rect(rect, span, note);
        let black = is_black(note);
        let fill = match (black, lit == Some(note) || hovered == Some(note)) {
            (true, false) => app::stop_black(&visuals),
            (true, true) => app::accent(&visuals),
            (false, false) => app::stop_white(&visuals),
            (false, true) => visuals.selection.bg_fill,
        };
        painter.rect_filled(key, 0.0, fill);
        if note % 12 == 0 {
            painter.text(
                egui::pos2(key.center().x, key.bottom() - 2.0),
                egui::Align2::CENTER_BOTTOM,
                note::name(note),
                egui::FontId::monospace(OCTAVE_TEXT),
                app::stop_black(&visuals).gamma_multiply(OCTAVE_ALPHA),
            );
        }
    }

    for mark in marks {
        root_mark(&painter, rect, span, mark, &visuals);
    }
    if let Some(note) = lit {
        audition_chip(ui, rect, span, note);
    }

    let response = match hovered {
        Some(note) => response.on_hover_text(format!(
            "Play {} at velocity {AUDITION_VELOCITY}",
            note::name(note)
        )),
        None => response,
    };
    match response.clicked() {
        true => response
            .interact_pointer_pos()
            .and_then(|at| key_at(rect, span, at)),
        false => None,
    }
}

/// A marker over one key: a triangle at the top of it, with its name over that where the
/// caller gave one.
fn root_mark(
    painter: &egui::Painter,
    rect: egui::Rect,
    span: Span,
    mark: &Mark,
    visuals: &egui::Visuals,
) {
    let centre = key_rect(rect, span, mark.note).center().x;
    let accent = app::accent(visuals);
    let mut top = rect.top() + MARK_TOP;
    if let Some(label) = &mark.label {
        let named = chip(
            painter,
            egui::pos2(centre, top),
            label,
            accent,
            visuals.text_color(),
        );
        top = named.bottom() + 1.0;
    }
    painter.add(egui::Shape::convex_polygon(
        vec![
            egui::pos2(centre - MARK_W / 2.0, top),
            egui::pos2(centre + MARK_W / 2.0, top),
            egui::pos2(centre, top + MARK_H),
        ],
        accent,
        egui::Stroke::NONE,
    ));
}

/// What is sounding, over the key sounding it.
///
/// ⚠️ Painted on a foreground layer above the keyboard, where the design puts it: the
/// key map's own rect ends at the keys, and a chip inside it would cover the root marks.
fn audition_chip(ui: &egui::Ui, rect: egui::Rect, span: Span, note: u8) {
    let painter = ui.ctx().layer_painter(egui::LayerId::new(
        egui::Order::Foreground,
        ui.id().with("audition"),
    ));
    let centre = key_rect(rect, span, note).center().x;
    chip(
        &painter,
        egui::pos2(centre, rect.top() - CHIP_TEXT - 4.0),
        &format!("{} · vel {AUDITION_VELOCITY}", note::name(note)),
        app::good(ui.visuals()),
        ui.visuals().text_color(),
    );
}

// ---- the zone lane ------------------------------------------------------------------

/// One zone as the lane draws it: the keys it answers, and what it is called.
pub struct Band {
    pub low: u8,
    pub top: u8,
    pub name: String,
    pub range_text: String,
    pub hint: String,
}

/// Which ends of a band can be dragged. `TopOnly` is the v2 table, where a zone's low is
/// derived from the zone below rather than stored.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Edges {
    TopOnly,
    Both,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Edge {
    Low,
    Top,
}

/// What the lane was asked for this frame.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum BandAct {
    /// A band was clicked.
    Pick(usize),
    /// A handle moved. `bounds` is every band's `(low, top)` after the clamp, in the
    /// order the bands were given — with `Edges::TopOnly`, the band above has moved too.
    Drag {
        zone: usize,
        edge: Edge,
        bounds: Vec<(u8, u8)>,
    },
}

const BANDS_H: f32 = 19.0;
const BAND_H: f32 = 17.0;
/// The room a band keeps at each end, so its text clears the handles.
const BAND_PAD: f32 = 14.0;
const BAND_GAP: f32 = 6.0;
const BAND_NAME: f32 = 10.0;
const HANDLE_W: f32 = 10.0;
const PILL: egui::Vec2 = egui::vec2(3.0, 11.0);
/// How much of the hatch colour a gap keeps.
const HATCH_ALPHA: f32 = 0.55;

/// The stretches of `span` no band covers, low to high.
fn gaps(bounds: &[(u8, u8)], span: Span) -> Vec<(u8, u8)> {
    let (low, high) = span.ends();
    let mut sorted = bounds.to_vec();
    sorted.sort_by_key(|(low, _)| *low);
    let mut out = Vec::new();
    let mut at = low;
    for (band_low, band_top) in sorted {
        if band_low > at {
            out.push((at, band_low.saturating_sub(1)));
        }
        at = at.max(band_top.saturating_add(1));
    }
    if at <= high {
        out.push((at, high));
    }
    out
}

/// Where a dragged handle lands, and what follows it.
///
/// A top stops one key short of the band above's low and never reaches its own low; a
/// low stops one key above the band below's top and never reaches its own top. With
/// [`Edges::TopOnly`] the band above's low follows the top it is derived from, so bands
/// may gap but never overlap.
fn clamped(
    bounds: &[(u8, u8)],
    span: Span,
    zone: usize,
    edge: Edge,
    note: u8,
    edges: Edges,
) -> Vec<(u8, u8)> {
    let mut next = bounds.to_vec();
    let (span_low, span_high) = span.ends();
    let Some(&(low, top)) = next.get(zone) else {
        return next;
    };
    match edge {
        Edge::Top => {
            let ceiling = match zone.checked_sub(1).map(|above| next[above].0) {
                Some(above_low) => above_low.saturating_sub(1),
                None => span_high,
            };
            let shown = note.min(ceiling).max(low.saturating_add(1)).min(span_high);
            next[zone].1 = shown;
            if let (Edges::TopOnly, Some(above)) = (edges, zone.checked_sub(1)) {
                next[above].0 = shown.saturating_add(1);
            }
        }
        Edge::Low => {
            let floor = match next.get(zone + 1) {
                Some(&(_, below_top)) => below_top.saturating_add(1),
                None => span_low,
            };
            next[zone].0 = note.max(floor).min(top.saturating_sub(1)).max(span_low);
        }
    }
    next
}

/// The zone lane: one band per zone, the keys between them hatched.
///
/// `selected` is the open row, `lit` the zone answering the auditioned key. The bands are
/// drawn in the order they are given, which is the file's.
pub fn bands(
    ui: &mut egui::Ui,
    span: Span,
    zones: &[Band],
    selected: Option<usize>,
    lit: Option<usize>,
    edges: Edges,
) -> Option<BandAct> {
    let width = ui.available_width().max(1.0);
    let (rect, response) = ui.allocate_exact_size(egui::vec2(width, BANDS_H), egui::Sense::click());
    let lane = egui::Rect::from_min_size(
        egui::pos2(rect.left(), rect.top() + 1.0),
        egui::vec2(rect.width(), BAND_H),
    );
    let visuals = ui.visuals().clone();
    let painter = ui.painter().clone();
    let bounds: Vec<(u8, u8)> = zones.iter().map(|band| (band.low, band.top)).collect();

    let mut hint = None;
    for (from, to) in gaps(&bounds, span) {
        let gap = egui::Rect::from_min_max(
            egui::pos2(span.x_of(rect, from), lane.top()),
            egui::pos2(span.x_after(rect, to), lane.bottom()),
        );
        let bad = app::bad(&visuals);
        hatch(&painter, gap, bad, HATCH_ALPHA);
        painter.rect_stroke(
            gap,
            RADIUS,
            egui::Stroke::new(1.0_f32, bad),
            egui::StrokeKind::Inside,
        );
        if response.hover_pos().is_some_and(|at| gap.contains(at)) {
            hint = Some(format!(
                "{}–{} answers nothing — no zone covers those keys",
                note::name(from),
                note::name(to)
            ));
        }
    }

    for (index, band) in zones.iter().enumerate() {
        let over = egui::Rect::from_min_max(
            egui::pos2(span.x_of(rect, band.low), lane.top()),
            egui::pos2(span.x_after(rect, band.top), lane.bottom()),
        );
        let worn = look(&visuals, selected == Some(index), lit == Some(index));
        painter.rect_filled(over, RADIUS, worn.fill);
        painter.rect_stroke(
            over,
            RADIUS,
            egui::Stroke::new(1.0_f32, worn.stroke),
            egui::StrokeKind::Inside,
        );
        let room = over.width() - BAND_PAD * 2.0;
        if room > 0.0 {
            let range = clipped(
                &painter,
                &band.range_text,
                egui::FontId::monospace(CHIP_TEXT),
                worn.ink.gamma_multiply(visuals.weak_text_alpha),
                room,
            );
            let name = clipped(
                &painter,
                &band.name,
                egui::FontId::new(BAND_NAME, app::bold()),
                worn.ink,
                room - range.size().x - BAND_GAP,
            );
            let left = over.left() + BAND_PAD;
            let after = left + name.size().x + BAND_GAP;
            write(&painter, left, over.center().y, name);
            write(&painter, after, over.center().y, range);
        }
        if response.hover_pos().is_some_and(|at| over.contains(at)) {
            hint = Some(band.hint.clone());
        }
    }

    let mut act = None;
    let mut grabs = Vec::new();
    for (index, band) in zones.iter().enumerate() {
        let wanted = match edges {
            Edges::TopOnly => [Some(Edge::Top), None],
            Edges::Both => [Some(Edge::Top), Some(Edge::Low)],
        };
        for edge in wanted.into_iter().flatten() {
            let grab = handle_rect(rect, lane, span, band, edge);
            grabs.push(grab);
            let at = handle(
                ui,
                &painter,
                grab,
                (index, edge),
                &drag_hint(band, edge, edges, index),
                &visuals,
            );
            if let Some(at) = at {
                let next = clamped(&bounds, span, index, edge, span.note_at(rect, at.x), edges);
                let shown = match edge {
                    Edge::Top => next[index].1,
                    Edge::Low => next[index].0,
                };
                chip(
                    &painter,
                    egui::pos2(grab.center().x, rect.top() - 1.0),
                    &note::name(shown),
                    app::accent(&visuals),
                    app::accent(&visuals),
                );
                if next != bounds {
                    act = Some(BandAct::Drag {
                        zone: index,
                        edge,
                        bounds: next,
                    });
                }
            }
        }
    }

    if let Some(hint) = hint {
        response.clone().on_hover_text(hint);
    }
    if act.is_some() {
        return act;
    }
    // A handle sits over the band it belongs to, and is the thing the click was for.
    let picked = response
        .clicked()
        .then(|| response.interact_pointer_pos())
        .flatten()
        .filter(|at| !grabs.iter().any(|grab| grab.contains(*at)))?;
    zones
        .iter()
        .position(|band| {
            picked.x >= span.x_of(rect, band.low) && picked.x < span.x_after(rect, band.top)
        })
        .map(BandAct::Pick)
}

/// The three states a band wears.
struct Look {
    fill: egui::Color32,
    stroke: egui::Color32,
    ink: egui::Color32,
}

fn look(visuals: &egui::Visuals, selected: bool, lit: bool) -> Look {
    match (lit, selected) {
        (true, _) => Look {
            fill: visuals.selection.bg_fill,
            stroke: app::good(visuals),
            ink: visuals.text_color(),
        },
        (false, true) => Look {
            fill: visuals.selection.bg_fill,
            stroke: app::accent(visuals),
            ink: visuals.selection.stroke.color,
        },
        (false, false) => Look {
            fill: visuals.faint_bg_color,
            stroke: visuals.widgets.noninteractive.bg_stroke.color,
            ink: visuals.weak_text_color(),
        },
    }
}

/// The grab zone for one end of a band, inside the band it belongs to.
fn handle_rect(
    rect: egui::Rect,
    lane: egui::Rect,
    span: Span,
    band: &Band,
    edge: Edge,
) -> egui::Rect {
    let left = match edge {
        Edge::Top => span.x_after(rect, band.top) - HANDLE_W - 1.0,
        Edge::Low => span.x_of(rect, band.low) + 1.0,
    };
    egui::Rect::from_min_size(
        egui::pos2(left, lane.top()),
        egui::vec2(HANDLE_W, lane.height()),
    )
}

/// What a handle says it does. With [`Edges::TopOnly`] a top carries the low it derives.
fn drag_hint(band: &Band, edge: Edge, edges: Edges, index: usize) -> String {
    let (what, note) = match edge {
        Edge::Top => ("top", band.top),
        Edge::Low => ("low", band.low),
    };
    let follows = match (edges, edge, index) {
        (Edges::TopOnly, Edge::Top, 1..) => ", and the low of the band above it",
        _ => "",
    };
    format!(
        "Drag to move {}'s {what} note{follows} — now {}",
        band.name,
        note::name(note)
    )
}

/// One end of a band, as a pill to grab. Answers with the pointer while it is dragged.
fn handle(
    ui: &egui::Ui,
    painter: &egui::Painter,
    grab: egui::Rect,
    which: (usize, Edge),
    hint: &str,
    visuals: &egui::Visuals,
) -> Option<egui::Pos2> {
    let response = ui.interact(
        grab,
        ui.id().with(("band_edge", which.0, which.1 == Edge::Top)),
        egui::Sense::click_and_drag(),
    );
    let dragging = response.dragged();
    if dragging {
        painter.rect_filled(grab, 0.0, visuals.widgets.active.weak_bg_fill);
    }
    let ink = match dragging {
        true => app::accent(visuals),
        false => app::caption(visuals),
    };
    painter.rect_filled(egui::Rect::from_center_size(grab.center(), PILL), 1.0, ink);
    response.clone().on_hover_text(hint);
    match dragging {
        true => response.interact_pointer_pos(),
        false => None,
    }
}

// ---- the size lane ------------------------------------------------------------------

/// One root as the size lane draws it: the keys it answers, and what it costs.
pub struct SizeCell {
    pub low: u8,
    pub top: u8,
    pub name: String,
    /// Megabytes this root keeps, and what it holds before a trim.
    pub kept: f32,
    pub original: f32,
    /// Whether a range trim still reaches this root.
    pub in_range: bool,
    pub hint: String,
}

const CELLS_H: f32 = 46.0;
const BAR_AREA: f32 = 32.0;
const BAR_REACH: f32 = 30.0;
const CELL_ROW: f32 = 14.0;
const CELL_NAME: f32 = 9.5;
const CELL_MB: f32 = 9.0;
/// How many keys a cell needs before its size is worth printing in it.
const MB_KEYS: usize = 4;
/// How much is kept before the reading counts as trimmed.
const TRIMMED: f32 = 0.05;

/// The size lane: one cell per root, its kept megabytes as a bar inside the ghost of
/// what it holds untrimmed. Returns the cell that was clicked.
pub fn size_cells(
    ui: &mut egui::Ui,
    span: Span,
    cells: &[SizeCell],
    selected: Option<usize>,
    lit: Option<usize>,
) -> Option<usize> {
    let width = ui.available_width().max(1.0);
    let (rect, response) = ui.allocate_exact_size(egui::vec2(width, CELLS_H), egui::Sense::click());
    let visuals = ui.visuals().clone();
    let painter = ui.painter().clone();
    let most = cells
        .iter()
        .map(|cell| cell.original)
        .fold(0.0_f32, f32::max);
    let pointer = response.hover_pos();

    let mut hint = None;
    for (index, cell) in cells.iter().enumerate() {
        let picked = selected == Some(index);
        let column = egui::Rect::from_min_max(
            egui::pos2(span.x_of(rect, cell.low), rect.top()),
            egui::pos2(span.x_after(rect, cell.top), rect.bottom()),
        );
        let hovered = pointer.is_some_and(|at| column.contains(at));
        let ground = match (picked, hovered) {
            (true, _) => Some(visuals.selection.bg_fill),
            (false, true) => Some(visuals.widgets.hovered.weak_bg_fill),
            (false, false) => None,
        };
        if let Some(fill) = ground {
            painter.rect_filled(column, 0.0, fill);
        }
        if hovered {
            hint = Some(cell.hint.clone());
        }

        let floor = rect.top() + BAR_AREA;
        let bars = column.shrink2(egui::vec2(2.0, 0.0));
        let height = |mb: f32| match most > 0.0 {
            true => (mb / most * BAR_REACH).round(),
            false => 0.0,
        };
        dashed(
            &painter,
            &[
                egui::pos2(bars.left(), floor),
                egui::pos2(bars.left(), floor - height(cell.original)),
                egui::pos2(bars.right(), floor - height(cell.original)),
                egui::pos2(bars.right(), floor),
            ],
            egui::Stroke::new(1.0_f32, app::unlit(&visuals)),
        );
        let kept = match cell.kept > 0.0 {
            true => height(cell.kept).max(2.0),
            false => 0.0,
        };
        let fill = match (cell.in_range, lit == Some(index), picked) {
            (false, _, _) => egui::Color32::TRANSPARENT,
            (true, true, _) => app::good(&visuals),
            (true, false, true) => app::accent(&visuals),
            (true, false, false) => visuals.weak_text_color(),
        };
        painter.rect_filled(
            egui::Rect::from_min_max(
                egui::pos2(bars.left(), floor - kept),
                egui::pos2(bars.right(), floor),
            ),
            RADIUS,
            fill,
        );

        let middle = rect.bottom() - CELL_ROW / 2.0;
        let name_ink = match (picked, cell.in_range) {
            (true, _) => visuals.selection.stroke.color,
            (false, true) => visuals.weak_text_color(),
            (false, false) => app::caption(&visuals),
        };
        let mb = (cell_keys(cell) >= MB_KEYS).then(|| {
            let ink = match (cell.kept < cell.original - TRIMMED, picked) {
                (true, _) => app::warn(&visuals),
                (false, true) => visuals.selection.stroke.color,
                (false, false) => app::caption(&visuals),
            };
            let text = format!("{:.1}", cell.kept);
            painter.layout_no_wrap(text, egui::FontId::monospace(CELL_MB), ink)
        });
        let taken = mb.as_ref().map_or(0.0, |galley| galley.size().x + 2.0);
        let name = clipped(
            &painter,
            &cell.name,
            egui::FontId::monospace(CELL_NAME),
            name_ink,
            bars.width() - taken,
        );
        write(&painter, bars.left(), middle, name);
        if let Some(galley) = mb {
            write(&painter, bars.right() - galley.size().x, middle, galley);
        }
    }

    if let Some(hint) = hint {
        response.clone().on_hover_text(hint);
    }
    let picked = response
        .clicked()
        .then(|| response.interact_pointer_pos())
        .flatten()?;
    cells.iter().position(|cell| {
        picked.x >= span.x_of(rect, cell.low) && picked.x < span.x_after(rect, cell.top)
    })
}

/// How many keys a cell spans.
fn cell_keys(cell: &SizeCell) -> usize {
    (cell.top.max(cell.low) - cell.low) as usize + 1
}

// ---- the per-key lane ---------------------------------------------------------------

/// What a per-key value means, and how it reads.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Scale {
    /// Decibels at full deflection.
    Db(f32),
    /// Cents at full deflection.
    Cents(i32),
}

impl Scale {
    /// The two ends of the caller's axis column. The unit belongs in the row's summary,
    /// not beside every number.
    pub fn axis_labels(&self) -> (String, String) {
        match self {
            Scale::Db(full) => (format!("+{full:.1}"), format!("−{full:.1}")),
            Scale::Cents(full) => (format!("+{full}"), format!("−{full}")),
        }
    }

    /// One value as it reads: `+1.5 dB`, `-12 c`.
    pub fn format(&self, value: f32) -> String {
        match self {
            Scale::Db(full) => {
                let tenths = (value * full * 10.0).round() as i32;
                let sign = match tenths < 0 {
                    true => "-",
                    false => "+",
                };
                format!("{sign}{}.{} dB", tenths.abs() / 10, tenths.abs() % 10)
            }
            Scale::Cents(full) => format!("{:+} c", (value * *full as f32).round() as i32),
        }
    }
}

const LANE_H: f32 = 38.0;
/// Where zero sits inside the lane.
const LANE_ZERO: f32 = 18.0;
/// How far a full value reaches from zero.
const LANE_REACH: f32 = 16.0;
/// Below this a value reads as nothing rather than as a small edit.
const LANE_QUIET: f32 = 0.08;
/// The steps a painted value snaps to across the whole lane.
const SNAP: f32 = 20.0;
/// The deadband around zero a painted value falls into.
const DEADBAND: f32 = 0.06;

/// One value per key, painted as bars either side of zero. A drag across the lane draws
/// over them: the keys it touched and what it set them to come back, in the order they
/// were painted.
pub fn lane(
    ui: &mut egui::Ui,
    span: Span,
    values: &[f32],
    painted: &[bool],
    scale: Scale,
) -> Vec<(u8, f32)> {
    let width = ui.available_width().max(1.0);
    let (rect, response) =
        ui.allocate_exact_size(egui::vec2(width, LANE_H), egui::Sense::click_and_drag());
    let visuals = ui.visuals().clone();
    let painter = ui.painter().clone();
    painter.rect_filled(rect, RADIUS, visuals.extreme_bg_color);
    painter.rect_stroke(
        rect,
        RADIUS,
        egui::Stroke::new(1.0_f32, visuals.widgets.noninteractive.bg_stroke.color),
        egui::StrokeKind::Inside,
    );
    let inner = rect.shrink(1.0);
    painter.hline(
        rect.x_range(),
        rect.top() + LANE_ZERO + 0.5,
        egui::Stroke::new(1.0_f32, app::unlit(&visuals)),
    );

    let (low, _) = span.ends();
    for (index, value) in values.iter().take(span.keys()).enumerate() {
        let note = low + index as u8;
        let value = value.clamp(-1.0, 1.0);
        let edited = painted.get(index).copied().unwrap_or(false);
        let tall = (value.abs() * LANE_REACH).round().max(1.0);
        let top = match value >= 0.0 {
            true => inner.top() + LANE_ZERO - 1.0 - tall,
            false => inner.top() + LANE_ZERO,
        };
        let left = span.x_of(inner, note);
        let bar = egui::Rect::from_min_size(
            egui::pos2(left, top),
            egui::vec2((span.x_after(inner, note) - left - 1.0).max(1.0), tall),
        );
        let ink = match (edited, value.abs() < LANE_QUIET) {
            (true, _) => app::accent(&visuals),
            (false, true) => app::unlit(&visuals),
            (false, false) => visuals.weak_text_color(),
        };
        painter.rect_filled(bar, 0.0, ink);
    }

    if let Some(at) = response.hover_pos() {
        let note = span.note_at(rect, at.x);
        let index = (note - low) as usize;
        let value = values.get(index).copied().unwrap_or(0.0);
        let edited = match painted.get(index).copied().unwrap_or(false) {
            true => " · edited",
            false => "",
        };
        response.clone().on_hover_text(format!(
            "{}  {}{edited}",
            note::name(note),
            scale.format(value)
        ));
    }

    match response.is_pointer_button_down_on() {
        true => strokes(ui, rect, span),
        false => Vec::new(),
    }
}

/// A value as the lane stores it: snapped to a twentieth, and to zero near the middle.
fn snap(value: f32) -> f32 {
    match value.abs() < DEADBAND {
        true => 0.0,
        false => (value * SNAP).round() / SNAP,
    }
}

/// Every key this frame's pointer positions painted, in the order they arrived. A drag
/// off the end of the lane keeps painting the key it left by, as the design has it.
fn strokes(ui: &egui::Ui, rect: egui::Rect, span: Span) -> Vec<(u8, f32)> {
    let mut out: Vec<(u8, f32)> = Vec::new();
    ui.input(|input| {
        for event in &input.events {
            let at = match event {
                egui::Event::PointerMoved(at) => *at,
                egui::Event::PointerButton {
                    pos,
                    button: egui::PointerButton::Primary,
                    pressed: true,
                    ..
                } => *pos,
                _ => continue,
            };
            let note = span.note_at(rect, at.x);
            let reach = (1.0 - 2.0 * (at.y - rect.top()) / rect.height()).clamp(-1.0, 1.0);
            let stroke = (note, snap(reach));
            if out.last() != Some(&stroke) {
                out.push(stroke);
            }
        }
    });
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// What a sample instrument covers, and what a piano library covers.
    const NSMP: Span = Span { low: 24, high: 96 };
    const NPNO: Span = Span { low: 21, high: 108 };

    const SCREEN: egui::Vec2 = egui::vec2(600.0, 240.0);

    /// A context dressed as the app dresses it: without the bold face bound, laying out
    /// a band's name panics.
    fn dressed() -> egui::Context {
        let ctx = egui::Context::default();
        ctx.set_fonts(crate::app::fonts());
        ctx.set_visuals(egui::Visuals::dark());
        ctx
    }

    /// One frame of `body`, answering what it painted and what it gave back. The rect a
    /// lane claims comes back with it, so the next frame can point at a key of it.
    fn frame<R>(
        ctx: &egui::Context,
        events: Vec<egui::Event>,
        height: f32,
        mut body: impl FnMut(&mut egui::Ui) -> R,
    ) -> (egui::FullOutput, egui::Rect, R) {
        let input = egui::RawInput {
            events,
            screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, SCREEN)),
            ..Default::default()
        };
        let mut answer = None;
        let output = ctx.run(input, |ctx| {
            ctx.style_mut(crate::app::metrics);
            egui::CentralPanel::default().show(ctx, |ui| {
                let at = egui::Rect::from_min_size(
                    ui.next_widget_position(),
                    egui::vec2(ui.available_width(), height),
                );
                answer = Some((at, body(ui)));
            });
        });
        let (at, answer) = answer.expect("the panel drew");
        (output, at, answer)
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

    /// Every word a frame painted, with the ink it was painted in.
    fn words(output: &egui::FullOutput) -> Vec<(String, egui::Color32)> {
        fn walk(shape: &egui::Shape, into: &mut Vec<(String, egui::Color32)>) {
            match shape {
                egui::Shape::Text(text) => {
                    let ink = text.override_text_color.or_else(|| {
                        text.galley
                            .job
                            .sections
                            .first()
                            .map(|section| section.format.color)
                    });
                    into.push((
                        text.galley.text().to_string(),
                        ink.unwrap_or(text.fallback_color),
                    ));
                }
                egui::Shape::Vec(shapes) => shapes.iter().for_each(|shape| walk(shape, into)),
                _ => {}
            }
        }
        let mut said = Vec::new();
        for clipped in &output.shapes {
            walk(&clipped.shape, &mut said);
        }
        said
    }

    fn press(at: egui::Pos2) -> Vec<egui::Event> {
        vec![
            egui::Event::PointerMoved(at),
            egui::Event::PointerButton {
                pos: at,
                button: egui::PointerButton::Primary,
                pressed: true,
                modifiers: egui::Modifiers::NONE,
            },
            egui::Event::PointerButton {
                pos: at,
                button: egui::PointerButton::Primary,
                pressed: false,
                modifiers: egui::Modifiers::NONE,
            },
        ]
    }

    // ---- geometry -------------------------------------------------------------------

    /// A lane splits its width by key count, and the key under a point is the key whose
    /// cell that point is in — which is what every hit test in the map rests on.
    #[test]
    fn a_lane_gives_each_key_a_cell_and_reads_it_back() {
        assert_eq!(NSMP.keys(), 73);
        assert_eq!(NPNO.keys(), 88, "a full piano");

        let rect = egui::Rect::from_min_size(egui::pos2(7.0, 3.0), egui::vec2(601.0, 19.0));
        for span in [NSMP, NPNO] {
            let unit = rect.width() / span.keys() as f32;
            for note in span.low..=span.high {
                let left = span.x_of(rect, note);
                assert!(
                    (span.x_after(rect, note) - left - unit).abs() < 0.001,
                    "{note} is not one key wide",
                );
                assert_eq!(span.note_at(rect, left + unit / 2.0), note);
            }
            assert!((span.x_of(rect, span.low) - rect.left()).abs() < 0.001);
            assert!((span.x_after(rect, span.high) - rect.right()).abs() < 0.001);
        }
    }

    /// A pointer past either end belongs to the key at that end rather than to no key:
    /// a drag off the lane keeps painting the key it left by.
    #[test]
    fn a_point_outside_the_span_clamps_to_the_end_it_is_past() {
        let rect = egui::Rect::from_min_size(egui::pos2(7.0, 3.0), egui::vec2(601.0, 19.0));
        for span in [NSMP, NPNO] {
            assert_eq!(span.note_at(rect, rect.left() - 500.0), span.low);
            assert_eq!(span.note_at(rect, rect.right() + 500.0), span.high);
            assert_eq!(span.x_of(rect, 0), span.x_of(rect, span.low));
            assert_eq!(span.x_of(rect, 127), span.x_of(rect, span.high));
            assert!(span.contains(span.low) && span.contains(span.high));
            assert!(!span.contains(span.low - 1) && !span.contains(span.high + 1));
        }
    }

    #[test]
    fn five_of_every_twelve_keys_are_black() {
        let pattern: Vec<bool> = (0..12u8).map(is_black).collect();
        assert_eq!(
            pattern,
            [false, true, false, true, false, false, true, false, true, false, true, false]
        );
        assert!(!is_black(60), "middle C is white");
        assert_eq!((0..12u8).filter(|note| is_black(*note)).count(), 5);
    }

    #[test]
    fn a_played_key_reads_as_its_distance_from_the_root() {
        assert_eq!(shifted(63, 60), "shifted +3 st");
        assert_eq!(shifted(57, 60), "shifted -3 st");
        assert_eq!(shifted(60, 60), "shifted +0 st");
    }

    #[test]
    fn an_audition_is_spent_once_its_hold_is_up() {
        let struck = Audition::new(60, 10.0);
        assert!(struck.live(10.0));
        assert!(struck.live(10.0 + Audition::HOLD / 2.0));
        assert!(!struck.live(11.0 + Audition::HOLD));
    }

    // ---- the keyboard ---------------------------------------------------------------

    /// The two spans the editors draw are a six-octave sample map and a full piano, and
    /// both are laid out by their white keys.
    #[test]
    fn the_keyboard_holds_the_white_and_black_keys_of_its_span() {
        assert_eq!(
            (white_count(NSMP), NSMP.keys() - white_count(NSMP)),
            (43, 30)
        );
        assert_eq!(
            (white_count(NPNO), NPNO.keys() - white_count(NPNO)),
            (52, 36)
        );
        assert_eq!(whites_before(NSMP, NSMP.low), 0);
        assert_eq!(whites_before(NSMP, 36), 7, "one octave of white keys");
    }

    /// The only words on the keyboard are the octaves, and there is one per C.
    #[test]
    fn the_keyboard_labels_every_c_and_nothing_else() {
        let ctx = dressed();
        let (output, _, _) = frame(&ctx, Vec::new(), KEYBOARD_H, |ui| {
            keyboard(ui, NSMP, None, &[])
        });
        let said: Vec<String> = words(&output).into_iter().map(|(text, _)| text).collect();
        assert_eq!(said, ["C1", "C2", "C3", "C4", "C5", "C6", "C7"]);
    }

    /// Clicking a key is how the map auditions, and a black key drawn over two whites is
    /// the key that answers a click on it.
    #[test]
    fn a_click_lands_on_the_key_under_it_black_keys_first() {
        let ctx = dressed();
        let (_, rect, _) = frame(&ctx, Vec::new(), KEYBOARD_H, |ui| {
            keyboard(ui, NSMP, None, &[])
        });
        for note in [60u8, 61, NSMP.low, NSMP.high] {
            let at = key_rect(rect, NSMP, note).center();
            let (_, _, clicked) = frame(&ctx, press(at), KEYBOARD_H, |ui| {
                keyboard(ui, NSMP, None, &[])
            });
            assert_eq!(clicked, Some(note), "at {at:?}");
        }
    }

    /// A lit key is the one thing on the keyboard that is not its own colour, and the two
    /// kinds of key light differently: a white one selects, a black one glows.
    #[test]
    fn the_auditioned_key_lights_and_the_others_keep_their_own_colour() {
        let ctx = dressed();
        let visuals = ctx.style().visuals.clone();
        let (_, rect, _) = frame(&ctx, Vec::new(), KEYBOARD_H, |ui| {
            keyboard(ui, NSMP, None, &[])
        });
        for (note, lit) in [
            (60u8, visuals.selection.bg_fill),
            (61, crate::app::accent(&visuals)),
        ] {
            let (output, _, _) = frame(&ctx, Vec::new(), KEYBOARD_H, |ui| {
                keyboard(ui, NSMP, Some(note), &[])
            });
            assert_eq!(fills(&output, key_rect(rect, NSMP, note)), vec![lit]);
            let quiet = key_rect(rect, NSMP, note + 12);
            let own = match is_black(note) {
                true => crate::app::stop_black(&visuals),
                false => crate::app::stop_white(&visuals),
            };
            assert_eq!(fills(&output, quiet), vec![own], "an octave up is unlit");
        }
    }

    /// A root marker names the key it points at, and an unlabelled one is the triangle
    /// alone — the piano lane marks 15 roots and cannot carry 15 names.
    #[test]
    fn a_root_marker_carries_its_name_only_when_it_was_given_one() {
        let ctx = dressed();
        let marks = [
            Mark {
                note: 72,
                label: Some("C5".to_string()),
            },
            Mark {
                note: 48,
                label: None,
            },
        ];
        let (output, _, _) = frame(&ctx, Vec::new(), KEYBOARD_H, |ui| {
            keyboard(ui, NSMP, None, &marks)
        });
        let said: Vec<String> = words(&output).into_iter().map(|(text, _)| text).collect();
        assert_eq!(
            said.iter().filter(|text| *text == "C5").count(),
            2,
            "the octave label and the marker's chip: {said:?}",
        );
    }

    // ---- the zone lane --------------------------------------------------------------

    fn bands_of(bounds: &[(u8, u8)]) -> Vec<Band> {
        bounds
            .iter()
            .enumerate()
            .map(|(index, (low, top))| Band {
                low: *low,
                top: *top,
                name: format!("Zone {}", index + 1),
                range_text: format!("{}–{}", note::name(*low), note::name(*top)),
                hint: format!("Zone {}", index + 1),
            })
            .collect()
    }

    /// The keys no zone answers are the ones the map hatches, and a map that covers
    /// everything hatches nothing.
    #[test]
    fn the_gaps_are_the_keys_no_zone_answers() {
        assert_eq!(gaps(&[(61, 96), (41, 60), (24, 38)], NSMP), [(39, 40)]);
        assert!(gaps(&[(61, 96), (41, 60), (24, 40)], NSMP).is_empty());
        assert_eq!(gaps(&[(30, 90)], NSMP), [(24, 29), (91, 96)]);
        assert_eq!(gaps(&[], NSMP), [(24, 96)]);
        // A zone reaching past the span leaves no gap outside it.
        assert!(gaps(&[(0, 127)], NSMP).is_empty());
    }

    /// v2 stores no low note: a zone's low is the key above the zone below's top, so
    /// moving one top moves the neighbour's low with it and the two never overlap.
    #[test]
    fn a_derived_low_follows_the_top_it_is_derived_from() {
        let bounds = [(61, 96), (41, 60), (24, 40)];
        let moved = clamped(&bounds, NSMP, 1, Edge::Top, 55, Edges::TopOnly);
        assert_eq!(moved[1], (41, 55));
        assert_eq!(moved[0], (56, 96), "the zone above starts a key higher");
        assert_eq!(moved[2], bounds[2], "and nothing else moves");

        // With both edges stored, the neighbour is left where it was and a gap opens.
        let apart = clamped(&bounds, NSMP, 1, Edge::Top, 55, Edges::Both);
        assert_eq!((apart[1], apart[0]), ((41, 55), (61, 96)));
    }

    /// The clamps are what keeps zones from overlapping or turning inside out.
    #[test]
    fn a_handle_stops_short_of_its_neighbour_and_of_its_own_other_end() {
        let bounds = [(61, 96), (41, 60), (24, 40)];
        let top = |note, edges| clamped(&bounds, NSMP, 1, Edge::Top, note, edges)[1].1;
        assert_eq!(
            top(70, Edges::Both),
            60,
            "a key short of the zone above's low"
        );
        assert_eq!(top(20, Edges::Both), 42, "a key above its own low");
        let low = |note| clamped(&bounds, NSMP, 1, Edge::Low, note, Edges::Both)[1].0;
        assert_eq!(low(30), 41, "a key above the zone below's top");
        assert_eq!(low(90), 59, "a key below its own top");

        // The ends of the span stand in for the neighbours the outermost zones lack.
        assert_eq!(
            clamped(&bounds, NSMP, 0, Edge::Top, 120, Edges::Both)[0].1,
            NSMP.high
        );
        assert_eq!(
            clamped(&bounds, NSMP, 2, Edge::Low, 0, Edges::Both)[2].0,
            NSMP.low
        );
    }

    /// Clicking a band opens its row; the handles at either end are not that click.
    #[test]
    fn a_click_on_a_band_picks_it() {
        let ctx = dressed();
        let zones = bands_of(&[(61, 96), (41, 60), (24, 40)]);
        let lane = |ui: &mut egui::Ui| bands(ui, NSMP, &zones, None, None, Edges::TopOnly);
        let (_, rect, _) = frame(&ctx, Vec::new(), BANDS_H, lane);

        let middle = egui::pos2(
            (NSMP.x_of(rect, 41) + NSMP.x_after(rect, 60)) / 2.0,
            rect.center().y,
        );
        let (_, _, act) = frame(&ctx, press(middle), BANDS_H, lane);
        assert_eq!(act, Some(BandAct::Pick(1)));

        // The hatch over a gap answers nothing; it is not a zone to open.
        let holed = bands_of(&[(61, 96), (41, 60)]);
        let lane = |ui: &mut egui::Ui| bands(ui, NSMP, &holed, None, None, Edges::Both);
        let (_, rect, _) = frame(&ctx, Vec::new(), BANDS_H, lane);
        let over_gap = egui::pos2(NSMP.x_of(rect, 30), rect.center().y);
        let (_, _, act) = frame(&ctx, press(over_gap), BANDS_H, lane);
        assert_eq!(act, None);
    }

    // ---- the size lane --------------------------------------------------------------

    fn size_cell(low: u8, top: u8, kept: f32, original: f32) -> SizeCell {
        SizeCell {
            low,
            top,
            name: note::name(low),
            kept,
            original,
            in_range: true,
            hint: String::new(),
        }
    }

    /// A cell narrower than four keys has no room for a size, and printing one there
    /// leaves two numbers overlapping instead of a readable lane.
    #[test]
    fn a_cells_size_is_printed_only_where_it_fits() {
        let ctx = dressed();
        let span = Span { low: 48, high: 95 };
        for (keys, shown) in [(3u8, false), (4, true)] {
            let cells = [size_cell(60, 60 + keys - 1, 1.0, 1.0)];
            let (output, _, _) = frame(&ctx, Vec::new(), CELLS_H, |ui| {
                size_cells(ui, span, &cells, None, None)
            });
            let said: Vec<String> = words(&output).into_iter().map(|(text, _)| text).collect();
            assert_eq!(
                said.contains(&"1.0".to_string()),
                shown,
                "{keys} keys wide: {said:?}",
            );
            assert!(said.contains(&"C4".to_string()), "the root is always named");
        }
    }

    /// A root that lost samples reads in warn ink, which is the whole point of the lane:
    /// what the trim has already taken is visible without opening a row.
    #[test]
    fn a_trimmed_root_prints_its_size_in_warn_ink() {
        let ctx = dressed();
        let visuals = ctx.style().visuals.clone();
        let span = Span { low: 48, high: 95 };
        let cells = [size_cell(60, 71, 0.5, 2.0), size_cell(72, 83, 2.0, 2.0)];
        let (output, _, _) = frame(&ctx, Vec::new(), CELLS_H, |ui| {
            size_cells(ui, span, &cells, None, None)
        });
        let said = words(&output);
        let ink = |text: &str| {
            said.iter()
                .find(|(painted, _)| painted == text)
                .map(|(_, ink)| *ink)
        };
        assert_eq!(ink("0.5"), Some(crate::app::warn(&visuals)));
        assert_eq!(ink("2.0"), Some(crate::app::caption(&visuals)));
    }

    /// Clicking a cell opens that root's row.
    #[test]
    fn a_click_on_a_cell_picks_its_root() {
        let ctx = dressed();
        let span = Span { low: 48, high: 95 };
        let cells = [size_cell(48, 59, 1.0, 1.0), size_cell(60, 71, 1.0, 1.0)];
        let lane = |ui: &mut egui::Ui| size_cells(ui, span, &cells, None, None);
        let (_, rect, _) = frame(&ctx, Vec::new(), CELLS_H, lane);
        let at = egui::pos2(
            (span.x_of(rect, 60) + span.x_after(rect, 71)) / 2.0,
            rect.center().y,
        );
        let (_, _, picked) = frame(&ctx, press(at), CELLS_H, lane);
        assert_eq!(picked, Some(1));
    }

    // ---- the per-key lane -----------------------------------------------------------

    #[test]
    fn a_painted_value_snaps_to_a_twentieth_and_to_zero_near_the_middle() {
        assert_eq!(snap(0.03), 0.0);
        assert_eq!(snap(-0.03), 0.0);
        assert_eq!(snap(0.47), 0.45);
        assert_eq!(snap(-0.47), -0.45);
        assert_eq!(snap(0.98), 1.0);
        assert_eq!(snap(0.06), 0.05, "the deadband is below 0.06, not at it");
    }

    #[test]
    fn a_scale_reads_its_axis_and_its_values_in_its_own_unit() {
        assert_eq!(
            Scale::Db(3.0).axis_labels(),
            ("+3.0".to_string(), "−3.0".to_string())
        );
        assert_eq!(
            Scale::Cents(25).axis_labels(),
            ("+25".to_string(), "−25".to_string())
        );
        assert_eq!(Scale::Db(3.0).format(0.5), "+1.5 dB");
        assert_eq!(Scale::Db(3.0).format(-0.5), "-1.5 dB");
        assert_eq!(Scale::Db(3.0).format(0.0), "+0.0 dB");
        assert_eq!(Scale::Cents(25).format(-0.48), "-12 c");
        assert_eq!(Scale::Cents(25).format(1.0), "+25 c");
        // A value that rounds to nothing reads as nothing, never as a signed zero.
        assert_eq!(Scale::Cents(25).format(-0.001), "+0 c");
    }

    /// Dragging across the lane is how a per-key table is filled in: every key the
    /// pointer crossed this frame comes back with the value it was drawn at.
    #[test]
    fn a_drag_across_the_lane_paints_the_keys_it_crossed() {
        let ctx = dressed();
        let span = Span { low: 60, high: 62 };
        let values = [0.0_f32; 3];
        let painted = [false; 3];
        let lane_of = |ui: &mut egui::Ui| lane(ui, span, &values, &painted, Scale::Db(3.0));

        let (_, rect, drawn) = frame(&ctx, Vec::new(), LANE_H, lane_of);
        assert!(drawn.is_empty(), "nothing is painted unasked");

        let unit = rect.width() / 3.0;
        let at = |note: u8, y: f32| egui::pos2(span.x_of(rect, note) + unit / 2.0, y);
        let start = at(60, rect.center().y);
        let events = vec![
            egui::Event::PointerMoved(start),
            egui::Event::PointerButton {
                pos: start,
                button: egui::PointerButton::Primary,
                pressed: true,
                modifiers: egui::Modifiers::NONE,
            },
            egui::Event::PointerMoved(at(61, rect.top() + 9.5)),
            egui::Event::PointerMoved(at(62, rect.top() + 28.5)),
        ];
        let (_, _, drawn) = frame(&ctx, events, LANE_H, lane_of);
        assert_eq!(drawn, vec![(60, 0.0), (61, 0.5), (62, -0.5)]);
    }

    // ---- the hatch ------------------------------------------------------------------

    /// A hatch is the only thing in the map drawn out of lines, and the lines that reach
    /// past a corner must stop at the edge rather than crossing the band beside it.
    #[test]
    fn a_hatch_paints_no_further_than_the_rect_it_fills() {
        let ctx = dressed();
        let over = egui::Rect::from_min_size(egui::pos2(40.0, 30.0), egui::vec2(60.0, 17.0));
        let (output, _, _) = frame(&ctx, Vec::new(), 1.0, |ui| {
            hatch(ui.painter(), over, egui::Color32::RED, 0.55);
        });

        let mut lines = 0_usize;
        for clipped in &output.shapes {
            let egui::Shape::Vec(shapes) = &clipped.shape else {
                continue;
            };
            let segments: Vec<&egui::Shape> = shapes
                .iter()
                .filter(|shape| matches!(shape, egui::Shape::LineSegment { .. }))
                .collect();
            if segments.is_empty() {
                continue;
            }
            assert_eq!(
                clipped.clip_rect.intersect(over),
                clipped.clip_rect,
                "the hatch may paint nowhere but its own rect",
            );
            for shape in segments {
                assert!(
                    shape.visual_bounding_rect().intersects(over),
                    "a line that never reaches the rect is wasted",
                );
                lines += 1;
            }
        }
        assert!(
            lines >= (over.width() / 5.0) as usize,
            "{lines} lines across the rect",
        );
    }
}
