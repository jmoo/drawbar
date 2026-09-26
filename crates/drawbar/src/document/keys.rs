//! The key map: the keyboard, the lanes over it, and the geometry they share.
//!
//! One geometry serves both. The white keys share the width equally and the black keys
//! hang between them. A key's cell in a lane matches its cell on the keyboard, so a band
//! edge, a size bar, a per-key bar, and a root marker all sit over the key they name.
//! Black cells are narrow and overlap the white cells on either side, as on a keyboard.
//!
//! Nothing here scrolls, pins, or holds state. A lane takes the width it is given, paints
//! what it is handed, and returns what the pointer asked for.

use eframe::egui;
use nord_format::note;

use crate::app;
use crate::audio::Finger;
use crate::icon::{painted, Glyph};

/// The velocity a click on the keyboard plays at.
pub const AUDITION_VELOCITY: u8 = 90;

/// The gap between two white keys.
const WHITE_GAP: f32 = 1.0;
/// The width of a black key, and how far its left edge sits before the white boundary it
/// hangs on, both in white-key units.
const BLACK_W: f32 = 0.6;
const BLACK_OFFSET: f32 = 0.3;

pub fn is_black(note: u8) -> bool {
    matches!(note % 12, 1 | 3 | 6 | 8 | 10)
}

/// The stretch of keyboard a lane covers, inclusive at both ends.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Span {
    pub low: u8,
    pub high: u8,
}

impl Span {
    /// The ends in order, so a reversed span still reads as the range between them and
    /// does not panic in a clamp.
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

    /// How many white keys the span holds. Never zero.
    fn whites(self) -> usize {
        let (low, high) = self.ends();
        (low..=high).filter(|note| !is_black(*note)).count().max(1)
    }

    /// How wide one white key's share of the rect is, gap included.
    fn unit(self, rect: egui::Rect) -> f32 {
        rect.width() / self.whites() as f32
    }

    /// The `index`-th white key of the span.
    fn white(self, index: usize) -> u8 {
        let (low, high) = self.ends();
        (low..=high)
            .filter(|note| !is_black(*note))
            .nth(index)
            .unwrap_or(high)
    }

    /// How many white keys come before `note`.
    fn whites_before(self, note: u8) -> usize {
        let (low, _) = self.ends();
        (low..note).filter(|note| !is_black(*note)).count()
    }

    /// The left edge of `note`'s cell. A note past either end lands on that end.
    pub fn x_of(self, rect: egui::Rect, note: u8) -> f32 {
        let (low, high) = self.ends();
        let note = note.clamp(low, high);
        let unit = self.unit(rect);
        let seen = self.whites_before(note) as f32;
        match is_black(note) {
            true => rect.left() + unit * (seen - BLACK_OFFSET),
            false => rect.left() + unit * seen,
        }
    }

    /// The right edge of `note`'s cell. Two white cells are a gap apart; a black cell
    /// overlaps the whites on either side of it.
    pub fn x_after(self, rect: egui::Rect, note: u8) -> f32 {
        let (low, high) = self.ends();
        let unit = self.unit(rect);
        match is_black(note.clamp(low, high)) {
            true => self.x_of(rect, note) + unit * BLACK_W,
            false => self.x_of(rect, note) + unit - WHITE_GAP,
        }
    }

    /// The key `x` falls in, clamped to the span.
    ///
    /// Black keys are drawn over the white keys beside them, so a black cell owns its
    /// whole x range: a lane has no vertical extent to tell them apart.
    pub fn note_at(self, rect: egui::Rect, x: f32) -> u8 {
        let (low, high) = self.ends();
        if x <= self.x_of(rect, low) {
            return low;
        }
        if x >= self.x_after(rect, high) {
            return high;
        }
        let black = (low..=high)
            .filter(|note| is_black(*note))
            .find(|note| x >= self.x_of(rect, *note) && x < self.x_after(rect, *note));
        if let Some(note) = black {
            return note;
        }
        // The gap between two white cells belongs to the key on its left.
        let unit = self.unit(rect);
        let index = match unit > 0.0 {
            true => ((x - rect.left()) / unit).floor(),
            false => 0.0,
        };
        self.white(index.clamp(0.0, (self.whites() - 1) as f32) as usize)
    }

    /// The middle of `note`'s cell, which is what a marker points at.
    fn center(self, rect: egui::Rect, note: u8) -> f32 {
        (self.x_of(rect, note) + self.x_after(rect, note)) / 2.0
    }
}

/// How far a played key is from the root that sounds it.
pub fn shifted(note: u8, root: u8) -> String {
    format!("shifted {:+} st", note as i16 - root as i16)
}

/// A key struck, and how hard. A click on the keyboard strikes at
/// [`AUDITION_VELOCITY`]; anything else carries the velocity it was played at.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Struck {
    pub note: u8,
    pub velocity: u8,
}

/// The last key struck, what struck it, and since when: the key the chip and the line
/// under the keyboard describe.
///
/// ⚠️ `std::time::Instant::now()` traps on `wasm32-unknown-unknown`, so the clock is
/// egui's frame time in seconds ([`egui::InputState::time`]), as in [`crate::log`].
pub struct Audition {
    pub struck: Struck,
    pub finger: Finger,
    pub started: f64,
}

impl Audition {
    /// How long a clicked key stays lit, in seconds.
    pub const HOLD: f64 = 2.2;

    pub fn new(struck: Struck, finger: Finger, now: f64) -> Audition {
        Audition {
            struck,
            finger,
            started: now,
        }
    }

    /// Whether it is still described: a click for [`Self::HOLD`], and a controller key
    /// for as long as it is among the keys held `down`.
    pub fn live(&self, now: f64, down: &[u8]) -> bool {
        match self.finger {
            Finger::Pointer => now - self.started < Audition::HOLD,
            Finger::Key(key) => down.contains(&key),
        }
    }

    /// How long a click stays described, so a repaint can be scheduled to clear it. A
    /// controller key clears on release, which brings its own frame.
    pub fn left(&self, now: f64) -> Option<f64> {
        match self.finger {
            Finger::Pointer => Some((Audition::HOLD - (now - self.started)).max(0.0)),
            Finger::Key(_) => None,
        }
    }
}

/// The keys to light: every key held `down`, and the last key struck.
pub fn lit(audition: Option<&Audition>, down: &[u8]) -> Vec<u8> {
    let mut lit = down.to_vec();
    lit.extend(audition.map(|held| held.struck.note));
    lit
}

/// A dashed outline through `corners` in order, since egui draws dashes only along a
/// line.
fn dashed(painter: &egui::Painter, corners: &[egui::Pos2], stroke: egui::Stroke) {
    const DASH: f32 = 3.0;
    for side in corners.windows(2) {
        painter.extend(egui::Shape::dashed_line(side, stroke, DASH, DASH));
    }
}

/// Diagonal lines across `rect`, for a stretch of keyboard no zone plays.
///
/// Clipped to `rect`, so lines that reach past a corner stop at the edge and do not cross
/// whatever is drawn beside it.
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

/// A galley painted left-aligned, vertically centered on `middle`.
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

/// The corner every rectangle in a document is drawn with.
pub(crate) const RADIUS: f32 = 2.0;

const CHIP_TEXT: f32 = 9.5;
const CHIP_PAD: f32 = 3.0;

/// A key worth pointing at: a zone's root, or a library's.
pub struct Mark {
    pub note: u8,
    /// The name beside the marker, where a name is useful.
    pub label: Option<String>,
}

/// How tall the keyboard is drawn, which is what a pinned region reserves room for.
pub const KEYBOARD_H: f32 = 58.0;
const BLACK_H: f32 = 35.0;
const OCTAVE_TEXT: f32 = 8.0;
/// The opacity of an octave label relative to the key's ink.
const OCTAVE_ALPHA: f32 = 0.7;
const MARK_TOP: f32 = 2.0;
const MARK_H: f32 = 4.0;
const MARK_W: f32 = 6.0;

