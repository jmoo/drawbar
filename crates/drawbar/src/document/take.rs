//! A take's lane in the WAV document: the waveform with the regions and loops the file
//! marks drawn over it, zoomed and panned in time, and the hit a click picks.
//!
//! A region is read by its label, as `nord piano build` reads it: an attack `060-b0-v22`
//! and its release `060-b2-v22` fill the lane, a loop fills it too,
//! and a dying attack's fade `060-b0-v22-fade` fills the lower half of the attack's end. A region labelled otherwise is
//! drawn by its label, so nothing the file marks is hidden.

use std::ops::Range;

use eframe::egui;
use nord_format::formats::npno::encode::{parse_stroke_name, LayerTag, Stem};
use nord_format::formats::npno::Bank;
use nord_format::note;
use nord_format::wav::{self, Audio};

use crate::app;

/// The shortest span the lane zooms to, in seconds.
const NARROWEST: f64 = 3.0;

/// The height of the lane.
const HEIGHT: f32 = 70.0;

/// The height of the row of times under the lane, and of the scroll bar.
const TICKS: f32 = 14.0;
const SCROLL: f32 = 12.0;

/// The text of a region's label, a time and a legend entry.
pub(super) const SMALL_TEXT: f32 = 10.0;
pub(super) const LEGEND_TEXT: f32 = 11.0;

/// The frames a summary block covers. A view wider than this per column is drawn from
/// the blocks, so the whole take draws without touching each of its samples.
const BLOCK: usize = 256;

/// The suffix a dying attack's fade region carries after its stroke's name.
const FADE_SUFFIX: &str = "-fade";

/// A lane's peak level in each column, from 0 up.
pub type Lane = Vec<f32>;

/// A hit: the root and layer its strokes are labelled with.
pub type Hit = (u8, LayerTag);

/// What a region of the take is, by its label.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Part {
    Attack,
    Resonance,
    Release,
    /// The end of a dying attack, where the build fades it to silence.
    Fade,
    /// A region whose label names no stroke.
    Other,
}

/// A region as the lane draws it.
#[derive(Clone, Debug, PartialEq)]
pub struct Mark {
    pub frames: Range<u32>,
    pub part: Part,
    pub hit: Option<Hit>,
    /// The label as the file holds it.
    pub label: String,
    /// What the lane writes in it: `C4 v22` on an attack, the label on a region that
    /// names no stroke, and nothing on a release or a fade, which sit under their attack.
    pub text: String,
}

/// A box the take's lane draws: a hit's regions together, or a region that names no
/// stroke.
#[derive(Clone, Debug, PartialEq)]
pub struct Boxed {
    pub frames: Range<u32>,
    pub hit: Option<Hit>,
    pub text: String,
    /// The labels of the regions inside, as the file holds them.
    pub labels: Vec<String>,
}

/// The regions and loops a take's file marks.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Marked {
    pub regions: Vec<Mark>,
    /// Each loop's frames, its end included.
    pub loops: Vec<Range<u32>>,
}

/// `v22` or `l01`, as the label writes the layer.
fn layer(tag: LayerTag) -> String {
    match tag {
        LayerTag::Value(value) => format!("v{value:02}"),
        LayerTag::Index(index) => format!("l{index:02}"),
    }
}

/// The region labelled `label` over `frames`, read as `nord piano build` reads it.
pub(super) fn mark(label: &str, frames: Range<u32>) -> Mark {
    let (stroke, fade) = match label.strip_suffix(FADE_SUFFIX) {
        Some(stroke) => (stroke, true),
        None => (label, false),
    };
    let other = || Mark {
        frames: frames.clone(),
        part: Part::Other,
        hit: None,
        label: label.to_string(),
        text: label.to_string(),
    };
    let Some((root, bank, tag)) = parse_stroke_name(stroke, Stem::None) else {
        return other();
    };
    let (part, text) = match (bank, fade) {
        (Bank::Attack, false) => (Part::Attack, format!("{} {}", note::name(root), layer(tag))),
        (Bank::Resonance, false) => (
            Part::Resonance,
            format!("{} {} b1", note::name(root), layer(tag)),
        ),
        (Bank::Release, false) => (Part::Release, String::new()),
        (Bank::Attack, true) => (Part::Fade, String::new()),
        // ⚠️ The build fades attacks only, and refuses a fade on any other bank.
        (_, true) => return other(),
    };
    Mark {
        frames,
        part,
        hit: Some((root, tag)),
        label: label.to_string(),
        text,
    }
}

impl Marked {
    /// The marks of `bytes`, or why they do not read.
    pub fn of(bytes: &[u8]) -> Result<Marked, String> {
        let marks = wav::read_marks(bytes).map_err(|e| e.to_string())?;
        Ok(Marked::from_marks(&marks))
    }

