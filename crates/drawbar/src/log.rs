//! The activity log: a bounded record of what the app did, oldest dropped first, and
//! the plain-language line the status strip shows above it.
//!
//! The two are written separately. The log keeps protocol detail (slot numbers, byte
//! counts, device status codes), and the status line keeps a sentence about sounds and
//! places. [`Log::say`] and [`Log::trouble`] write both. The whole log opens in a popover
//! over the status line.

use std::collections::VecDeque;

use eframe::egui;

use crate::app::{bad, caption, tint, warn};
use crate::icon::{painted, sized, Glyph};

/// Entries kept before the oldest is dropped. A session that opens a hundred files
/// still fits; a runaway loop cannot grow the app without bound.
const CAPACITY: usize = 500;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Level {
    Info,
    Warn,
    Error,
}

impl Level {
    pub fn color(self, visuals: &egui::Visuals) -> egui::Color32 {
        match self {
            Level::Info => visuals.weak_text_color(),
            Level::Warn => crate::app::warn(visuals),
            Level::Error => crate::app::bad(visuals),
        }
    }
}

pub struct Entry {
    pub level: Level,
    pub text: String,
    /// Seconds since the app started.
    pub at: f64,
}

pub struct Log {
    entries: VecDeque<Entry>,
    /// egui's frame time, refreshed by [`Log::tick`].
    ///
    /// ⚠️ `std::time::Instant::now()` traps on `wasm32-unknown-unknown`, so the log
    /// cannot read a clock of its own; the timeline is elapsed seconds, not wall time.
    clock: f64,
    /// The sentence the status strip shows when nothing is running.
    status: (Level, String),
}

impl Default for Log {
    fn default() -> Log {
        Log {
            entries: VecDeque::new(),
            clock: 0.0,
            status: (Level::Info, "Ready.".to_string()),
        }
    }
}

impl Log {
    /// Take this frame's time. Call once per frame, before anything that logs.
    pub fn tick(&mut self, ctx: &egui::Context) {
        self.clock = ctx.input(|i| i.time);
    }

    pub fn info(&mut self, text: impl Into<String>) {
        self.push(Level::Info, text);
    }

    pub fn warn(&mut self, text: impl Into<String>) {
        self.push(Level::Warn, text);
    }

    pub fn error(&mut self, text: impl Into<String>) {
        self.push(Level::Error, text);
    }

    /// Say something in the status strip, and record it in the log as well.
    pub fn say(&mut self, text: impl Into<String>) {
        let text = text.into();
        self.status = (Level::Info, text.clone());
        self.push(Level::Info, text);
    }

    /// Like [`Log::say`], for something that went wrong.
    pub fn trouble(&mut self, text: impl Into<String>) {
        let text = text.into();
        self.status = (Level::Error, text.clone());
        self.push(Level::Error, text);
    }

    pub fn status(&self) -> (Level, &str) {
        (self.status.0, self.status.1.as_str())
    }

