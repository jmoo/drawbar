//! The activity log: a bounded record of what the app did, oldest dropped first, and
//! the plain-language line the status strip shows above it.
//!
//! The log keeps protocol detail (slot numbers, byte counts, device status codes), and
//! the status line a sentence about sounds and places. [`Log::say`] and [`Log::trouble`]
//! write both.

use std::collections::VecDeque;

use eframe::egui;

use crate::app::{bad, caption, tint, warn};
use crate::bring::Asks;
use crate::icon::{painted, sized, Glyph};
use crate::link::{Held, Keyboard};
use crate::queue::{Queue, Waiting};
use crate::strings::{display_name, place};
use crate::workspace::Workspace;

/// Entries kept before the oldest is dropped. A session that opens a hundred files
/// still fits; a runaway loop cannot grow the app without bound.
const CAPACITY: usize = 500;

/// The most bytes one entry keeps for its Undo.
pub const KEPT: usize = 1 << 20;

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
    /// What its Undo button takes back, until it is pressed.
    pub undo: Option<Undo>,
    /// Counts entries from the session's first, so one can be found after older ones go.
    serial: u64,
}

/// What drawbar did on its own, as the one change that takes it back.
#[derive(Debug, PartialEq, Eq)]
pub enum Undo {
    /// Files linked on their own: unlink each, and keep it off that keyboard.
    Unlink { keyboard: Keyboard, ids: Vec<u64> },
    /// Sends queued for drawbar's own reasons: take each entry out of the queue, while it
    /// still waits for the slot it was queued for.
    Unqueue(Vec<Waiting>),
    /// Files brought in from the keyboard, each as it was before.
    Restore(Vec<Restored>),
    /// Links drawbar moved or removed: put each back, unless another file is linked to
    /// one of those slots now.
    Relink {
        keyboard: Keyboard,
        links: Vec<(u64, Held)>,
    },
    /// Files unlinked from a slot another file kept: link each back, in place of that one.
    Reclaim {
        keyboard: Keyboard,
        links: Vec<(u64, Held)>,
    },
}

/// A file as it was before it was brought in, and the link bringing it in left.
#[derive(Debug, PartialEq, Eq)]
pub struct Restored {
    pub id: u64,
    pub bytes: Vec<u8>,
    pub keyboard: Keyboard,
    pub before: Held,
    /// The link it was brought in under. Its base is what the file was saved as.
    pub after: Held,
}

impl Undo {
    /// Take the change back, and say what was taken back. A file that is gone is left
    /// alone; a file brought in is put back only while it is as bringing it in left it.
    /// `None` when every file it names is gone.
    pub fn perform(
        self,
        workspace: &mut Workspace,
        queue: &mut Queue,
        asks: &mut Asks,
        log: &mut Log,
    ) -> Option<String> {
        match self {
            Undo::Unlink { keyboard, mut ids } => {
                ids.retain(|id| here(workspace, *id));
                for id in &ids {
                    workspace.keep_off(*id, &keyboard);
                }
                (!ids.is_empty()).then(|| format!("Unlinked {}.", named(workspace, &ids, "files")))
            }
            Undo::Unqueue(entries) => {
                let still = |&(id, class, at): &Waiting| {
                    queue
                        .entry(id)
                        .is_some_and(|held| (held.class, held.at) == (class, at))
                };
                let ids: Vec<u64> = entries
                    .iter()
                    .filter(|it| still(it))
                    .map(|it| it.0)
                    .collect();
                ids.iter().for_each(|id| queue.forget(*id));
                Some(match ids.is_empty() {
                    true => "Not undone: the queue changed since.".to_string(),
                    false => format!(
                        "Took {} out of the queue.",
                        named(workspace, &ids, "sounds")
                    ),
                })
            }
            Undo::Restore(kept) => restore(kept, workspace, asks, log),
            Undo::Relink {
                keyboard,
                mut links,
            } => {
                links.retain(|(id, _)| here(workspace, *id));
                let ours = |id| links.iter().any(|(it, _)| *it == id);
                let taken = links.iter().find_map(|(_, held)| {
                    let other = workspace.linked_at(&keyboard, held.class(), &held.spot)?;
                    (!ours(other)).then_some((other, held))
                });
                if let Some((other, held)) = taken {
                    let name = named(workspace, &[other], "files");
                    return Some(format!(
                        "Not undone: {name} is linked to {} now.",
                        spot(held)
                    ));
                }
                relink(workspace, &keyboard, links)
            }
            Undo::Reclaim {
                keyboard,
                mut links,
            } => {
                links.retain(|(id, _)| here(workspace, *id));
                relink(workspace, &keyboard, links)
            }
        }
    }

