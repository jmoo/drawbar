//! The browser's library: a folder tree in the origin private file system, which every
//! browser drawbar runs in keeps for the page and nothing else can see, or a folder on
//! this computer the user picked, in browsers that let a page open one.
//!
//! Reads and folder changes run on the page's one thread. A file is written under
//! `.drawbar/tmp/` in chunks, then moved over its path. In the private file system it is
//! written by `library-writer.js`, a dedicated worker served beside the page, because the
//! handles that write in place exist only there. From the first write the worker also
//! holds `.drawbar/lock` open, and a second tab that finds it held only reads the
//! library. Those handles cannot reach a picked folder, so there the page writes through
//! a writable stream, and a Web Lock named for the folder keeps a second tab to reading.
//!
//! Commands run one at a time in a task of their own, in the order they were sent. A
//! library let go runs the commands it was sent before it lets go of its lock, and the
//! next library opens only then.

use std::cell::{Cell, RefCell};
use std::collections::{BTreeMap, HashMap, VecDeque};
use std::future::Future;
use std::io;
use std::ops::{AsyncFnMut, Range};
use std::pin::Pin;
use std::rc::Rc;
use std::sync::{Arc, Weak};

use eframe::egui;
use js_sys::{Array, Function, Object, Promise, Reflect, Uint8Array};
use wasm_bindgen::closure::Closure;
use wasm_bindgen::{JsCast as _, JsValue};
use wasm_bindgen_futures::{spawn_local, JsFuture};
use web_sys::{
    File, FileSystemDirectoryHandle, FileSystemFileHandle, FileSystemGetDirectoryOptions,
    FileSystemGetFileOptions, FileSystemHandle, FileSystemHandleKind,
    FileSystemHandlePermissionDescriptor, FileSystemPermissionMode, FileSystemWritableFileStream,
    LockOptions, MessageEvent, StorageManager, Worker,
};

use super::exec::{self, Children, Fs, Kind, TMP, WORKING};
use super::{names, Cmd, Event, Failure, Fingerprint, Outside, Stat};
use crate::js::{describe, field};
use crate::ondisk::OnDisk;
use crate::rewrite::{Pieces, Rewrite};
use crate::room::measure as size;

/// Where the writer is served, beside the page. The version keeps a cached writer from
/// an older release from answering a newer page.
fn writer_url() -> String {
    format!("library-writer.js?v={}", crate::sheet::VERSION)
}

const LOCK: &str = ".drawbar/lock";

/// How much of a file crosses to the writer, or back from a read, at once.
const CHUNK: usize = 4 * 1024 * 1024;

/// Where a library is in the browser.
#[derive(Clone, Debug)]
pub enum Root {
    /// The browser's own storage for the page, in every browser.
    Private,
    /// A folder on this computer the user picked.
    Picked(Picked),
}

/// A folder the user picked, and the id drawbar knows it by.
#[derive(Clone, Debug)]
pub struct Picked {
    /// ⚠️ Names the folder's Web Lock, so every tab must know one folder by one id: a
    /// folder picked again takes the id the recent list already gives it.
    pub id: u32,
    pub handle: FileSystemDirectoryHandle,
}

impl PartialEq for Root {
    fn eq(&self, other: &Root) -> bool {
        match (self, other) {
            (Root::Private, Root::Private) => true,
            (Root::Picked(a), Root::Picked(b)) => a.id == b.id,
            _ => false,
        }
    }
}

impl Eq for Root {}

impl Root {
    /// The picked folder's name, or `None` for the browser's own library.
    pub fn name(&self) -> Option<String> {
        match self {
            Root::Private => None,
            Root::Picked(picked) => Some(picked.handle.name()),
        }
    }
}

/// The library in the browser's own storage, which needs no one to find it.
pub fn default_root() -> Option<Root> {
    Some(Root::Private)
}

/// Commands queued for the task that runs them.
#[derive(Default)]
struct Inbox {
    cmds: VecDeque<Cmd>,
    /// Resolves the promise the task waits on while the queue is empty.
    wake: Option<Function>,
    /// No command follows those queued; the task ends once they have run.
    closed: bool,
}

impl Inbox {
    fn wake(inbox: &RefCell<Inbox>) {
        let wake = inbox.borrow_mut().wake.take();
        if let Some(wake) = wake {
            let _ = wake.call0(&JsValue::NULL);
        }
    }
}

thread_local! {
    /// Settles once the library started last has run its last command and let go of its
    /// lock.
    static LET_GO: RefCell<Option<Promise>> = const { RefCell::new(None) };
}

/// What the browser says about keeping drawbar's files.
#[derive(Clone, Default)]
struct Room {
    /// Whether the browser has promised not to evict them. `None` until it has said.
    kept: Option<bool>,
    /// Bytes the origin uses and may use, where the browser tells.
    used: Option<(u64, u64)>,
}

pub struct Backend {
    root: Root,
    inbox: Rc<RefCell<Inbox>>,
    events: Rc<RefCell<VecDeque<Event>>>,
    room: Rc<RefCell<Room>>,
}

impl Backend {
    /// Open the library at `root` once the one started before it has let go.
    pub fn start(ctx: &egui::Context, root: Root) -> Backend {
        crate::ondisk::repaint_with(ctx);
        let backend = Backend {
            root: root.clone(),
            inbox: Rc::default(),
            events: Rc::default(),
            room: Rc::default(),
        };
        let mut done = None;
        let let_go = Promise::new(&mut |resolve, _| done = Some(resolve));
        let before = LET_GO.with(|held| held.replace(Some(let_go)));
        let (inbox, events, room) = (
            backend.inbox.clone(),
            backend.events.clone(),
            backend.room.clone(),
        );
        let ctx = ctx.clone();
        spawn_local(async move {
            if let Some(before) = before {
                let _ = JsFuture::from(before).await;
            }
            drive(root, inbox, events, room, ctx).await;
            if let Some(done) = done {
                let _ = done.call0(&JsValue::NULL);
            }
        });
        backend
    }