    /// `marks` as the lanes draw them.
    pub fn from_marks(marks: &wav::Marks) -> Marked {
        Marked {
            regions: marks
                .regions
                .iter()
                .map(|region| {
                    let end = region.start.saturating_add(region.frames);
                    mark(&region.label, region.start..end)
                })
                .collect(),
            loops: marks
                .loops
                .iter()
                .map(|found| found.start..found.end.saturating_add(1))
                .collect(),
        }
    }

    /// What the take holds, for the header: its hits, regions and loops, or that it is
    /// raw, as recorded, with none.
    pub fn stated(&self) -> String {
        if self.regions.is_empty() && self.loops.is_empty() {
            return "raw: no regions".to_string();
        }
        let hits: std::collections::BTreeSet<Hit> =
            self.regions.iter().filter_map(|mark| mark.hit).collect();
        let counted = |n: usize, one: &str| match n {
            1 => format!("1 {one}"),
            _ => format!("{n} {one}s"),
        };
        format!(
            "{} · {} · {}",
            counted(hits.len(), "sample"),
            counted(self.regions.len(), "region"),
            counted(self.loops.len(), "loop")
        )
    }

    /// What the lane draws: a box around each hit's regions, named by its attack, and
    /// one around each region that names no stroke, in take order.
    pub fn boxes(&self) -> Vec<Boxed> {
        let mut boxes: Vec<Boxed> = Vec::new();
        for mark in &self.regions {
            let held = mark
                .hit
                .and_then(|hit| boxes.iter_mut().find(|boxed| boxed.hit == Some(hit)));
            match held {
                Some(boxed) => {
                    boxed.frames = boxed.frames.start.min(mark.frames.start)
                        ..boxed.frames.end.max(mark.frames.end);
                    boxed.labels.push(mark.label.clone());
                    if boxed.text.is_empty() {
                        boxed.text = mark.text.clone();
                    }
                }
                None => boxes.push(Boxed {
                    frames: mark.frames.clone(),
                    hit: mark.hit,
                    text: mark.text.clone(),
                    labels: vec![mark.label.clone()],
                }),
            }
        }
        for boxed in boxes.iter_mut().filter(|boxed| boxed.text.is_empty()) {
            boxed.text = boxed.labels[0].clone();
        }
        boxes.sort_by_key(|boxed| boxed.frames.start);
        boxes
    }

    /// The frames from the first of `hit`'s regions to the end of its last.
    pub(super) fn extent(&self, hit: Hit) -> Option<Range<u32>> {
        let mut spans = self
            .regions
            .iter()
            .filter(|mark| mark.hit == Some(hit))
            .map(|mark| mark.frames.clone());
        let first = spans.next()?;
        Some(spans.fold(first, |all, span| {
            all.start.min(span.start)..all.end.max(span.end)
        }))
    }
}

/// The take's peak level in every [`BLOCK`] frames, louder channel taken, read once, so a
/// wide view draws from these and not from every sample.
#[derive(Default)]
pub struct Summary {
    blocks: Vec<f32>,
}

/// The loudest of `samples`, as a level from 0 up.
fn peak(samples: &[f32]) -> f32 {
    samples.iter().fold(0.0, |peak: f32, s| peak.max(s.abs()))
}

impl Summary {
    pub fn of(audio: &Audio) -> Summary {
        let channels = usize::from(audio.channels.max(1));
        Summary {
            blocks: audio.samples.chunks(BLOCK * channels).map(peak).collect(),
        }
    }

    /// The take's loudest peak, which tops its lanes.
    pub fn loudest(&self) -> f32 {
        self.blocks.iter().copied().fold(0.0, f32::max)
    }
}

/// The take's peak level, louder channel taken, in each of `columns` over `frames`: from
/// the summary where a column spans a block or more, and from the samples where it does
/// not.
///
/// ⚠️ Mono, and the level only: a lane shows how loud the take is where, which is what a
/// cut is placed by, at twice the height a wave drawn about its zero line would have.
pub(super) fn levels(
    audio: &Audio,
    summary: &Summary,
    frames: Range<usize>,
    columns: usize,
) -> Lane {
    let channels = usize::from(audio.channels.max(1));
    let total = audio.samples.len() / channels;
    let frames = frames.start.min(total)..frames.end.min(total);
    let len = frames.len();
    if columns == 0 || len == 0 {
        return Vec::new();
    }
    // ⚠️ In 64-bit: a long take times the column count overflows a 32-bit `usize`, and
    // wasm is a 32-bit target.
    let edge =
        |column: usize| frames.start + (column as u64 * len as u64 / columns as u64) as usize;
    let wide = len / columns >= BLOCK;
    (0..columns)
        .map(|column| {
            let from = edge(column);
            let to = edge(column + 1).max(from + 1).min(total);
            match wide {
                true => {
                    let last = ((to - 1) / BLOCK).min(summary.blocks.len().saturating_sub(1));
                    peak(&summary.blocks[from / BLOCK..=last])
                }
                false => peak(&audio.samples[from * channels..to * channels]),
            }
        })
        .collect()
}

