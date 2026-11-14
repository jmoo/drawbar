//! The browser's worker for one [`Task`]: started from the store's bootstrap with the
//! task as its first message, it runs the task, posts `{said}` each time the task's
//! [`Progress`] moves, posts the answer, and closes. A fault posts `{failed}`.
//!
//! A worker gets its own memory, so a task's working memory never competes with the
//! page's, and ending the worker frees all of it at once: dropping the [`Job`] does.

use std::cell::RefCell;
use std::sync::mpsc::channel;

use eframe::egui;
use js_sys::Array;
use serde::de::DeserializeOwned;
use wasm_bindgen::closure::Closure;
use wasm_bindgen::{JsCast as _, JsValue};
use web_sys::{
    Blob, BlobPropertyBag, DedicatedWorkerGlobalScope, MessageEvent, Url, WorkerOptions, WorkerType,
};

use super::tasks::{self, Task};
use super::{Job, Progress};
use crate::js::{describe, field};
use crate::store::{self, wire};

/// The name the browser's tools show for a task's worker.
const LABEL: &str = "drawbar work";

/// What a job of a worker holds of it: dropping it ends the worker.
pub struct Running {
    worker: web_sys::Worker,
    url: String,
    _hear: Closure<dyn FnMut(MessageEvent)>,
    _fail: Closure<dyn FnMut(JsValue)>,
}

impl Drop for Running {
    fn drop(&mut self) {
        self.worker.terminate();
        let _ = Url::revoke_object_url(&self.url);
    }
}

/// Start a worker on `task`. Where it cannot start, or stops without answering, the job
/// [`super::Answer::Died`] and its progress says why.
pub(super) fn start<T: DeserializeOwned + Send + 'static>(
    ctx: &egui::Context,
    task: &Task,
) -> Job<T> {
    let (tx, rx) = channel();
    let progress = Progress::default();
    let worker = match spawn(ctx, task, &progress, tx) {
        Ok(running) => Some(running),
        Err(why) => {
            progress.say(format!("the work could not start: {why}"));
            None
        }
    };
    Job {
        rx,
        progress,
        _worker: worker,
    }
}

fn spawn<T: DeserializeOwned + Send + 'static>(
    ctx: &egui::Context,
    task: &Task,
    progress: &Progress,
    tx: std::sync::mpsc::Sender<T>,
) -> Result<Running, String> {
    let js = |e: JsValue| describe(&e);
    let (message, transfer) = store::to_js(wire::encode(task)?);
    for (name, value) in [
        ("kind", JsValue::from_str("job")),
        ("glue", JsValue::from_str(&store::glue())),
        ("module", wasm_bindgen::module()),
    ] {
        js_sys::Reflect::set(&message, &name.into(), &value).map_err(js)?;
    }

    let parts = Array::of1(&JsValue::from_str(store::BOOTSTRAP));
    let kind = BlobPropertyBag::new();
    kind.set_type("text/javascript");
    let blob = Blob::new_with_str_sequence_and_options(&parts, &kind).map_err(js)?;
    let url = Url::create_object_url_with_blob(&blob).map_err(js)?;
    let options = WorkerOptions::new();
    options.set_type(WorkerType::Module);
    options.set_name(LABEL);
    let worker = match web_sys::Worker::new_with_options(&url, &options) {
        Ok(worker) => worker,
        Err(e) => {
            let _ = Url::revoke_object_url(&url);
            return Err(js(e));
        }
    };

    // The answer goes once: the sender is taken then, and taken on a fault too, which
    // leaves the job died.
    let tx = std::rc::Rc::new(RefCell::new(Some(tx)));
    let hear = {
        let (tx, progress, ctx) = (tx.clone(), progress.clone(), ctx.clone());
        Closure::<dyn FnMut(MessageEvent)>::new(move |event: MessageEvent| {
            let data = event.data();
            if let Some(said) = field(&data, "said").and_then(|said| said.as_string()) {
                progress.say(said);
            } else if let Some(why) = field(&data, "failed") {
                progress.say(format!("the work stopped: {}", describe(&why)));
                tx.borrow_mut().take();
            } else if let Some(tx) = tx.borrow_mut().take() {
                match store::from_js(&data).and_then(wire::decode::<T>) {
                    Ok(answer) => {
                        let _ = tx.send(answer);
                    }
                    Err(why) => progress.say(format!("the work's answer was unreadable: {why}")),
                }
            }
            ctx.request_repaint();
        })
    };
    let fail = {
        let (tx, progress, ctx) = (tx, progress.clone(), ctx.clone());
        Closure::<dyn FnMut(JsValue)>::new(move |event: JsValue| {
            let why = field(&event, "message").map_or_else(|| "it failed".into(), |m| describe(&m));
            progress.say(format!("the work stopped: {why}"));
            tx.borrow_mut().take();
            ctx.request_repaint();
        })
    };
    worker.set_onmessage(Some(hear.as_ref().unchecked_ref()));
    worker.set_onerror(Some(fail.as_ref().unchecked_ref()));
    worker.set_onmessageerror(Some(fail.as_ref().unchecked_ref()));
    let running = Running {
        worker,
        url,
        _hear: hear,
        _fail: fail,
    };
    running
        .worker
        .post_message_with_transfer(&message, &transfer)
        .map_err(js)?;
    Ok(running)
}

thread_local! {
    /// The scope of the worker running a task, which its progress is posted to; none on
    /// the page.
    static TELLING: RefCell<Option<DedicatedWorkerGlobalScope>> = const { RefCell::new(None) };
}

/// Post `words` to the page, where this is a task's worker.
pub(super) fn tell(words: &str) {
    TELLING.with(|scope| {
        if let Some(scope) = &*scope.borrow() {
            let said = store::object(&[("said", words.into())]);
            let _ = scope.post_message(&said);
        }
    });
}

/// A task's worker: the store's entry point hands over the page's first message, `{kind:
/// "job"}` beside the task.
pub(crate) fn serve(start: &JsValue) {
    let scope: DedicatedWorkerGlobalScope = js_sys::global().unchecked_into();
    TELLING.with(|telling| *telling.borrow_mut() = Some(scope.clone()));
    let answered = store::from_js(start)
        .and_then(wire::decode::<Task>)
        .and_then(|task| tasks::answer(task, &Progress::default()));
    TELLING.with(|telling| telling.borrow_mut().take());
    let posted = match answered {
        Ok(message) => {
            let (message, transfer) = store::to_js(message);
            scope.post_message_with_transfer(&message, &transfer)
        }
        Err(why) => scope.post_message(&store::object(&[("failed", why.into())])),
    };
    if let Err(e) = posted {
        let failed = store::object(&[("failed", describe(&e).into())]);
        let _ = scope.post_message(&failed);
    }
    scope.close();
}