    pub fn root(&self) -> &Root {
        &self.root
    }

    pub fn label(&self) -> String {
        match &self.root {
            Root::Private => "this browser".to_string(),
            Root::Picked(picked) => format!("the folder {} on this computer", picked.handle.name()),
        }
    }

    pub fn reveal(&self) -> Option<String> {
        None
    }

    /// How much of the browser's storage drawbar takes, and whether it is kept. Empty
    /// for a picked folder, which is not the browser's to keep.
    pub fn room(&self) -> String {
        if matches!(self.root, Root::Picked(_)) {
            return String::new();
        }
        let room = self.room.borrow();
        let used = room
            .used
            .map(|(used, quota)| format!("{} of {} used", size(used), size(quota)));
        let kept = room.kept.map(|kept| match kept {
            true => "kept",
            false => "may be cleared when space runs low",
        });
        [used.as_deref(), kept]
            .into_iter()
            .flatten()
            .collect::<Vec<_>>()
            .join(", ")
    }

    pub fn send(&mut self, cmd: Cmd) {
        self.inbox.borrow_mut().cmds.push_back(cmd);
        Inbox::wake(&self.inbox);
    }

    pub fn try_recv(&mut self) -> Option<Event> {
        self.events.borrow_mut().pop_front()
    }

    /// The next answer already here. The page cannot wait for one that is not.
    pub fn recv(&mut self) -> Option<Event> {
        self.try_recv()
    }

    /// Run every command already sent, then let the library go. The page cannot wait,
    /// so they run after this returns, and nothing hears their answers.
    pub fn finish(&mut self) {
        self.inbox.borrow_mut().closed = true;
        Inbox::wake(&self.inbox);
    }
}

impl Drop for Backend {
    fn drop(&mut self) {
        self.finish();
    }
}

/// Run each command as it arrives, until the backend is finished.
async fn drive(
    root: Root,
    inbox: Rc<RefCell<Inbox>>,
    events: Rc<RefCell<VecDeque<Event>>>,
    room: Rc<RefCell<Room>>,
    ctx: egui::Context,
) {
    let private = root == Root::Private;
    let mut fs = Folder::open(root, room.clone(), inbox.clone()).await;
    while let Some(cmd) = next(&inbox).await {
        let mut answer = |event| {
            events.borrow_mut().push_back(event);
            ctx.request_repaint();
        };
        match &mut fs {
            Ok(fs) => {
                exec::run(fs, cmd, &mut answer).await;
                fs.dirs.borrow_mut().clear();
            }
            Err(why) => answer(refused(cmd, why)),
        }
        if private && inbox.borrow().cmds.is_empty() {
            measure(&room).await;
        }
        ctx.request_repaint();
    }
}

/// The next command, waiting for one while there is none, or `None` once the backend
/// is finished and none is left.
async fn next(inbox: &Rc<RefCell<Inbox>>) -> Option<Cmd> {
    loop {
        {
            let mut held = inbox.borrow_mut();
            if let Some(cmd) = held.cmds.pop_front() {
                return Some(cmd);
            }
            if held.closed {
                return None;
            }
        }
        let arrives = Promise::new(&mut |resolve, _| inbox.borrow_mut().wake = Some(resolve));
        let _ = JsFuture::from(arrives).await;
    }
}

/// The answer to a command when there is no library to run it on.
fn refused(cmd: Cmd, why: &str) -> Event {
    match cmd {
        Cmd::Open => Event::Opened(Err(why.to_string())),
        Cmd::Scan { .. } => Event::Scanned(Err(why.to_string())),
        Cmd::Check { .. } => Event::Checked(Err(why.to_string())),
        Cmd::Walk(dir) => Event::Walked { dir, ran: 0 },
        Cmd::Fingerprint(_) => Event::Fingerprinted(Vec::new()),
        Cmd::Read { files, .. } => Event::Read(
            files
                .into_iter()
                .map(|(id, _, _)| (id, Err(Failure::Io(why.to_string()))))
                .collect(),
        ),
        Cmd::Save { id, path, .. } => Event::Saved {
            id,
            path,
            result: Err(Failure::Io(why.to_string())),
        },
        Cmd::Import { id, path, .. } => Event::Imported {
            id,
            path,
            result: Err(Failure::Io(why.to_string())),
        },
        Cmd::Rewrite { id, path, .. } => Event::Rewritten {
            id,
            path,
            result: Err(Failure::Io(why.to_string())),
        },
        Cmd::Move { from, to } => Event::Moved {
            from,
            to,
            result: Err(why.to_string()),
        },
        _ => Event::Failed(why.to_string()),
    }
}

fn storage() -> Option<StorageManager> {
    let navigator = web_sys::window()?.navigator();
    Some(field(&navigator, "storage")?.unchecked_into())
}

/// Read how much the origin uses, and whether its storage is kept.
///
/// ⚠️ `estimate()` arrived in Safari 17, so it is looked up before it is called.
async fn measure(room: &Rc<RefCell<Room>>) {
    let Some(storage) = storage() else {
        return;
    };
    if let Ok(asked) = storage.persisted() {
        if let Ok(kept) = JsFuture::from(asked).await {
            room.borrow_mut().kept = kept.as_bool();
        }
    }
    if field(&storage, "estimate").is_none() {
        return;
    }
    let Some(estimate) = storage.estimate().ok() else {
        return;
    };
    if let Ok(estimate) = JsFuture::from(estimate).await {
        let get = |name| Some(field(&estimate, name)?.as_f64()? as u64);
        if let (Some(used), Some(quota)) = (get("usage"), get("quota")) {
            room.borrow_mut().used = Some((used, quota));
        }
    }
}

/// Ask the browser to keep drawbar's files through a shortage of space. The answer
/// comes whenever it comes; Firefox asks the user first.
fn ask_to_keep(room: Rc<RefCell<Room>>) {
    let Some(asked) = storage().and_then(|storage| storage.persist().ok()) else {
        return;
    };
    spawn_local(async move {
        if let Ok(kept) = JsFuture::from(asked).await {
            room.borrow_mut().kept = kept.as_bool();
        }
    });
}

