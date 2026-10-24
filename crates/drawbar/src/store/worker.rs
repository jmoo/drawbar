//! The dedicated worker that keeps a library in the browser, and the page's side of it.
//!
//! The page starts the worker from [`BOOTSTRAP`] and posts it the URL of the bundle's
//! wasm-bindgen glue, the module the page already compiled, and where the library is.
//! The worker instantiates the same bundle and calls [`serve`], which answers `{ready}`
//! or `{failed}`. From then on each command is `{id, text, attached}`, a
//! [`wire::Message`], and is answered by `{id, text, attached}` holding every answer it
//! gave, in order; or by `{id, failed}`. A command with `{whole}` answers the whole view
//! rather than what changed since the last answer, which the page lost. The worker runs
//! commands one at a time, in the order they arrive. `{close}` lets the library go, and
//! is answered by `{closed}`. A fault in the worker is posted as `{failed}`, after which
//! it answers nothing. `{listed, answered}` says how many files its listings have found
//! and how many requests its storage has answered, each time either moves.
//!
//! Started for rescues instead, the worker keeps no library: each message is a
//! [`RescueIo`] on the files of one directory of the origin private file system, which
//! only a worker can write in place.

use std::cell::{Cell, RefCell};
use std::collections::{BTreeMap, VecDeque};
use std::rc::Rc;
use std::sync::atomic::Ordering;
use std::sync::Arc;

use js_sys::{Array, ArrayBuffer, Function, Object, Promise, Reflect, Uint8Array};
use wasm_bindgen::closure::Closure;
use wasm_bindgen::prelude::wasm_bindgen;
use wasm_bindgen::{JsCast as _, JsValue};
use wasm_bindgen_futures::JsFuture;
use web_sys::{
    Blob, BlobPropertyBag, DedicatedWorkerGlobalScope, FileSystemDirectoryHandle, MessageEvent,
    Url, WorkerOptions, WorkerType,
};

use serde::{Deserialize, Serialize};
use toshokan::io::{Io, Reply, Root};
use toshokan::web::{CryptoRandom, DateClock, Executor, Folder};
use toshokan::RelPath;

use super::counted::{Counted, Signs};
use super::keeper::{self, Keeper};
use super::keys::{swapped, Volume};
use super::wire::{self, Attachment, Message};
use super::{storage, Cmd, Event};
use crate::js::{describe, field};

/// The worker's whole script: it loads the page's bundle and hands over to it.
const BOOTSTRAP: &str = r#"
const stopped = (why) => self.postMessage({ failed: String(why?.message ?? why) });
self.addEventListener("error", (event) => stopped(event.message));
self.addEventListener("unhandledrejection", (event) => stopped(event.reason));
self.onmessage = async ({ data }) => {
  self.onmessage = null;
  try {
    const bundle = await import(data.glue);
    await bundle.default({ module_or_path: data.module });
    await bundle.drawbarWorker(data);
  } catch (error) {
    stopped(error?.stack || error);
  }
};
"#;

#[wasm_bindgen]
extern "C" {
    /// The URL of the glue this code was bound by, which the worker imports.
    #[wasm_bindgen(thread_local_v2, js_namespace = ["import", "meta"], js_name = url)]
    static GLUE: String;
}

/// One request of a worker started by [`Worker::rescues`], on the file of this name in
/// its directory. [`Worker::rescue`] answers [`RescueIo::Free`] with whether no file has
/// the name, and the others with `true` once done.
#[derive(Debug, Serialize, Deserialize)]
pub enum RescueIo {
    Free(String),
    /// Make an empty file, never over one.
    Create(String),
    Write {
        name: String,
        offset: u64,
        #[serde(with = "wire::bytes")]
        bytes: Vec<u8>,
    },
    /// Make what was written durable, where the storage can.
    Sync(String),
    Rename {
        from: String,
        to: String,
    },
    Remove(String),
}

/// Why [`Worker::run`] gives no answers to a command.
#[derive(Debug)]
pub enum Unrun {
    /// The worker did not run it: why.
    Failed(String),
    /// The worker ran it, and the page could not read its answers: why.
    Unread(String),
}

/// The page's side of a library's worker. Dropping it ends the worker, which lets go of
/// what it held.
pub struct Worker {
    worker: web_sys::Worker,
    replies: Rc<Replies>,
    /// Whether an answer was lost, so the next must show the whole view.
    lost: Cell<bool>,
    /// Whether the page takes the next answer for one it cannot read.
    unread: Cell<bool>,
    _hear: Closure<dyn FnMut(MessageEvent)>,
    _fail: Closure<dyn FnMut(JsValue)>,
}

