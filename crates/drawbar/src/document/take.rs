//! A take's lane in the WAV document: the waveform with the regions and loops the file
//! marks drawn over it, zoomed and panned in time, and the hit a click picks.
//!
//! A region is read by its label, as `nord piano build` reads it: an attack `060-b0-v22`
//! and its release `060-b2-v22` fill the lane, a loop fills it under the attack's label,
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

/// The height of each channel's band in the lane, and the least height of the lane.
const BAND: f32 = 110.0;
const LEAST: f32 = 110.0;

/// The room a loop leaves at the top of the lane, so the label of its attack shows.
const LABEL_ROW: f32 = 14.0;

/// The height of the row of times under the lane, and of the scroll bar.
const TICKS: f32 = 14.0;
const SCROLL: f32 = 12.0;

/// The text of a region's label, a time and a legend entry.
const SMALL_TEXT: f32 = 10.0;
const LEGEND_TEXT: f32 = 11.0;

/// The frames a summary block covers. A view wider than this per column is drawn from
/// the blocks, so the whole take draws without touching each of its samples.
const BLOCK: usize = 256;

/// The suffix a dying attack's fade region carries after its stroke's name.
const FADE_SUFFIX: &str = "-fade";

/// One channel's envelope: the lowest and highest level in each column.
pub type Lane = Vec<(f32, f32)>;

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
fn mark(label: &str, frames: Range<u32>) -> Mark {
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
        Ok(Marked {
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
        })
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
            counted(hits.len(), "hit"),
            counted(self.regions.len(), "region"),
            counted(self.loops.len(), "loop")
        )
    }

    /// The frames from the first of `hit`'s regions to the end of its last.
    fn extent(&self, hit: Hit) -> Option<Range<u32>> {
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

/// Each channel's lowest and highest level in every [`BLOCK`] frames of the take, read
/// once, so a wide view draws from these and not from every sample.
#[derive(Default)]
pub struct Summary {
    blocks: Vec<Lane>,
}

impl Summary {
    pub fn of(audio: &Audio) -> Summary {
        let channels = usize::from(audio.channels.max(1));
        let mut blocks = vec![Vec::new(); channels];
        for frames in audio.samples.chunks(BLOCK * channels) {
            for (channel, lane) in blocks.iter_mut().enumerate() {
                let (low, high) = frames
                    .iter()
                    .skip(channel)
                    .step_by(channels)
                    .fold((f32::INFINITY, f32::NEG_INFINITY), |(low, high), &s| {
                        (low.min(s), high.max(s))
                    });
                lane.push(match low <= high {
                    true => (low, high),
                    false => (0.0, 0.0),
                });
            }
        }
        Summary { blocks }
    }
}

/// Every channel's envelope across `columns` over `frames` of the take, from the
/// summary where a column spans a block or more, and from the samples where it does not.
fn envelopes(audio: &Audio, summary: &Summary, frames: Range<usize>, columns: usize) -> Vec<Lane> {
    let channels = usize::from(audio.channels.max(1));
    let total = audio.samples.len() / channels;
    let frames = frames.start.min(total)..frames.end.min(total);
    let len = frames.len();
    if columns == 0 || len == 0 {
        return vec![Vec::new(); channels];
    }
    // ⚠️ In 64-bit: a long take times the column count overflows a 32-bit `usize`, and
    // wasm is a 32-bit target.
    let edge =
        |column: usize| frames.start + (column as u64 * len as u64 / columns as u64) as usize;
    let wide = len / columns >= BLOCK;
    let fold = |levels: &mut dyn Iterator<Item = (f32, f32)>| {
        let (low, high) = levels.fold((f32::INFINITY, f32::NEG_INFINITY), |(low, high), (l, h)| {
            (low.min(l), high.max(h))
        });
        match low <= high {
            true => (low, high),
            false => (0.0, 0.0),
        }
    };
    (0..channels)
        .map(|channel| {
            (0..columns)
                .map(|column| {
                    let from = edge(column);
                    let to = edge(column + 1).max(from + 1).min(total);
                    match wide {
                        true => {
                            let blocks = &summary.blocks[channel];
                            let last = ((to - 1) / BLOCK).min(blocks.len().saturating_sub(1));
                            fold(&mut blocks[from / BLOCK..=last].iter().copied())
                        }
                        false => fold(
                            &mut audio.samples[from * channels..to * channels]
                                .iter()
                                .skip(channel)
                                .step_by(channels)
                                .map(|&s| (s, s)),
                        ),
                    }
                })
                .collect()
        })
        .collect()
}

/// The envelopes last drawn, and the columns and frames they were taken over.
#[derive(Default)]
pub struct Drawn {
    key: Option<(usize, Range<usize>)>,
    lanes: Vec<Lane>,
}

impl Drawn {
    fn lanes(
        &mut self,
        audio: &Audio,
        summary: &Summary,
        frames: Range<usize>,
        columns: usize,
    ) -> &[Lane] {
        let key = (columns, frames.clone());
        if self.key.as_ref() != Some(&key) {
            self.lanes = envelopes(audio, summary, frames, columns);
            self.key = Some(key);
        }
        &self.lanes
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

    /// Show `factor` times the span in view, about its middle.
    fn zoom(&mut self, factor: f64, total: f64, rate: f64) {
        let shown = self.shown(total);
        let middle = (shown.start + shown.end) / 2.0;
        let half = (shown.end - shown.start) * factor / 2.0;
        self.show(middle - half, middle + half, total, rate);
    }

    /// Show the picked hit's regions, with a margin of a second or 15% each side.
    fn fit(&mut self, marked: &Marked, total: f64, rate: f64) {
        let Some(extent) = self.picked.and_then(|hit| marked.extent(hit)) else {
            return;
        };
        let (from, to) = (f64::from(extent.start), f64::from(extent.end));
        let margin = ((to - from) * 0.15).max(rate);
        self.show(from - margin, to + margin, total, rate);
    }
}

/// `m:ss`, or `m:ss.s` to `decimals`.
fn clock(seconds: f64, decimals: usize) -> String {
    let minutes = (seconds / 60.0).floor();
    let rest = seconds - minutes * 60.0;
    let width = match decimals {
        0 => 2,
        _ => 3 + decimals,
    };
    format!("{minutes:.0}:{rest:0width$.decimals$}")
}

/// The step between times under a lane showing `span` seconds: at most 8 of them.
fn step(span: f64) -> f64 {
    const STEPS: [f64; 14] = [
        0.01, 0.02, 0.05, 0.1, 0.2, 0.5, 1.0, 2.0, 5.0, 10.0, 15.0, 30.0, 60.0, 120.0,
    ];
    STEPS
        .into_iter()
        .find(|step| span / step <= 8.0)
        .unwrap_or(120.0)
}

/// The decimals a time on a lane of `step` seconds needs.
fn decimals(step: f64) -> usize {
    match step {
        s if s >= 1.0 => 0,
        s if s >= 0.1 => 1,
        _ => 2,
    }
}

/// The ink a part is drawn in.
fn ink(part: Part, visuals: &egui::Visuals) -> egui::Color32 {
    match part {
        Part::Attack | Part::Resonance => app::good(visuals),
        Part::Release => app::caption(visuals),
        Part::Fade => app::unlit(visuals),
        Part::Other => app::warn(visuals),
    }
}

/// The lane's label for a part, in the legend.
fn named(part: Part) -> &'static str {
    match part {
        Part::Attack => "attack",
        Part::Resonance => "resonance",
        Part::Release => "release",
        Part::Fade => "fade out",
        Part::Other => "other region",
    }
}

/// A legend entry: a swatch and its word.
fn legend(ui: &mut egui::Ui, fill: egui::Color32, edge: egui::Color32, word: &str) {
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
pub const PICKED: &str = "Selected";
pub const WHOLE: &str = "Whole take";

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
        for part in [
            Part::Attack,
            Part::Resonance,
            Part::Release,
            Part::Fade,
            Part::Other,
        ] {
            if marks.regions.iter().any(|mark| mark.part == part) {
                let edge = ink(part, &visuals);
                legend(ui, edge.gamma_multiply(0.18), edge, named(part));
            }
        }
        if !marks.loops.is_empty() {
            let edge = app::accent(&visuals);
            legend(ui, edge.gamma_multiply(0.18), edge, "loop");
        }
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
            .add_enabled(view.picked.is_some(), egui::Button::new(PICKED))
            .clicked()
        {
            view.fit(marks, total, rate);
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
    let width = ui.available_width().max(64.0);
    let height = (BAND * channels as f32).max(LEAST);
    let (rect, response) =
        ui.allocate_exact_size(egui::vec2(width, height), egui::Sense::click_and_drag());
    let span = (shown.end - shown.start).max(1.0);
    let x = |frame: f64| rect.left() + ((frame - shown.start) / span) as f32 * rect.width();
    let frame_at = |at: f32| shown.start + f64::from((at - rect.left()) / rect.width()) * span;

    // The region under `at`: a lower-half one where `at` is in the lower half, as it is
    // drawn over the attack there.
    let under = |at: egui::Pos2| {
        let frame = frame_at(at.x);
        let lower = at.y > rect.center().y;
        let holds = |mark: &&Mark| {
            f64::from(mark.frames.start) <= frame && frame < f64::from(mark.frames.end)
        };
        let half = |mark: &&Mark| mark.part == Part::Fade;
        let mut found = marks.regions.iter().filter(holds);
        match lower {
            true => found
                .clone()
                .find(half)
                .or_else(|| found.find(|m| !half(m))),
            false => found.find(|m| !half(m)),
        }
    };
    if response.dragged() && response.drag_delta().x != 0.0 {
        let shift = -f64::from(response.drag_delta().x / rect.width()) * span;
        view.show(shown.start + shift, shown.end + shift, total, rate);
    }
    if response.clicked() {
        if let Some(hit) = response
            .interact_pointer_pos()
            .and_then(under)
            .and_then(|m| m.hit)
        {
            view.picked = Some(hit);
        }
    }
    if let Some(mark) = response.hover_pos().and_then(under) {
        response.clone().on_hover_text_at_pointer(&mark.label);
    }

    let visuals = ui.visuals().clone();
    let painter = ui.painter_at(rect);
    painter.rect_filled(rect, 2.0, visuals.extreme_bg_color);
    let in_view = |frames: &Range<u32>| {
        f64::from(frames.end) > shown.start && f64::from(frames.start) < shown.end
    };
    let boxed = |mark: &Mark| {
        let top = match mark.part {
            Part::Fade => rect.center().y,
            _ => rect.top(),
        };
        let left = x(f64::from(mark.frames.start));
        let right = x(f64::from(mark.frames.end)).max(left + 2.0);
        egui::Rect::from_min_max(egui::pos2(left, top), egui::pos2(right, rect.bottom()))
    };
    let picked = |mark: &Mark| mark.hit.is_some() && mark.hit == view.picked;
    let regions: Vec<&Mark> = marks
        .regions
        .iter()
        .filter(|m| in_view(&m.frames))
        .collect();
    for mark in &regions {
        let fill = match picked(mark) {
            true => ink(mark.part, &visuals).gamma_multiply(0.45),
            false => ink(mark.part, &visuals).gamma_multiply(0.18),
        };
        painter.rect_filled(boxed(mark), 2.0, fill);
    }
    let accent = app::accent(&visuals);
    for found in marks.loops.iter().filter(|frames| in_view(frames)) {
        let left = x(f64::from(found.start));
        let right = x(f64::from(found.end)).max(left + 2.0);
        let band = egui::Rect::from_min_max(
            egui::pos2(left, rect.top() + LABEL_ROW),
            egui::pos2(right, rect.bottom()),
        );
        painter.rect_filled(band, 0.0, accent.gamma_multiply(0.18));
        painter.rect_stroke(
            band,
            0.0,
            egui::Stroke::new(1.0_f32, accent),
            egui::StrokeKind::Inside,
        );
    }

    let columns = (rect.width() * ui.ctx().pixels_per_point()).round() as usize;
    let frames = shown.start.floor() as usize..shown.end.ceil() as usize;
    let lanes = drawn.lanes(audio, summary, frames, columns);
    let wave = match playing {
        true => accent,
        false => visuals.text_color(),
    };
    let band = rect.height() / channels as f32;
    for (channel, lane) in lanes.iter().enumerate() {
        let middle = rect.top() + band * (channel as f32 + 0.5);
        painter.hline(
            rect.x_range(),
            middle,
            egui::Stroke::new(1.0_f32, app::unlit(&visuals)),
        );
        if lane.is_empty() {
            continue;
        }
        let column = (rect.width() / lane.len() as f32).max(1.0);
        let half = band / 2.0 - 1.0;
        for (i, (low, high)) in lane.iter().enumerate() {
            let left = rect.left() + rect.width() * i as f32 / lane.len() as f32;
            let top = middle - high.clamp(-1.0, 1.0) * half;
            let bottom = middle - low.clamp(-1.0, 1.0) * half;
            painter.rect_filled(
                egui::Rect::from_min_max(
                    egui::pos2(left, top),
                    egui::pos2(left + column, bottom.max(top + 1.0)),
                ),
                0.0,
                wave,
            );
        }
    }

    for mark in &regions {
        // ⚠️ A picked hit is outlined in the body text color, not the accent: the accent
        // is the loop's.
        let edge = match picked(mark) {
            true => egui::Stroke::new(2.0_f32, visuals.text_color()),
            false => egui::Stroke::new(1.0_f32, ink(mark.part, &visuals)),
        };
        let at = boxed(mark);
        painter.rect_stroke(at, 2.0, edge, egui::StrokeKind::Inside);
        if !mark.text.is_empty() {
            painter.with_clip_rect(at.intersect(rect)).text(
                at.left_top() + egui::vec2(3.0, 2.0),
                egui::Align2::LEFT_TOP,
                &mark.text,
                egui::FontId::monospace(SMALL_TEXT),
                visuals.text_color(),
            );
        }
    }
    painter.rect_stroke(
        rect,
        2.0,
        egui::Stroke::new(1.0_f32, visuals.widgets.noninteractive.bg_stroke.color),
        egui::StrokeKind::Inside,
    );

    // The times under the lane.
    let (row, _) = ui.allocate_exact_size(egui::vec2(width, TICKS), egui::Sense::hover());
    let step = step(span / rate);
    let places = decimals(step);
    let mut tick = (shown.start / rate / step).ceil() * step;
    while tick * rate <= shown.end {
        ui.painter().text(
            egui::pos2(x(tick * rate), row.top()),
            egui::Align2::CENTER_TOP,
            clock(tick, places),
            egui::FontId::proportional(SMALL_TEXT),
            app::caption(&visuals),
        );
        tick += step;
    }

    if view.span.is_some() {
        scroll(ui, view, total, rate);
    }
}

/// The scroll bar under a zoomed lane: drag its thumb, or click beside it to page.
fn scroll(ui: &mut egui::Ui, view: &mut View, total: f64, rate: f64) {
    let width = ui.available_width().max(64.0);
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
    fn selected_shows_the_picked_hit_with_a_margin() {
        let (rate, total) = (1000.0, 100_000.0);
        let marked = Marked {
            regions: vec![
                mark("060-b0-v22", 40_000..44_000),
                mark("060-b2-v22", 44_000..50_000),
            ],
            loops: Vec::new(),
        };
        let mut view = View {
            picked: Some((60, LayerTag::Value(22))),
            ..View::default()
        };
        view.fit(&marked, total, rate);
        // 10 s of regions: 15% is 1.5 s each side.
        assert_eq!(view.shown(total), 38_500.0..51_500.0);
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
        let summary = Summary::of(&audio);
        let frames = 0..BLOCK * 64;
        // 8 blocks a column: the summary's path.
        let wide = envelopes(&audio, &summary, frames.clone(), 8);
        for (channel, lane) in wide.iter().enumerate() {
            let direct = envelopes(&audio, &Summary::default(), frames.clone(), BLOCK * 64);
            for (column, levels) in lane.iter().enumerate() {
                let span = &direct[channel][column * BLOCK * 8..(column + 1) * BLOCK * 8];
                let low = span.iter().map(|l| l.0).fold(f32::INFINITY, f32::min);
                let high = span.iter().map(|l| l.1).fold(f32::NEG_INFINITY, f32::max);
                assert_eq!(*levels, (low, high), "channel {channel}, column {column}");
            }
        }
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