/// An error from the browser, as the kind of I/O error it is.
fn failure(name: &str, message: &str) -> io::Error {
    let kind = match name {
        "NotFoundError" => io::ErrorKind::NotFound,
        "QuotaExceededError" => io::ErrorKind::StorageFull,
        _ => io::ErrorKind::Other,
    };
    match name {
        "" | "Error" => io::Error::new(kind, message.to_string()),
        _ => io::Error::new(kind, format!("{name}: {message}")),
    }
}

fn failed(err: JsValue) -> io::Error {
    let text = |name| field(&err, name).and_then(|value| value.as_string());
    match (text("name"), text("message")) {
        (Some(name), Some(message)) => failure(&name, &message),
        _ => io::Error::other(describe(&err)),
    }
}

async fn settle<T: wasm_bindgen::JsCast>(promise: Promise) -> io::Result<T> {
    let value = JsFuture::from(promise).await.map_err(failed)?;
    value
        .dyn_into()
        .map_err(|value| io::Error::other(format!("the browser answered {value:?}")))
}

/// The dedicated worker that writes files, and the requests waiting on its answers.
struct Writer {
    worker: Worker,
    waiting: Rc<RefCell<BTreeMap<u32, Function>>>,
    next: Cell<u32>,
    /// Why the worker stopped, once it has.
    stopped: Rc<RefCell<Option<String>>>,
    _hear: Closure<dyn FnMut(MessageEvent)>,
    _fail: Closure<dyn FnMut(JsValue)>,
}

impl Writer {
    fn start() -> io::Result<Writer> {
        let worker = Worker::new(&writer_url()).map_err(failed)?;
        let waiting: Rc<RefCell<BTreeMap<u32, Function>>> = Rc::default();
        let stopped: Rc<RefCell<Option<String>>> = Rc::default();
        let hear = {
            let waiting = waiting.clone();
            Closure::<dyn FnMut(MessageEvent)>::new(move |event: MessageEvent| {
                let reply = event.data();
                let Some(id) = field(&reply, "id").and_then(|id| id.as_f64()) else {
                    return;
                };
                if let Some(resolve) = waiting.borrow_mut().remove(&(id as u32)) {
                    let _ = resolve.call1(&JsValue::NULL, &reply);
                }
            })
        };
        let fail = {
            let (waiting, stopped) = (waiting.clone(), stopped.clone());
            Closure::<dyn FnMut(JsValue)>::new(move |event: JsValue| {
                let why = field(&event, "message")
                    .and_then(|message| message.as_string())
                    .unwrap_or_else(|| "it did not load".to_string());
                let why = format!("drawbar's library writer stopped: {why}");
                let reply = Object::new();
                let _ = Reflect::set(&reply, &"message".into(), &why.as_str().into());
                *stopped.borrow_mut() = Some(why);
                for (_, resolve) in std::mem::take(&mut *waiting.borrow_mut()) {
                    let _ = resolve.call1(&JsValue::NULL, &reply);
                }
            })
        };
        worker.set_onmessage(Some(hear.as_ref().unchecked_ref()));
        worker.set_onerror(Some(fail.as_ref().unchecked_ref()));
        Ok(Writer {
            worker,
            waiting,
            next: Cell::new(0),
            stopped,
            _hear: hear,
            _fail: fail,
        })
    }

    /// Send one request, with `data` handed over rather than copied, and wait for its
    /// answer.
    async fn ask(&self, op: &str, path: &str, extra: &[(&str, JsValue)]) -> io::Result<JsValue> {
        if let Some(why) = self.stopped.borrow().clone() {
            return Err(io::Error::other(why));
        }
        let id = self.next.get();
        self.next.set(id.wrapping_add(1));
        let request = Object::new();
        let set = |key: &str, value: &JsValue| Reflect::set(&request, &key.into(), value);
        set("id", &id.into()).map_err(failed)?;
        set("op", &op.into()).map_err(failed)?;
        set("path", &path.into()).map_err(failed)?;
        let transfer = Array::new();
        for (key, value) in extra {
            set(key, value).map_err(failed)?;
            if value.is_instance_of::<js_sys::ArrayBuffer>() {
                transfer.push(value);
            }
        }
        let answered = Promise::new(&mut |resolve, _| {
            self.waiting.borrow_mut().insert(id, resolve);
        });
        if let Err(e) = self.worker.post_message_with_transfer(&request, &transfer) {
            self.waiting.borrow_mut().remove(&id);
            return Err(failed(e));
        }
        let reply = JsFuture::from(answered).await.map_err(failed)?;
        if field(&reply, "ok").and_then(|ok| ok.as_bool()) == Some(true) {
            return Ok(field(&reply, "value").unwrap_or(JsValue::UNDEFINED));
        }
        let text = |name| field(&reply, name).and_then(|value| value.as_string());
        Err(failure(
            &text("name").unwrap_or_default(),
            &text("message").unwrap_or_default(),
        ))
    }

    /// Write `contents` to a new file at `temp`, flushed, in chunks. A file begun and not
    /// finished is deleted.
    async fn write(&self, temp: &str, contents: Contents<'_>) -> io::Result<()> {
        let wrote = async {
            self.ask("begin", temp, &[]).await?;
            contents
                .each(async |at, data| {
                    let at = (at as f64).into();
                    let data = data.into();
                    self.ask("write", temp, &[("at", at), ("data", data)])
                        .await
                        .map(|_| ())
                })
                .await?;
            self.ask("end", temp, &[]).await
        }
        .await;
        if wrote.is_err() {
            let _ = self.ask("abandon", temp, &[]).await;
        }
        wrote.map(|_| ())
    }
}