/// What waits for the worker: each command sent and not yet answered, by the function
/// that resolves its answer, and the start and the close.
#[derive(Default)]
struct Replies {
    waiting: RefCell<BTreeMap<u32, Function>>,
    next: Cell<u32>,
    /// Why the worker stopped answering.
    failed: RefCell<Option<String>>,
    started: RefCell<Option<Function>>,
    closed: RefCell<Option<Function>>,
    /// How many files the worker's listings of the folder have found so far, and how
    /// many requests its storage answered.
    signs: Rc<Cell<(usize, usize)>>,
}

impl Replies {
    fn fail(&self, why: String) {
        let failed = object(&[("failed", JsValue::from_str(&why))]);
        let waiting = std::mem::take(&mut *self.waiting.borrow_mut()).into_values();
        let pending = [self.started.take(), self.closed.take()];
        for answer in waiting.chain(pending.into_iter().flatten()) {
            let _ = answer.call1(&JsValue::NULL, &failed);
        }
        self.failed.borrow_mut().get_or_insert(why);
    }
}

impl Worker {
    /// Start the worker that keeps the library in `folder`, with this install's own state
    /// for it at `local` in the origin private file system, and wait until it is ready.
    pub async fn start(folder: Folder, local: &RelPath) -> Result<Worker, String> {
        let (kind, dir, rename) = match folder {
            Folder::Private(path) => ("private", JsValue::from_str(path.as_str()), true),
            Folder::Picked { dir, rename } => ("picked", dir.into(), rename),
        };
        Worker::spawn(&[
            ("kind", kind.into()),
            ("folder", dir),
            ("rename", rename.into()),
            ("local", local.as_str().into()),
        ])
        .await
    }

    /// Start a worker that writes rescues in `dir` of the origin private file system,
    /// and wait until it is ready.
    pub async fn rescues(dir: &RelPath) -> Result<Worker, String> {
        Worker::spawn(&[("kind", "rescues".into()), ("folder", dir.as_str().into())]).await
    }

    /// Do `io` in the directory of a worker started by [`Worker::rescues`].
    pub async fn rescue(&self, io: RescueIo) -> Result<bool, String> {
        if let Some(why) = &*self.replies.failed.borrow() {
            return Err(why.clone());
        }
        let reply = self.reply(&io).await?;
        from_js(&reply).and_then(wire::decode::<Result<bool, String>>)?
    }

    /// Start the worker, with `start` beside the bundle in its first message.
    async fn spawn(start: &[(&str, JsValue)]) -> Result<Worker, String> {
        let parts = Array::of1(&JsValue::from_str(BOOTSTRAP));
        let kind = BlobPropertyBag::new();
        kind.set_type("text/javascript");
        let blob = Blob::new_with_str_sequence_and_options(&parts, &kind).map_err(js)?;
        let url = Url::create_object_url_with_blob(&blob).map_err(js)?;
        let options = WorkerOptions::new();
        options.set_type(WorkerType::Module);
        options.set_name("drawbar library");
        let worker = web_sys::Worker::new_with_options(&url, &options).map_err(js)?;

        let replies = Rc::new(Replies::default());
        let (started, answer) = deferred();
        *replies.started.borrow_mut() = Some(answer);
        let hear = {
            let replies = Rc::clone(&replies);
            Closure::<dyn FnMut(MessageEvent)>::new(move |event: MessageEvent| {
                let data = event.data();
                let id = field(&data, "id").and_then(|id| id.as_f64());
                let count = |name| field(&data, name).and_then(|count| count.as_f64());
                if let (Some(listed), Some(answered)) = (count("listed"), count("answered")) {
                    replies.signs.set((listed as usize, answered as usize));
                    return;
                }
                let answer = match id {
                    Some(id) => replies.waiting.borrow_mut().remove(&(id as u32)),
                    None if field(&data, "closed").is_some() => replies.closed.take(),
                    None => replies.started.take(),
                };
                match (answer, field(&data, "failed")) {
                    (Some(answer), _) => {
                        let _ = answer.call1(&JsValue::NULL, &data);
                    }
                    (None, Some(why)) => {
                        replies.fail(format!("the library's worker stopped: {}", describe(&why)));
                    }
                    (None, None) => {}
                }
            })
        };
        let fail = {
            let replies = Rc::clone(&replies);
            Closure::<dyn FnMut(JsValue)>::new(move |event: JsValue| {
                let why = field(&event, "message")
                    .and_then(|message| message.as_string())
                    .unwrap_or_else(|| "it could not be loaded".into());
                replies.fail(format!("the library's worker stopped: {why}"));
            })
        };
        worker.set_onmessage(Some(hear.as_ref().unchecked_ref()));
        worker.set_onerror(Some(fail.as_ref().unchecked_ref()));
        worker.set_onmessageerror(Some(fail.as_ref().unchecked_ref()));

        let bundle = [
            ("glue", GLUE.with(|url| JsValue::from_str(url))),
            ("module", wasm_bindgen::module()),
        ];
        let start = object(&[&bundle[..], start].concat());
        let posted = worker.post_message(&start);
        let worker = Worker {
            worker,
            replies,
            lost: Cell::new(false),
            unread: Cell::new(false),
            _hear: hear,
            _fail: fail,
        };
        posted.map_err(js)?;
        let ready = JsFuture::from(started).await;
        let _ = Url::revoke_object_url(&url);
        let ready = ready.map_err(js)?;
        match field(&ready, "failed") {
            Some(why) => Err(describe(&why)),
            None => Ok(worker),
        }
    }

