//! The dedicated worker that performs a page's requests, and the page's side of it.
//!
//! The page starts the worker from [`BOOTSTRAP`] and posts it the URL of the
//! bundle's wasm-bindgen glue (built with `--target web`), the module the page
//! already compiled, and the roots. The worker instantiates the same bundle and
//! calls [`serve`], which answers `{ready, folder, local}` with each root's
//! capabilities as [`wire::capability_bits`], or `{failed}`. From then on each
//! request is `{id, request}` and each reply `{id, reply}`, both one transferred
//! buffer in the [`wire`] encoding. The worker performs requests one at a time,
//! in the order they arrive.

use std::cell::{Cell, RefCell};
use std::collections::{BTreeMap, VecDeque};
use std::rc::Rc;

use js_sys::{Array, ArrayBuffer, Function, Uint8Array};
use wasm_bindgen::closure::Closure;
use wasm_bindgen::prelude::wasm_bindgen;
use wasm_bindgen::{JsCast, JsValue};
use web_sys::{
    Blob, BlobPropertyBag, DedicatedWorkerGlobalScope, FileSystemDirectoryHandle, MessageEvent,
    Url, WorkerOptions, WorkerType,
};

use super::{deferred, describe, field, object, wait, wire, Executor, Folder};
use crate::asynch::Fs;
use crate::io::{Capabilities, Io, IoError, IoResult};
use crate::path::RelPath;
use crate::Root;

/// The worker's whole script: it loads the page's bundle and hands over to it.
const BOOTSTRAP: &str = r#"
self.onmessage = async ({ data }) => {
  self.onmessage = null;
  try {
    const bundle = await import(data.glue);
    await bundle.default({ module_or_path: data.module });
    await bundle.toshokanWorker(data);
  } catch (error) {
    self.postMessage({ failed: String(error?.stack ?? error) });
  }
};
"#;

#[wasm_bindgen]
extern "C" {
    /// The URL of the glue this code was bound by, which the worker imports.
    #[wasm_bindgen(thread_local_v2, js_namespace = ["import", "meta"], js_name = url)]
    static GLUE: String;
}

/// The storage of one library, performed by a dedicated worker. Dropping it ends
/// the worker, which releases its locks.
pub struct Worker {
    worker: web_sys::Worker,
    capabilities: [Capabilities; 2],
    replies: Rc<Replies>,
    _hear: Closure<dyn FnMut(MessageEvent)>,
    _fail: Closure<dyn FnMut(JsValue)>,
}

/// Requests sent and not yet answered, each by the function that resolves its
/// answer with the reply buffer, or with a string saying why none will come.
#[derive(Default)]
struct Replies {
    waiting: RefCell<BTreeMap<u32, Function>>,
    next: Cell<u32>,
    /// Why the worker stopped answering.
    failed: RefCell<Option<String>>,
    started: RefCell<Option<Function>>,
}

impl Replies {
    fn fail(&self, why: String) {
        let why_js = JsValue::from_str(&why);
        for (_, answer) in std::mem::take(&mut *self.waiting.borrow_mut()) {
            let _ = answer.call1(&JsValue::NULL, &why_js);
        }
        if let Some(started) = self.started.take() {
            let _ = started.call1(&JsValue::NULL, &object(&[("failed", why_js)]));
        }
        self.failed.borrow_mut().get_or_insert(why);
    }
}