    /// The bytes it keeps.
    pub fn weight(&self) -> usize {
        match self {
            Undo::Restore(kept) => kept.iter().map(|kept| kept.bytes.len()).sum(),
            Undo::Unlink { .. } | Undo::Unqueue(_) | Undo::Relink { .. } | Undo::Reclaim { .. } => {
                0
            }
        }
    }
}

/// Put back each file still as bringing it in left it: saved as what it was brought in as,
/// and linked as bringing it in linked it. It is not brought in again on its own.
fn restore(
    kept: Vec<Restored>,
    workspace: &mut Workspace,
    asks: &mut Asks,
    log: &mut Log,
) -> Option<String> {
    let mut put = Vec::new();
    let mut changed = Vec::new();
    for Restored {
        id,
        bytes,
        keyboard,
        before,
        after,
    } in kept
    {
        let Some(entity) = workspace.get(id) else {
            continue;
        };
        let left = !entity.is_unsaved()
            && entity.saved_print() == Some(after.base)
            && entity.held_on(&keyboard) == Some(&after);
        if !left {
            changed.push(id);
            continue;
        }
        workspace.replace_bytes(id, bytes, log);
        workspace.mark_saved(id);
        workspace.link(id, &keyboard, before);
        asks.undid(id);
        put.push(id);
    }
    let (put, changed) = (
        named(workspace, &put, "sounds"),
        named(workspace, &changed, "sounds"),
    );
    match (put.is_empty(), changed.is_empty()) {
        (true, true) => None,
        (true, false) => Some(format!("Not undone: {changed} changed since.")),
        (false, true) => Some(format!("Put back {put}.")),
        (false, false) => Some(format!("Put back {put}; {changed} changed since.")),
    }
}

fn relink(
    workspace: &mut Workspace,
    keyboard: &Keyboard,
    links: Vec<(u64, Held)>,
) -> Option<String> {
    let ids: Vec<u64> = links.iter().map(|(id, _)| *id).collect();
    for (id, held) in links {
        workspace.link(id, keyboard, held);
    }
    (!ids.is_empty()).then(|| format!("Linked {} again.", named(workspace, &ids, "files")))
}

/// Whether a file is still on this computer.
fn here(workspace: &Workspace, id: u64) -> bool {
    workspace.get(id).is_some_and(|it| it.kept)
}

/// One file by its name, several by their count, none as nothing.
fn named(workspace: &Workspace, ids: &[u64], many: &str) -> String {
    match ids {
        [] => String::new(),
        [id] => format!(
            "“{}”",
            display_name(workspace.get(*id).map_or("", |it| it.name.as_str()))
        ),
        _ => format!("{} {many}", ids.len()),
    }
}

/// The slot a link names, as the user knows it.
fn spot(held: &Held) -> String {
    match held.location() {
        Some(at) => place(held.class(), at),
        None => held.name().unwrap_or_default().to_string(),
    }
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
    /// The serial the next entry takes.
    next: u64,
    /// An Undo pressed in the popover, until taken.
    pressed: Option<Undo>,
}