/// What a write puts in a file: bytes this tab holds, a file the browser handed it,
/// which crosses to the file a slice at a time without passing through this tab's memory,
/// or an edit of a file resting in the library, laid out as the pieces of that file it
/// keeps and the bytes the edit holds.
#[derive(Clone, Copy)]
enum Contents<'a> {
    Bytes(&'a [u8]),
    Blob(&'a web_sys::Blob),
    Edited(&'a Pieces, &'a File),
}

impl Contents<'_> {
    /// Hand the file to `write` a chunk at a time, each with where it goes.
    async fn each(
        &self,
        mut write: impl AsyncFnMut(u64, js_sys::ArrayBuffer) -> io::Result<()>,
    ) -> io::Result<()> {
        match self {
            Contents::Bytes(bytes) => {
                let read = async |range: Range<u64>| {
                    Ok(buffer(&bytes[range.start as usize..range.end as usize]))
                };
                chunked(bytes.len() as u64, read, write).await
            }
            Contents::Blob(blob) => {
                let read = async |range: Range<u64>| {
                    let slice = blob
                        .slice_with_f64_and_f64(range.start as f64, range.end as f64)
                        .map_err(failed)?;
                    let data: js_sys::ArrayBuffer = settle(slice.array_buffer()).await?;
                    match u64::from(data.byte_length()) == range.end - range.start {
                        true => Ok(data),
                        false => Err(io::Error::other("the file changed while it was copied")),
                    }
                };
                chunked(blob.size() as u64, read, write).await
            }
            Contents::Edited(pieces, from) => {
                let read = async |range| crate::ondisk::slice(from, range).await;
                let put = async |at, bytes: Vec<u8>| write(at, buffer(&bytes)).await;
                crate::rewrite::stream(pieces, read, put).await
            }
        }
    }
}

/// Hand `len` bytes, each chunk `read` gives, to `write`, in order.
async fn chunked(
    len: u64,
    mut read: impl AsyncFnMut(Range<u64>) -> io::Result<js_sys::ArrayBuffer>,
    mut write: impl AsyncFnMut(u64, js_sys::ArrayBuffer) -> io::Result<()>,
) -> io::Result<()> {
    let mut at = 0;
    while at < len {
        let end = len.min(at + CHUNK as u64);
        write(at, read(at..end).await?).await?;
        at = end;
    }
    Ok(())
}

/// `bytes`, copied into a buffer of their own that can be handed to the writer.
fn buffer(bytes: &[u8]) -> js_sys::ArrayBuffer {
    let data = Uint8Array::new_with_length(bytes.len() as u32);
    data.copy_from(bytes);
    data.buffer()
}

/// The writer, started if it has not been.
fn started(writer: &mut Option<Writer>) -> io::Result<&Writer> {
    if writer.is_none() {
        *writer = Some(Writer::start()?);
    }
    Ok(writer.as_ref().expect("started above"))
}

impl Drop for Writer {
    /// Stopping the worker closes its handles, the lock's among them.
    fn drop(&mut self) {
        self.worker.terminate();
    }
}

/// The Web Lock held while this tab writes a picked folder. Letting go of it releases
/// the lock.
struct Held(Function);

impl Drop for Held {
    fn drop(&mut self) {
        let _ = self.0.call0(&JsValue::NULL);
    }
}

/// Take the Web Lock `name` if no other tab of this site holds it, and hold it until the
/// answer is dropped. `None` when another tab holds it.
async fn hold(name: &str) -> io::Result<Option<Held>> {
    let navigator = web_sys::window()
        .ok_or_else(|| io::Error::other("the page has no window"))?
        .navigator();
    if field(&navigator, "locks").is_none() {
        return Err(io::Error::other("this browser cannot lock a folder"));
    }
    let mut answer = None;
    let answered = Promise::new(&mut |resolve, _| answer = Some(resolve));
    let answer = answer.expect("a promise runs its executor at once");
    let granted = Closure::once_into_js(move |lock: JsValue| -> Promise {
        if lock.is_null() {
            let _ = answer.call1(&JsValue::NULL, &JsValue::FALSE);
            return Promise::resolve(&JsValue::UNDEFINED);
        }
        let mut release = None;
        let holding = Promise::new(&mut |resolve, _| release = Some(resolve));
        let _ = answer.call1(&JsValue::NULL, &release.into());
        holding
    });
    let options = LockOptions::new();
    options.set_if_available(true);
    // Settles only once the lock is let go, or at once when the request is refused.
    let requested = navigator.locks().request_with_options_and_callback(
        name,
        &options,
        granted.unchecked_ref(),
    );
    let first = Promise::race(&Array::of2(&answered, &requested));
    let answer = JsFuture::from(first).await.map_err(failed)?;
    Ok(answer.dyn_into::<Function>().ok().map(Held))
}

/// How a library's files are written, and how a second tab is kept to reading it.
enum Writes {
    /// Through `library-writer.js`, which holds `.drawbar/lock`. The origin private file
    /// system only. Started with the first write, since a library only read needs none.
    Worker(Option<Writer>),
    /// Through a writable stream on the page, for a picked folder, under the Web Lock
    /// named `lock` while `held`.
    Streams { lock: String, held: Option<Held> },
}

/// A library's folder: the origin private file system's root, or a folder the user
/// picked.
struct Folder {
    root: FileSystemDirectoryHandle,
    writes: Writes,
    /// `.drawbar/` and its folders have been made, once, for this page.
    prepared: bool,
    /// Names the next temporary file under `.drawbar/tmp/`.
    temps: u64,
    room: Rc<RefCell<Room>>,
    /// Whether the browser has been asked to keep the files.
    asked: bool,
    /// The commands for this library, closed once it is let go.
    inbox: Rc<RefCell<Inbox>>,
    /// The folders found so far by the command running, by path, so a path is not looked
    /// up again from the root one name at a time.
    ///
    /// ⚠️ Safari and Firefox keep a handle on its folder wherever it moves, so the handles
    /// are let go after each command, and after every move, new folder or removal this
    /// tab makes, whether it succeeded or not.
    dirs: RefCell<HashMap<String, FileSystemDirectoryHandle>>,
    /// The files left resting, each at the path it is at now, so a move takes their
    /// snapshots again where they went.
    resting: RefCell<Vec<(Weak<OnDisk>, String)>>,
}