impl Worker {
    /// Starts a worker for `folder`, with the directory at `local` in the origin
    /// private file system as the local root, and waits until it is ready.
    pub async fn start(folder: Folder, local: &RelPath) -> Result<Self, IoError> {
        let other = |error: JsValue| IoError::Other(describe(&error));
        let parts = Array::of1(&JsValue::from_str(BOOTSTRAP));
        let kind = BlobPropertyBag::new();
        kind.set_type("text/javascript");
        let blob = Blob::new_with_str_sequence_and_options(&parts, &kind).map_err(other)?;
        let url = Url::create_object_url_with_blob(&blob).map_err(other)?;
        let options = WorkerOptions::new();
        options.set_type(WorkerType::Module);
        options.set_name("toshokan");
        let worker = web_sys::Worker::new_with_options(&url, &options).map_err(other)?;

        let replies = Rc::new(Replies::default());
        let (started, answer) = deferred();
        *replies.started.borrow_mut() = Some(answer);
        let hear = {
            let replies = Rc::clone(&replies);
            Closure::<dyn FnMut(MessageEvent)>::new(move |event: MessageEvent| {
                let data = event.data();
                let answered = field(&data, "id")
                    .and_then(|id| id.as_f64())
                    .and_then(|id| replies.waiting.borrow_mut().remove(&(id as u32)));
                let answer = answered.or_else(|| replies.started.take());
                let reply = field(&data, "reply").unwrap_or(data);
                if let Some(answer) = answer {
                    let _ = answer.call1(&JsValue::NULL, &reply);
                }
            })
        };
        let fail = {
            let replies = Rc::clone(&replies);
            Closure::<dyn FnMut(JsValue)>::new(move |event: JsValue| {
                let why = field(&event, "message")
                    .and_then(|message| message.as_string())
                    .unwrap_or_else(|| "the worker could not be loaded".into());
                replies.fail(format!("the storage worker failed: {why}"));
            })
        };
        worker.set_onmessage(Some(hear.as_ref().unchecked_ref()));
        worker.set_onerror(Some(fail.as_ref().unchecked_ref()));
        worker.set_onmessageerror(Some(fail.as_ref().unchecked_ref()));

        let (kind, dir, rename) = match folder {
            Folder::Private(path) => ("private", JsValue::from_str(path.as_str()), true),
            Folder::Picked { dir, rename } => ("picked", dir.into(), rename),
        };
        let start = object(&[
            ("glue", GLUE.with(|url| JsValue::from_str(url))),
            ("module", wasm_bindgen::module()),
            ("kind", kind.into()),
            ("folder", dir),
            ("rename", rename.into()),
            ("local", local.as_str().into()),
        ]);
        let posted = worker.post_message(&start);
        let worker = Self {
            worker,
            capabilities: [Capabilities::NONE; 2],
            replies,
            _hear: hear,
            _fail: fail,
        };
        posted.map_err(other)?;
        let ready = wait(started).await.map_err(other);
        let _ = Url::revoke_object_url(&url);
        let ready = ready?;
        if let Some(why) = field(&ready, "failed") {
            return Err(IoError::Other(describe(&why)));
        }
        let bits = |root| {
            field(&ready, root)
                .and_then(|bits| bits.as_f64())
                .map(|bits| wire::capabilities_from_bits(bits as u8))
                .ok_or_else(|| {
                    IoError::Other("the storage worker started without capabilities".into())
                })
        };
        let mut worker = worker;
        worker.capabilities = [bits("folder")?, bits("local")?];
        Ok(worker)
    }
}

impl Fs for Worker {
    fn capabilities(&self, root: Root) -> Capabilities {
        self.capabilities[match root {
            Root::Folder => 0,
            Root::Local => 1,
        }]
    }

    async fn perform(&self, io: Io) -> IoResult {
        if let Some(why) = &*self.replies.failed.borrow() {
            return Err(IoError::Other(why.clone()));
        }
        let request = Uint8Array::from(&wire::encode_request(&io)?[..]).buffer();
        let id = self.replies.next.get();
        self.replies.next.set(id.wrapping_add(1));
        let (replied, answer) = deferred();
        self.replies.waiting.borrow_mut().insert(id, answer);
        let message = object(&[("id", id.into()), ("request", request.clone().into())]);
        if let Err(error) = self
            .worker
            .post_message_with_transfer(&message, &Array::of1(&request))
        {
            self.replies.waiting.borrow_mut().remove(&id);
            return Err(IoError::Other(describe(&error)));
        }
        let reply = wait(replied)
            .await
            .map_err(|error| IoError::Other(describe(&error)))?;
        match reply.dyn_into::<ArrayBuffer>() {
            Ok(buffer) => wire::decode_result(&Uint8Array::new(&buffer).to_vec()),
            Err(why) => Err(IoError::Other(describe(&why))),
        }
    }
}