/// The levels last drawn, and the columns and frames they were taken over.
#[derive(Default)]
pub struct Drawn {
    key: Option<(usize, Range<usize>)>,
    lane: Lane,
}

impl Drawn {
    pub(super) fn lane(
        &mut self,
        audio: &Audio,
        summary: &Summary,
        frames: Range<usize>,
        columns: usize,
    ) -> &[f32] {
        let key = (columns, frames.clone());
        if self.key.as_ref() != Some(&key) {
            self.lane = levels(audio, summary, frames, columns);
            self.key = Some(key);
        }
        &self.lane
    }
}
/// What the operator has asked of the lane: the frames in view and the hit picked. It
/// belongs to the document, not to its bytes, so a gain keeps both.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct View {
    /// The frames in view; `None` for the whole take.
    span: Option<Range<f64>>,
    /// The hit picked, by a click on one of its regions.
    pub picked: Option<Hit>,
    /// Where on the scroll bar's thumb it was grabbed, in frames from its start.
    grab: Option<f64>,
    /// How much deeper than [`SPAN_DB`] the lane's scale reaches, in steps.
    pub depth: i8,
}

impl View {
    /// The frames in view of a take of `total` frames.
    pub fn shown(&self, total: f64) -> Range<f64> {
        self.span.clone().unwrap_or(0.0..total)
    }

    /// Show `from` to `to`, held to the take and to the narrowest span, of a take of
    /// `total` frames at `rate`.
    fn show(&mut self, from: f64, to: f64, total: f64, rate: f64) {
        let span = (to - from).clamp((NARROWEST * rate).min(total), total);
        self.span = match span >= total {
            true => None,
            false => {
                let from = from.clamp(0.0, total - span);
                Some(from..from + span)
            }
        };
    }

    /// The sample after the one picked in `marked`, in take order, or before it where
    /// not `forward`: the first or the last where none is picked, and none past either
    /// end.
    pub fn beside(&self, marked: &Marked, forward: bool) -> Option<Hit> {
        let mut hits: Vec<Hit> = marked
            .boxes()
            .iter()
            .filter_map(|boxed| boxed.hit)
            .collect();
        if !forward {
            hits.reverse();
        }
        match self.picked {
            None => hits.first().copied(),
            Some(picked) => hits
                .iter()
                .position(|hit| *hit == picked)
                .and_then(|at| hits.get(at + 1).copied()),
        }
    }

    /// Pick `hit` of `marked`, in a take of `total` frames at `rate`, and bring it into
    /// view, centred, where the lane is zoomed and it is not all shown.
    pub fn pick(&mut self, hit: Hit, marked: &Marked, total: f64, rate: f64) {
        self.picked = Some(hit);
        let (Some(frames), Some(shown)) = (marked.extent(hit), self.span.clone()) else {
            return;
        };
        let (start, end) = (f64::from(frames.start), f64::from(frames.end));
        if start < shown.start || end > shown.end {
            let half = (shown.end - shown.start) / 2.0;
            let middle = (start + end) / 2.0;
            self.show(middle - half, middle + half, total, rate);
        }
    }

    /// Show `factor` times the span in view, about its middle.
    fn zoom(&mut self, factor: f64, total: f64, rate: f64) {
        let shown = self.shown(total);
        let middle = (shown.start + shown.end) / 2.0;
        let half = (shown.end - shown.start) * factor / 2.0;
        self.show(middle - half, middle + half, total, rate);
    }
}

/// `m:ss`, or `m:ss.s` to `decimals`.
pub(super) fn clock(seconds: f64, decimals: usize) -> String {
    let minutes = (seconds / 60.0).floor();
    let rest = seconds - minutes * 60.0;
    let width = match decimals {
        0 => 2,
        _ => 3 + decimals,
    };
    format!("{minutes:.0}:{rest:0width$.decimals$}")
}

/// The step between times under a lane showing `span` seconds: at most 8 of them.
pub(super) fn step(span: f64) -> f64 {
    const STEPS: [f64; 14] = [
        0.01, 0.02, 0.05, 0.1, 0.2, 0.5, 1.0, 2.0, 5.0, 10.0, 15.0, 30.0, 60.0, 120.0,
    ];
    STEPS
        .into_iter()
        .find(|step| span / step <= 8.0)
        .unwrap_or(120.0)
}

/// The decimals a time on a lane of `step` seconds needs.
pub(super) fn decimals(step: f64) -> usize {
    match step {
        s if s >= 1.0 => 0,
        s if s >= 0.1 => 1,
        _ => 2,
    }
}