    /// How many files the worker's listings of the folder have found so far, and how
    /// many requests its storage answered, as it last said.
    pub fn signs(&self) -> Rc<Cell<(usize, usize)>> {
        Rc::clone(&self.replies.signs)
    }

    /// Why the worker stopped answering, once it has.
    pub fn stopped(&self) -> Option<String> {
        self.replies.failed.borrow().clone()
    }

    /// Make the worker fault, or never finish the command it takes next, as a bug in it
    /// would.
    #[doc(hidden)]
    pub fn fault(&self, fault: super::Fault) {
        let name = match fault {
            super::Fault::Trap => "trap",
            super::Fault::Stall => "stall",
            super::Fault::Unread => return self.unread.set(true),
        };
        let _ = self.worker.post_message(&object(&[(name, true.into())]));
    }

    /// Take the last answer for lost, as one the page could not read is, so the next
    /// shows the whole view.
    #[doc(hidden)]
    pub fn lose(&self) {
        self.lost.set(true);
    }

    /// End the worker, which does not answer, and fail what waits on it.
    pub fn abandon(&self) {
        self.worker.terminate();
        self.replies
            .fail("the library's worker was let go".to_string());
    }

    /// Run `cmd` in the worker, and every answer it gave.
    pub async fn run(&self, cmd: Cmd) -> Result<Vec<Event>, Unrun> {
        if let Some(why) = &*self.replies.failed.borrow() {
            return Err(Unrun::Failed(why.clone()));
        }
        let reply = self.reply(&cmd).await.map_err(Unrun::Failed)?;
        let decoded = match self.unread.take() {
            true => Err("the page was made unable to read it".to_string()),
            false => from_js(&reply).and_then(wire::decode),
        };
        self.lost.set(decoded.is_err());
        decoded.map_err(Unrun::Unread)
    }

    /// The worker's reply to `cmd`, as it came.
    async fn reply(&self, cmd: &impl Serialize) -> Result<JsValue, String> {
        let (message, transfer) = to_js(wire::encode(cmd)?);
        let id = self.replies.next.get();
        self.replies.next.set(id.wrapping_add(1));
        Reflect::set(&message, &"id".into(), &id.into()).map_err(js)?;
        let lost = self.lost.take();
        if lost {
            Reflect::set(&message, &"whole".into(), &true.into()).map_err(js)?;
        }
        let (replied, answer) = deferred();
        self.replies.waiting.borrow_mut().insert(id, answer);
        if let Err(e) = self.worker.post_message_with_transfer(&message, &transfer) {
            self.replies.waiting.borrow_mut().remove(&id);
            self.lost.set(lost);
            return Err(js(e));
        }
        let reply = JsFuture::from(replied).await.map_err(js)?;
        if let Some(why) = field(&reply, "failed") {
            return Err(describe(&why));
        }
        Ok(reply)
    }

    /// Let the library go, once every command sent has run.
    pub async fn close(&self) {
        if self.replies.failed.borrow().is_some() {
            return;
        }
        let (closed, answer) = deferred();
        *self.replies.closed.borrow_mut() = Some(answer);
        if self
            .worker
            .post_message(&object(&[("close", true.into())]))
            .is_ok()
        {
            let _ = JsFuture::from(closed).await;
        }
    }
}