/// A folder of the library and the name of one entry in it.
type Spot = (FileSystemDirectoryHandle, String);

impl Folder {
    /// The folder at `root`, or why this browser gives the page no storage. Some private
    /// windows refuse it.
    async fn open(
        root: Root,
        room: Rc<RefCell<Room>>,
        inbox: Rc<RefCell<Inbox>>,
    ) -> Result<Folder, String> {
        let (root, writes) = match root {
            Root::Private => (Folder::private().await?, Writes::Worker(None)),
            Root::Picked(picked) => (
                picked.handle,
                Writes::Streams {
                    lock: format!("drawbar library {}", picked.id),
                    held: None,
                },
            ),
        };
        Ok(Folder {
            root,
            writes,
            prepared: false,
            temps: 0,
            room,
            asked: false,
            inbox,
            dirs: RefCell::default(),
            resting: RefCell::default(),
        })
    }

    async fn private() -> Result<FileSystemDirectoryHandle, String> {
        let unkept = |why: String| format!("this browser gives drawbar no storage here: {why}");
        let storage = storage()
            .filter(|storage| field(storage, "getDirectory").is_some())
            .ok_or_else(|| unkept("it has no origin private file system".to_string()))?;
        settle(storage.get_directory())
            .await
            .map_err(|e| unkept(e.to_string()))
    }

    /// The folder at `path`, made where missing when `create` is set.
    async fn dir(&self, path: &str, create: bool) -> io::Result<FileSystemDirectoryHandle> {
        let options = FileSystemGetDirectoryOptions::new();
        options.set_create(create);
        let mut dir = self.root.clone();
        let mut at = String::new();
        for name in path.split('/').filter(|name| !name.is_empty()) {
            at = joined(&at, name);
            let known = self.dirs.borrow().get(&at).cloned();
            dir = match known {
                Some(known) => known,
                None => {
                    let found: FileSystemDirectoryHandle =
                        settle(dir.get_directory_handle_with_options(name, &options)).await?;
                    self.dirs.borrow_mut().insert(at.clone(), found.clone());
                    found
                }
            };
        }
        Ok(dir)
    }

    /// The folder `path` is in, which must exist, and its last name.
    async fn spot(&self, path: &str) -> io::Result<Spot> {
        let (parent, leaf) = path.rsplit_once('/').unwrap_or(("", path));
        Ok((self.dir(parent, false).await?, leaf.to_string()))
    }

    async fn file(&self, path: &str) -> io::Result<FileSystemFileHandle> {
        let (dir, leaf) = self.spot(path).await?;
        settle(dir.get_file_handle(&leaf)).await
    }

    /// What is at `path`, file or folder.
    async fn handle(&self, path: &str) -> io::Result<FileSystemHandle> {
        let (dir, leaf) = self.spot(path).await?;
        match settle::<FileSystemHandle>(dir.get_file_handle(&leaf)).await {
            Err(e) if e.to_string().starts_with("TypeMismatchError") => {
                settle(dir.get_directory_handle(&leaf)).await
            }
            found => found,
        }
    }

    /// Whether anything, file or folder, is at `path`.
    async fn taken(&self, path: &str) -> io::Result<bool> {
        match self.handle(path).await {
            Ok(_) => Ok(true),
            Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(false),
            Err(e) => Err(e),
        }
    }

    /// Write `contents` to a new file under `.drawbar/tmp/`, flushed, and return its path.
    ///
    /// The file takes `path`'s extension: Chrome reads a file moved to a new extension
    /// whole, for a Safe Browsing check, before it lets the move land.
    async fn stage(&mut self, path: &str, contents: Contents<'_>) -> io::Result<String> {
        let extension = path
            .rsplit('/')
            .next()
            .and_then(|leaf| leaf.rsplit_once('.'))
            .map_or(String::new(), |(_, extension)| format!(".{extension}"));
        let temp = format!("{TMP}/{}{extension}", self.temps);
        self.temps += 1;
        let private = matches!(self.writes, Writes::Worker(_));
        if private && !std::mem::replace(&mut self.asked, true) {
            ask_to_keep(self.room.clone());
        }
        let wrote = match &mut self.writes {
            Writes::Worker(writer) => started(writer)?.write(&temp, contents).await,
            Writes::Streams { .. } => self.stream(&temp, contents).await,
        };
        match wrote {
            Ok(()) => Ok(temp),
            Err(e) if private && e.kind() == io::ErrorKind::StorageFull => Err(self.full().await),
            Err(e) => Err(e),
        }
    }

    /// Write `contents` to a new file at `temp` through a writable stream, which the
    /// browser applies to the file only as it closes.
    async fn stream(&self, temp: &str, contents: Contents<'_>) -> io::Result<()> {
        let (dir, leaf) = self.spot(temp).await?;
        let options = FileSystemGetFileOptions::new();
        options.set_create(true);
        let file: FileSystemFileHandle =
            settle(dir.get_file_handle_with_options(&leaf, &options)).await?;
        let stream: FileSystemWritableFileStream = settle(file.create_writable()).await?;
        let wrote = async {
            let mut end = 0;
            contents
                .each(async |at, data| {
                    if at != end {
                        let seeking = stream.seek_with_f64(at as f64).map_err(failed)?;
                        JsFuture::from(seeking).await.map_err(failed)?;
                    }
                    end = at + u64::from(data.byte_length());
                    let writing = stream.write_with_buffer_source(&data).map_err(failed)?;
                    JsFuture::from(writing).await.map_err(failed)?;
                    Ok(())
                })
                .await?;
            JsFuture::from(stream.close()).await.map_err(failed)
        }
        .await;
        if let Err(e) = wrote {
            let _ = JsFuture::from(stream.abort()).await;
            let _ = JsFuture::from(dir.remove_entry(&leaf)).await;
            return Err(e);
        }
        Ok(())
    }