/// The ink a part is drawn in.
pub(super) fn ink(part: Part, visuals: &egui::Visuals) -> egui::Color32 {
    match part {
        Part::Attack | Part::Resonance => app::good(visuals),
        Part::Release => app::caption(visuals),
        Part::Fade => app::unlit(visuals),
        Part::Other => app::warn(visuals),
    }
}

/// A legend entry: a swatch and its word.
pub(super) fn legend(ui: &mut egui::Ui, fill: egui::Color32, edge: egui::Color32, word: &str) {
    let (rect, _) = ui.allocate_exact_size(egui::Vec2::splat(10.0), egui::Sense::hover());
    ui.painter().rect_filled(rect, 2.0, fill);
    ui.painter().rect_stroke(
        rect,
        2.0,
        egui::Stroke::new(1.0_f32, edge),
        egui::StrokeKind::Inside,
    );
    ui.label(egui::RichText::new(word).size(LEGEND_TEXT));
}

/// The labels of the lane's zoom buttons.
pub const ZOOM_IN: &str = "Zoom in";
pub const ZOOM_OUT: &str = "Zoom out";
pub const WHOLE: &str = "Whole file";

/// Draw the take's lane: its caption, legend and zoom, the waveform with the marks over
/// it, the times under it, and a scroll bar while zoomed.
pub fn ui(
    ui: &mut egui::Ui,
    audio: &Audio,
    summary: &Summary,
    drawn: &mut Drawn,
    marked: &Result<Marked, String>,
    view: &mut View,
    playing: bool,
) {
    let rate = f64::from(audio.rate.max(1));
    let channels = usize::from(audio.channels.max(1));
    let total = (audio.samples.len() / channels) as f64;
    let none = Marked::default();
    let marks = marked.as_ref().unwrap_or(&none);
    let boxes = marks.boxes();
    let shown = view.shown(total);

    ui.horizontal_wrapped(|ui| {
        ui.label(crate::panel::caps("Take"));
        let step = step((shown.end - shown.start) / rate);
        let places = decimals(step);
        ui.label(
            egui::RichText::new(format!(
                "{} – {}",
                clock(shown.start / rate, places),
                clock(shown.end / rate, places)
            ))
            .size(LEGEND_TEXT)
            .color(app::caption(ui.visuals())),
        );
        let visuals = ui.visuals().clone();
        if marked.as_ref().is_ok_and(|marks| marks.regions.is_empty()) {
            ui.label(
                egui::RichText::new("no regions")
                    .size(LEGEND_TEXT)
                    .color(app::caption(&visuals)),
            );
        }
        ui.add_space(8.0);
        if ui.button(ZOOM_IN).clicked() {
            view.zoom(0.5, total, rate);
        }
        if ui
            .add_enabled(view.span.is_some(), egui::Button::new(ZOOM_OUT))
            .clicked()
        {
            view.zoom(2.0, total, rate);
        }
        if ui
            .add_enabled(view.span.is_some(), egui::Button::new(WHOLE))
            .clicked()
        {
            view.span = None;
        }
    });
    if let Err(why) = marked {
        ui.label(
            egui::RichText::new(format!("Its regions do not read: {why}"))
                .color(app::warn(ui.visuals())),
        );
    }

    let shown = view.shown(total);
    let width = lane_width(ui);
    let (rect, response) =
        ui.allocate_exact_size(egui::vec2(width, HEIGHT), egui::Sense::click_and_drag());
    let span = (shown.end - shown.start).max(1.0);
    let x = |frame: f64| rect.left() + ((frame - shown.start) / span) as f32 * rect.width();
    let frame_at = |at: f32| shown.start + f64::from((at - rect.left()) / rect.width()) * span;
    let under = |at: egui::Pos2| {
        let frame = frame_at(at.x);
        boxes.iter().find(|boxed| {
            f64::from(boxed.frames.start) <= frame && frame < f64::from(boxed.frames.end)
        })
    };
    if response.dragged() && response.drag_delta().x != 0.0 {
        let shift = -f64::from(response.drag_delta().x / rect.width()) * span;
        view.show(shown.start + shift, shown.end + shift, total, rate);
    }
    if response.clicked() {
        if let Some(hit) = response
            .interact_pointer_pos()
            .and_then(under)
            .and_then(|boxed| boxed.hit)
        {
            view.picked = Some(hit);
        }
    }
    if let Some(boxed) = response.hover_pos().and_then(under) {
        response
            .clone()
            .on_hover_text_at_pointer(boxed.labels.join("\n"));
    }

    let visuals = ui.visuals().clone();
    let painter = ui.painter_at(rect);
    painter.rect_filled(rect, 2.0, visuals.extreme_bg_color);
    let shown_boxes: Vec<&Boxed> = boxes
        .iter()
        .filter(|boxed| {
            f64::from(boxed.frames.end) > shown.start && f64::from(boxed.frames.start) < shown.end
        })
        .collect();
    let at = |boxed: &Boxed| {
        let left = x(f64::from(boxed.frames.start));
        let right = x(f64::from(boxed.frames.end)).max(left + 2.0);
        egui::Rect::from_min_max(
            egui::pos2(left, rect.top()),
            egui::pos2(right, rect.bottom()),
        )
    };
    let picked = |boxed: &Boxed| boxed.hit.is_some() && boxed.hit == view.picked;
    let part = |boxed: &Boxed| match boxed.hit {
        Some(_) => Part::Attack,
        None => Part::Other,
    };
    for boxed in &shown_boxes {
        let fill = match picked(boxed) {
            true => ink(part(boxed), &visuals).gamma_multiply(0.45),
            false => ink(part(boxed), &visuals).gamma_multiply(0.18),
        };
        painter.rect_filled(at(boxed), 2.0, fill);
    }

    let columns = (rect.width() * ui.ctx().pixels_per_point()).round() as usize;
    let frames = shown.start.floor() as usize..shown.end.ceil() as usize;
    level(
        &painter,
        rect,
        drawn.lane(audio, summary, frames, columns),
        summary.loudest(),
        span_db(view.depth),
        playing,
    );

    for boxed in &shown_boxes {
        // ⚠️ A picked hit is outlined in the body text color, not the accent: the accent
        // is the loop's.
        let edge = match picked(boxed) {
            true => egui::Stroke::new(2.0_f32, visuals.text_color()),
            false => egui::Stroke::new(1.0_f32, ink(part(boxed), &visuals)),
        };
        let at = at(boxed);
        painter.rect_stroke(at, 2.0, edge, egui::StrokeKind::Inside);
        painter.with_clip_rect(at.intersect(rect)).text(
            at.left_top() + egui::vec2(3.0, 2.0),
            egui::Align2::LEFT_TOP,
            &boxed.text,
            egui::FontId::monospace(SMALL_TEXT),
            visuals.text_color(),
        );
    }
    painter.rect_stroke(
        rect,
        2.0,
        egui::Stroke::new(1.0_f32, visuals.widgets.noninteractive.bg_stroke.color),
        egui::StrokeKind::Inside,
    );
    depth(ui, rect, &mut view.depth);

    times(ui, rect, shown.clone(), rate);

    if view.span.is_some() {
        scroll(ui, view, total, rate);
    }
}

