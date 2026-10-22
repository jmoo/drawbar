//! The browser's library: a folder tree in the origin private file system, which every
//! browser drawbar runs in keeps for the page and nothing else can see, or a folder on
//! this computer the user picked, in browsers that let a page open one.
//!
//! A library's facts do not open in the browser on this build: its commands on the facts
//! are refused, and the app keeps what is made in memory.
//!
//! Reads and folder changes run on the page's one thread. A file is written under
//! `.drawbar/tmp/` in chunks, then moved over its path. In the private file system it is
//! written by `library-writer.js`, a dedicated worker served beside the page, because the
//! handles that write in place exist only there. Those handles cannot reach a picked
//! folder, so there the page writes through a writable stream. Where the browser will not
//! move a file in a picked folder, the staged file is copied over its path instead, and a
//! rename copies the file and removes the old one.
//!
//! Commands run one at a time in a task of their own, in the order they were sent. A
//! library let go runs the commands it was sent before it lets go, and the next library
//! opens only then.

use std::cell::{Cell, RefCell};
use std::collections::{BTreeMap, HashMap, VecDeque};
use std::io;
use std::rc::Rc;
use std::sync::{Arc, Weak};

use eframe::egui;
use js_sys::{Array, Function, Object, Promise, Reflect, Uint8Array};
use wasm_bindgen::closure::Closure;
use wasm_bindgen::{JsCast as _, JsValue};
use wasm_bindgen_futures::{spawn_local, JsFuture};
use web_sys::{
    File, FileSystemDirectoryHandle, FileSystemFileHandle, FileSystemGetDirectoryOptions,
    FileSystemHandle, FileSystemHandleKind, FileSystemHandlePermissionDescriptor,
    FileSystemPermissionMode, MessageEvent, StorageManager, Worker,
};

use super::dom::failure;
use super::exec::{self, Fs};
use super::{Cmd, Event, Failure, FileCmd, Fingerprint, Stat};
use crate::js::{describe, field};
use crate::ondisk::OnDisk;
use crate::room::measure as size;

/// Where the writer is served, beside the page. The version keeps a cached writer from
/// an older release from answering a newer page.
fn writer_url() -> String {
    format!("library-writer.js?v={}", crate::sheet::VERSION)
}

const LOCK: &str = ".drawbar/lock";

/// Where a file is written before it is moved over its path, and where a write to the
/// instrument leaves a slot's former occupant.
pub(crate) const TMP: &str = ".drawbar/tmp";

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
    /// It has been said once that the desktop app must not open it at the same time
    /// ([`crate::libraries::SHARED`]).
    pub warned: bool,
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

