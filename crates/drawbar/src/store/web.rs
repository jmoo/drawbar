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
use std::collections::{BTreeMap, VecDeque};
use std::future::Future;
use std::io;
use std::pin::Pin;
use std::rc::Rc;

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

use super::exec::{self, Entry, Fs, Kind, MOST_ENTRIES, TMP, WORKING};
use super::{names, Cmd, Event, Failure, Stat};
use crate::js::{describe, field};

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
    used: Option<(f64, f64)>,
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
    let mut fs = Folder::open(root, room.clone()).await;
    while let Some(cmd) = next(&inbox).await {
        let event = match &mut fs {
            Ok(fs) => exec::run(fs, cmd).await,
            Err(why) => Some(refused(cmd, why)),
        };
        if let Some(event) = event {
            events.borrow_mut().push_back(event);
        }
        if inbox.borrow().cmds.is_empty() {
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
        Cmd::Save { id, path, .. } => Event::Saved {
            id,
            path,
            result: Err(Failure::Io(why.to_string())),
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
        let get = |name| field(&estimate, name)?.as_f64();
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

/// `bytes` in the largest unit that keeps it above one.
fn size(bytes: f64) -> String {
    const K: f64 = 1024.0;
    match bytes {
        b if b < K * K => format!("{:.0} kB", b / K),
        b if b < K * K * K => format!("{:.1} MB", b / (K * K)),
        b => format!("{:.1} GB", b / (K * K * K)),
    }
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

    /// Write `bytes` to a new file at `temp`, flushed, in chunks. A file begun and not
    /// finished is deleted.
    async fn write(&self, temp: &str, bytes: &[u8]) -> io::Result<()> {
        let wrote = async {
            self.ask("begin", temp, &[]).await?;
            for (n, chunk) in bytes.chunks(CHUNK).enumerate() {
                let data = Uint8Array::new_with_length(chunk.len() as u32);
                data.copy_from(chunk);
                let at = (n * CHUNK) as f64;
                self.ask(
                    "write",
                    temp,
                    &[("at", at.into()), ("data", data.buffer().into())],
                )
                .await?;
            }
            self.ask("end", temp, &[]).await
        }
        .await;
        if wrote.is_err() {
            let _ = self.ask("abandon", temp, &[]).await;
        }
        wrote.map(|_| ())
    }
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
}

/// A folder of the library and the name of one entry in it.
type Spot = (FileSystemDirectoryHandle, String);

impl Folder {
    /// The folder at `root`, or why this browser gives the page no storage. Some private
    /// windows refuse it.
    async fn open(root: Root, room: Rc<RefCell<Room>>) -> Result<Folder, String> {
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
        for name in path.split('/').filter(|name| !name.is_empty()) {
            dir = settle(dir.get_directory_handle_with_options(name, &options)).await?;
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

    /// Write `bytes` to a new file under `.drawbar/tmp/`, flushed, and return its path.
    ///
    /// The file takes `path`'s extension: Chrome reads a file moved to a new extension
    /// whole, for a Safe Browsing check, before it lets the move land.
    async fn stage(&mut self, path: &str, bytes: &[u8]) -> io::Result<String> {
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
            Writes::Worker(writer) => started(writer)?.write(&temp, bytes).await,
            Writes::Streams { .. } => self.stream(&temp, bytes).await,
        };
        match wrote {
            Ok(()) => Ok(temp),
            Err(e) if private && e.kind() == io::ErrorKind::StorageFull => Err(self.full().await),
            Err(e) => Err(e),
        }
    }

    /// Write `bytes` to a new file at `temp` through a writable stream, which the browser
    /// applies to the file only as it closes.
    async fn stream(&self, temp: &str, bytes: &[u8]) -> io::Result<()> {
        let (dir, leaf) = self.spot(temp).await?;
        let options = FileSystemGetFileOptions::new();
        options.set_create(true);
        let file: FileSystemFileHandle =
            settle(dir.get_file_handle_with_options(&leaf, &options)).await?;
        let stream: FileSystemWritableFileStream = settle(file.create_writable()).await?;
        let wrote = async {
            for chunk in bytes.chunks(CHUNK) {
                let data = Uint8Array::new_with_length(chunk.len() as u32);
                data.copy_from(chunk);
                let writing = stream.write_with_buffer_source(&data).map_err(failed)?;
                JsFuture::from(writing).await.map_err(failed)?;
            }
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

    /// Breadth first, so the top of a large tree is listed before the bound is reached.
    async fn list(&self) -> io::Result<Vec<Entry>> {
        let mut into = Vec::new();
        let mut looked = 0;
        let mut folders = VecDeque::from([(self.root.clone(), String::new())]);
        while let Some((dir, prefix)) = folders.pop_front() {
            if looked >= MOST_ENTRIES {
                into.push(Entry {
                    path: prefix,
                    kind: Kind::Unwalked,
                });
                continue;
            }
            let found = match entries(&dir).await {
                Ok(found) => found,
                // A folder inside that cannot be read is left unlisted, not the library.
                Err(_) if !prefix.is_empty() => {
                    into.push(Entry {
                        path: prefix,
                        kind: Kind::Unwalked,
                    });
                    continue;
                }
                Err(e) => return Err(e),
            };
            let room = MOST_ENTRIES - looked;
            if found.len() > room {
                into.push(Entry {
                    path: prefix.clone(),
                    kind: Kind::Unwalked,
                });
            }
            for (name, handle) in found.into_iter().take(room) {
                looked += 1;
                let path = match prefix.is_empty() {
                    true => name.clone(),
                    false => format!("{prefix}/{name}"),
                };
                let kind = match handle.kind() {
                    FileSystemHandleKind::Directory => {
                        if !name.starts_with('.') {
                            folders.push_back((handle.unchecked_into(), path.clone()));
                        }
                        Kind::Dir
                    }
                    _ if exec::opens(&name) => {
                        Kind::File(stat(&snapshot(handle.unchecked_ref()).await?))
                    }
                    _ => Kind::Other,
                };
                into.push(Entry { path, kind });
            }
        }
        Ok(into)
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
        let len = file.size() as usize;
        let mut bytes = Vec::new();
        bytes.try_reserve_exact(len).map_err(|_| {
            io::Error::new(
                io::ErrorKind::OutOfMemory,
                format!("{} does not fit in this tab's memory", size(len as f64)),
            )
        })?;
        while bytes.len() < len {
            let at = bytes.len();
            let end = len.min(at + CHUNK);
            let slice = file
                .slice_with_f64_and_f64(at as f64, end as f64)
                .map_err(failed)?;
            let buffer: js_sys::ArrayBuffer = settle(slice.array_buffer()).await?;
            let chunk = Uint8Array::new(&buffer);
            if chunk.length() as usize != end - at {
                return Err(io::Error::other("the file changed while it was read"));
            }
            bytes.resize(end, 0);
            chunk.copy_to(&mut bytes[at..end]);
        }
        Ok(bytes)
    }

    async fn stat(&self, path: &str) -> io::Result<Option<Stat>> {
        match self.handle(path).await {
            Ok(handle) if handle.kind() == FileSystemHandleKind::Directory => Ok(Some(Stat {
                len: 0,
                modified: None,
            })),
            Ok(handle) => Ok(Some(stat(&snapshot(handle.unchecked_ref()).await?))),
            Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(e),
        }
    }

    /// ⚠️ The check and the move are two steps. No other drawbar writes between them,
    /// because this tab holds the library's lock, but in a picked folder another program
    /// may, and the move replaces what it wrote.
    async fn create(&mut self, path: &str, bytes: &[u8]) -> io::Result<()> {
        if self.stat(path).await?.is_some() {
            return Err(io::ErrorKind::AlreadyExists.into());
        }
        let temp = self.stage(path, bytes).await?;
        self.place(&temp, path).await
    }

    async fn replace(&mut self, path: &str, bytes: &[u8]) -> io::Result<()> {
        let temp = self.stage(path, bytes).await?;
        self.place(&temp, path).await
    }

    /// ⚠️ Chrome cannot move a folder whole, so there a folder moves file by file, and
    /// one interrupted leaves its files split between the two names, none lost.
    async fn rename(&mut self, from: &str, to: &str) -> io::Result<()> {
        if to == from || names::inside(to, from) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "it cannot move into itself",
            ));
        }
        let same = from.rsplit_once('/').map(|(dir, _)| dir)
            == to.rsplit_once('/').map(|(dir, _)| dir)
            && names::key(from) == names::key(to);
        if !same && self.stat(to).await?.is_some() {
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

    async fn make_dir(&mut self, path: &str) -> io::Result<()> {
        self.dir(path, true).await.map(|_| ())
    }

    async fn remove_file(&mut self, path: &str) -> io::Result<()> {
        let (dir, leaf) = self.spot(path).await?;
        settle::<FileSystemFileHandle>(dir.get_file_handle(&leaf)).await?;
        JsFuture::from(dir.remove_entry(&leaf))
            .await
            .map_err(failed)?;
        Ok(())
    }

    async fn remove_dir(&mut self, path: &str) -> io::Result<()> {
        let (dir, leaf) = self.spot(path).await?;
        settle::<FileSystemDirectoryHandle>(dir.get_directory_handle(&leaf)).await?;
        JsFuture::from(dir.remove_entry(&leaf))
            .await
            .map_err(failed)?;
        Ok(())
    }
}