impl Drop for Worker {
    fn drop(&mut self) {
        self.worker.terminate();
    }
}

/// The worker's entry point. The script [`Worker::start`] runs in the worker calls it
/// with the page's first message, once the bundle is instantiated.
#[wasm_bindgen(js_name = drawbarWorker)]
pub async fn serve(start: JsValue) {
    let scope: DedicatedWorkerGlobalScope = js_sys::global().unchecked_into();
    let signs = Arc::new(Signs::default());
    let served = match field(&start, "kind")
        .and_then(|kind| kind.as_string())
        .as_deref()
    {
        Some("rescues") => rescues(&start).await.map(Served::Rescues),
        _ => keeper(&start, signs.clone())
            .await
            .map(|keeper| Served::Library(Box::new(keeper))),
    };
    let served = match served {
        Ok(served) => served,
        Err(why) => {
            let _ = scope.post_message(&object(&[("failed", why.into())]));
            return;
        }
    };
    let queue = Rc::new(Queue {
        served: RefCell::new(Some(served)),
        scope: scope.clone(),
        waiting: RefCell::default(),
        running: Cell::new(false),
        signs,
    });
    let hear = Closure::<dyn FnMut(MessageEvent)>::new(move |event: MessageEvent| {
        queue.push(event.data());
    });
    scope.set_onmessage(Some(hear.as_ref().unchecked_ref()));
    // The handler lives as long as the worker.
    hear.forget();
    let _ = scope.post_message(&object(&[("ready", true.into())]));
}

/// How often the worker tells the page what its storage has done.
const TELL_SIGNS_MS: i32 = 200;

/// The keeper of the library the page's first message names.
async fn keeper(start: &JsValue, signs: Arc<Signs>) -> Result<Keeper<Counted<Executor>>, String> {
    let path = |name| {
        let text = field(start, name).and_then(|path| path.as_string());
        let text = text.ok_or_else(|| format!("the page sent no {name} path"))?;
        RelPath::new(&text).map_err(|error| error.to_string())
    };
    let kind = field(start, "kind").and_then(|kind| kind.as_string());
    let folder = match kind.as_deref() {
        Some("private") => Folder::Private(path("folder")?),
        Some("picked") => Folder::Picked {
            dir: field(start, "folder")
                .and_then(|dir| dir.dyn_into::<FileSystemDirectoryHandle>().ok())
                .ok_or("the page sent no picked folder")?,
            rename: field(start, "rename").and_then(|rename| rename.as_bool()) == Some(true),
        },
        _ => return Err("the page sent no kind of folder".into()),
    };
    let picked = matches!(folder, Folder::Picked { .. });
    let executor = Executor::new(folder, &path("local")?)
        .await
        .map_err(|e| e.to_string())?;
    let volume = match picked {
        true => probe(&executor).await,
        false => Volume {
            folds_case: false,
            folds_form: false,
        },
    };
    let env = keeper::env(
        Box::new(DateClock),
        Box::new(CryptoRandom),
        Box::new(volume),
    );
    Ok(Keeper::new(Counted::new(executor, signs), env))
}

/// The storage of the directory the page's first message names, for rescues.
async fn rescues(start: &JsValue) -> Result<Executor, String> {
    let dir = field(start, "folder").and_then(|dir| dir.as_string());
    let dir = RelPath::new(&dir.ok_or("the page sent no folder")?).map_err(|e| e.to_string())?;
    Executor::new(Folder::Private(dir.clone()), &dir)
        .await
        .map_err(|e| e.to_string())
}

/// Do `io` in the directory `fs` is on.
async fn rescue(fs: &Executor, io: RescueIo) -> Result<bool, String> {
    let at = |name: &str| RelPath::new(name).map_err(|e| e.to_string());
    let root = Root::Folder;
    let io = match io {
        RescueIo::Free(name) => Io::Stat {
            root,
            path: at(&name)?,
        },
        RescueIo::Create(name) => Io::Create {
            root,
            path: at(&name)?,
            bytes: Vec::new(),
        },
        RescueIo::Write {
            name,
            offset,
            bytes,
        } => Io::Write {
            root,
            path: at(&name)?,
            offset,
            bytes,
        },
        RescueIo::Sync(_) if !fs.capabilities(root).fsync => return Ok(true),
        RescueIo::Sync(name) => Io::Sync {
            root,
            path: at(&name)?,
        },
        RescueIo::Rename { from, to } => Io::Rename {
            root,
            from: at(&from)?,
            to: at(&to)?,
        },
        RescueIo::Remove(name) => Io::Remove {
            root,
            path: at(&name)?,
        },
    };
    match fs.perform(io).await.map_err(|e| e.to_string())? {
        Reply::Stat(found) => Ok(found.is_none()),
        _ => Ok(true),
    }
}