    /// Why a write that ran out of room failed, in the terms the user can act on.
    async fn full(&self) -> io::Error {
        measure(&self.room).await;
        let room = self.room.borrow().used;
        let used = room.map_or(String::new(), |(used, quota)| {
            format!(" ({} of {} used)", size(used), size(quota))
        });
        io::Error::new(
            io::ErrorKind::StorageFull,
            format!("this browser has no room left for drawbar's files{used}"),
        )
    }

    /// Move the file at `temp` to `path`, over whatever file is there.
    ///
    /// ⚠️ `move(folder, name)` with both arguments: Safari has no one-argument form.
    /// Chrome, Firefox and Safari all replace a file already at the name.
    async fn place(&self, temp: &str, path: &str) -> io::Result<()> {
        let placed = async {
            let file = self.file(temp).await?;
            let (dir, leaf) = self.spot(path).await?;
            move_to(&file, &dir, &leaf).await
        }
        .await;
        if placed.is_err() {
            if let Ok((dir, leaf)) = self.spot(temp).await {
                let _ = JsFuture::from(dir.remove_entry(&leaf)).await;
            }
        }
        placed
    }

    /// [`Fs::rename`], leaving the folder handles as they are.
    ///
    /// ⚠️ Chrome cannot move a folder whole, so there a folder moves file by file, and
    /// one interrupted leaves its files split between the two names, none lost.
    async fn relocate(&self, from: &str, to: &str) -> io::Result<()> {
        if to == from || names::inside(to, from) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "it cannot move into itself",
            ));
        }
        let same = from.rsplit_once('/').map(|(dir, _)| dir)
            == to.rsplit_once('/').map(|(dir, _)| dir)
            && names::key(from) == names::key(to);
        if !same && self.taken(to).await? {
            return Err(io::ErrorKind::AlreadyExists.into());
        }
        let handle = self.handle(from).await?;
        let (dir, leaf) = self.spot(to).await?;
        if handle.kind() == FileSystemHandleKind::File || field(&handle, "move").is_some() {
            return move_to(&handle, &dir, &leaf).await;
        }
        move_tree(handle.unchecked_ref(), &dir, &leaf).await?;
        let (parent, name) = self.spot(from).await?;
        JsFuture::from(parent.remove_entry(&name))
            .await
            .map_err(failed)?;
        Ok(())
    }

    /// Take again the snapshot of every resting file a move from `from` to `to` carried,
    /// where it is now. One that cannot be taken stays as it was, and its reads fail.
    async fn follow(&self, from: &str, to: &str) {
        self.resting
            .borrow_mut()
            .retain(|(file, _)| file.strong_count() > 0);
        let carried: Vec<(Arc<OnDisk>, String)> = self
            .resting
            .borrow_mut()
            .iter_mut()
            .filter_map(|(file, path)| {
                let rest = match path.strip_prefix(from)? {
                    rest if rest.is_empty() || rest.starts_with('/') => rest.to_string(),
                    _ => return None,
                };
                *path = format!("{to}{rest}");
                Some((file.upgrade()?, path.clone()))
            })
            .collect();
        for (file, path) in carried {
            if let Ok(handle) = self.file(&path).await {
                if let Ok(snapshot) = snapshot(&handle).await {
                    file.resnapshot(snapshot);
                }
            }
        }
    }

    /// Stop following the resting files at `path`, which was written over or deleted:
    /// what they rest in is gone.
    fn forget(&self, path: &str) {
        self.resting
            .borrow_mut()
            .retain(|(file, at)| at != path && file.strong_count() > 0);
    }

    /// [`Fs::remove_dir`], leaving the folder handles as they are.
    async fn unmake(&self, path: &str) -> io::Result<()> {
        let (dir, leaf) = self.spot(path).await?;
        settle::<FileSystemDirectoryHandle>(dir.get_directory_handle(&leaf)).await?;
        JsFuture::from(dir.remove_entry(&leaf))
            .await
            .map_err(failed)?;
        Ok(())
    }
}

/// Call `move(dir, name)` on a file or folder handle.
async fn move_to(handle: &JsValue, dir: &FileSystemDirectoryHandle, name: &str) -> io::Result<()> {
    let call = field(handle, "move")
        .and_then(|call| call.dyn_into::<Function>().ok())
        .ok_or_else(|| io::Error::other("this browser cannot move a file"))?;
    let moving = call
        .call2(handle, dir, &name.into())
        .map_err(failed)?
        .dyn_into::<Promise>()
        .map_err(|_| io::Error::other("this browser cannot move a file"))?;
    JsFuture::from(moving).await.map_err(failed)?;
    Ok(())
}

/// `name` in the folder at `dir`, both joined by `/`.
fn joined(dir: &str, name: &str) -> String {
    match dir.is_empty() {
        true => name.to_string(),
        false => format!("{dir}/{name}"),
    }
}

/// The entries of a folder, sorted by name.
async fn entries(dir: &FileSystemDirectoryHandle) -> io::Result<Vec<(String, FileSystemHandle)>> {
    let iterator = dir.entries();
    let mut found = Vec::new();
    loop {
        let step = JsFuture::from(iterator.next().map_err(failed)?)
            .await
            .map_err(failed)?;
        if field(&step, "done").and_then(|done| done.as_bool()) == Some(true) {
            break;
        }
        let pair: Array = field(&step, "value")
            .ok_or_else(|| io::Error::other("a folder listed an empty entry"))?
            .unchecked_into();
        let (Some(name), Ok(handle)) = (
            pair.get(0).as_string(),
            pair.get(1).dyn_into::<FileSystemHandle>(),
        ) else {
            continue;
        };
        found.push((name, handle));
    }
    found.sort_by(|a, b| a.0.cmp(&b.0));
    Ok(found)
}

fn stat(file: &File) -> Stat {
    let modified = file.last_modified();
    Stat {
        len: file.size() as u64,
        modified: (modified.is_finite() && modified >= 0.0)
            .then(|| (modified as u64).saturating_mul(1_000_000)),
    }
}

