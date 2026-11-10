//! The picked hit, close up: its own stretch of the take, zoomed down to 50 ms, with each
//! cut point the file marks for it drawn as a marker over its regions.
//!
//! A marker is dragged to move its cut. While it moves both lanes draw the cut where it
//! is; let go, and the region or loop it belongs to is written to the file's marks, one
//! edit a drag. A marker stops at the cuts either side of it, so a loop never leaves its
//! attack and no region turns inside out.

use std::ops::{Range, RangeInclusive};

use eframe::egui;
use nord_format::wav::{self, Audio};

use super::take::{self, Drawn, Hit, Marked, Part, Summary};
use crate::app;

/// The shortest span the lane zooms to, in seconds.
const NARROWEST: f64 = 0.05;

/// The height of the lane.
const HEIGHT: f32 = 160.0;

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

    /// The cut as a set's path names it.
    pub fn slug(self) -> &'static str {
        match self {
            Cut::AttackStart => "attack-start",
            Cut::LoopStart => "loop-start",
            Cut::LoopEnd => "loop-end",
            Cut::FadeFrom => "fade-from",
            Cut::AttackEnd => "attack-end",
            Cut::ReleaseStart => "release-start",
            Cut::ReleaseEnd => "release-end",
        }
    }

    pub fn from_slug(slug: &str) -> Option<Cut> {
        [
            Cut::AttackStart,
            Cut::LoopStart,
            Cut::LoopEnd,
            Cut::FadeFrom,
            Cut::AttackEnd,
            Cut::ReleaseStart,
            Cut::ReleaseEnd,
        ]
        .into_iter()
        .find(|cut| cut.slug() == slug)
    }

    /// Whether the cut ends its span, so a drag from left of two cuts on one frame takes
    /// the one that ends.
    fn ends(self) -> bool {
        matches!(self, Cut::LoopEnd | Cut::AttackEnd | Cut::ReleaseEnd)
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
    /// The attack's label as the file holds it, which a set names the hit by.
    pub label: String,
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
                    cuts.label = mark.label.clone();
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

    /// Where `cut` may sit in a take of `total` frames: between the cuts either side of
    /// it, so each span keeps a frame and the loop two, inside its attack. `None` where
    /// the hit has no such cut.
    pub fn room(&self, cut: Cut, total: u32) -> Option<RangeInclusive<u32>> {
        let attack = self.attack.as_ref();
        let found = self.looped.as_ref();
        let fade = self.fade.as_ref();
        let release = self.release.as_ref();
        let (low, high) = match cut {
            Cut::AttackStart => {
                let attack = attack?;
                let high = [
                    Some(attack.end.saturating_sub(1)),
                    found.map(|found| found.start),
                    fade.map(|fade| fade.start),
                ];
                (0, high.into_iter().flatten().min()?)
            }
            Cut::AttackEnd => {
                let attack = attack?;
                let low = [
                    Some(attack.start + 1),
                    found.map(|found| found.end),
                    fade.map(|fade| fade.start + 1),
                ];
                (low.into_iter().flatten().max()?, total)
            }
            Cut::LoopStart => (attack?.start, found?.end.saturating_sub(2)),
            Cut::LoopEnd => (found?.start + 2, attack?.end),
            Cut::FadeFrom => (attack?.start, fade?.end.saturating_sub(1)),
            Cut::ReleaseStart => (0, release?.end.saturating_sub(1)),
            Cut::ReleaseEnd => (release?.start + 1, total),
        };
        Some(low..=high.min(total))
    }
}

/// `marks` with `hit`'s `cut` moved to `frame`, in a take of `total` frames: its region,
/// or its loop, and an attack's fade with the attack's end. Refused where the hit has no
/// such cut, or `frame` is not in its [`Cuts::room`].
pub fn moved(
    marks: &wav::Marks,
    hit: Hit,
    cut: Cut,
    frame: u32,
    total: u32,
) -> Result<wav::Marks, String> {
    let marked = Marked::from_marks(marks);
    let cuts = Cuts::of(&marked, hit);
    let room = cuts
        .room(cut, total)
        .ok_or_else(|| format!("{}: {} has no {}", cuts.label, cuts.name, cut.name()))?;
    if !room.contains(&frame) {
        return Err(format!(
            "{} {} at frame {frame}: it sits from {} to {}",
            cuts.name,
            cut.name(),
            room.start(),
            room.end()
        ));
    }
    // The first region of each part, as [`Cuts::of`] takes it; the regions are read in
    // the order the file holds them.
    let first = |parts: &[Part]| {
        marked
            .regions
            .iter()
            .position(|mark| mark.hit == Some(hit) && parts.contains(&mark.part))
    };
    let mut out = marks.clone();
    let start = |region: &mut wav::Region, frame: u32| {
        let end = region.start + region.frames;
        region.start = frame;
        region.frames = end - frame;
    };
    let end = |region: &mut wav::Region, frame: u32| region.frames = frame - region.start;
    let attack = first(&[Part::Attack, Part::Resonance]);
    let fade = first(&[Part::Fade]);
    let release = first(&[Part::Release]);
    match cut {
        Cut::AttackStart => start(&mut out.regions[attack.unwrap()], frame),
        Cut::AttackEnd => {
            end(&mut out.regions[attack.unwrap()], frame);
            if let Some(fade) = fade {
                end(&mut out.regions[fade], frame);
            }
        }
        Cut::FadeFrom => start(&mut out.regions[fade.unwrap()], frame),
        Cut::ReleaseStart => start(&mut out.regions[release.unwrap()], frame),
        Cut::ReleaseEnd => end(&mut out.regions[release.unwrap()], frame),
        Cut::LoopStart | Cut::LoopEnd => {
            let found = cuts.looped.as_ref().unwrap();
            let at = marked.loops.iter().position(|each| each == found).unwrap();
            // ⚠️ A loop's end is its last frame, not the one after.
            match cut {
                Cut::LoopStart => out.loops[at].start = frame,
                _ => out.loops[at].end = frame - 1,
            }
        }
    }
    Ok(out)
}