    fn push(&mut self, level: Level, text: impl Into<String>) {
        if self.entries.len() == CAPACITY {
            self.entries.pop_front();
        }
        self.entries.push_back(Entry {
            level,
            text: text.into(),
            at: self.clock,
        });
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    pub fn iter(&self) -> impl Iterator<Item = &Entry> {
        self.entries.iter()
    }

    pub fn clear(&mut self) {
        self.entries.clear();
    }

    /// The whole log as plain lines, for the clipboard.
    pub fn transcript(&self) -> String {
        written(self.entries.iter())
    }

    /// The newest `most` entries, in the shape [`Log::transcript`] writes.
    pub fn tail(&self, most: usize) -> String {
        written(
            self.entries
                .iter()
                .skip(self.entries.len() - most.min(self.len())),
        )
    }

    /// The newest entry.
    pub fn last(&self) -> Option<&Entry> {
        self.entries.back()
    }

    /// How many entries say something went wrong.
    pub fn problems(&self) -> usize {
        self.entries
            .iter()
            .filter(|entry| entry.level != Level::Info)
            .count()
    }

    /// The popover over the status line: the log, newest first, all of it or only the
    /// problems. Returns whether it stays open: Escape, its close button, or a click
    /// anywhere outside it and `spared` closes it.
    ///
    /// `spared` is the status line, whose own controls toggle the popover; a click there
    /// is theirs to handle.
    pub fn popover(
        &mut self,
        ctx: &egui::Context,
        problems_only: &mut bool,
        spared: egui::Rect,
    ) -> bool {
        let screen = ctx.screen_rect();
        let size = egui::vec2(
            POPOVER.x.min(screen.width() - 20.0),
            POPOVER.y.min(screen.height() - 120.0),
        );
        let at = egui::pos2(screen.left() + 10.0, screen.bottom() - 34.0 - size.y);
        let mut open = true;
        let shown = egui::Area::new(egui::Id::new("activity"))
            .order(egui::Order::Foreground)
            .fixed_pos(at)
            .show(ctx, |ui| {
                let visuals = ui.visuals().clone();
                egui::Frame::new()
                    .fill(visuals.panel_fill)
                    .stroke(visuals.widgets.noninteractive.bg_stroke)
                    .corner_radius(12)
                    .shadow(visuals.popup_shadow)
                    .show(ui, |ui| {
                        ui.set_min_size(size);
                        ui.set_max_size(size);
                        open = self.head(ui, problems_only);
                        self.rows(ui, *problems_only);
                    });
            });
        let (escape, clicked) = ctx.input(|input| {
            let outside = input.pointer.any_pressed()
                && input
                    .pointer
                    .interact_pos()
                    .is_some_and(|at| !shown.response.rect.contains(at) && !spared.contains(at));
            (input.key_pressed(egui::Key::Escape), outside)
        });
        open && !escape && !clicked
    }

    /// The popover's header: its title, the filter, copy, clear, and close. Returns false
    /// when close was clicked.
    fn head(&mut self, ui: &mut egui::Ui, problems_only: &mut bool) -> bool {
        let quiet = caption(ui.visuals());
        let mut open = true;
        let (rect, _) =
            ui.allocate_exact_size(egui::vec2(ui.available_width(), 44.0), egui::Sense::hover());
        let mut bar = ui.new_child(
            egui::UiBuilder::new()
                .max_rect(rect.shrink2(egui::vec2(14.0, 0.0)))
                .layout(egui::Layout::left_to_right(egui::Align::Center)),
        );
        bar.spacing_mut().item_spacing.x = 8.0;
        bar.add(sized(Glyph::ScrollText, 15.0, quiet));
        bar.label(
            egui::RichText::new("Activity").font(egui::FontId::new(13.0, crate::app::bold())),
        );
        bar.label(egui::RichText::new("newest first").size(11.5).color(quiet));
        bar.with_layout(egui::Layout::right_to_left(egui::Align::Center), |bar| {
            bar.spacing_mut().item_spacing.x = 4.0;
            crate::panel::flat(bar);
            let button = |glyph| {
                egui::Button::image(sized(glyph, 14.0, quiet))
                    .image_tint_follows_text_color(false)
                    .corner_radius(7.0)
                    .min_size(egui::Vec2::splat(28.0))
            };
            if bar.add(button(Glyph::X)).on_hover_text("close").clicked() {
                open = false;
            }
            if bar
                .add(button(Glyph::Copy))
                .on_hover_text("Copy the whole log, to attach it to a report")
                .clicked()
            {
                bar.ctx().copy_text(self.transcript());
            }
            if bar
                .add(egui::Button::new(egui::RichText::new("Clear").size(11.5)).corner_radius(7.0))
                .on_hover_text("empty the log")
                .clicked()
            {
                self.clear();
            }
            bar.add_space(4.0);
            let problems = format!("Problems · {}", self.problems());
            segments(
                bar,
                problems_only,
                [("All", false), (problems.as_str(), true)],
            );
        });
        open
    }

    /// The entries, newest first.
    fn rows(&self, ui: &mut egui::Ui, problems_only: bool) {
        egui::ScrollArea::vertical()
            .id_salt("activity_rows")
            .auto_shrink([false; 2])
            .show(ui, |ui| {
                ui.spacing_mut().item_spacing.y = 1.0;
                let shown = self
                    .entries
                    .iter()
                    .rev()
                    .filter(|entry| !problems_only || entry.level != Level::Info);
                let mut any = false;
                for entry in shown {
                    any = true;
                    row(ui, entry);
                }
                if !any {
                    ui.add_space(12.0);
                    ui.horizontal(|ui| {
                        ui.add_space(14.0);
                        ui.label(egui::RichText::new("Nothing to show.").italics().weak());
                    });
                }
            });
    }
}

/// The popover's size, before a small window shrinks it.
const POPOVER: egui::Vec2 = egui::vec2(640.0, 360.0);

/// The width of a row's time and of its glyph's column.
const TIME: f32 = 62.0;
const MARK: f32 = 16.0;

/// One entry: when, how it went, and what happened. An error's row is tinted.
fn row(ui: &mut egui::Ui, entry: &Entry) {
    let visuals = ui.visuals().clone();
    let width = ui.available_width() - 16.0;
    let text_left = TIME + MARK + 16.0;
    let galley = ui.painter().layout(
        entry.text.clone(),
        egui::FontId::proportional(12.5),
        visuals.text_color(),
        width - text_left - 10.0,
    );
    let height = (galley.size().y + 8.0).max(26.0);
    let (rect, response) = ui.allocate_exact_size(
        egui::vec2(ui.available_width(), height),
        egui::Sense::hover(),
    );
    let rect = rect.shrink2(egui::vec2(8.0, 0.0));
    let fill = match (entry.level, response.hovered()) {
        (Level::Error, _) => Some(tint(bad(&visuals), 0.09)),
        (_, true) => Some(visuals.widgets.hovered.weak_bg_fill),
        (_, false) => None,
    };
    if let Some(fill) = fill {
        ui.painter().rect_filled(rect, 6.0, fill);
    }
    let top = rect.top() + 13.0;
    ui.painter().text(
        egui::pos2(rect.left() + 6.0, top),
        egui::Align2::LEFT_CENTER,
        format!("{:>7.1}s", entry.at),
        egui::FontId::monospace(11.0),
        visuals.weak_text_color(),
    );
    let (glyph, ink) = match entry.level {
        Level::Info => (Glyph::Info, caption(&visuals)),
        Level::Warn => (Glyph::CircleAlert, warn(&visuals)),
        Level::Error => (Glyph::CircleX, bad(&visuals)),
    };
    let mark = egui::Rect::from_center_size(
        egui::pos2(rect.left() + TIME + MARK / 2.0, top),
        egui::Vec2::splat(13.0),
    );
    painted(ui, glyph, mark, ink);
    ui.painter().galley(
        egui::pos2(rect.left() + text_left, rect.top() + 4.0),
        galley,
        visuals.text_color(),
    );
}

/// A segmented control: one segment per choice, the chosen one raised.
fn segments<const N: usize>(ui: &mut egui::Ui, value: &mut bool, choices: [(&str, bool); N]) {
    let visuals = ui.visuals().clone();
    let font = egui::FontId::proportional(11.5);
    let galleys: Vec<_> = choices
        .iter()
        .map(|(label, _)| {
            ui.painter()
                .layout_no_wrap(label.to_string(), font.clone(), visuals.text_color())
        })
        .collect();
    let width: f32 = galleys
        .iter()
        .map(|galley| galley.size().x + 16.0)
        .sum::<f32>()
        + 4.0;
    let (track, _) = ui.allocate_exact_size(egui::vec2(width, 26.0), egui::Sense::hover());
    ui.painter()
        .rect_filled(track, 8.0, crate::app::canvas(&visuals));
    let mut x = track.left() + 2.0;
    for ((_, choice), galley) in choices.iter().zip(galleys) {
        let segment = egui::Rect::from_min_size(
            egui::pos2(x, track.top() + 2.0),
            egui::vec2(galley.size().x + 16.0, 22.0),
        );
        let response = ui.interact(
            segment,
            ui.id().with(("segment", *choice)),
            egui::Sense::click(),
        );
        let on = *value == *choice;
        if on {
            ui.painter().rect_filled(segment, 6.0, visuals.panel_fill);
        }
        let ink = match on || response.hovered() {
            true => visuals.widgets.active.fg_stroke.color,
            false => caption(&visuals),
        };
        ui.painter()
            .galley(segment.center() - galley.size() / 2.0, galley, ink);
        if response.clicked() {
            *value = *choice;
        }
        x = segment.right();
    }
}

fn written<'a>(entries: impl Iterator<Item = &'a Entry>) -> String {
    entries
        .map(|entry| format!("{:>8.1}s  {}\n", entry.at, entry.text))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The strip's line and the log's line are written together, so a user reading only
    /// the strip is never told less than happened.
    #[test]
    fn a_plain_line_reaches_the_strip_and_the_log_alike() {
        let mut log = Log::default();
        assert_eq!(log.status(), (Level::Info, "Ready."));
        log.say("Sent “Africa Split” to Programs 7:4.");
        assert_eq!(log.status().0, Level::Info);
        assert_eq!(
            log.last().unwrap().text,
            "Sent “Africa Split” to Programs 7:4."
        );
        log.trouble("Could not read the instrument.");
        assert_eq!(
            log.status(),
            (Level::Error, "Could not read the instrument.")
        );
        assert_eq!(log.len(), 2);
    }

    /// Detail written straight to the log never displaces the sentence on the strip.
    #[test]
    fn protocol_detail_stays_out_of_the_status_line() {
        let mut log = Log::default();
        log.say("Reading Programs — bank 1…");
        log.info("bank 1: 43 of 50 slots hold something");
        assert_eq!(log.status().1, "Reading Programs — bank 1…");
    }

    #[test]
    fn a_tail_carries_the_newest_entries_and_no_more_than_it_was_asked_for() {
        let mut log = Log::default();
        for n in 0..250 {
            log.info(format!("line {n}"));
        }
        let tail: Vec<_> = log.tail(200).lines().map(str::to_string).collect();
        assert_eq!(tail.len(), 200);
        assert!(tail[0].ends_with("line 50"), "{:?}", tail[0]);
        assert!(tail[199].ends_with("line 249"), "{:?}", tail[199]);
    }

    #[test]
    fn a_tail_of_a_short_log_is_the_whole_log() {
        let mut log = Log::default();
        assert_eq!(log.tail(200), "");
        log.info("only this");
        assert_eq!(log.tail(200), log.transcript());
    }

    #[test]
    fn a_problem_is_anything_said_above_info() {
        let mut log = Log::default();
        log.say("Read Programs.");
        log.info("bank 1: 43 of 50 slots hold something");
        assert_eq!(log.problems(), 0);
        log.warn("one slot answered twice");
        log.trouble("Could not read the instrument.");
        assert_eq!(log.problems(), 2);
    }

    #[test]
    fn the_oldest_entry_is_dropped_once_the_ring_is_full() {
        let mut log = Log::default();
        for n in 0..CAPACITY + 10 {
            log.info(format!("line {n}"));
        }
        assert_eq!(log.len(), CAPACITY);
        assert_eq!(log.iter().next().unwrap().text, "line 10");
        assert_eq!(log.last().unwrap().text, format!("line {}", CAPACITY + 9));
    }
}
