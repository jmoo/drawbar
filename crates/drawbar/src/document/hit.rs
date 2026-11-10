//! The picked hit, close up: its own stretch of the take, zoomed down to 50 ms, with each
//! cut point the file marks for it drawn as a marker over its regions.
//!
//! The markers show where the file cuts; they are not yet dragged.

use std::ops::Range;

use eframe::egui;
use nord_format::wav::Audio;

use super::take::{self, Drawn, Hit, Marked, Part, Summary};
use crate::app;

/// The shortest span the lane zooms to, in seconds.
const NARROWEST: f64 = 0.05;

/// The height of each channel's band in the lane, and the least height of the lane.
const BAND: f32 = 100.0;
const LEAST: f32 = 150.0;

/// The width of a marker's line and the side of its handle.
const LINE: f32 = 2.0;
const HANDLE: f32 = 10.0;

/// How near a marker the pointer must be to name it, in points.
const NEAR: f32 = 6.0;

/// A cut point of a hit.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Cut {
    AttackStart,
    LoopStart,
    LoopEnd,
    /// Where a dying attack's fade to silence starts.
    FadeFrom,
    AttackEnd,
    ReleaseStart,
    ReleaseEnd,
}

impl Cut {
    pub fn name(self) -> &'static str {
        match self {
            Cut::AttackStart => "attack start",
            Cut::LoopStart => "loop start",
            Cut::LoopEnd => "loop end",
            Cut::FadeFrom => "fade from",
            Cut::AttackEnd => "attack end",
            Cut::ReleaseStart => "release start",
            Cut::ReleaseEnd => "release end",
        }
    }

    fn ink(self, visuals: &egui::Visuals) -> egui::Color32 {
        match self {
            Cut::AttackStart | Cut::AttackEnd => take::ink(Part::Attack, visuals),
            Cut::LoopStart | Cut::LoopEnd => app::accent(visuals),
            Cut::FadeFrom => take::ink(Part::Fade, visuals),
            Cut::ReleaseStart | Cut::ReleaseEnd => take::ink(Part::Release, visuals),
        }
    }
}

/// A hit's strokes as the file cuts them, each over its frames.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Cuts {
    pub name: String,
    pub attack: Option<Range<u32>>,
    pub release: Option<Range<u32>>,
    pub fade: Option<Range<u32>>,
    /// The loop inside the attack, its end included.
    pub looped: Option<Range<u32>>,
}

impl Cuts {
    pub fn of(marked: &Marked, hit: Hit) -> Cuts {
        let mut cuts = Cuts::default();
        for mark in marked.regions.iter().filter(|mark| mark.hit == Some(hit)) {
            let span = Some(mark.frames.clone());
            match mark.part {
                Part::Attack | Part::Resonance if cuts.attack.is_none() => {
                    cuts.name = mark.text.clone();
                    cuts.attack = span;
                }
                Part::Release if cuts.release.is_none() => cuts.release = span,
                Part::Fade if cuts.fade.is_none() => cuts.fade = span,
                _ => {}
            }
        }
        cuts.looped = cuts.attack.as_ref().and_then(|attack| {
            marked
                .loops
                .iter()
                .find(|found| found.start >= attack.start && found.end <= attack.end)
                .cloned()
        });
        cuts
    }

    /// Each cut point and the frame it sits at, in take order.
    pub fn points(&self) -> Vec<(Cut, u32)> {
        let mut points = Vec::new();
        let mut both = |span: &Option<Range<u32>>, from: Cut, to: Cut| {
            if let Some(span) = span {
                points.push((from, span.start));
                points.push((to, span.end));
            }
        };
        both(&self.attack, Cut::AttackStart, Cut::AttackEnd);
        both(&self.looped, Cut::LoopStart, Cut::LoopEnd);
        both(&self.release, Cut::ReleaseStart, Cut::ReleaseEnd);
        if let Some(fade) = &self.fade {
            points.push((Cut::FadeFrom, fade.start));
        }
        points.sort_by_key(|(_, at)| *at);
        points
    }

    /// The frames from the first cut to the last.
    fn extent(&self) -> Option<Range<u32>> {
        let points = self.points();
        Some(points.first()?.1..points.last()?.1)
    }
}

/// What the hit's lane shows: the hit it was opened on, and the frames in view, or the
/// whole hit.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Close {
    hit: Option<Hit>,
    span: Option<Range<f64>>,
}

impl Close {
    /// The whole hit, with a margin of 50 ms or 10% each side, within the take.
    fn whole(cuts: &Cuts, total: f64, rate: f64) -> Range<f64> {
        let Some(extent) = cuts.extent() else {
            return 0.0..total;
        };
        let (from, to) = (f64::from(extent.start), f64::from(extent.end));
        let margin = ((to - from) * 0.1).max(NARROWEST * rate);
        (from - margin).max(0.0)..(to + margin).min(total)
    }

    pub fn shown(&self, cuts: &Cuts, total: f64, rate: f64) -> Range<f64> {
        self.span
            .clone()
            .unwrap_or_else(|| Close::whole(cuts, total, rate))
    }