async fn snapshot(handle: &FileSystemFileHandle) -> io::Result<File> {
    settle(handle.get_file()).await
}

/// Whether the page may read and write the folder at `dir`, asking the user when `ask`
/// is set and the browser has not said.
///
/// ⚠️ Asking needs a click the user has just made.
pub async fn permission(dir: &FileSystemDirectoryHandle, ask: bool) -> io::Result<bool> {
    let descriptor = FileSystemHandlePermissionDescriptor::new();
    descriptor.set_mode(FileSystemPermissionMode::Readwrite);
    let state = match ask {
        false => dir.query_permission_with_descriptor(&descriptor),
        true => dir.request_permission_with_descriptor(&descriptor),
    };
    let state = JsFuture::from(state.unchecked_into::<Promise>())
        .await
        .map_err(failed)?;
    Ok(state.as_string().as_deref() == Some("granted"))
}

/// Move everything in `from` into a folder `name` of `into`, file by file, and remove
/// each folder it empties. For browsers that cannot move a folder whole.
fn move_tree<'a>(
    from: &'a FileSystemDirectoryHandle,
    into: &'a FileSystemDirectoryHandle,
    name: &'a str,
) -> Pin<Box<dyn Future<Output = io::Result<()>> + 'a>> {
    Box::pin(async move {
        let options = FileSystemGetDirectoryOptions::new();
        options.set_create(true);
        let to: FileSystemDirectoryHandle =
            settle(into.get_directory_handle_with_options(name, &options)).await?;
        for (leaf, handle) in entries(from).await? {
            match handle.kind() {
                FileSystemHandleKind::Directory => {
                    move_tree(handle.unchecked_ref(), &to, &leaf).await?;
                    JsFuture::from(from.remove_entry(&leaf))
                        .await
                        .map_err(failed)?;
                }
                _ => move_to(&handle, &to, &leaf).await?,
            }
        }
        Ok(())
    })
}

impl Fs for Folder {
    fn stopped(&self) -> bool {
        self.inbox.borrow().closed
    }

    fn waiting(&mut self) -> Option<Cmd> {
        self.inbox.borrow_mut().cmds.pop_front()
    }

    fn hold(&mut self, cmd: Cmd) {
        self.inbox.borrow_mut().cmds.push_front(cmd);
    }

    async fn prepare(&mut self) -> io::Result<()> {
        if self.prepared {
            return Ok(());
        }
        for dir in [TMP, WORKING] {
            self.dir(dir, true).await?;
        }
        if let Writes::Worker(writer) = &mut self.writes {
            started(writer)?;
        }
        self.prepared = true;
        Ok(())
    }

    async fn lock(&mut self) -> io::Result<bool> {
        match &mut self.writes {
            Writes::Worker(writer) => {
                let held = started(writer)?.ask("lock", LOCK, &[]).await?;
                Ok(held.as_bool() == Some(true))
            }
            Writes::Streams { held: Some(_), .. } => Ok(true),
            Writes::Streams { lock, held } => {
                *held = hold(lock).await?;
                Ok(held.is_some())
            }
        }
    }