/// The root of the browser's private storage for the page, or why there is none.
pub(crate) async fn private_root() -> io::Result<FileSystemDirectoryHandle> {
    Folder::private().await.map_err(io::Error::other)
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
    /// A command that writes ([`exec::writes`]) taken from `cmds` is running.
    writing: bool,
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

    /// Whether a command that writes has been sent and not run through yet.
    pub fn busy(&self) -> bool {
        let inbox = self.inbox.borrow();
        let writes = |cmd: &Cmd| matches!(cmd, Cmd::File(file) if file.writes());
        inbox.writing || inbox.cmds.iter().any(writes)
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
    let mut fs = Folder::open(root).await;
    while let Some(cmd) = next(&inbox).await {
        let mut answer = |event| {
            events.borrow_mut().push_back(event);
            ctx.request_repaint();
        };
        let cmd = match cmd {
            Cmd::File(file) => file,
            cmd => {
                answer(unopened(cmd));
                continue;
            }
        };
        inbox.borrow_mut().writing = cmd.writes();
        match &mut fs {
            Ok(fs) => {
                // ⚠️ The folder handles outlast only the commands that read: a rescan
                // looks again for what moved outside this tab, and a write may move it.
                let reads = !cmd.writes();
                if !reads {
                    fs.dirs.borrow_mut().clear();
                }
                exec::run(fs, cmd, &mut answer).await;
                if !reads {
                    fs.dirs.borrow_mut().clear();
                }
            }
            Err(why) => answer(refused(cmd, why)),
        }
        inbox.borrow_mut().writing = false;
        if private && inbox.borrow().cmds.is_empty() {
            measure(&room).await;
        }
        ctx.request_repaint();
    }
    if let Ok(fs) = &mut fs {
        fs.let_go().await;
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

/// Why a library in the browser does not open on this build.
const UNOPENED: &str = "a library in the browser opens with a later build";

/// The answer to a command on the library's facts, which the browser does not keep yet.
fn unopened(cmd: Cmd) -> Event {
    match cmd {
        Cmd::Open => Event::Opened(Err(UNOPENED.to_string())),
        Cmd::Rescan | Cmd::Refresh => Event::Scanned(Err(UNOPENED.to_string())),
        cmd => Event::Committed {
            label: format!("{cmd:?}"),
            result: Err(UNOPENED.to_string()),
        },
    }
}

/// The answer to a command when there is no library to run it on.
fn refused(cmd: FileCmd, why: &str) -> Event {
    match cmd {
        FileCmd::Read { files, .. } => Event::Read(
            files
                .into_iter()
                .map(|(id, _, _)| (id, Err(Failure::Io(why.to_string()))))
                .collect(),
        ),
        FileCmd::MakeDir(_) | FileCmd::RemoveDir(_) => Event::Failed(why.to_string()),
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

fn failed(err: JsValue) -> io::Error {
    let text = |name| field(&err, name).and_then(|value| value.as_string());
    match (text("name"), text("message")) {
        (Some(name), Some(message)) => failure(&name, &message),
        _ => io::Error::other(describe(&err)),
    }
}

pub(crate) async fn settle<T: wasm_bindgen::JsCast>(promise: Promise) -> io::Result<T> {
    let value = JsFuture::from(promise).await.map_err(failed)?;
    value
        .dyn_into()
        .map_err(|value| io::Error::other(format!("the browser answered {value:?}")))
}

/// The dedicated worker that writes files, and the requests waiting on its answers.
pub(crate) struct Writer {
    worker: Worker,
    waiting: Rc<RefCell<BTreeMap<u32, Function>>>,
    next: Cell<u32>,
    /// Why the worker stopped, once it has.
    stopped: Rc<RefCell<Option<String>>>,
    _hear: Closure<dyn FnMut(MessageEvent)>,
    _fail: Closure<dyn FnMut(JsValue)>,
}

impl Writer {
    pub(crate) fn start() -> io::Result<Writer> {
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
    pub(crate) async fn ask(
        &self,
        op: &str,
        path: &str,
        extra: &[(&str, JsValue)],
    ) -> io::Result<JsValue> {
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
}

/// `bytes`, copied into a buffer of their own that can be handed to the writer.
pub(crate) fn buffer(bytes: &[u8]) -> js_sys::ArrayBuffer {
    let data = Uint8Array::new_with_length(bytes.len() as u32);
    data.copy_from(bytes);
    data.buffer()
}

impl Drop for Writer {
    /// Stopping the worker closes its handles, the lock's among them.
    fn drop(&mut self) {
        self.worker.terminate();
    }
}

/// How a library's files are written.
enum Writes {
    /// Through `library-writer.js`, which holds `.drawbar/lock`. The origin private file
    /// system only. Started with the first write, since a library only read needs none.
    Worker(Option<Writer>),
    /// Through a writable stream on the page, for a picked folder.
    Streams,
}

/// A library's folder: the origin private file system's root, or a folder the user
/// picked.
///
/// Neither reaches outside itself through a link. The private file system holds none,
/// and Chromium hides a link at any depth below a picked folder, so a handle is never
/// given through one.
struct Folder {
    root: FileSystemDirectoryHandle,
    writes: Writes,
    /// The folders found so far, by path, so a path is not looked up again from the root
    /// one name at a time.
    ///
    /// ⚠️ Safari and Firefox keep a handle on its folder wherever it moves, so the handles
    /// are let go around each command that is not a read, and after every move, new
    /// folder or removal this tab makes, whether it succeeded or not.
    dirs: RefCell<HashMap<String, FileSystemDirectoryHandle>>,
    /// The files left resting, each at the path it is at now, so a move takes their
    /// snapshots again where they went.
    resting: RefCell<Vec<(Weak<OnDisk>, String)>>,
}

/// A folder of the library and the name of one entry in it.
type Spot = (FileSystemDirectoryHandle, String);

impl Folder {
    /// Let go of the library's lock before the next library opens, which may be this one
    /// again. A worker stopped by dropping it closes its handles only when the browser
    /// gets to it.
    async fn let_go(&mut self) {
        match &mut self.writes {
            Writes::Worker(Some(writer)) => {
                let _ = writer.ask("unlock", LOCK, &[]).await;
            }
            Writes::Worker(None) | Writes::Streams => {}
        }
    }

    /// The folder at `root`, or why this browser gives the page no storage. Some private
    /// windows refuse it.
    async fn open(root: Root) -> Result<Folder, String> {
        let (root, writes) = match root {
            Root::Private => (Folder::private().await?, Writes::Worker(None)),
            Root::Picked(picked) => (picked.handle, Writes::Streams),
        };
        Ok(Folder {
            root,
            writes,
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

/// Call `move(dir, name)` on a file or folder handle. A browser that has no `move`
/// refuses as [`io::ErrorKind::Unsupported`], as one that will not move it does.
///
/// ⚠️ `move(folder, name)` with both arguments: Safari has no one-argument form.
/// Chrome, Firefox and Safari all replace a file already at the name.
pub(crate) async fn move_to(
    handle: &JsValue,
    dir: &FileSystemDirectoryHandle,
    name: &str,
) -> io::Result<()> {
    let cannot = || {
        io::Error::new(
            io::ErrorKind::Unsupported,
            "this browser cannot move a file",
        )
    };
    let call = field(handle, "move")
        .and_then(|call| call.dyn_into::<Function>().ok())
        .ok_or_else(cannot)?;
    let moving = call
        .call2(handle, dir, &name.into())
        .map_err(failed)?
        .dyn_into::<Promise>()
        .map_err(|_| cannot())?;
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

impl Fs for Folder {
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