    /// Show `from` to `to`, held to the take and to the narrowest span.
    fn show(&mut self, from: f64, to: f64, total: f64, rate: f64) {
        let span = (to - from).clamp((NARROWEST * rate).min(total), total);
        let from = from.clamp(0.0, total - span);
        self.span = Some(from..from + span);
    }

    fn zoom(&mut self, factor: f64, cuts: &Cuts, total: f64, rate: f64) {
        let shown = self.shown(cuts, total, rate);
        let middle = (shown.start + shown.end) / 2.0;
        let half = (shown.end - shown.start) * factor / 2.0;
        self.show(middle - half, middle + half, total, rate);
    }

    /// Show the loop, with half its length each side, and at least 20 ms.
    fn fit_loop(&mut self, cuts: &Cuts, total: f64, rate: f64) {
        if let Some(found) = &cuts.looped {
            let (from, to) = (f64::from(found.start), f64::from(found.end));
            let margin = ((to - from) * 0.5).max(0.02 * rate);
            self.show(from - margin, to + margin, total, rate);
        }
    }
}

/// The label of the button that shows the loop, and of the one that shows the whole hit.
pub const LOOP: &str = "Loop";
pub const WHOLE: &str = "Whole hit";

/// Draw the picked hit's lane: its caption, legend and zoom, the waveform with its
/// regions and cut points over it, and the times under it. Nothing where the take marks
/// no regions; a prompt where it does and none is picked.
#[allow(clippy::too_many_arguments)]
pub fn ui(
    ui: &mut egui::Ui,
    audio: &Audio,
    summary: &Summary,
    drawn: &mut Drawn,
    marked: &Result<Marked, String>,
    picked: Option<Hit>,
    close: &mut Close,
    playing: bool,
) {
    let Ok(marks) = marked else {
        return;
    };
    if marks.regions.is_empty() {
        return;
    }
    if close.hit != picked {
        *close = Close {
            hit: picked,
            span: None,
        };
    }
    let Some(hit) = picked else {
        ui.horizontal(|ui| {
            ui.label(crate::panel::caps("Hit"));
            ui.label(
                egui::RichText::new("pick a hit in the take")
                    .size(take::LEGEND_TEXT)
                    .color(app::caption(ui.visuals())),
            );
        });
        return;
    };
    let rate = f64::from(audio.rate.max(1));
    let channels = usize::from(audio.channels.max(1));
    let total = (audio.samples.len() / channels) as f64;
    let cuts = Cuts::of(marks, hit);
    let shown = close.shown(&cuts, total, rate);

    ui.horizontal_wrapped(|ui| {
        ui.label(crate::panel::caps("Hit"));
        ui.label(egui::RichText::new(&cuts.name).monospace());
        let places = take::decimals(take::step((shown.end - shown.start) / rate)) + 1;
        ui.label(
            egui::RichText::new(format!(
                "{} – {}",
                take::clock(shown.start / rate, places),
                take::clock(shown.end / rate, places)
            ))
            .size(take::LEGEND_TEXT)
            .color(app::caption(ui.visuals())),
        );
        let visuals = ui.visuals().clone();
        let spans = [
            (cuts.attack.is_some(), Part::Attack, "attack"),
            (cuts.release.is_some(), Part::Release, "release"),
            (cuts.fade.is_some(), Part::Fade, "fade out"),
        ];
        for (_, part, word) in spans.into_iter().filter(|(held, ..)| *held) {
            let edge = take::ink(part, &visuals);
            take::legend(ui, edge.gamma_multiply(0.18), edge, word);
        }
        if cuts.looped.is_some() {
            let edge = app::accent(&visuals);
            take::legend(ui, edge.gamma_multiply(0.18), edge, "loop");
        }
        ui.add_space(8.0);
        if ui.button(take::ZOOM_IN).clicked() {
            close.zoom(0.5, &cuts, total, rate);
        }
        if ui.button(take::ZOOM_OUT).clicked() {
            close.zoom(2.0, &cuts, total, rate);
        }
        if ui
            .add_enabled(cuts.looped.is_some(), egui::Button::new(LOOP))
            .clicked()
        {
            close.fit_loop(&cuts, total, rate);
        }
        if ui
            .add_enabled(close.span.is_some(), egui::Button::new(WHOLE))
            .clicked()
        {
            close.span = None;
        }
    });

    let shown = close.shown(&cuts, total, rate);
    let width = ui.available_width().max(64.0);
    let height = (BAND * channels as f32).max(LEAST);
    let (rect, response) =
        ui.allocate_exact_size(egui::vec2(width, height), egui::Sense::click_and_drag());
    let span = (shown.end - shown.start).max(1.0);
    let x = |frame: f64| rect.left() + ((frame - shown.start) / span) as f32 * rect.width();
    if response.dragged() && response.drag_delta().x != 0.0 {
        let shift = -f64::from(response.drag_delta().x / rect.width()) * span;
        close.show(shown.start + shift, shown.end + shift, total, rate);
    }
    let points = cuts.points();
    let near = response.hover_pos().and_then(|at| {
        points
            .iter()
            .map(|(cut, frame)| (cut, frame, (x(f64::from(*frame)) - at.x).abs()))
            .filter(|(.., off)| *off <= NEAR)
            .min_by(|a, b| a.2.total_cmp(&b.2))
    });
    if let Some((cut, frame, _)) = near {
        let said = format!(
            "{} {}",
            cut.name(),
            take::clock(f64::from(*frame) / rate, 3)
        );
        response.clone().on_hover_text_at_pointer(said);
    }

    let visuals = ui.visuals().clone();
    let painter = ui.painter_at(rect);
    painter.rect_filled(rect, 2.0, visuals.extreme_bg_color);
    let spanned = |span: &Range<u32>, top: f32, ink: egui::Color32| {
        let left = x(f64::from(span.start));
        let right = x(f64::from(span.end)).max(left + 2.0);
        let at = egui::Rect::from_min_max(egui::pos2(left, top), egui::pos2(right, rect.bottom()));
        painter.rect_filled(at, 0.0, ink.gamma_multiply(0.18));
    };
    if let Some(attack) = &cuts.attack {
        spanned(attack, rect.top(), take::ink(Part::Attack, &visuals));
    }
    if let Some(found) = &cuts.looped {
        spanned(found, rect.top(), app::accent(&visuals));
    }
    if let Some(release) = &cuts.release {
        spanned(release, rect.top(), take::ink(Part::Release, &visuals));
    }
    if let Some(fade) = &cuts.fade {
        spanned(fade, rect.center().y, take::ink(Part::Fade, &visuals));
    }

    let columns = (rect.width() * ui.ctx().pixels_per_point()).round() as usize;
    let frames = shown.start.floor() as usize..shown.end.ceil() as usize;
    take::waves(
        &painter,
        rect,
        drawn.lanes(audio, summary, frames, columns),
        playing,
    );

    for (cut, frame) in &points {
        let at = x(f64::from(*frame));
        let ink = cut.ink(&visuals);
        painter.rect_filled(
            egui::Rect::from_min_max(
                egui::pos2(at - LINE / 2.0, rect.top()),
                egui::pos2(at + LINE / 2.0, rect.bottom()),
            ),
            0.0,
            ink,
        );
        painter.rect_filled(
            egui::Rect::from_center_size(
                egui::pos2(at, rect.bottom() - 4.0 - HANDLE / 2.0),
                egui::Vec2::splat(HANDLE),
            ),
            3.0,
            ink,
        );
    }
    painter.rect_stroke(
        rect,
        2.0,
        egui::Stroke::new(1.0_f32, visuals.widgets.noninteractive.bg_stroke.color),
        egui::StrokeKind::Inside,
    );
    take::times(ui, rect, shown, rate);
}

