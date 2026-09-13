//! Work that outlives the frame that asked for it: coding a piano library, or laying
//! one out again from a plan.
//!
//! ⚠️ wasm has one thread. There the work runs where it is asked for and the frame
//! waits on it; the caller sees the same [`Job`] either way and polls it the same way.

use std::sync::mpsc::{channel, Receiver, TryRecvError};
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

/// Where a job is when it is asked.
#[derive(Debug, PartialEq, Eq)]
pub enum Answer<T> {
    Running,
    Answered(T),
    /// The worker is gone and no answer is coming: it panicked, or its answer has already
    /// been taken.
    Died,
}

/// One piece of work in flight, answering once.
pub struct Job<T> {
    rx: Receiver<T>,
    progress: Progress,
}

impl<T> Job<T> {
    /// Whether the answer is here, still coming, or never coming.
    ///
    /// ⚠️ A job answers once, and the worker goes with its answer. Take the answer and
    /// drop the job: polling the same job again says [`Answer::Died`], which is the
    /// truth about the worker and not about the answer already in hand.
    pub fn poll(&self) -> Answer<T> {
        match self.rx.try_recv() {
            Ok(answer) => Answer::Answered(answer),
            Err(TryRecvError::Empty) => Answer::Running,
            Err(TryRecvError::Disconnected) => Answer::Died,
        }
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

    /// Poll until the job stops saying it is running.
    fn settled<T>(job: &Job<T>) -> Answer<T> {
        loop {
            match job.poll() {
                Answer::Running => std::thread::yield_now(),
                answer => return answer,
            }
        }
    }

    #[test]
    fn a_job_answers_once() {
        let job = run(&egui::Context::default(), |_| 7);
        assert_eq!(settled(&job), Answer::Answered(7));
        assert_eq!(settled(&job), Answer::Died, "an answer is taken once");
    }

    /// ⚠️ A worker that panics answers nothing. A caller that could not tell that from
    /// "still running" would keep the job, and whatever waits on it, for the rest of the
    /// session.
    #[test]
    #[cfg(not(target_arch = "wasm32"))]
    fn a_worker_that_panics_is_reported_as_dead() {
        let job: Job<u32> = run(&egui::Context::default(), |_| panic!("the work gave up"));
        assert_eq!(settled(&job), Answer::Died);
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