    /// A picked folder is written only while the browser lets the page write it.
    async fn probe(&mut self) -> io::Result<()> {
        if matches!(self.writes, Writes::Worker(_)) {
            return Ok(());
        }
        match permission(&self.root, false).await? {
            true => Ok(()),
            false => Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "this browser does not let drawbar write to the folder",
            )),
        }
    }

    /// Every file's snapshot is asked for before any is waited on, so the browser looks
    /// them up together.
    async fn children(&self, dir: &str, room: usize) -> io::Result<Children> {
        let found = entries(&self.dir(dir, false).await?).await?;
        let more = found.len() > room;
        let found: Vec<(String, FileSystemHandle)> = found.into_iter().take(room).collect();
        let snapshots: Vec<Option<JsFuture>> = found
            .iter()
            .map(|(name, handle)| {
                let file = handle.kind() == FileSystemHandleKind::File && exec::opens(name);
                file.then(|| {
                    JsFuture::from(handle.unchecked_ref::<FileSystemFileHandle>().get_file())
                })
            })
            .collect();
        let mut children = Vec::new();
        for ((name, handle), snapshot) in found.into_iter().zip(snapshots) {
            let kind = match (handle.kind(), snapshot) {
                (FileSystemHandleKind::Directory, _) => {
                    let path = joined(dir, &name);
                    self.dirs.borrow_mut().insert(path, handle.unchecked_into());
                    Some(Kind::Dir)
                }
                (_, Some(snapshot)) => match snapshot.await.map_err(failed) {
                    Ok(file) => Some(Kind::File(stat(file.unchecked_ref()))),
                    Err(e) if e.kind() == io::ErrorKind::NotFound => None,
                    Err(e) => Some(Kind::Unread(e.to_string())),
                },
                _ => Some(Kind::Other),
            };
            children.push((name, kind));
        }
        Ok((children, more))
    }

    async fn names(&self, dir: &str) -> io::Result<Vec<String>> {
        let dir = self.dir(dir, false).await?;
        Ok(entries(&dir)
            .await?
            .into_iter()
            .map(|(name, _)| name)
            .collect())
    }

    /// ⚠️ Each read takes a fresh snapshot of the file. A snapshot taken before a write
    /// fails to read afterwards rather than mixing old bytes with new.
    async fn read(&self, path: &str) -> io::Result<Vec<u8>> {
        let file = snapshot(&self.file(path).await?).await?;
        let whole = file.size() as u64;
        let unfit = || {
            io::Error::new(
                io::ErrorKind::OutOfMemory,
                format!("{} does not fit in this tab's memory", size(whole)),
            )
        };
        let len = usize::try_from(whole).map_err(|_| unfit())?;
        let mut bytes = Vec::new();
        bytes.try_reserve_exact(len).map_err(|_| unfit())?;
        while bytes.len() < len {
            let at = bytes.len();
            let end = len.min(at + CHUNK);
            let chunk = crate::ondisk::slice(&file, at as u64..end as u64).await?;
            bytes.extend_from_slice(&chunk);
        }
        Ok(bytes)
    }

    async fn stat(&self, path: &str) -> io::Result<Option<Stat>> {
        match self.handle(path).await {
            Ok(handle) if handle.kind() == FileSystemHandleKind::Directory => Ok(None),
            Ok(handle) => Ok(Some(stat(&snapshot(handle.unchecked_ref()).await?))),
            Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(e),
        }
    }

    /// Every file is asked for before any is waited on, and then every snapshot, so the
    /// browser looks them up together.
    async fn stats(&self, paths: &[&str]) -> Vec<io::Result<Option<Stat>>> {
        let mut handles = Vec::with_capacity(paths.len());
        for path in paths {
            handles.push(match self.spot(path).await {
                Ok((dir, leaf)) => Ok(JsFuture::from(dir.get_file_handle(&leaf))),
                Err(e) => Err(e),
            });
        }
        let mut snapshots = Vec::with_capacity(paths.len());
        for handle in handles {
            snapshots.push(match handle {
                Ok(handle) => handle.await.map_err(failed).map(|handle| {
                    JsFuture::from(handle.unchecked_into::<FileSystemFileHandle>().get_file())
                }),
                Err(e) => Err(e),
            });
        }
        let mut stats = Vec::with_capacity(paths.len());
        for (path, snapshot) in paths.iter().zip(snapshots) {
            let snapshot = match snapshot {
                Ok(snapshot) => snapshot.await.map_err(failed),
                Err(e) => Err(e),
            };
            stats.push(match snapshot {
                Ok(file) => Ok(Some(stat(file.unchecked_ref()))),
                Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(None),
                Err(e) if e.to_string().starts_with("TypeMismatchError") => self.stat(path).await,
                Err(e) => Err(e),
            });
        }
        stats
    }

    /// ⚠️ The check and the move are two steps. No other drawbar writes between them,
    /// because this tab holds the library's lock, but in a picked folder another program
    /// may, and the move replaces what it wrote.
    async fn create(&mut self, path: &str, bytes: &[u8]) -> io::Result<()> {
        if self.taken(path).await? {
            return Err(io::ErrorKind::AlreadyExists.into());
        }
        let temp = self.stage(path, Contents::Bytes(bytes)).await?;
        self.place(&temp, path).await
    }

    async fn replace(&mut self, path: &str, bytes: &[u8]) -> io::Result<()> {
        let temp = self.stage(path, Contents::Bytes(bytes)).await?;
        self.forget(path);
        self.place(&temp, path).await
    }

    /// ⚠️ As [`Fs::create`], the check and the move are two steps.
    async fn copy_in(&mut self, path: &str, from: &Outside, over: bool) -> io::Result<()> {
        if !over && self.taken(path).await? {
            return Err(io::ErrorKind::AlreadyExists.into());
        }
        let temp = self.stage(path, Contents::Blob(from)).await?;
        if over {
            self.forget(path);
        }
        self.place(&temp, path).await
    }

    /// ⚠️ As [`Fs::create`], the check and the move are two steps.
    async fn copy(&mut self, path: &str, from: &str, over: bool) -> io::Result<()> {
        let from = snapshot(&self.file(from).await?).await?;
        self.copy_in(path, &from, over).await
    }

    /// ⚠️ As [`Fs::create`], the check and the move are two steps.
    async fn rewrite(
        &mut self,
        path: &str,
        from: &OnDisk,
        edit: &Rewrite,
        over: Option<Stat>,
    ) -> io::Result<()> {
        if over.is_none() && self.taken(path).await? {
            return Err(io::ErrorKind::AlreadyExists.into());
        }
        let snapshot = from
            .snapshot()
            .ok_or_else(|| io::Error::other("drawbar no longer reads that file"))?;
        let pieces = edit.pieces(from)?;
        let temp = self
            .stage(path, Contents::Edited(&pieces, &snapshot))
            .await?;
        if let Some(held) = over {
            if !matches!(self.stat(path).await, Ok(Some(now)) if now == held) {
                if let Ok((dir, leaf)) = self.spot(&temp).await {
                    let _ = JsFuture::from(dir.remove_entry(&leaf)).await;
                }
                return Err(crate::rewrite::changed(
                    "the file changed while its edit was written".into(),
                ));
            }
            self.forget(path);
        }
        self.place(&temp, path).await
    }

    async fn rename(&mut self, from: &str, to: &str) -> io::Result<()> {
        let moved = self.relocate(from, to).await;
        self.dirs.get_mut().clear();
        if moved.is_ok() {
            self.follow(from, to).await;
        }
        moved
    }

    async fn make_dir(&mut self, path: &str) -> io::Result<()> {
        let made = self.dir(path, true).await.map(|_| ());
        self.dirs.get_mut().clear();
        made
    }

    async fn remove_file(&mut self, path: &str) -> io::Result<()> {
        self.forget(path);
        let (dir, leaf) = self.spot(path).await?;
        settle::<FileSystemFileHandle>(dir.get_file_handle(&leaf)).await?;
        JsFuture::from(dir.remove_entry(&leaf))
            .await
            .map_err(failed)?;
        Ok(())
    }

    async fn remove_dir(&mut self, path: &str) -> io::Result<()> {
        let removed = self.unmake(path).await;
        self.dirs.get_mut().clear();
        removed
    }

    /// The file is indexed through a snapshot taken now, and followed through the moves
    /// this tab makes.
    async fn rest(
        &self,
        path: &str,
        known: Option<Fingerprint>,
    ) -> io::Result<Option<Arc<OnDisk>>> {
        let snapshot = snapshot(&self.file(path).await?).await?;
        let Some(file) = OnDisk::open(snapshot, known.and_then(|print| print.crc)).await? else {
            return Ok(None);
        };
        let file = Arc::new(file);
        self.resting
            .borrow_mut()
            .push((Arc::downgrade(&file), path.to_string()));
        Ok(Some(file))
    }
}
