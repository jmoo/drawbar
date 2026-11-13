//! Work that outlives the frame that started it: encoding a piano library, or laying one
//! out again from a plan.
//!
//! ⚠️ wasm has one thread, so there [`run`] runs the work inline and the frame waits for
//! it, and `spawn` runs work that waits on the browser as a task of its own. Work
//! [`hand`]ed off by name runs in a worker of its own instead, and the page stays live.
//! The caller gets the same [`Job`] every way and polls it the same way.

use std::sync::mpsc::{channel, Receiver, TryRecvError};
use std::sync::{Arc, Mutex};

use eframe::egui;

mod tasks;
#[cfg(target_arch = "wasm32")]
pub(crate) mod worker;

pub use tasks::{hand, FindNoise, FoundNoise, Task, Work};

/// The latest progress message from a job.
#[derive(Clone, Default)]
pub struct Progress(Arc<Mutex<String>>);

impl Progress {
    /// Say `words`, and in a task's worker, tell the page.
    pub fn say(&self, words: impl Into<String>) {
        let words = words.into();
        #[cfg(target_arch = "wasm32")]
        worker::tell(&words);
        if let Ok(mut held) = self.0.lock() {
            *held = words;
        }
    }

    pub fn said(&self) -> String {
        self.0.lock().map(|held| held.clone()).unwrap_or_default()
    }
}

/// A job's state when polled.
#[derive(Debug, PartialEq, Eq)]
pub enum Answer<T> {
    Running,
    Answered(T),
    /// The worker is gone and no answer is coming: it panicked, or its answer has already
    /// been taken.
    Died,
}

/// One piece of work in progress, which answers once.
pub struct Job<T> {
    rx: Receiver<T>,
    progress: Progress,
    /// The worker running the job, which ends when the job is dropped.
    #[cfg(target_arch = "wasm32")]
    _worker: Option<worker::Running>,
}

impl<T> Job<T> {
    /// Whether the answer is here, still coming, or never coming.
    ///
    /// ⚠️ A job answers once, and the worker exits after answering. Take the answer and
    /// drop the job: polling it again returns [`Answer::Died`], which describes the
    /// worker, not the answer already taken.
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

    /// [`Self::poll`], blocking until the worker answers or exits, so never
    /// [`Answer::Running`].
    #[cfg(test)]
    pub fn wait(&self) -> Answer<T> {
        match self.rx.recv() {
            Ok(answer) => Answer::Answered(answer),
            Err(_) => Answer::Died,
        }
    }
}

/// Start `work`, and request a repaint when it answers.
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
    Job {
        rx,
        progress,
        _worker: None,
    }
}

/// Start `work`, a future the browser answers whenever it answers, and request a repaint
/// when it has.
#[cfg(target_arch = "wasm32")]
pub fn spawn<T: 'static, F: std::future::Future<Output = T> + 'static>(
    ctx: &egui::Context,
    work: impl FnOnce(Progress) -> F,
) -> Job<T> {
    let (tx, rx) = channel();
    let progress = Progress::default();
    let working = work(progress.clone());
    let ctx = ctx.clone();
    wasm_bindgen_futures::spawn_local(async move {
        let _ = tx.send(working.await);
        ctx.request_repaint();
    });
    Job {
        rx,
        progress,
        _worker: None,
    }
}

/// Seconds since the Unix epoch, or `None` when the clock reads before it or past what a
/// `u32` holds.
///
/// ⚠️ `SystemTime::now()` traps on `wasm32-unknown-unknown`, so the web build reads the
/// browser's clock.
#[cfg(not(target_arch = "wasm32"))]
pub(crate) fn unix_seconds() -> Option<u32> {
    let elapsed = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .ok()?;
    u32::try_from(elapsed.as_secs()).ok()
}

#[cfg(target_arch = "wasm32")]
pub(crate) fn unix_seconds() -> Option<u32> {
    let seconds = js_sys::Date::now() / 1000.0;
    (0.0..=f64::from(u32::MAX))
        .contains(&seconds)
        .then_some(seconds as u32)
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
}