impl Drop for Worker {
    fn drop(&mut self) {
        self.worker.terminate();
    }
}

/// The worker's entry point. The script [`Worker::start`] runs in the worker
/// calls it with the page's first message, once the bundle is instantiated.
#[wasm_bindgen(js_name = toshokanWorker)]
pub async fn serve(start: JsValue) {
    let scope: DedicatedWorkerGlobalScope = js_sys::global().unchecked_into();
    let executor = match executor(&start).await {
        Ok(executor) => executor,
        Err(error) => {
            let failed = object(&[("failed", error.to_string().into())]);
            let _ = scope.post_message(&failed);
            return;
        }
    };
    let ready = object(&[
        ("ready", true.into()),
        (
            "folder",
            wire::capability_bits(executor.capabilities(Root::Folder)).into(),
        ),
        (
            "local",
            wire::capability_bits(executor.capabilities(Root::Local)).into(),
        ),
    ]);
    let queue = Rc::new(Queue {
        executor,
        scope: scope.clone(),
        waiting: RefCell::default(),
        running: Cell::new(false),
    });
    let hear = Closure::<dyn FnMut(MessageEvent)>::new(move |event: MessageEvent| {
        queue.push(event.data());
    });
    scope.set_onmessage(Some(hear.as_ref().unchecked_ref()));
    // The handler lives as long as the worker.
    hear.forget();
    let _ = scope.post_message(&ready);
}

async fn executor(start: &JsValue) -> Result<Executor, IoError> {
    let path = |name| {
        let text = field(start, name).and_then(|path| path.as_string());
        let text = text.ok_or_else(|| IoError::Other(format!("the page sent no {name} path")))?;
        RelPath::new(&text).map_err(|error| IoError::Other(error.to_string()))
    };
    let folder = match field(start, "kind")
        .and_then(|kind| kind.as_string())
        .as_deref()
    {
        Some("private") => Folder::Private(path("folder")?),
        Some("picked") => Folder::Picked {
            dir: field(start, "folder")
                .and_then(|dir| dir.dyn_into::<FileSystemDirectoryHandle>().ok())
                .ok_or_else(|| IoError::Other("the page sent no picked folder".into()))?,
            rename: field(start, "rename").and_then(|rename| rename.as_bool()) == Some(true),
        },
        _ => return Err(IoError::Other("the page sent no kind of folder".into())),
    };
    Executor::new(folder, &path("local")?).await
}

/// Requests waiting for the executor, performed one at a time in arrival order.
struct Queue {
    executor: Executor,
    scope: DedicatedWorkerGlobalScope,
    waiting: RefCell<VecDeque<JsValue>>,
    running: Cell<bool>,
}

impl Queue {
    fn push(self: &Rc<Self>, message: JsValue) {
        self.waiting.borrow_mut().push_back(message);
        if !self.running.replace(true) {
            wasm_bindgen_futures::spawn_local(Rc::clone(self).drain());
        }
    }

    async fn drain(self: Rc<Self>) {
        loop {
            let next = self.waiting.borrow_mut().pop_front();
            let Some(message) = next else {
                break;
            };
            self.answer(message).await;
        }
        self.running.set(false);
    }

    async fn answer(&self, message: JsValue) {
        let request = field(&message, "request")
            .and_then(|request| request.dyn_into::<ArrayBuffer>().ok())
            .map(|request| Uint8Array::new(&request).to_vec());
        let result = match request.as_deref().map(wire::decode_request) {
            Some(Ok(io)) => self.executor.perform(io).await,
            Some(Err(wire::Malformed(what))) => Err(IoError::Other(format!(
                "the storage worker was sent {what}"
            ))),
            None => Err(IoError::Other(
                "the storage worker was sent no request".into(),
            )),
        };
        let reply = Uint8Array::from(&wire::encode_result(&result)[..]).buffer();
        let id = field(&message, "id").unwrap_or(JsValue::NULL);
        let answer = object(&[("id", id), ("reply", reply.clone().into())]);
        let _ = self
            .scope
            .post_message_with_transfer(&answer, &Array::of1(&reply));
    }
}