impl Default for Log {
    fn default() -> Log {
        Log {
            entries: VecDeque::new(),
            clock: 0.0,
            status: (Level::Info, "Ready.".to_string()),
            next: 0,
            pressed: None,
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
            undo: None,
            serial: self.next,
        });
        self.next += 1;
    }

    /// Give the newest entry an Undo, unless it would keep more than [`KEPT`] bytes.
    /// Returns the entry's serial, for [`Log::entry_mut`].
    pub fn offer(&mut self, undo: Undo) -> Option<u64> {
        let entry = self.entries.back_mut()?;
        entry.undo = (undo.weight() <= KEPT).then_some(undo);
        Some(entry.serial)
    }

    /// The entry [`Log::offer`] returned `serial` for, while the log still holds it.
    pub fn entry_mut(&mut self, serial: u64) -> Option<&mut Entry> {
        self.entries.iter_mut().find(|entry| entry.serial == serial)
    }

    /// Take back what the Undo pressed in the popover since the last call undoes, and say
    /// what it took back.
    pub fn undo_pressed(&mut self, workspace: &mut Workspace, queue: &mut Queue, asks: &mut Asks) {
        let Some(undo) = self.pressed.take() else {
            return;
        };
        if let Some(said) = undo.perform(workspace, queue, asks, self) {
            self.say(said);
        }
    }

    /// Press the Undo of the newest entry that says `text`, as a click in the popover
    /// does. Returns whether it had one.
    #[cfg(test)]
    pub fn press(&mut self, text: &str) -> bool {
        let entry = self
            .entries
            .iter_mut()
            .rev()
            .find(|entry| entry.text.contains(text));
        self.pressed = entry.and_then(|entry| entry.undo.take());
        self.pressed.is_some()
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
                        if let Some(pressed) = self.rows(ui, *problems_only) {
                            self.pressed = Some(pressed);
                        }
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
                .on_hover_text("Copy the whole log")
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

    /// The entries, newest first. Returns the Undo pressed, taken off its entry.
    fn rows(&mut self, ui: &mut egui::Ui, problems_only: bool) -> Option<Undo> {
        let mut pressed = None;
        egui::ScrollArea::vertical()
            .id_salt("activity_rows")
            .auto_shrink([false; 2])
            .show(ui, |ui| {
                ui.spacing_mut().item_spacing.y = 1.0;
                let shown = self
                    .entries
                    .iter_mut()
                    .rev()
                    .filter(|entry| !problems_only || entry.level != Level::Info);
                let mut any = false;
                for entry in shown {
                    any = true;
                    if row(ui, entry) {
                        pressed = entry.undo.take();
                    }
                }
                if !any {
                    ui.add_space(12.0);
                    ui.horizontal(|ui| {
                        ui.add_space(14.0);
                        ui.label(egui::RichText::new("Nothing to show.").italics().weak());
                    });
                }
            });
        pressed
    }
}

/// The popover's size, before a small window shrinks it.
const POPOVER: egui::Vec2 = egui::vec2(640.0, 360.0);

/// The width of a row's time and of its glyph's column.
const TIME: f32 = 62.0;
const MARK: f32 = 16.0;

/// The width kept at a row's right for its Undo button.
const UNDO: f32 = 56.0;

/// One entry: when, how it went, and what happened, and its Undo where it has one. An
/// error's row is tinted. Returns whether Undo was pressed.
fn row(ui: &mut egui::Ui, entry: &Entry) -> bool {
    let visuals = ui.visuals().clone();
    let width = ui.available_width() - 16.0;
    let text_left = TIME + MARK + 16.0;
    let undo = match entry.undo {
        Some(_) => UNDO,
        None => 0.0,
    };
    let galley = ui.painter().layout(
        entry.text.clone(),
        egui::FontId::proportional(12.5),
        visuals.text_color(),
        width - text_left - 10.0 - undo,
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
    if entry.undo.is_none() {
        return false;
    }
    let button = egui::Rect::from_center_size(
        egui::pos2(rect.right() - UNDO / 2.0, top),
        egui::vec2(UNDO - 8.0, 22.0),
    );
    let label = egui::RichText::new("Undo").size(11.5);
    ui.put(button, egui::Button::new(label).corner_radius(7.0))
        .clicked()
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