/// How the volume holding a folder the user picked compares names, as a look at it,
/// which writes nothing, finds: a name's other case reaching the same entry. Where
/// nothing there has a name with a case to swap: ignoring case on macOS and Windows,
/// as their volumes usually do. On macOS also in one normalization form.
async fn probe(fs: &Executor) -> Volume {
    let mac = platform().contains("mac");
    let usual = mac || platform().contains("win");
    let listed = storage::list(fs, Root::Folder, &RelPath::ROOT).await;
    let names: Vec<String> = listed.into_iter().flatten().map(|(name, _)| name).collect();
    let twin = names.iter().take(64).find_map(|name| {
        let twin = swapped(name);
        (twin != *name).then_some(twin)
    });
    let folds_case = match twin.and_then(|twin| RelPath::new(&twin).ok()) {
        Some(twin) => {
            let reached = storage::meta(fs, Root::Folder, &twin).await;
            let reached = reached.ok().flatten().is_some();
            reached && !names.iter().any(|name| name == twin.as_str())
        }
        None => usual,
    };
    Volume {
        folds_case,
        folds_form: mac,
    }
}

/// The platform the browser runs on, in lower case, as it says.
fn platform() -> String {
    let navigator = field(&js_sys::global(), "navigator").unwrap_or(JsValue::UNDEFINED);
    let named = field(&navigator, "userAgentData").and_then(|data| field(&data, "platform"));
    let named = named.or_else(|| field(&navigator, "platform"));
    named
        .and_then(|name| name.as_string())
        .unwrap_or_default()
        .to_lowercase()
}

/// What a worker serves: a library, or a directory rescues are written in.
enum Served {
    Library(Box<Keeper<Counted<Executor>>>),
    Rescues(Executor),
}

/// Messages waiting for the keeper, run one at a time in arrival order.
struct Queue {
    /// Taken while a command runs.
    served: RefCell<Option<Served>>,
    scope: DedicatedWorkerGlobalScope,
    waiting: RefCell<VecDeque<JsValue>>,
    running: Cell<bool>,
    signs: Arc<Signs>,
}

impl Queue {
    fn push(self: &Rc<Self>, message: JsValue) {
        self.waiting.borrow_mut().push_back(message);
        if !self.running.replace(true) {
            wasm_bindgen_futures::spawn_local(Rc::clone(self).drain());
            wasm_bindgen_futures::spawn_local(Rc::clone(self).tell_signs());
        }
    }

    /// Tell the page how many files the folder's listings have found and how many
    /// requests the storage answered, each time either moved, while commands run.
    async fn tell_signs(self: Rc<Self>) {
        let mut told = None;
        loop {
            let waited = Promise::new(&mut |resolve, _| {
                let scope = &self.scope;
                let ms = TELL_SIGNS_MS;
                let _ = scope.set_timeout_with_callback_and_timeout_and_arguments_0(&resolve, ms);
            });
            let _ = JsFuture::from(waited).await;
            let listed = self.signs.listed.load(Ordering::Relaxed);
            let answered = self.signs.answered.load(Ordering::Relaxed);
            if told != Some((listed, answered)) {
                told = Some((listed, answered));
                let said = [("listed", listed), ("answered", answered)];
                let said = said.map(|(name, count)| (name, JsValue::from(count as f64)));
                let _ = self.scope.post_message(&object(&said));
            }
            if !self.running.get() {
                return;
            }
        }
    }

    async fn drain(self: Rc<Self>) {
        loop {
            let next = self.waiting.borrow_mut().pop_front();
            let Some(message) = next else {
                break;
            };
            if field(&message, "trap").is_some() {
                std::process::abort();
            }
            if field(&message, "stall").is_some() {
                let never = Promise::new(&mut |_, _| {});
                let _ = JsFuture::from(never).await;
            }
            let served = self.served.take();
            let Some(mut served) = served else {
                let id = field(&message, "id").unwrap_or(JsValue::NULL);
                let closed = object(&[("id", id), ("failed", "the library is closed".into())]);
                let _ = self.scope.post_message(&closed);
                continue;
            };
            if field(&message, "close").is_some() {
                if let Served::Library(mut keeper) = served {
                    keeper.close().await;
                }
                let _ = self.scope.post_message(&object(&[("closed", true.into())]));
                continue;
            }
            match &mut served {
                Served::Library(keeper) => self.answer(keeper, &message).await,
                Served::Rescues(fs) => self.rescued(fs, &message).await,
            }
            *self.served.borrow_mut() = Some(served);
        }
        self.running.set(false);
    }