/// How far under the take's loudest peak a lane shows, in dB: the baseline, as the
/// mock-up draws it. A release is some 30 dB under its attack, and the take's quiet
/// gaps some 60: under a 60 dB floor from full scale, releases filled a few pixels.
const SPAN_DB: f32 = 72.0;

/// How much one step of a lane's + and − deepens or shallows its scale, in dB.
const DEPTH_STEP_DB: f32 = 24.0;

/// The steps a lane's scale may be drawn at, from [`SPAN_DB`]: 48 to 120 dB.
const DEPTHS: std::ops::RangeInclusive<i8> = -1..=2;

/// What + says, on hover.
pub const DEEPER: &str =
    "Draw the scale deeper, so quiet sound stands taller: the scale, not the gain";

/// What − says, on hover.
pub const SHALLOWER: &str = "Draw the scale shallower: the scale, not the gain";

/// The decibels a lane drawn `depth` steps deep shows under the take's loudest peak.
pub(super) fn span_db(depth: i8) -> f32 {
    SPAN_DB + DEPTH_STEP_DB * f32::from(depth)
}

/// The height a peak of `peak` of full scale stands to, of a lane's room, in a take whose
/// loudest peak is `loudest`: in decibels over `span` dB under that.
fn height(peak: f32, loudest: f32, span: f32) -> f32 {
    match peak > 0.0 && loudest > 0.0 {
        true => ((20.0 * (peak / loudest).log10() + span) / span).clamp(0.0, 1.0),
        false => 0.0,
    }
}

/// The width of the strip right of a lane that holds its + and −.
pub(super) const GUTTER: f32 = 26.0;

/// The width a lane is drawn at in `ui`, leaving its [`GUTTER`].
pub(super) fn lane_width(ui: &egui::Ui) -> f32 {
    (ui.available_width() - GUTTER).max(64.0)
}