#[cfg(test)]
mod tests {
    use super::*;
    use nord_format::formats::npno::encode::LayerTag;

    fn marked() -> Marked {
        let region = |start: u32, end: u32, label: &str| take::mark(label, start..end);
        Marked {
            regions: vec![
                region(1000, 9000, "060-b0-v22"),
                region(9000, 9500, "060-b2-v22"),
                region(20_000, 30_000, "067-b0-v00"),
                region(26_000, 30_000, "067-b0-v00-fade"),
            ],
            loops: vec![5000..7000, 25_000..25_100],
        }
    }

    #[test]
    fn a_hit_s_cuts_are_its_regions_and_the_loop_in_its_attack() {
        let c4 = Cuts::of(&marked(), (60, LayerTag::Value(22)));
        assert_eq!(c4.name, "C4 v22");
        assert_eq!(c4.looped, Some(5000..7000));
        assert_eq!(
            c4.points(),
            [
                (Cut::AttackStart, 1000),
                (Cut::LoopStart, 5000),
                (Cut::LoopEnd, 7000),
                (Cut::AttackEnd, 9000),
                (Cut::ReleaseStart, 9000),
                (Cut::ReleaseEnd, 9500),
            ]
        );
        let g4 = Cuts::of(&marked(), (67, LayerTag::Value(0)));
        assert_eq!(g4.release, None);
        assert_eq!(g4.fade, Some(26_000..30_000));
        assert!(g4.points().contains(&(Cut::FadeFrom, 26_000)));
        assert_eq!(Cuts::of(&marked(), (61, LayerTag::Value(0))).points(), []);
    }

    #[test]
    fn the_hit_shows_whole_with_a_margin_and_zooms_to_50_ms() {
        let (rate, total) = (1000.0, 100_000.0);
        let cuts = Cuts::of(&marked(), (60, LayerTag::Value(22)));
        let mut close = Close::default();
        // 8.5 s of cuts: 10% is 850 frames each side.
        assert_eq!(close.shown(&cuts, total, rate), 150.0..10_350.0);
        for _ in 0..20 {
            close.zoom(0.5, &cuts, total, rate);
        }
        let shown = close.shown(&cuts, total, rate);
        assert_eq!(shown.end - shown.start, NARROWEST * rate);
        close.fit_loop(&cuts, total, rate);
        assert_eq!(close.shown(&cuts, total, rate), 4000.0..8000.0);
    }
}