    async fn rescued(&self, fs: &Executor, message: &JsValue) {
        let id = field(message, "id").unwrap_or(JsValue::NULL);
        let done = match from_js(message).and_then(wire::decode::<RescueIo>) {
            Ok(io) => rescue(fs, io).await,
            Err(why) => Err(format!("the rescue worker was sent {why}")),
        };
        let posted = wire::encode(&done).and_then(|message| {
            let (reply, transfer) = to_js(message);
            let _ = Reflect::set(&reply, &"id".into(), &id);
            self.scope
                .post_message_with_transfer(&reply, &transfer)
                .map_err(js)
        });
        if let Err(why) = posted {
            let failed = object(&[("id", id), ("failed", why.into())]);
            let _ = self.scope.post_message(&failed);
        }
    }

    async fn answer(&self, keeper: &mut Keeper<Counted<Executor>>, message: &JsValue) {
        let id = field(message, "id").unwrap_or(JsValue::NULL);
        if field(message, "whole").is_some() {
            keeper.forget_shown();
        }
        let answered = match from_js(message).and_then(wire::decode::<Cmd>) {
            Ok(cmd) => {
                let mut events = Vec::new();
                keeper.run(cmd, &mut |event| events.push(event)).await;
                wire::encode(&events)
            }
            Err(why) => Err(format!("the library's worker was sent {why}")),
        };
        let posted = match answered {
            Ok(message) => {
                let (reply, transfer) = to_js(message);
                let _ = Reflect::set(&reply, &"id".into(), &id);
                self.scope
                    .post_message_with_transfer(&reply, &transfer)
                    .map_err(js)
            }
            Err(why) => Err(why),
        };
        if let Err(why) = posted {
            keeper.forget_shown();
            let failed = object(&[("id", id), ("failed", why.into())]);
            let _ = self.scope.post_message(&failed);
        }
    }
}

/// `message` as the worker posts it, with the buffers its bytes are in handed over.
fn to_js(message: Message) -> (JsValue, Array) {
    let (attached, transfer) = (Array::new(), Array::new());
    for attachment in message.attached {
        match attachment {
            Attachment::Bytes(bytes) => {
                let buffer = Uint8Array::from(&bytes[..]).buffer();
                transfer.push(&buffer);
                attached.push(&buffer);
            }
            Attachment::File(file) => {
                attached.push(&file);
            }
        }
    }
    let object = object(&[("text", message.text.into()), ("attached", attached.into())]);
    (object, transfer)
}

/// The message a posted `value` holds.
fn from_js(value: &JsValue) -> Result<Message, String> {
    let text = field(value, "text").and_then(|text| text.as_string());
    let text = text.ok_or("no text")?;
    let attached = field(value, "attached").ok_or("no attachments")?;
    let attached: Array = attached
        .dyn_into()
        .map_err(|_| "attachments that are no list")?;
    let attached = attached.iter().map(|value| {
        if let Some(buffer) = value.dyn_ref::<ArrayBuffer>() {
            return Ok(Attachment::Bytes(Uint8Array::new(buffer).to_vec()));
        }
        value
            .dyn_into::<web_sys::File>()
            .map(Attachment::File)
            .map_err(|_| "an attachment that is neither bytes nor a file".to_string())
    });
    Ok(Message {
        text,
        attached: attached.collect::<Result<_, _>>()?,
    })
}

fn object(fields: &[(&str, JsValue)]) -> JsValue {
    let object = Object::new();
    for (name, value) in fields {
        let _ = Reflect::set(&object, &JsValue::from_str(name), value);
    }
    object.into()
}

/// A promise and the function that resolves it.
fn deferred() -> (Promise, Function) {
    let mut resolve = None;
    let promise = Promise::new(&mut |done, _| resolve = Some(done));
    (
        promise,
        resolve.expect("a promise runs its executor at once"),
    )
}

fn js(error: JsValue) -> String {
    describe(&error)
}