/// The + at the top and − at the bottom of the strip right of a lane drawn in `rect`,
/// marking the top and foot of its scale, which draw it deeper and shallower, and the
/// scale it is drawn at between them.
pub(super) fn depth(ui: &mut egui::Ui, rect: egui::Rect, depth: &mut i8) {
    let size = egui::Vec2::splat(18.0);
    let left = rect.right() + (GUTTER - size.x) / 2.0;
    for (top, word, hint, by) in [
        (rect.top(), "+", DEEPER, 1),
        (rect.bottom() - size.y, "−", SHALLOWER, -1),
    ] {
        let at = egui::Rect::from_min_size(egui::pos2(left, top), size);
        let next = *depth + by;
        let pressed = ui
            .scope_builder(egui::UiBuilder::new().max_rect(at), |ui| {
                ui.add_enabled(DEPTHS.contains(&next), egui::Button::new(word).small())
                    .on_hover_text(hint)
                    .clicked()
            })
            .inner;
        if pressed {
            *depth = next;
        }
    }
    ui.painter().text(
        egui::pos2(rect.right() + GUTTER / 2.0, rect.center().y),
        egui::Align2::CENTER_CENTER,
        format!("{:.0}\ndB", span_db(*depth)),
        egui::FontId::proportional(LEGEND_TEXT),
        app::caption(ui.visuals()),
    );
}

/// The take's level across `rect`, rising from a baseline along its bottom on a decibel
/// scale `span` dB deep, topped by the take's `loudest` peak, with a line at every 12 or
/// 24 dB of full scale, named, so a deeper scale is not taken for a gain: in the accent
/// while the file is sounding, and the body text color when it is not.
pub(super) fn level(
    painter: &egui::Painter,
    rect: egui::Rect,
    lane: &[f32],
    loudest: f32,
    span: f32,
    playing: bool,
) {
    let visuals = painter.ctx().style().visuals.clone();
    let ink = match playing {
        true => app::accent(&visuals),
        false => visuals.text_color(),
    };
    let base = rect.bottom() - 1.0;
    painter.hline(
        rect.x_range(),
        base,
        egui::Stroke::new(1.0_f32, app::unlit(&visuals)),
    );
    if lane.is_empty() {
        return;
    }
    let column = (rect.width() / lane.len() as f32).max(1.0);
    let room = rect.height() - 2.0;
    for (i, peak) in lane.iter().enumerate() {
        let left = rect.left() + rect.width() * i as f32 / lane.len() as f32;
        let top = base - height(*peak, loudest, span) * room;
        painter.rect_filled(
            egui::Rect::from_min_max(
                egui::pos2(left, top.min(base - 1.0)),
                egui::pos2(left + column, base),
            ),
            0.0,
            ink,
        );
    }
    if loudest <= 0.0 {
        return;
    }
    let top_db = 20.0 * loudest.log10();
    let step = match span <= SPAN_DB {
        true => 12.0,
        false => 24.0,
    };
    let caption = app::caption(&visuals);
    let mut line = (top_db / step).floor() * step;
    while line > top_db - span {
        let y = base - (line - top_db + span) / span * room;
        painter.hline(
            rect.x_range(),
            y,
            egui::Stroke::new(1.0_f32, caption.gamma_multiply(0.35)),
        );
        painter.text(
            egui::pos2(rect.left() + 3.0, y - 1.0),
            egui::Align2::LEFT_BOTTOM,
            format!("{line:.0} dB"),
            egui::FontId::proportional(LEGEND_TEXT),
            caption,
        );
        line -= step;
    }
}
/// The row of times under a lane drawn in `rect` over `shown` frames at `rate`.
pub(super) fn times(ui: &mut egui::Ui, rect: egui::Rect, shown: Range<f64>, rate: f64) {
    let (row, _) = ui.allocate_exact_size(egui::vec2(rect.width(), TICKS), egui::Sense::hover());
    let span = (shown.end - shown.start).max(1.0);
    let x = |frame: f64| rect.left() + ((frame - shown.start) / span) as f32 * rect.width();
    let step = step(span / rate);
    let places = decimals(step);
    let mut tick = (shown.start / rate / step).ceil() * step;
    while tick * rate <= shown.end {
        ui.painter().text(
            egui::pos2(x(tick * rate), row.top()),
            egui::Align2::CENTER_TOP,
            clock(tick, places),
            egui::FontId::proportional(SMALL_TEXT),
            app::caption(ui.visuals()),
        );
        tick += step;
    }
}