/// The rect of `note`'s key: its cell, as deep as that kind of key is drawn.
fn key_rect(rect: egui::Rect, span: Span, note: u8) -> egui::Rect {
    let bottom = match is_black(note) {
        true => rect.top() + BLACK_H,
        false => rect.bottom(),
    };
    egui::Rect::from_min_max(
        egui::pos2(span.x_of(rect, note), rect.top()),
        egui::pos2(span.x_after(rect, note), bottom),
    )
}

/// The keys of `span` in paint order: black keys last, so a white key painted after a
/// black key cannot cover the part of it that overlaps.
fn layered(span: Span) -> impl Iterator<Item = u8> {
    let (low, high) = span.ends();
    (low..=high)
        .filter(|note| !is_black(*note))
        .chain((low..=high).filter(|note| is_black(*note)))
}

/// The key under `at`. Black keys are drawn over the white keys, so they are tested
/// first; below the black keys' depth, the white key gets the hit.
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
/// `lit` are the keys sounding, and `chip` is the key that gets the chip saying what it
/// plays. Returns the key clicked, struck at [`AUDITION_VELOCITY`].
pub fn keyboard(
    ui: &mut egui::Ui,
    span: Span,
    lit: &[u8],
    chip: Option<(u8, &str)>,
    marks: &[Mark],
) -> Option<Struck> {
    let width = ui.available_width().max(1.0);
    let (rect, response) =
        ui.allocate_exact_size(egui::vec2(width, KEYBOARD_H), egui::Sense::click());
    let visuals = ui.visuals().clone();
    let painter = ui.painter().clone();
    let hovered = response.hover_pos().and_then(|at| key_at(rect, span, at));

    for note in layered(span) {
        let key = key_rect(rect, span, note);
        let black = is_black(note);
        let fill = match (black, lit.contains(&note) || hovered == Some(note)) {
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
    if let Some((note, said)) = chip {
        audition_chip(ui, rect, span, note, said);
    }

    let response = match hovered {
        Some(note) => response.on_hover_text(format!(
            "Play {} at velocity {AUDITION_VELOCITY}",
            note::name(note)
        )),
        None => response,
    };
    if !response.clicked() {
        return None;
    }
    let note = key_at(rect, span, response.interact_pointer_pos()?)?;
    Some(Struck {
        note,
        velocity: AUDITION_VELOCITY,
    })
}

/// How tall the line under the keyboard is, whatever it says.
pub const LINE_H: f32 = 20.0;

/// The line under the keyboard: whether the last struck key sounded, and a sentence
/// describing it.
///
/// ⚠️ Always one row, drawn empty when no key is lit. A sentence too long for it is
/// truncated, with the full text on hover. A line that grew with its sentence would shift
/// everything below it on every strike.
pub fn line(ui: &mut egui::Ui, said: Option<(bool, &str)>) {
    const TEXT: f32 = 11.0;
    const GLYPH: f32 = 12.0;
    const INDENT: f32 = 19.0;

    let room = ui.available_width().max(INDENT);
    let (line, response) = ui.allocate_exact_size(egui::vec2(room, LINE_H), egui::Sense::hover());
    let Some((sounded, words)) = said else {
        return;
    };
    let (glyph, ink) = match sounded {
        true => (Glyph::AudioLines, app::good(ui.visuals())),
        false => (Glyph::CircleAlert, app::warn(ui.visuals())),
    };
    painted(
        ui,
        glyph,
        egui::Rect::from_center_size(
            egui::pos2(line.left() + GLYPH / 2.0, line.center().y),
            egui::Vec2::splat(GLYPH),
        ),
        ink,
    );
    let text = ui.visuals().weak_text_color();
    let mut job = egui::text::LayoutJob::default();
    job.append(
        words,
        0.0,
        egui::TextFormat::simple(egui::FontId::proportional(TEXT), text),
    );
    job.wrap = egui::text::TextWrapping::truncate_at_width(line.width() - INDENT);
    let galley = ui.painter().layout_job(job);
    let cut = galley.elided;
    ui.painter().galley(
        egui::pos2(
            line.left() + INDENT,
            line.center().y - galley.size().y / 2.0,
        ),
        galley,
        text,
    );
    if cut {
        response.on_hover_text(words);
    }
}

/// A marker over one key: a triangle at its top, with the caller's label above it, if
/// any.
fn root_mark(
    painter: &egui::Painter,
    rect: egui::Rect,
    span: Span,
    mark: &Mark,
    visuals: &egui::Visuals,
) {
    let center = span.center(rect, mark.note);
    let accent = app::accent(visuals);
    let mut top = rect.top() + MARK_TOP;
    if let Some(label) = &mark.label {
        let named = chip(
            painter,
            egui::pos2(center, top),
            label,
            accent,
            visuals.text_color(),
        );
        top = named.bottom() + 1.0;
    }
    painter.add(egui::Shape::convex_polygon(
        vec![
            egui::pos2(center - MARK_W / 2.0, top),
            egui::pos2(center + MARK_W / 2.0, top),
            egui::pos2(center, top + MARK_H),
        ],
        accent,
        egui::Stroke::NONE,
    ));
}

/// What is sounding, over the key sounding it.
///
/// ⚠️ Painted on a foreground layer above the keyboard: the key map's own rect ends at
/// the keys, and a chip inside it would cover the root marks.
fn audition_chip(ui: &egui::Ui, rect: egui::Rect, span: Span, note: u8, said: &str) {
    let painter = ui.ctx().layer_painter(egui::LayerId::new(
        egui::Order::Foreground,
        ui.id().with("audition"),
    ));
    let center = span.center(rect, note);
    chip(
        &painter,
        egui::pos2(center, rect.top() - CHIP_TEXT - 4.0),
        said,
        app::good(ui.visuals()),
        ui.visuals().text_color(),
    );
}

/// One zone as the lane draws it: the keys it covers, and its name.
pub struct Band {
    pub low: u8,
    pub top: u8,
    pub name: String,
    pub range_text: String,
    pub hint: String,
}

/// Which ends of a band can be dragged. `TopOnly` is the v2 table, where a zone's low is
/// derived from the zone below and not stored. `Fixed` is a body whose zones cannot be
/// written, where a handle that moved and snapped back would mislead.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Edges {
    Fixed,
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
    /// order the bands were given. With `Edges::TopOnly`, the band above has moved too.
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
/// The opacity of a gap's hatch.
const HATCH_ALPHA: f32 = 0.55;

/// The stretches of `span` no band covers, low to high.
pub fn gaps(bounds: &[(u8, u8)], span: Span) -> Vec<(u8, u8)> {
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

/// How far a lane's rows may be dragged into each other.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Room {
    /// Whether the neighboring row gives up the keys this one takes. If not, an edge
    /// stops one key short of it, and the rows may leave a gap but never overlap.
    shared: bool,
    /// The fewest keys a row or its neighbor may be left with.
    fewest: u8,
}

/// The fewest keys a band may be left with. A band whose ends met would stack its two
/// handles, and the lower one could not be grabbed again.
const BAND_KEYS: u8 = 2;

/// Where a dragged edge lands, and what moves with it.
///
/// ⚠️ The neighbor is the next row along the keyboard, not the next index: a lane draws
/// its rows in file order, which is not keyboard order.
///
/// An edge with nowhere to land, because the row or its neighbor is already down to
/// [`Room::fewest`] keys, leaves every bound unchanged instead of stepping onto the next
/// row.
fn dragged(
    bounds: &[(u8, u8)],
    span: Span,
    row: usize,
    edge: Edge,
    note: u8,
    room: Room,
) -> Vec<(u8, u8)> {
    let mut next = bounds.to_vec();
    let (span_low, span_high) = span.ends();
    let Some(&(low, top)) = next.get(row) else {
        return next;
    };
    // How far the row's own two ends stay apart, so that it keeps `fewest` keys.
    let apart = room.fewest.saturating_sub(1);
    match edge {
        Edge::Top => {
            let above = next
                .iter()
                .enumerate()
                .filter(|(index, (their_low, _))| *index != row && *their_low > low)
                .min_by_key(|(_, (their_low, _))| *their_low)
                .map(|(index, _)| index);
            let ceiling = match (above, room.shared) {
                (Some(above), true) => next[above].1.saturating_sub(room.fewest),
                (Some(above), false) => next[above].0.saturating_sub(1),
                (None, _) => span_high,
            };
            let floor = low.saturating_add(apart);
            if floor > ceiling {
                return next;
            }
            let landed = note.clamp(floor, ceiling);
            next[row].1 = landed;
            if let (Some(above), true) = (above, room.shared) {
                next[above].0 = landed.saturating_add(1);
            }
        }
        Edge::Low => {
            let below = next
                .iter()
                .enumerate()
                .filter(|(index, (_, their_top))| *index != row && *their_top < top)
                .max_by_key(|(_, (_, their_top))| *their_top)
                .map(|(index, _)| index);
            let floor = match (below, room.shared) {
                (Some(below), true) => next[below].0.saturating_add(room.fewest),
                (Some(below), false) => next[below].1.saturating_add(1),
                (None, _) => span_low,
            };
            let ceiling = top.saturating_sub(apart);
            if floor > ceiling {
                return next;
            }
            let landed = note.clamp(floor, ceiling);
            next[row].0 = landed;
            if let (Some(below), true) = (below, room.shared) {
                next[below].1 = landed.saturating_sub(1);
            }
        }
    }
    next
}

/// Where a dragged band edge lands, and what moves with it.
///
/// A band keeps [`BAND_KEYS`] keys and stops one key short of the band beside it. With
/// [`Edges::TopOnly`], the only shape with one handle per band, the band above's low
/// follows the top it is derived from.
fn clamped(
    bounds: &[(u8, u8)],
    span: Span,
    zone: usize,
    edge: Edge,
    note: u8,
    edges: Edges,
) -> Vec<(u8, u8)> {
    let room = Room {
        shared: edges == Edges::TopOnly,
        fewest: BAND_KEYS,
    };
    dragged(bounds, span, zone, edge, note, room)
}

/// The zone lane: one band per zone, the keys between them hatched.
///
/// `selected` is the open row, and `lit` the zone playing the auditioned key. Bands are
/// drawn in the order given, which is file order.
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
                "No zone covers the keys {}–{}",
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

    let (act, grabs) = drag_edges(
        ui,
        &painter,
        rect,
        lane,
        span,
        &bounds,
        edges,
        |index, edge| drag_hint(&zones[index], edge, edges, index),
        |index, edge, note| clamped(&bounds, span, index, edge, note, edges),
    );

    if let Some(hint) = hint {
        response.clone().on_hover_text(hint);
    }
    if act.is_some() {
        return act;
    }
    let picked = picked_at(&response, &grabs)?;
    row_at(rect, span, &bounds, picked.x).map(BandAct::Pick)
}

/// Where a click on a lane landed.
///
/// ⚠️ A handle sits over its row, and a click on it belongs to the handle, so it is
/// never a pick.
fn picked_at(response: &egui::Response, grabs: &[egui::Rect]) -> Option<egui::Pos2> {
    response
        .clicked()
        .then(|| response.interact_pointer_pos())
        .flatten()
        .filter(|at| !grabs.iter().any(|grab| grab.contains(*at)))
}

/// The row whose keys `x` falls in.
fn row_at(rect: egui::Rect, span: Span, bounds: &[(u8, u8)], x: f32) -> Option<usize> {
    bounds
        .iter()
        .position(|(low, top)| x >= span.x_of(rect, *low) && x < span.x_after(rect, *top))
}

/// The three looks a band can have.
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

/// The movable ends of a lane: one pill per end, the chip that follows a drag, and the
/// grab rects where a click is not a pick.
///
/// `moved` computes the lane's bounds when one end is dragged to a note. It is the only
/// difference between a zone lane and a root lane.
#[allow(clippy::too_many_arguments)]
fn drag_edges(
    ui: &egui::Ui,
    painter: &egui::Painter,
    rect: egui::Rect,
    lane: egui::Rect,
    span: Span,
    bounds: &[(u8, u8)],
    edges: Edges,
    hint: impl Fn(usize, Edge) -> String,
    moved: impl Fn(usize, Edge, u8) -> Vec<(u8, u8)>,
) -> (Option<BandAct>, Vec<egui::Rect>) {
    let visuals = ui.visuals().clone();
    let wanted = match edges {
        Edges::Fixed => [None, None],
        Edges::TopOnly => [Some(Edge::Top), None],
        Edges::Both => [Some(Edge::Top), Some(Edge::Low)],
    };
    let mut act = None;
    let mut grabs = Vec::new();
    for (index, ends) in bounds.iter().enumerate() {
        for edge in wanted.into_iter().flatten() {
            let grab = handle_rect(rect, lane, span, *ends, edge);
            grabs.push(grab);
            let at = handle(
                ui,
                painter,
                grab,
                (index, edge),
                &hint(index, edge),
                &visuals,
            );
            let Some(at) = at else { continue };
            let next = moved(index, edge, span.note_at(rect, at.x));
            let shown = match edge {
                Edge::Top => next[index].1,
                Edge::Low => next[index].0,
            };
            chip(
                painter,
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
    (act, grabs)
}

/// The grab zone for one end of a band, inside the band it belongs to.
fn handle_rect(
    rect: egui::Rect,
    lane: egui::Rect,
    span: Span,
    (low, top): (u8, u8),
    edge: Edge,
) -> egui::Rect {
    let left = match edge {
        Edge::Top => span.x_after(rect, top) - HANDLE_W - 1.0,
        Edge::Low => span.x_of(rect, low) + 1.0,
    };
    egui::Rect::from_min_size(
        egui::pos2(left, lane.top()),
        egui::vec2(HANDLE_W, lane.height()),
    )
}

/// A handle's tooltip. With [`Edges::TopOnly`] a top handle also moves the low derived
/// from it.
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
        "Drag to move {}'s {what} note{follows} (now {})",
        band.name,
        note::name(note)
    )
}

/// One end of a band, as a pill to grab. Returns the pointer position while it is
/// dragged.
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

/// One root as the size lane draws it: the keys it covers, and its size.
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
/// How many keys a cell needs before its size is printed in it.
const MB_KEYS: usize = 4;
/// How many megabytes a root must lose before its cell reads as trimmed.
const TRIMMED: f32 = 0.05;

/// Where a dragged root boundary lands.
///
/// The two roots on either side of a boundary share it: what one gives up the other
/// takes, and neither is left without a key. The outer end of the lowest or highest root
/// has no neighbor, so it covers or uncovers keys instead. `bounds` may be in any order;
/// the neighbor is the next cell along the keyboard, not the next index.
pub fn boundary(
    bounds: &[(u8, u8)],
    span: Span,
    cell: usize,
    edge: Edge,
    note: u8,
) -> Vec<(u8, u8)> {
    let room = Room {
        shared: true,
        fewest: 1,
    };
    dragged(bounds, span, cell, edge, note, room)
}

/// A root boundary handle's tooltip.
fn root_hint(cell: &SizeCell, edge: Edge) -> String {
    let (what, note) = match edge {
        Edge::Top => ("top", cell.top),
        Edge::Low => ("low", cell.low),
    };
    format!(
        "Drag to move root {}'s {what} key (now {})",
        cell.name,
        note::name(note)
    )
}

/// The size lane: one cell per root, with its kept megabytes as a bar inside a dashed
/// outline of its untrimmed size, and a handle at each end of the keys it covers.
pub fn size_cells(
    ui: &mut egui::Ui,
    span: Span,
    cells: &[SizeCell],
    selected: Option<usize>,
    lit: Option<usize>,
    edges: Edges,
) -> Option<BandAct> {
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

    let bounds: Vec<(u8, u8)> = cells.iter().map(|cell| (cell.low, cell.top)).collect();
    let (act, grabs) = drag_edges(
        ui,
        &painter,
        rect,
        egui::Rect::from_min_max(rect.min, egui::pos2(rect.right(), rect.top() + BAR_AREA)),
        span,
        &bounds,
        edges,
        |index, edge| root_hint(&cells[index], edge),
        |index, edge, note| boundary(&bounds, span, index, edge, note),
    );

    if let Some(hint) = hint {
        response.clone().on_hover_text(hint);
    }
    if act.is_some() {
        return act;
    }
    let picked = picked_at(&response, &grabs)?;
    row_at(rect, span, &bounds, picked.x).map(BandAct::Pick)
}

/// How many keys a cell spans.
fn cell_keys(cell: &SizeCell) -> usize {
    (cell.top.max(cell.low) - cell.low) as usize + 1
}

/// One zone as the velocity field draws it: the keys it covers, the velocities it plays
/// them at, and its name.
pub struct VelBlock {
    pub low: u8,
    pub top: u8,
    /// The window's ends, inclusive and in order.
    pub window: (u8, u8),
    pub name: String,
    pub hint: String,
}

/// Which velocity edges a block offers. `Fixed` is the wide generations' window, which
/// the format defines and nothing writes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Handles {
    Fixed,
    Draggable,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum VelEdge {
    Min,
    Max,
}

/// What the velocity field was asked for this frame.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum VelocityAct {
    /// A block was clicked.
    Pick(usize),
    /// A handle moved. `window` is that zone's window after the clamp.
    Drag {
        zone: usize,
        edge: VelEdge,
        window: (u8, u8),
    },
}

/// The lowest and highest velocity a key can be struck at.
pub const VELOCITY_LOW: u8 = 1;
pub const VELOCITY_HIGH: u8 = 127;

const FIELD_H: f32 = 84.0;
const AXIS_W: f32 = 22.0;
const AXIS_GAP: f32 = 6.0;
const AXIS_TEXT: f32 = 9.0;
/// Where the field's grid lines sit, as a fraction of its height.
const GRID: [f32; 2] = [0.5, 0.75];
const BLOCK_NAME: f32 = 10.0;
const BLOCK_PAD: f32 = 6.0;
const BLOCK_TOP: f32 = 8.0;
/// The smallest size a block is drawn at, so a one-velocity window stays visible.
const BLOCK_MIN_H: f32 = 2.0;
const BLOCK_MIN_W: f32 = 3.0;
/// A block's fill opacity when selected, and when not.
const BLOCK_ALPHA: f32 = 0.45;
const QUIET_ALPHA: f32 = 0.08;
/// The handle: the pill, and the grab area around it.
const PILL_H: f32 = 4.0;
const GRAB_H: f32 = 10.0;
const PILL_SHARE: f32 = 0.56;
const PILL_MAX: f32 = 120.0;
/// The opacity of a hole's hatch.
const HOLE_ALPHA: f32 = 0.5;

/// The key × velocity regions no block covers, by the zone whose keys they leave silent.
///
/// A zone's keys play at a velocity if its own window covers it, or the window of any
/// block whose keys overlap it does. The holes are what remains of `1..=127` after those
/// windows are combined.
pub fn velocity_holes(blocks: &[VelBlock]) -> Vec<(usize, u8, u8)> {
    let mut out = Vec::new();
    for (index, block) in blocks.iter().enumerate() {
        let mut covered: Vec<(u8, u8)> = blocks
            .iter()
            .enumerate()
            .filter(|(other, over)| {
                *other == index || (over.low <= block.top && over.top >= block.low)
            })
            .map(|(_, over)| ordered(over.window))
            .collect();
        covered.sort_by_key(|(low, _)| *low);
        let mut at = VELOCITY_LOW;
        for (low, high) in covered {
            if low > at {
                out.push((index, at, low.saturating_sub(1)));
            }
            at = at.max(high.saturating_add(1));
        }
        if at <= VELOCITY_HIGH {
            out.push((index, at, VELOCITY_HIGH));
        }
    }
    out
}

/// A window's ends in order, so an inverted window still reads as a range.
fn ordered(window: (u8, u8)) -> (u8, u8) {
    (window.0.min(window.1), window.0.max(window.1))
}

/// The velocity field: one rectangle per zone over the keys it covers, with the key ×
/// velocity regions nothing plays hatched.
///
/// The axis is drawn here because it labels this widget's vertical scale.
pub fn velocity(
    ui: &mut egui::Ui,
    span: Span,
    blocks: &[VelBlock],
    selected: Option<usize>,
    handles: Handles,
) -> Option<VelocityAct> {
    let width = ui.available_width().max(1.0);
    let (whole, response) =
        ui.allocate_exact_size(egui::vec2(width, FIELD_H), egui::Sense::click());
    let visuals = ui.visuals().clone();
    let painter = ui.painter().clone();
    let rect = egui::Rect::from_min_max(
        egui::pos2(whole.left() + AXIS_W + AXIS_GAP, whole.top()),
        whole.max,
    );

    for (share, text) in [(0.0, VELOCITY_HIGH), (0.5, 64), (1.0, VELOCITY_LOW)] {
        let galley = painter.layout_no_wrap(
            text.to_string(),
            egui::FontId::monospace(AXIS_TEXT),
            app::caption(&visuals),
        );
        let middle = rect.top() + (rect.height() - galley.size().y) * share + galley.size().y / 2.0;
        write(
            &painter,
            whole.left() + AXIS_W - galley.size().x,
            middle,
            galley,
        );
    }

    painter.rect_filled(rect, RADIUS, visuals.extreme_bg_color);
    painter.rect_stroke(
        rect,
        RADIUS,
        egui::Stroke::new(1.0_f32, visuals.widgets.noninteractive.bg_stroke.color),
        egui::StrokeKind::Inside,
    );
    for share in GRID {
        painter.hline(
            rect.x_range(),
            rect.top() + rect.height() * share,
            egui::Stroke::new(1.0_f32, app::unlit(&visuals).gamma_multiply(0.5)),
        );
    }

    let mut hint = None;
    for (zone, from, to) in velocity_holes(blocks) {
        let block = &blocks[zone];
        let hole = cell(rect, span, block, (from, to));
        hatch(&painter, hole, app::bad(&visuals), HOLE_ALPHA);
        if response.hover_pos().is_some_and(|at| hole.contains(at)) {
            hint = Some(format!(
                "Nothing plays {}–{} at velocity {from}–{to}",
                note::name(block.low),
                note::name(block.top)
            ));
        }
    }

    for (index, block) in blocks.iter().enumerate() {
        let over = cell(rect, span, block, block.window);
        let picked = selected == Some(index);
        let worn = look(&visuals, picked, false);
        let alpha = match picked {
            true => BLOCK_ALPHA,
            false => QUIET_ALPHA,
        };
        let fill = match picked {
            true => worn.fill,
            false => visuals.weak_text_color(),
        };
        painter.rect_filled(over, RADIUS, fill.gamma_multiply(alpha));
        painter.rect_stroke(
            over,
            RADIUS,
            egui::Stroke::new(
                match picked {
                    true => 2.0_f32,
                    false => 1.0,
                },
                worn.stroke,
            ),
            egui::StrokeKind::Inside,
        );
        let (low, high) = ordered(block.window);
        let room = over.width() - BLOCK_PAD * 2.0;
        if room > 0.0 {
            let ink = match picked {
                true => visuals.text_color(),
                false => visuals.weak_text_color(),
            };
            let window = clipped(
                &painter,
                &format!("vel {low}–{high}"),
                egui::FontId::monospace(CHIP_TEXT),
                ink,
                room,
            );
            let name = clipped(
                &painter,
                &block.name,
                egui::FontId::new(BLOCK_NAME, app::bold()),
                ink,
                room - window.size().x - BAND_GAP,
            );
            let left = over.left() + BLOCK_PAD;
            let middle = over.top() + BLOCK_TOP;
            let after = left + name.size().x + BAND_GAP;
            write(&painter, left, middle, name);
            write(&painter, after, middle, window);
        }
        if response.hover_pos().is_some_and(|at| over.contains(at)) {
            hint = Some(block.hint.clone());
        }
    }

    let mut act = None;
    let mut grabs = Vec::new();
    if handles == Handles::Draggable {
        for (index, block) in blocks.iter().enumerate() {
            for edge in [VelEdge::Max, VelEdge::Min] {
                let grab = vel_handle_rect(rect, span, block, edge);
                grabs.push(grab);
                let at = vel_handle(ui, &painter, grab, (index, edge), block, edge, &visuals);
                let Some(at) = at else { continue };
                let window = vel_clamped(block.window, edge, velocity_at(rect, at.y));
                let shown = match edge {
                    VelEdge::Max => window.1,
                    VelEdge::Min => window.0,
                };
                chip(
                    &painter,
                    egui::pos2(grab.center().x, y_of(rect, f32::from(shown)) - CHIP_TEXT),
                    &format!("vel {shown}"),
                    app::accent(&visuals),
                    app::accent(&visuals),
                );
                if window != ordered(block.window) {
                    act = Some(VelocityAct::Drag {
                        zone: index,
                        edge,
                        window,
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
    let picked = picked_at(&response, &grabs)?;
    blocks
        .iter()
        .position(|block| cell(rect, span, block, block.window).contains(picked))
        .map(VelocityAct::Pick)
}

/// Where one velocity sits in the field.
fn y_of(rect: egui::Rect, velocity: f32) -> f32 {
    let reach = f32::from(VELOCITY_HIGH - VELOCITY_LOW);
    let share = ((velocity - f32::from(VELOCITY_LOW)) / reach).clamp(0.0, 1.0);
    rect.bottom() - rect.height() * share
}

/// The velocity `y` falls on, clamped to the field.
fn velocity_at(rect: egui::Rect, y: f32) -> u8 {
    let share = ((rect.bottom() - y) / rect.height().max(1.0)).clamp(0.0, 1.0);
    let reach = f32::from(VELOCITY_HIGH - VELOCITY_LOW);
    (f32::from(VELOCITY_LOW) + share * reach).round() as u8
}

/// One block's rectangle: its keys across, a velocity range down.
fn cell(rect: egui::Rect, span: Span, block: &VelBlock, window: (u8, u8)) -> egui::Rect {
    let (low, high) = ordered(window);
    let left = span.x_of(rect, block.low);
    let top = y_of(rect, f32::from(high));
    egui::Rect::from_min_size(
        egui::pos2(left, top),
        egui::vec2(
            (span.x_after(rect, block.top) - left).max(BLOCK_MIN_W),
            (y_of(rect, f32::from(low)) - top).max(BLOCK_MIN_H),
        ),
    )
}

/// Where a dragged velocity edge lands: neither end reaches the other, and a drag past
/// the field's ends stops at them.
fn vel_clamped(window: (u8, u8), edge: VelEdge, velocity: u8) -> (u8, u8) {
    let (low, high) = ordered(window);
    match edge {
        VelEdge::Max => (
            low,
            velocity
                .max(low.saturating_add(1))
                .clamp(VELOCITY_LOW, VELOCITY_HIGH),
        ),
        VelEdge::Min => (
            velocity
                .min(high.saturating_sub(1))
                .clamp(VELOCITY_LOW, VELOCITY_HIGH),
            high,
        ),
    }
}

/// The grab zone for one end of a block, straddling the edge it moves.
fn vel_handle_rect(rect: egui::Rect, span: Span, block: &VelBlock, edge: VelEdge) -> egui::Rect {
    let (low, high) = ordered(block.window);
    let at = y_of(
        rect,
        f32::from(match edge {
            VelEdge::Max => high,
            VelEdge::Min => low,
        }),
    );
    let over = cell(rect, span, block, block.window);
    egui::Rect::from_center_size(
        egui::pos2(over.center().x, at),
        egui::vec2(over.width(), GRAB_H),
    )
}

/// One end of a block, as a pill with a halo of the field's background.
fn vel_handle(
    ui: &egui::Ui,
    painter: &egui::Painter,
    grab: egui::Rect,
    which: (usize, VelEdge),
    block: &VelBlock,
    edge: VelEdge,
    visuals: &egui::Visuals,
) -> Option<egui::Pos2> {
    let response = ui.interact(
        grab,
        ui.id().with(("vel_edge", which.0, which.1 == VelEdge::Max)),
        egui::Sense::click_and_drag(),
    );
    let dragging = response.dragged();
    let ink = match dragging {
        true => app::accent(visuals),
        false => visuals.weak_text_color(),
    };
    let pill = egui::Rect::from_center_size(
        grab.center(),
        egui::vec2((grab.width() * PILL_SHARE).min(PILL_MAX), PILL_H),
    );
    painter.rect_filled(pill.expand(1.0), RADIUS, visuals.extreme_bg_color);
    painter.rect_filled(pill, RADIUS, ink);
    let (low, high) = ordered(block.window);
    response.clone().on_hover_text(format!(
        "Drag to move {}'s velocity {} (now {})",
        block.name,
        match edge {
            VelEdge::Max => "max",
            VelEdge::Min => "min",
        },
        match edge {
            VelEdge::Max => high,
            VelEdge::Min => low,
        }
    ));
    match dragging {
        true => response.interact_pointer_pos(),
        false => None,
    }
}

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
/// Below this a value is drawn as zero, not as a small edit.
const LANE_QUIET: f32 = 0.08;
/// The steps a painted value snaps to across the whole lane.
const SNAP: f32 = 20.0;
/// A painted value this close to zero snaps to zero.
const DEADBAND: f32 = 0.06;

/// One value per key, drawn as bars on either side of zero. A drag across the lane paints
/// over them and returns each key it touched with its new value, in paint order.
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
            egui::vec2((span.x_after(inner, note) - left).max(1.0), tall),
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

    let painting =
        response.is_pointer_button_down_on() && ui.input(|input| input.pointer.primary_down());
    match painting {
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

/// Every key this frame's pointer positions painted, in arrival order. A drag off the end
/// of the lane keeps painting the end key.
fn strokes(ui: &egui::Ui, rect: egui::Rect, span: Span) -> Vec<(u8, f32)> {
    let mut out: Vec<(u8, f32)> = Vec::new();
    ui.input(|input| {
        // ⚠️ A move before this frame's press is the pointer on its way to the lane, not a
        // stroke. A frame with no press continues the drag already in progress.
        let opened = input.events.iter().rposition(|event| {
            matches!(
                event,
                egui::Event::PointerButton {
                    button: egui::PointerButton::Primary,
                    pressed: true,
                    ..
                }
            )
        });
        for event in &input.events[opened.unwrap_or(0)..] {
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

    /// `note` as a click on the keyboard strikes it.
    fn clicked(note: u8) -> Struck {
        Struck {
            note,
            velocity: AUDITION_VELOCITY,
        }
    }

    /// A context with the app's fonts: without the bold family, laying out a band's name
    /// panics.
    fn dressed() -> egui::Context {
        let ctx = egui::Context::default();
        ctx.set_fonts(crate::app::fonts());
        ctx.set_visuals(egui::Visuals::dark());
        ctx
    }

    /// One frame of `body`, returning what it painted, the rect the widget claims, and
    /// what `body` returned. The rect lets the next frame point at a key.
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

    /// The fills a frame painted at `rect`, in paint order.
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

    /// Every hit test in the map depends on this: the key under a point is the key whose
    /// cell holds it.
    #[test]
    fn a_lane_gives_each_key_a_cell_and_reads_it_back() {
        assert_eq!(NSMP.keys(), 73);
        assert_eq!(NPNO.keys(), 88, "a full piano");

        let rect = egui::Rect::from_min_size(egui::pos2(7.0, 3.0), egui::vec2(601.0, 19.0));
        for span in [NSMP, NPNO] {
            let unit = rect.width() / span.whites() as f32;
            for note in span.low..=span.high {
                let left = span.x_of(rect, note);
                let wide = span.x_after(rect, note) - left;
                let wanted = match is_black(note) {
                    true => unit * BLACK_W,
                    false => unit - WHITE_GAP,
                };
                assert!((wide - wanted).abs() < 0.001, "{note} is {wide} wide");
                assert_eq!(span.note_at(rect, span.center(rect, note)), note);
            }
            // The span starts at the left edge and the last white key ends at the right,
            // a gap short of it.
            assert!((span.x_of(rect, span.low) - rect.left()).abs() < 0.001);
            assert!((span.x_after(rect, span.high) - rect.right() + WHITE_GAP).abs() < 0.001,);
        }
    }

    /// The key rects a keyboard frame painted, in the order it painted them.
    fn key_shapes(output: &egui::FullOutput, rect: egui::Rect) -> Vec<egui::Rect> {
        fn walk(shape: &egui::Shape, rect: egui::Rect, into: &mut Vec<egui::Rect>) {
            match shape {
                egui::Shape::Rect(drawn)
                    if drawn.rect.top() == rect.top()
                        && (drawn.rect.height() == KEYBOARD_H
                            || drawn.rect.height() == BLACK_H) =>
                {
                    into.push(drawn.rect)
                }
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

    /// The lane and the keyboard share one geometry. If they drifted apart, every band
    /// would sit off the key it names.
    #[test]
    fn a_keys_cell_is_the_key_the_keyboard_paints() {
        let ctx = dressed();
        for span in [NSMP, NPNO] {
            let (output, rect, _) = frame(&ctx, Vec::new(), KEYBOARD_H, |ui| {
                keyboard(ui, span, &[], None, &[])
            });
            let painted = key_shapes(&output, rect);
            assert_eq!(painted.len(), span.keys(), "one rect per key");
            for note in span.low..=span.high {
                let deep = match is_black(note) {
                    true => BLACK_H,
                    false => KEYBOARD_H,
                };
                let cell = egui::Rect::from_min_max(
                    egui::pos2(span.x_of(rect, note), rect.top()),
                    egui::pos2(span.x_after(rect, note), rect.top() + deep),
                );
                let at = painted.iter().filter(|drawn| **drawn == cell).count();
                assert_eq!(
                    at,
                    1,
                    "{} is painted {at} times at its cell {cell:?}",
                    note::name(note),
                );
            }
        }
    }

    /// ⚠️ C#4's center is where C4's cell ends and D4's begins. The keyboard paints it
    /// after both, or the white key beside it would cover the part that overlaps.
    #[test]
    fn a_black_key_is_centered_on_the_boundary_and_painted_over_the_whites_it_hangs_between() {
        let ctx = dressed();
        for span in [NSMP, NPNO] {
            let (output, rect, _) = frame(&ctx, Vec::new(), KEYBOARD_H, |ui| {
                keyboard(ui, span, &[], None, &[])
            });
            let painted = key_shapes(&output, rect);
            for black in (span.low..=span.high).filter(|note| is_black(*note)) {
                let off = span.center(rect, black) - span.x_of(rect, black + 1);
                assert!(
                    off.abs() < 0.001,
                    "{} sits {off} from the boundary between {} and {}",
                    note::name(black),
                    note::name(black - 1),
                    note::name(black + 1),
                );
                let cell = key_rect(rect, span, black);
                let Some(at) = painted.iter().position(|drawn| *drawn == cell) else {
                    panic!("{} is not painted at {cell:?}", note::name(black));
                };
                let covered = painted[at + 1..]
                    .iter()
                    .find(|drawn| drawn.intersects(cell));
                assert!(
                    covered.is_none(),
                    "{} is painted under {covered:?}, which hides the part it overlaps",
                    note::name(black),
                );
            }
        }
    }

    /// A drag off the lane keeps painting the end key.
    #[test]
    fn a_point_outside_the_span_clamps_to_the_end_it_is_past() {
        let rect = egui::Rect::from_min_size(egui::pos2(7.0, 3.0), egui::vec2(601.0, 19.0));
        for span in [NSMP, NPNO] {
            assert_eq!(span.note_at(rect, rect.left() - 500.0), span.low);
            assert_eq!(span.note_at(rect, rect.right() + 500.0), span.high);
            assert_eq!(span.x_of(rect, 0), span.x_of(rect, span.low));
            assert_eq!(span.x_of(rect, 127), span.x_of(rect, span.high));
            assert_eq!(span.note_at(rect, span.x_of(rect, span.low)), span.low);
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
    fn a_clicked_key_is_described_until_its_hold_is_up() {
        let struck = Audition::new(clicked(60), Finger::Pointer, 0.0);
        assert!(struck.live(0.0, &[]));
        assert!(struck.live(Audition::HOLD / 2.0, &[]));
        assert!(
            !struck.live(Audition::HOLD, &[60]),
            "a held key is not the click"
        );
        assert_eq!(
            struck.left(Audition::HOLD / 2.0),
            Some(Audition::HOLD / 2.0)
        );
    }

    #[test]
    fn a_played_key_is_described_until_it_is_let_go() {
        let struck = Audition::new(clicked(60), Finger::Key(60), 10.0);
        assert!(struck.live(10.0 + Audition::HOLD * 10.0, &[48, 60]));
        assert!(!struck.live(10.0, &[48]));
        assert_eq!(struck.left(10.0), None);
    }

    #[test]
    fn every_key_held_lights_with_the_last_key_struck() {
        let struck = Audition::new(clicked(72), Finger::Pointer, 0.0);
        assert_eq!(lit(Some(&struck), &[60, 64]), [60, 64, 72]);
        assert_eq!(lit(None, &[60, 64]), [60, 64]);
    }

    /// The two spans the editors draw are a six-octave sample map and a full piano, and
    /// both are laid out by their white keys.
    #[test]
    fn the_keyboard_holds_the_white_and_black_keys_of_its_span() {
        assert_eq!((NSMP.whites(), NSMP.keys() - NSMP.whites()), (43, 30));
        assert_eq!((NPNO.whites(), NPNO.keys() - NPNO.whites()), (52, 36));
        assert_eq!(NSMP.whites_before(NSMP.low), 0);
        assert_eq!(NSMP.whites_before(36), 7, "one octave of white keys");
        assert_eq!(NSMP.white(0), NSMP.low);
        assert_eq!(NPNO.white(51), NPNO.high, "the last white key is C8");
    }

    /// The only words on the keyboard are the octaves, and there is one per C.
    #[test]
    fn the_keyboard_labels_every_c_and_nothing_else() {
        let ctx = dressed();
        let (output, _, _) = frame(&ctx, Vec::new(), KEYBOARD_H, |ui| {
            keyboard(ui, NSMP, &[], None, &[])
        });
        let said: Vec<String> = words(&output).into_iter().map(|(text, _)| text).collect();
        assert_eq!(said, ["C1", "C2", "C3", "C4", "C5", "C6", "C7"]);
    }

    /// A click on a black key drawn over two white keys plays the black key.
    #[test]
    fn a_click_lands_on_the_key_under_it_black_keys_first() {
        let ctx = dressed();
        let (_, rect, _) = frame(&ctx, Vec::new(), KEYBOARD_H, |ui| {
            keyboard(ui, NSMP, &[], None, &[])
        });
        for note in [60u8, 61, NSMP.low, NSMP.high] {
            let at = key_rect(rect, NSMP, note).center();
            let (_, _, struck) = frame(&ctx, press(at), KEYBOARD_H, |ui| {
                keyboard(ui, NSMP, &[], None, &[])
            });
            assert_eq!(struck, Some(clicked(note)), "at {at:?}");
        }
    }

    /// A lit white key takes the selection color, and a lit black key the accent.
    #[test]
    fn every_lit_key_lights_and_the_others_keep_their_own_color() {
        let ctx = dressed();
        let visuals = ctx.style().visuals.clone();
        let (_, rect, _) = frame(&ctx, Vec::new(), KEYBOARD_H, |ui| {
            keyboard(ui, NSMP, &[], None, &[])
        });
        let (output, _, _) = frame(&ctx, Vec::new(), KEYBOARD_H, |ui| {
            keyboard(ui, NSMP, &[60, 61], Some((61, "lit")), &[])
        });
        for (note, lit) in [
            (60u8, visuals.selection.bg_fill),
            (61, crate::app::accent(&visuals)),
        ] {
            assert_eq!(fills(&output, key_rect(rect, NSMP, note)), vec![lit]);
            let quiet = key_rect(rect, NSMP, note + 12);
            let own = match is_black(note) {
                true => crate::app::stop_black(&visuals),
                false => crate::app::stop_white(&visuals),
            };
            assert_eq!(fills(&output, quiet), vec![own], "an octave up is unlit");
        }
    }

    /// An unlabeled marker is only the triangle: the piano lane marks many roots and has
    /// no room for all their names.
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
            keyboard(ui, NSMP, &[], None, &marks)
        });
        let said: Vec<String> = words(&output).into_iter().map(|(text, _)| text).collect();
        assert_eq!(
            said.iter().filter(|text| *text == "C5").count(),
            2,
            "the octave label and the marker's chip: {said:?}",
        );
    }

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

    #[test]
    fn the_gaps_are_the_keys_no_zone_covers() {
        assert_eq!(gaps(&[(61, 96), (41, 60), (24, 38)], NSMP), [(39, 40)]);
        assert!(gaps(&[(61, 96), (41, 60), (24, 40)], NSMP).is_empty());
        assert_eq!(gaps(&[(30, 90)], NSMP), [(24, 29), (91, 96)]);
        assert_eq!(gaps(&[], NSMP), [(24, 96)]);
        // A zone reaching past the span leaves no gap outside it.
        assert!(gaps(&[(0, 127)], NSMP).is_empty());
    }

    /// v2 stores no low note: a zone's low is the key above the zone below's top, so
    /// moving one top moves the neighbor's low with it and the two never overlap.
    #[test]
    fn a_derived_low_follows_the_top_it_is_derived_from() {
        let bounds = [(61, 96), (41, 60), (24, 40)];
        let moved = clamped(&bounds, NSMP, 1, Edge::Top, 55, Edges::TopOnly);
        assert_eq!(moved[1], (41, 55));
        assert_eq!(moved[0], (56, 96), "the zone above starts one key higher");
        assert_eq!(moved[2], bounds[2], "and nothing else moves");

        // Upward too: the zone above gives up the keys this one takes, down to the
        // fewest it may keep.
        let up = clamped(&bounds, NSMP, 1, Edge::Top, 70, Edges::TopOnly);
        assert_eq!((up[1], up[0]), ((41, 70), (71, 96)));
        assert_eq!(
            clamped(&bounds, NSMP, 1, Edge::Top, 127, Edges::TopOnly)[0],
            (95, 96),
            "the band above keeps the keys it needs to stay grabbable",
        );

        // With both edges stored, the neighbor stays where it was and a gap opens.
        let apart = clamped(&bounds, NSMP, 1, Edge::Top, 55, Edges::Both);
        assert_eq!((apart[1], apart[0]), ((41, 55), (61, 96)));
    }

    /// A band the drag would leave overlapping its neighbor does not move at all: the keys
    /// on either side of a band edge belong to one band or the other, never to both.
    #[test]
    fn a_band_with_no_room_left_refuses_the_drag() {
        let bounds = [(61, 96), (60, 60)];
        assert_eq!(
            clamped(&bounds, NSMP, 1, Edge::Top, 55, Edges::Both),
            bounds
        );
        assert_eq!(
            clamped(&bounds, NSMP, 1, Edge::Top, 90, Edges::Both),
            bounds
        );
        // The top has no room left, but the low still has keys below it.
        assert_eq!(
            clamped(&bounds, NSMP, 1, Edge::Low, 30, Edges::Both)[1],
            (30, 60),
        );

        // The neighbor is the next band along the keyboard, not the one before it in the
        // file: a top dragged into it stops one key short.
        let jumbled = [(41, 55), (61, 96), (24, 40)];
        let moved = clamped(&jumbled, NSMP, 0, Edge::Top, 70, Edges::Both);
        assert_eq!(moved[0], (41, 60), "a key short of the band above's low");
        assert_eq!(moved[1], jumbled[1], "and the band above stays where it is");
    }

    /// The clamps keep zones from overlapping or turning inside out.
    #[test]
    fn a_handle_stops_short_of_its_neighbor_and_of_its_own_other_end() {
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

        // The ends of the span act as the neighbors the outermost zones lack.
        assert_eq!(
            clamped(&bounds, NSMP, 0, Edge::Top, 120, Edges::Both)[0].1,
            NSMP.high
        );
        assert_eq!(
            clamped(&bounds, NSMP, 2, Edge::Low, 0, Edges::Both)[2].0,
            NSMP.low
        );
    }

    /// Clicking a band opens its row. With no handles, its ends pick it too.
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

        // With no edges to grab, the whole band takes a click, including the ends a
        // handle would otherwise cover.
        let fixed = |ui: &mut egui::Ui| bands(ui, NSMP, &zones, None, None, Edges::Fixed);
        let (_, rect, _) = frame(&ctx, Vec::new(), BANDS_H, fixed);
        let end = egui::pos2(
            NSMP.x_after(rect, 60) - HANDLE_W / 2.0 - 1.0,
            rect.center().y,
        );
        let (_, _, act) = frame(&ctx, press(end), BANDS_H, fixed);
        assert_eq!(act, Some(BandAct::Pick(1)));

        // A click on the hatch over a gap picks nothing.
        let holed = bands_of(&[(61, 96), (41, 60)]);
        let lane = |ui: &mut egui::Ui| bands(ui, NSMP, &holed, None, None, Edges::Both);
        let (_, rect, _) = frame(&ctx, Vec::new(), BANDS_H, lane);
        let over_gap = egui::pos2(NSMP.x_of(rect, 30), rect.center().y);
        let (_, _, act) = frame(&ctx, press(over_gap), BANDS_H, lane);
        assert_eq!(act, None);
    }

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

    /// A cell narrower than four keys has no room for its size, and a number printed there
    /// would overlap its neighbor's.
    #[test]
    fn a_cells_size_is_printed_only_where_it_fits() {
        let ctx = dressed();
        let span = Span { low: 48, high: 95 };
        for (keys, shown) in [(3u8, false), (4, true)] {
            let cells = [size_cell(60, 60 + keys - 1, 1.0, 1.0)];
            let (output, _, _) = frame(&ctx, Vec::new(), CELLS_H, |ui| {
                size_cells(ui, span, &cells, None, None, Edges::Fixed)
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

    /// A trimmed root's size is in warn ink, so what the trim has taken is visible without
    /// opening a row.
    #[test]
    fn a_trimmed_root_prints_its_size_in_warn_ink() {
        let ctx = dressed();
        let visuals = ctx.style().visuals.clone();
        let span = Span { low: 48, high: 95 };
        let cells = [size_cell(60, 71, 0.5, 2.0), size_cell(72, 83, 2.0, 2.0)];
        let (output, _, _) = frame(&ctx, Vec::new(), CELLS_H, |ui| {
            size_cells(ui, span, &cells, None, None, Edges::Fixed)
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
        let lane = |ui: &mut egui::Ui| size_cells(ui, span, &cells, None, None, Edges::Both);
        let (_, rect, _) = frame(&ctx, Vec::new(), CELLS_H, lane);
        let at = egui::pos2(
            (span.x_of(rect, 60) + span.x_after(rect, 71)) / 2.0,
            rect.center().y,
        );
        let (_, _, picked) = frame(&ctx, press(at), CELLS_H, lane);
        assert_eq!(picked, Some(BandAct::Pick(1)));
    }

    #[test]
    fn a_dragged_root_boundary_moves_keys_from_one_root_to_the_next() {
        let span = Span { low: 48, high: 95 };
        let bounds = [(48, 59), (60, 71), (72, 95)];

        let up = boundary(&bounds, span, 1, Edge::Top, 77);
        assert_eq!((up[1], up[2]), ((60, 77), (78, 95)));
        assert_eq!(up[0], bounds[0], "and the root below is left alone");

        let down = boundary(&bounds, span, 1, Edge::Low, 55);
        assert_eq!((down[0], down[1]), ((48, 54), (55, 71)));

        // Neither root may be left without a key.
        assert_eq!(boundary(&bounds, span, 1, Edge::Top, 127)[2], (95, 95));
        assert_eq!(boundary(&bounds, span, 1, Edge::Low, 0)[0], (48, 48));
        assert_eq!(boundary(&bounds, span, 1, Edge::Low, 127)[1], (71, 71));

        // The outer ends have no root to share with: they cover and uncover keys.
        assert_eq!(boundary(&bounds, span, 0, Edge::Low, 48), bounds);
        assert_eq!(boundary(&bounds, span, 2, Edge::Top, 90)[2], (72, 90));
    }

    fn vel_blocks(of: &[(u8, u8, (u8, u8))]) -> Vec<VelBlock> {
        of.iter()
            .enumerate()
            .map(|(index, (low, top, window))| VelBlock {
                low: *low,
                top: *top,
                window: *window,
                name: format!("Zone {}", index + 1),
                hint: format!("Zone {}", index + 1),
            })
            .collect()
    }

    /// A hole is a key and velocity no zone plays. Two zones over the same keys cover for
    /// each other.
    #[test]
    fn a_velocity_hole_is_what_no_block_over_those_keys_covers() {
        let full = vel_blocks(&[(61, 96, (1, 127)), (24, 60, (1, 127))]);
        assert!(velocity_holes(&full).is_empty());

        let narrow = vel_blocks(&[(61, 96, (1, 64))]);
        assert_eq!(velocity_holes(&narrow), [(0, 65, 127)]);

        // Both ends left open, and the zone is named by the keys the hole silences.
        let middle = vel_blocks(&[(61, 96, (40, 80))]);
        assert_eq!(velocity_holes(&middle), [(0, 1, 39), (0, 81, 127)]);

        // Stacked over the same keys: neither alone covers the field, together they do.
        let stacked = vel_blocks(&[(61, 96, (1, 64)), (61, 96, (65, 127))]);
        assert!(velocity_holes(&stacked).is_empty());

        // Over different keys, neither covers for the other, so each keeps its hole.
        let apart = vel_blocks(&[(61, 96, (1, 64)), (24, 60, (65, 127))]);
        assert_eq!(
            velocity_holes(&apart),
            [(0, 65, 127), (1, 1, 64)],
            "each zone's own uncovered band"
        );
    }

    /// The clamps keep a window from turning inside out.
    #[test]
    fn a_velocity_handle_stops_short_of_its_own_other_end() {
        assert_eq!(vel_clamped((1, 127), VelEdge::Max, 90), (1, 90));
        assert_eq!(vel_clamped((40, 80), VelEdge::Min, 90), (79, 80));
        assert_eq!(vel_clamped((40, 80), VelEdge::Max, 10), (40, 41));
        assert_eq!(vel_clamped((1, 127), VelEdge::Min, 0), (1, 127));
        assert_eq!(vel_clamped((1, 127), VelEdge::Max, 200), (1, 127));
        // An inverted window still reads as the range between its ends.
        assert_eq!(vel_clamped((80, 40), VelEdge::Max, 100), (40, 100));
    }

    /// Clicking a block opens its row, and the field with no handles has nothing a
    /// pointer can move.
    #[test]
    fn a_click_on_a_velocity_block_picks_it() {
        let ctx = dressed();
        let blocks = vel_blocks(&[(61, 96, (1, 127)), (24, 60, (1, 64))]);
        let field = |ui: &mut egui::Ui| velocity(ui, NSMP, &blocks, None, Handles::Fixed);
        let (_, rect, act) = frame(&ctx, Vec::new(), FIELD_H, field);
        assert_eq!(act, None, "nothing is picked without a click");

        let lane = egui::Rect::from_min_max(
            egui::pos2(rect.left() + AXIS_W + AXIS_GAP, rect.top()),
            rect.max,
        );
        let at = egui::pos2(
            (NSMP.x_of(lane, 24) + NSMP.x_after(lane, 60)) / 2.0,
            lane.bottom() - 4.0,
        );
        let (_, _, act) = frame(&ctx, press(at), FIELD_H, field);
        assert_eq!(act, Some(VelocityAct::Pick(1)));
    }

    /// The field's top is the highest velocity and its bottom the lowest, as the axis
    /// labels say.
    #[test]
    fn the_field_reads_velocity_from_the_bottom_up() {
        let rect = egui::Rect::from_min_size(egui::pos2(0.0, 10.0), egui::vec2(200.0, FIELD_H));
        assert_eq!(y_of(rect, f32::from(VELOCITY_LOW)), rect.bottom());
        assert_eq!(y_of(rect, f32::from(VELOCITY_HIGH)), rect.top());
        assert_eq!(velocity_at(rect, rect.bottom()), VELOCITY_LOW);
        assert_eq!(velocity_at(rect, rect.top()), VELOCITY_HIGH);
        assert_eq!(velocity_at(rect, rect.bottom() + 50.0), VELOCITY_LOW);
        assert_eq!(velocity_at(rect, rect.top() - 50.0), VELOCITY_HIGH);
        assert_eq!(velocity_at(rect, y_of(rect, 64.0)), 64);
    }

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
        // A value that rounds to zero reads as +0, never as -0.
        assert_eq!(Scale::Cents(25).format(-0.001), "+0 c");
    }

    /// Every key the pointer crossed this frame comes back with the value it was painted
    /// at.
    #[test]
    fn a_drag_across_the_lane_paints_the_keys_it_crossed() {
        let ctx = dressed();
        let span = Span { low: 60, high: 62 };
        let values = [0.0_f32; 3];
        let painted = [false; 3];
        let lane_of = |ui: &mut egui::Ui| lane(ui, span, &values, &painted, Scale::Db(3.0));

        let (_, rect, drawn) = frame(&ctx, Vec::new(), LANE_H, lane_of);
        assert!(drawn.is_empty(), "nothing is painted without a drag");

        let at = |note: u8, y: f32| egui::pos2(span.center(rect, note), y);
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

        let release = |button| {
            vec![egui::Event::PointerButton {
                pos: start,
                button,
                pressed: false,
                modifiers: egui::Modifiers::NONE,
            }]
        };
        frame(&ctx, release(egui::PointerButton::Primary), LANE_H, lane_of);

        // The pointer crossing the lane on its way to the key it presses paints nothing:
        // a stroke starts where the button goes down.
        let events = vec![
            egui::Event::PointerMoved(at(62, rect.top() + 28.5)),
            egui::Event::PointerMoved(start),
            egui::Event::PointerButton {
                pos: start,
                button: egui::PointerButton::Primary,
                pressed: true,
                modifiers: egui::Modifiers::NONE,
            },
            egui::Event::PointerMoved(at(61, rect.top() + 9.5)),
        ];
        let (_, _, drawn) = frame(&ctx, events, LANE_H, lane_of);
        assert_eq!(drawn, vec![(60, 0.0), (61, 0.5)]);
        frame(&ctx, release(egui::PointerButton::Primary), LANE_H, lane_of);

        // Only the primary button paints. A secondary drag is someone reaching for a
        // menu, not an edit to every key it crosses.
        let events = vec![
            egui::Event::PointerMoved(start),
            egui::Event::PointerButton {
                pos: start,
                button: egui::PointerButton::Secondary,
                pressed: true,
                modifiers: egui::Modifiers::NONE,
            },
            egui::Event::PointerMoved(at(61, rect.top() + 9.5)),
        ];
        let (_, _, drawn) = frame(&ctx, events, LANE_H, lane_of);
        assert!(drawn.is_empty(), "a secondary drag painted {drawn:?}");
    }

    /// Lines that reach past a corner must stop at the edge and not cross the band beside
    /// it.
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
