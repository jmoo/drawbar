//! The activity log: a bounded record of what the app did, oldest dropped first, and
//! the one plain-words line the status strip shows above it.
//!
//! The two are written separately on purpose. The log keeps protocol detail — slot
//! numbers, byte counts, device status codes — and the status line keeps a sentence
//! about sounds and places. [`Log::say`] and [`Log::trouble`] write both.

use std::collections::VecDeque;

use eframe::egui;

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
    /// The one sentence the status strip shows when nothing is running.
    status: (Level, String),
    /// Whether the strip is expanded into the full log.
    pub open: bool,
}

impl Default for Log {
    fn default() -> Log {
        Log {
            entries: VecDeque::new(),
            clock: 0.0,
            status: (Level::Info, "Ready.".to_string()),
            open: false,
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

    /// The same, for something that went wrong.
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

    /// The newest entry, for the collapsed header's one-line summary.
    pub fn last(&self) -> Option<&Entry> {
        self.entries.back()
    }

    pub fn ui(&mut self, ui: &mut egui::Ui) {
        egui::ScrollArea::vertical()
            .auto_shrink([false; 2])
            .stick_to_bottom(true)
            .show(ui, |ui| {
                for entry in &self.entries {
                    ui.horizontal_wrapped(|ui| {
                        ui.spacing_mut().item_spacing.x = 6.0;
                        ui.label(
                            egui::RichText::new(format!("{:>8.1}s", entry.at))
                                .monospace()
                                .weak(),
                        );
                        ui.label(
                            egui::RichText::new(&entry.text)
                                .monospace()
                                .color(entry.level.color(ui.visuals())),
                        );
                    });
                }
            });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The strip's line and the log's line are written together, so an operator reading
    /// only the strip is never told less than happened.
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