/// The scroll bar under a zoomed lane: drag its thumb, or click beside it to page.
fn scroll(ui: &mut egui::Ui, view: &mut View, total: f64, rate: f64) {
    let width = lane_width(ui);
    let (bar, response) =
        ui.allocate_exact_size(egui::vec2(width, SCROLL), egui::Sense::click_and_drag());
    let shown = view.shown(total);
    let span = shown.end - shown.start;
    let at = |frame: f64| bar.left() + (frame / total) as f32 * bar.width();
    let frame_at = |x: f32| f64::from((x - bar.left()) / bar.width()) * total;
    let left = at(shown.start);
    let thumb = egui::Rect::from_min_max(
        egui::pos2(left, bar.top() + 2.0),
        egui::pos2(at(shown.end).max(left + 24.0), bar.bottom() - 2.0),
    );
    if let Some(pointer) = response.interact_pointer_pos() {
        let frame = frame_at(pointer.x);
        if response.drag_started() || response.clicked() {
            view.grab = (shown.start..=shown.end)
                .contains(&frame)
                .then_some(frame - shown.start);
            if view.grab.is_none() {
                let page = match frame < shown.start {
                    true => -span,
                    false => span,
                };
                view.show(shown.start + page, shown.end + page, total, rate);
            }
        } else if let (true, Some(grab)) = (response.dragged(), view.grab) {
            view.show(frame - grab, frame - grab + span, total, rate);
        }
    }
    if response.drag_stopped() {
        view.grab = None;
    }
    let visuals = ui.visuals();
    ui.painter().rect_filled(bar, 6.0, visuals.faint_bg_color);
    ui.painter().rect_stroke(
        bar,
        6.0,
        egui::Stroke::new(1.0_f32, visuals.widgets.noninteractive.bg_stroke.color),
        egui::StrokeKind::Inside,
    );
    ui.painter()
        .rect_filled(thumb, 4.0, visuals.widgets.hovered.bg_stroke.color);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn frames(mark: &Mark) -> (Part, Option<Hit>, &str) {
        (mark.part, mark.hit, mark.text.as_str())
    }

    #[test]
    fn a_region_is_read_by_its_label_as_the_build_reads_it() {
        let c4 = Some((60, LayerTag::Value(22)));
        assert_eq!(
            frames(&mark("060-b0-v22", 0..10)),
            (Part::Attack, c4, "C4 v22")
        );
        assert_eq!(frames(&mark("060-b2-v22", 0..10)), (Part::Release, c4, ""));
        assert_eq!(
            frames(&mark("060-b0-v22-fade", 0..10)),
            (Part::Fade, c4, "")
        );
        assert_eq!(
            frames(&mark("061-b1-l00", 0..10)),
            (
                Part::Resonance,
                Some((61, LayerTag::Index(0))),
                "C#4 l00 b1"
            )
        );
        for other in [
            "take start",
            "060-b2-v22-fade",
            "060-b0",
            "Grand-060-b0-v22",
        ] {
            assert_eq!(
                frames(&mark(other, 0..10)),
                (Part::Other, None, other),
                "{other}"
            );
        }
    }

    #[test]
    fn a_take_s_marks_read_from_its_file_with_each_loop_s_end_included() {
        let audio = Audio {
            rate: 35_002,
            channels: 1,
            samples: vec![0.0; 1000],
        };
        let bytes = wav::write(&audio, wav::Encoding::Int { bits: 24 }).unwrap();
        let marks = wav::Marks {
            regions: vec![
                wav::Region {
                    start: 100,
                    frames: 400,
                    label: "060-b0-v22".into(),
                },
                wav::Region {
                    start: 500,
                    frames: 200,
                    label: "060-b2-v22".into(),
                },
            ],
            loops: vec![wav::Loop {
                start: 300,
                end: 399,
                play: wav::Play::Forward,
            }],
        };
        let marked = Marked::of(&wav::with_marks(&bytes, &marks).unwrap()).unwrap();
        assert_eq!(marked.regions[0].frames, 100..500);
        assert_eq!(marked.regions[1].part, Part::Release);
        assert_eq!(marked.loops, vec![300..400]);
        assert_eq!(
            marked.extent((60, LayerTag::Value(22))),
            Some(100..700),
            "a hit spans its attack and release"
        );
        assert_eq!(Marked::of(&bytes).unwrap(), Marked::default());
    }

    #[test]
    fn the_view_zooms_about_its_middle_and_stays_within_the_take() {
        let (rate, total) = (1000.0, 100_000.0);
        let mut view = View::default();
        assert_eq!(view.shown(total), 0.0..total);
        view.zoom(0.5, total, rate);
        assert_eq!(view.shown(total), 25_000.0..75_000.0);
        for _ in 0..10 {
            view.zoom(0.5, total, rate);
        }
        let shown = view.shown(total);
        assert_eq!(
            shown.end - shown.start,
            NARROWEST * rate,
            "no narrower than 3 s"
        );
        view.show(-5000.0, 5000.0, total, rate);
        assert_eq!(view.shown(total), 0.0..10_000.0, "held at the start");
        view.show(99_000.0, 109_000.0, total, rate);
        assert_eq!(view.shown(total), 90_000.0..total, "held at the end");
        view.zoom(64.0, total, rate);
        assert_eq!(
            view.span, None,
            "zoomed out past the take is the whole take"
        );
    }

    #[test]
    fn a_wide_view_draws_from_the_summary_as_the_samples_would() {
        let samples: Vec<f32> = (0..BLOCK * 64 * 2)
            .map(|i| ((i * 7919) % 2001) as f32 / 1000.0 - 1.0)
            .collect();
        let audio = Audio {
            rate: 44_100,
            channels: 2,
            samples,
        };
        let frames = 0..BLOCK * 64;
        // 8 blocks a column: the summary's path.
        let wide = levels(&audio, &Summary::of(&audio), frames.clone(), 8);
        let direct = levels(&audio, &Summary::default(), frames, BLOCK * 64);
        for (column, level) in wide.iter().enumerate() {
            let span = &direct[column * BLOCK * 8..(column + 1) * BLOCK * 8];
            assert_eq!(
                *level,
                span.iter().copied().fold(0.0, f32::max),
                "column {column}"
            );
        }
    }

    #[test]
    fn a_lane_stands_in_decibels_under_the_take_s_loudest() {
        let loudest = 0.5;
        let down = |db: f32| loudest * 10f32.powf(-db / 20.0);
        let span = span_db(0);
        assert_eq!(height(loudest, loudest, span), 1.0);
        assert!(
            (height(down(36.0), loudest, span) - 0.5).abs() < 1e-4,
            "36 dB down is half"
        );
        assert_eq!(
            height(down(80.0), loudest, span),
            0.0,
            "80 dB down is under the floor"
        );
        assert_eq!(height(0.0, loudest, span), 0.0);
        assert_eq!(height(0.5, 0.0, span), 0.0, "a silent take draws nothing");
        let deeper = span_db(1);
        assert_eq!(deeper, 96.0);
        assert!(
            (height(down(48.0), loudest, deeper) - 0.5).abs() < 1e-4,
            "deeper, 48 dB down is half"
        );
    }

    #[test]
    fn a_lane_s_level_is_the_louder_channel_s_peak() {
        let audio = Audio {
            rate: 44_100,
            channels: 2,
            samples: vec![0.25, -0.5, -0.75, 0.125],
        };
        assert_eq!(levels(&audio, &Summary::default(), 0..2, 2), [0.5, 0.75]);
    }

    #[test]
    fn the_lane_boxes_each_hit_s_regions_together() {
        let marked = Marked {
            regions: vec![
                mark("060-b0-v22", 100..500),
                mark("060-b2-v22", 495..700),
                mark("intro", 0..50),
                mark("067-b0-v00", 800..1200),
                mark("067-b0-v00-fade", 1000..1200),
            ],
            loops: Vec::new(),
        };
        let boxes = marked.boxes();
        let drawn: Vec<(Range<u32>, &str, usize)> = boxes
            .iter()
            .map(|boxed| {
                (
                    boxed.frames.clone(),
                    boxed.text.as_str(),
                    boxed.labels.len(),
                )
            })
            .collect();
        assert_eq!(
            drawn,
            [
                (0..50, "intro", 1),
                (100..700, "C4 v22", 2),
                (800..1200, "G4 v00", 2),
            ]
        );
    }
    /// The arrows step through the samples in take order, past regions that name none,
    /// and bring a sample out of view into it.
    #[test]
    fn the_arrows_pick_the_samples_either_side_and_follow_them() {
        let marked = Marked {
            regions: vec![
                mark("intro", 0..50),
                mark("060-b0-v22", 100..500),
                mark("067-b0-v00", 300_000..301_000),
            ],
            loops: Vec::new(),
        };
        let (total, rate) = (400_000.0, 44_100.0);
        let mut view = View::default();
        let first = view.beside(&marked, true).unwrap();
        let last = view.beside(&marked, false).unwrap();
        assert_ne!(first, last, "none picked: → the first, ← the last");
        view.show(0.0, 150_000.0, total, rate);
        view.pick(first, &marked, total, rate);
        assert_eq!(view.span, Some(0.0..150_000.0), "in view: the lane stays");
        assert_eq!(
            view.beside(&marked, false),
            None,
            "nothing before the first"
        );
        let second = view.beside(&marked, true).unwrap();
        view.pick(second, &marked, total, rate);
        let shown = view.shown(total);
        assert!(
            shown.start <= 300_000.0 && 301_000.0 <= shown.end,
            "{shown:?}"
        );
        assert_eq!(view.beside(&marked, true), None, "nothing after the last");
    }

    #[test]
    fn times_read_in_minutes_and_seconds_to_the_step() {
        assert_eq!(clock(522.282, 0), "8:42");
        assert_eq!(clock(65.24, 1), "1:05.2");
        assert_eq!(clock(3.0, 2), "0:03.00");
        assert_eq!(step(522.0), 120.0);
        assert_eq!(step(3.0), 0.5);
    }
}
