//! Work that outlives the frame that asked for it: coding a piano library, or laying
//! one out again from a plan.
//!
//! ⚠️ wasm has one thread. There the work runs where it is asked for and the frame
//! waits on it; the caller sees the same [`Job`] either way and polls it the same way.

use std::sync::mpsc::{channel, Receiver};
use std::sync::{Arc, Mutex};

use eframe::egui;

/// The last thing a job said about where it is.
#[derive(Clone, Default)]
pub struct Progress(Arc<Mutex<String>>);

impl Progress {
    pub fn say(&self, words: impl Into<String>) {
        if let Ok(mut held) = self.0.lock() {
            *held = words.into();
        }
    }

    pub fn said(&self) -> String {
        self.0.lock().map(|held| held.clone()).unwrap_or_default()
    }
}

/// One piece of work in flight, answering once.
pub struct Job<T> {
    rx: Receiver<T>,
    progress: Progress,
}

impl<T> Job<T> {
    /// The answer, the first time it is there; `None` while the work runs and forever
    /// after the answer has been taken.
    pub fn poll(&self) -> Option<T> {
        self.rx.try_recv().ok()
    }

    pub fn progress(&self) -> String {
        self.progress.said()
    }
}

/// Start `work`, and ask for a repaint when it answers.
#[cfg(not(target_arch = "wasm32"))]
pub fn run<T: Send + 'static>(
    ctx: &egui::Context,
    work: impl FnOnce(&Progress) -> T + Send + 'static,
) -> Job<T> {
    let (tx, rx) = channel();
    let progress = Progress::default();
    let reported = progress.clone();
    let ctx = ctx.clone();
    std::thread::spawn(move || {
        let _ = tx.send(work(&reported));
        ctx.request_repaint();
    });
    Job { rx, progress }
}

#[cfg(target_arch = "wasm32")]
pub fn run<T: Send + 'static>(
    _ctx: &egui::Context,
    work: impl FnOnce(&Progress) -> T + Send + 'static,
) -> Job<T> {
    let (tx, rx) = channel();
    let progress = Progress::default();
    let _ = tx.send(work(&progress));
    Job { rx, progress }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_job_answers_once() {
        let job = run(&egui::Context::default(), |_| 7);
        let answer = loop {
            if let Some(answer) = job.poll() {
                break answer;
            }
            std::thread::yield_now();
        };
        assert_eq!(answer, 7);
        assert_eq!(job.poll(), None, "an answer is taken once");
    }

    #[test]
    fn progress_reads_back_the_last_thing_said() {
        let progress = Progress::default();
        assert_eq!(progress.said(), "");
        progress.say("resampling 1 of 3");
        progress.say("coding 3 strokes");
        assert_eq!(progress.said(), "coding 3 strokes");
    }
}