/// What the hit's lane shows: the hit it was opened on, and the frames in view, or the
/// whole hit.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Close {
    hit: Option<Hit>,
    span: Option<Range<f64>>,
    /// The cut being dragged, and the frame it is at.
    dragging: Option<(Cut, u32)>,
}

impl Close {
    /// The cut being dragged on `hit`, and the frame it is at.
    pub fn dragging(&self, hit: Option<Hit>) -> Option<(Cut, u32)> {
        self.dragging.filter(|_| hit.is_some() && hit == self.hit)
    }

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
///
/// `marked` holds the cut being dragged where it is now. The cut and the frame it was let
/// go at, once a drag ends.
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
) -> Option<(Cut, u32)> {
    let Ok(marks) = marked else {
        return None;
    };
    if marks.regions.is_empty() {
        return None;
    }
    if close.hit != picked {
        *close = Close {
            hit: picked,
            ..Close::default()
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
        return None;
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
    let (rect, response) =
        ui.allocate_exact_size(egui::vec2(width, HEIGHT), egui::Sense::click_and_drag());
    let span = (shown.end - shown.start).max(1.0);
    let x = |frame: f64| rect.left() + ((frame - shown.start) / span) as f32 * rect.width();
    let frame_at = |at: f32| shown.start + f64::from((at - rect.left()) / rect.width()) * span;
    let points = cuts.points();
    // The marker nearest `at`, within reach; of two on one frame, the one that ends its
    // span when `at` is left of them, and the one that starts it when right.
    let near = |at: egui::Pos2| {
        points
            .iter()
            .map(|&(cut, frame)| (cut, frame, x(f64::from(frame)) - at.x))
            .filter(|(.., off)| off.abs() <= NEAR)
            .min_by(|a, b| {
                let side = |(cut, _, off): &(Cut, u32, f32)| cut.ends() != (*off > 0.0);
                a.2.abs().total_cmp(&b.2.abs()).then(side(a).cmp(&side(b)))
            })
            .map(|(cut, frame, _)| (cut, frame))
    };
    let frames = total as u32;
    if response.drag_started() {
        let pressed = ui.input(|input| input.pointer.press_origin());
        close.dragging = pressed.and_then(near);
        // ⚠️ The view holds still under a drag: the whole hit's extent moves with the
        // cut, and the view would move with it.
        if close.dragging.is_some() {
            close.span = Some(shown.clone());
        }
    }
    let mut dropped = None;
    if response.dragged() {
        match (close.dragging, response.interact_pointer_pos()) {
            (Some((cut, _)), Some(pointer)) => {
                if let Some(room) = cuts.room(cut, frames) {
                    let frame = frame_at(pointer.x).round().max(0.0) as u32;
                    let frame = frame.max(*room.start()).min(*room.end());
                    close.dragging = Some((cut, frame));
                }
            }
            (None, _) if response.drag_delta().x != 0.0 => {
                let shift = -f64::from(response.drag_delta().x / rect.width()) * span;
                close.show(shown.start + shift, shown.end + shift, total, rate);
            }
            _ => {}
        }
    }
    if response.drag_stopped() {
        dropped = close.dragging.take();
    }
    let held = close
        .dragging
        .or_else(|| response.hover_pos().and_then(near));
    if let Some((cut, frame)) = held {
        ui.ctx().set_cursor_icon(egui::CursorIcon::ResizeHorizontal);
        let said = format!("{} {}", cut.name(), take::clock(f64::from(frame) / rate, 3));
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
    // ⚠️ The loop fills the lower half, so the attack it sits in shows over it.
    if let Some(found) = &cuts.looped {
        spanned(found, rect.center().y, app::accent(&visuals));
    }
    if let Some(release) = &cuts.release {
        spanned(release, rect.top(), take::ink(Part::Release, &visuals));
    }
    if let Some(fade) = &cuts.fade {
        spanned(fade, rect.center().y, take::ink(Part::Fade, &visuals));
    }

    let columns = (rect.width() * ui.ctx().pixels_per_point()).round() as usize;
    let frames = shown.start.floor() as usize..shown.end.ceil() as usize;
    take::level(
        &painter,
        rect,
        drawn.lane(audio, summary, frames, columns),
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
    dropped
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

    /// `marked()` as the file holds it.
    fn marks() -> wav::Marks {
        let region = |start: u32, end: u32, label: &str| wav::Region {
            start,
            frames: end - start,
            label: label.into(),
        };
        let looped = |start: u32, end: u32| wav::Loop {
            start,
            end: end - 1,
            play: wav::Play::Forward,
        };
        wav::Marks {
            regions: vec![
                region(1000, 9000, "060-b0-v22"),
                region(9000, 9500, "060-b2-v22"),
                region(20_000, 30_000, "067-b0-v00"),
                region(26_000, 30_000, "067-b0-v00-fade"),
            ],
            loops: vec![looped(5000, 7000), looped(25_000, 25_100)],
        }
    }

    #[test]
    fn a_cut_stops_at_the_cuts_either_side_of_it() {
        let c4 = Cuts::of(&marked(), (60, LayerTag::Value(22)));
        let total = 100_000;
        assert_eq!(c4.room(Cut::AttackStart, total), Some(0..=5000));
        assert_eq!(c4.room(Cut::LoopStart, total), Some(1000..=6998));
        assert_eq!(c4.room(Cut::LoopEnd, total), Some(5002..=9000));
        assert_eq!(c4.room(Cut::AttackEnd, total), Some(7000..=total));
        assert_eq!(c4.room(Cut::ReleaseStart, total), Some(0..=9499));
        assert_eq!(c4.room(Cut::ReleaseEnd, total), Some(9001..=total));
        assert_eq!(c4.room(Cut::FadeFrom, total), None, "C4 v22 has no fade");
        let g4 = Cuts::of(&marked(), (67, LayerTag::Value(0)));
        assert_eq!(
            g4.room(Cut::AttackEnd, total),
            Some(26_001..=total),
            "past its fade's start, and its loop's end"
        );
        assert_eq!(g4.room(Cut::FadeFrom, total), Some(20_000..=29_999));
    }

    #[test]
    fn a_moved_cut_moves_its_region_or_loop_and_nothing_else() {
        let c4 = (60, LayerTag::Value(22));
        let g4 = (67, LayerTag::Value(0));
        let at = |marks: &wav::Marks, i: usize| {
            let region = &marks.regions[i];
            (region.start, region.start + region.frames)
        };
        let moved = |hit, cut, frame| super::moved(&marks(), hit, cut, frame, 100_000).unwrap();

        let out = moved(c4, Cut::AttackStart, 800);
        assert_eq!(at(&out, 0), (800, 9000));
        assert_eq!(out.regions[1..], marks().regions[1..]);
        assert_eq!(out.loops, marks().loops);

        let out = moved(c4, Cut::LoopEnd, 8000);
        assert_eq!((out.loops[0].start, out.loops[0].end), (5000, 7999));
        assert_eq!(out.regions, marks().regions);

        let out = moved(c4, Cut::ReleaseStart, 8900);
        assert_eq!(at(&out, 1), (8900, 9500));

        let out = moved(g4, Cut::AttackEnd, 31_000);
        assert_eq!(at(&out, 2), (20_000, 31_000));
        assert_eq!(
            at(&out, 3),
            (26_000, 31_000),
            "the fade ends with its attack"
        );

        let out = moved(g4, Cut::FadeFrom, 27_000);
        assert_eq!(at(&out, 3), (27_000, 30_000));
        assert_eq!(at(&out, 2), (20_000, 30_000));

        let past = super::moved(&marks(), c4, Cut::LoopStart, 7500, 100_000);
        assert!(
            past.unwrap_err().contains("loop start"),
            "past the loop's end"
        );
        assert!(super::moved(&marks(), c4, Cut::FadeFrom, 5000, 100_000).is_err());
    }

    #[test]
    fn a_cut_names_itself_in_a_set_s_path() {
        for cut in [
            Cut::AttackStart,
            Cut::LoopStart,
            Cut::LoopEnd,
            Cut::FadeFrom,
            Cut::AttackEnd,
            Cut::ReleaseStart,
            Cut::ReleaseEnd,
        ] {
            assert_eq!(Cut::from_slug(cut.slug()), Some(cut));
        }
        assert_eq!(Cut::from_slug("attack"), None);
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
