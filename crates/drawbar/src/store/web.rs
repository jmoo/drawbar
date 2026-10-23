//! The browser's library: a folder tree in the origin private file system, which every
//! browser drawbar runs in keeps for the page and nothing else can see, or a folder on
//! this computer the user picked, in browsers that let a page open one.
//!
//! The library is kept in a dedicated worker of its own ([`super::worker`]), which runs
//! toshokan over the browser's storage and every command but a read. The page reads the
//! files itself, from the folder's handles, so a file's bytes cross to the worker only to
//! be written. Commands run one at a time, in the order they were sent. A library let go
//! runs the commands it was sent before it lets go, and the next library opens only then.
//!
//! Tabs of one library tell each other of their commits, and each refreshes when it hears
//! of one. A picked folder is also looked at again when the tab comes back into view, as
//! a program outside the browser may have written it.

use std::cell::RefCell;
use std::collections::{HashMap, VecDeque};
use std::future::Future;
use std::io;
use std::rc::Rc;
use std::sync::{Arc, Weak};
use std::task::Poll;
use std::time::Duration;

use eframe::egui;
use js_sys::{Function, Promise};
use toshokan::web::{Folder as Kept, Hint, Hints};
use toshokan::RelPath;
use wasm_bindgen::{JsCast as _, JsValue};
use wasm_bindgen_futures::{spawn_local, JsFuture};
use web_sys::{
    File, FileSystemDirectoryHandle, FileSystemFileHandle, FileSystemGetDirectoryOptions,
    FileSystemHandle, FileSystemHandleKind, FileSystemHandlePermissionDescriptor,
    FileSystemPermissionMode, StorageManager,
};

use super::dom::failure;
use super::exec::{self, Fs};
use super::worker::Worker;
use super::{Cmd, Event, Failure, Fingerprint, Intent, LibPath, Rescue, Stat};
use crate::js::{describe, field};
use crate::ondisk::OnDisk;
use crate::room::measure as size;

/// How much of a file a read takes at once.
const CHUNK: usize = 4 * 1024 * 1024;

/// How often a picked folder, which another program may write, is looked at again
/// while the tab is in view.
const LOOK_EVERY: Duration = Duration::from_secs(60);

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
    /// ⚠️ Names the folder's own state in the browser's storage and the hints its tabs
    /// share, so every tab must know one folder by one id: a folder picked again takes
    /// the id the recent list already gives it.
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

    /// Where this install keeps its own state for the library, in the origin private
    /// file system: under drawbar's own folder of the browser's library, which no scan
    /// lists.
    pub fn local(&self) -> RelPath {
        let name = match self {
            Root::Private => "private".to_string(),
            Root::Picked(picked) => format!("picked-{}", picked.id),
        };
        RelPath::new(&format!("{}/local/{name}", super::keys::DIR))
            .expect("a local root's path is a library path")
    }

    /// The folder as toshokan's storage takes it.
    fn kept(&self) -> Kept {
        match self {
            Root::Private => Kept::Private(RelPath::ROOT),
            Root::Picked(picked) => Kept::picked(picked.handle.clone()),
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
    /// How many commands that write were sent and are not answered yet.
    writing: usize,
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
    /// Settles once the library started last has run its last command and let go of it.
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

/// What the page hears back from the task that runs the library.
#[derive(Default)]
struct Heard {
    events: VecDeque<Event>,
    /// What other tabs or the folder said to read again, oldest first.
    hints: VecDeque<Hint>,
    /// The slots' former occupants this install keeps for the library, found once it
    /// opened.
    rescues: Vec<Rescue>,
    /// Why the library's worker stopped answering, once it has. Nothing is heard after.
    stopped: Option<String>,
    /// Whether the browser no longer lets the page into the picked folder.
    denied: bool,
    /// How many files the worker's listings of the folder have found so far, and how
    /// many requests its storage answered.
    signs: Option<Rc<std::cell::Cell<(usize, usize)>>>,
    /// The library's worker, once started.
    worker: Option<Rc<Worker>>,
    /// When the command the worker runs now was sent; `None` while none runs.
    running: Option<f64>,
    /// How many requests the worker's storage had answered when [`Backend::waiting`]
    /// last looked, and when.
    seen: (usize, f64),
    /// The fault a test asked the worker to show with the next command.
    fault: Option<super::Fault>,
}

pub struct Backend {
    root: Root,
    inbox: Rc<RefCell<Inbox>>,
    heard: Rc<RefCell<Heard>>,
    room: Rc<RefCell<Room>>,
}

impl Backend {
    /// Open the library at `root` once the one started before it has let go.
    pub fn start(ctx: &egui::Context, root: Root) -> Backend {
        crate::ondisk::repaint_with(ctx);
        let backend = Backend {
            root: root.clone(),
            inbox: Rc::default(),
            heard: Rc::default(),
            room: Rc::default(),
        };
        let mut done = None;
        let let_go = Promise::new(&mut |resolve, _| done = Some(resolve));
        let before = LET_GO.with(|held| held.replace(Some(let_go)));
        let (inbox, heard) = (backend.inbox.clone(), backend.heard.clone());
        spawn_local(measuring(Rc::downgrade(&backend.room), ctx.clone()));
        let ctx = ctx.clone();
        spawn_local(async move {
            if let Some(before) = before {
                let _ = JsFuture::from(before).await;
            }
            drive(root, inbox, heard, ctx).await;
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

    /// How much of the browser's storage drawbar takes, and whether it is kept. For a
    /// picked folder, which is not the browser's to keep, that is what drawbar keeps of
    /// it here: drafts and slot edits.
    pub fn room(&self) -> String {
        let room = self.room.borrow();
        let used = room
            .used
            .map(|(used, quota)| format!("{} of {} used", size(used), size(quota)));
        let kept = room.kept.map(|kept| match kept {
            true => "kept",
            false => "may be cleared when space runs low",
        });
        let said = [used.as_deref(), kept].into_iter().flatten();
        let said = said.collect::<Vec<_>>().join(", ");
        match &self.root {
            Root::Picked(_) if !said.is_empty() => {
                format!("drafts and slot edits in this browser: {said}")
            }
            _ => said,
        }
    }

    pub fn send(&mut self, cmd: Cmd) {
        let mut inbox = self.inbox.borrow_mut();
        inbox.writing += usize::from(cmd.writes());
        inbox.cmds.push_back(cmd);
        drop(inbox);
        Inbox::wake(&self.inbox);
    }

    pub fn try_recv(&mut self) -> Option<Event> {
        self.heard.borrow_mut().events.pop_front()
    }

    /// What another tab or the folder said to read again, oldest first.
    pub fn hinted(&mut self) -> Option<Hint> {
        self.heard.borrow_mut().hints.pop_front()
    }

    /// The slots' former occupants this install keeps for the library, found once it
    /// opened, each once.
    pub fn rescued(&mut self) -> Vec<Rescue> {
        std::mem::take(&mut self.heard.borrow_mut().rescues)
    }

    /// Why the library's worker stopped, once it has: it answers nothing more, and a
    /// fresh backend opens the library again.
    pub fn stopped(&self) -> Option<String> {
        self.heard.borrow().stopped.clone()
    }

    /// Whether the browser no longer lets the page into the picked folder, as when the
    /// user took the permission back: the library must be opened again once let in.
    pub fn denied(&self) -> bool {
        self.heard.borrow().denied
    }

    /// Make the worker fault, or never finish, with the next command it takes, as a bug
    /// in it would.
    #[doc(hidden)]
    pub fn fault(&self, fault: super::Fault) {
        self.heard.borrow_mut().fault = Some(fault);
    }

    /// How long the command running now has gone without a sign of life: no answer, and
    /// no file found. `None` while none runs.
    pub fn waiting(&self) -> Option<std::time::Duration> {
        let now = js_sys::Date::now();
        let mut heard = self.heard.borrow_mut();
        let running = heard.running?;
        let answered = heard.signs.as_ref().map_or(0, |signs| signs.get().1);
        heard.seen = match heard.seen {
            (_, at) if at < running => (answered, running),
            (before, _) if before != answered => (answered, now),
            seen => seen,
        };
        let waited = (now - heard.seen.1).max(0.0) / 1000.0;
        Some(std::time::Duration::from_secs_f64(waited))
    }

    /// Let the library go without waiting for its worker, which stopped or does not
    /// answer: the worker ends, and what waited on it fails.
    pub fn abandon(&mut self) {
        let worker = self.heard.borrow_mut().worker.take();
        if let Some(worker) = worker {
            worker.abandon();
        }
        self.finish();
    }

    /// How many files the listings of the folder have found so far, as the worker last
    /// said.
    pub fn listed(&self) -> usize {
        let heard = self.heard.borrow();
        heard.signs.as_ref().map_or(0, |signs| signs.get().0)
    }

    /// Whether a command that writes has been sent and not answered yet.
    pub fn busy(&self) -> bool {
        self.inbox.borrow().writing > 0
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

/// Run each command as it arrives, until the backend is finished, and pass on what other
/// tabs and the folder say meanwhile.
async fn drive(
    root: Root,
    inbox: Rc<RefCell<Inbox>>,
    heard: Rc<RefCell<Heard>>,
    ctx: egui::Context,
) {
    let private = root == Root::Private;
    let folder = Folder::open(root.clone()).await;
    let local = root.local();
    let worker = Worker::start(root.kept(), &local).await.map(Rc::new);
    heard.borrow_mut().signs = worker.as_ref().ok().map(|worker| worker.signs());
    heard.borrow_mut().worker = worker.as_ref().ok().cloned();
    let look = (!private).then_some(LOOK_EVERY);
    let mut hints = Hints::new(local.as_str(), look).ok();
    let mut persisting = false;
    if let Some(hints) = &mut hints {
        let _ = hints.watch(&root.kept()).await;
    }
    loop {
        let cmd = match &mut hints {
            Some(hints) => match either(next(&inbox), hints.next()).await {
                Either::Left(cmd) => cmd,
                Either::Right(hint) => {
                    heard.borrow_mut().hints.push_back(hint);
                    ctx.request_repaint();
                    continue;
                }
            },
            None => next(&inbox).await,
        };
        let Some(cmd) = cmd else {
            break;
        };
        let writes = cmd.writes();
        if let Ok(folder) = &folder {
            folder.forget();
        }
        let answers = match (cmd, &folder, &worker) {
            (Cmd::Read { files, room }, Ok(folder), _) => {
                vec![Event::Read(exec::read_all(folder, files, room).await)]
            }
            (cmd @ Cmd::Read { .. }, Err(why), _) => refused(&cmd)(why),
            (cmd, _, Ok(worker)) => {
                let refusal = refused(&cmd);
                if let Some(fault) = heard.borrow_mut().fault.take() {
                    worker.fault(fault);
                }
                heard.borrow_mut().running = Some(js_sys::Date::now());
                let ran = worker.run(cmd).await;
                heard.borrow_mut().running = None;
                match ran {
                    Ok(answers) => answers,
                    Err(why) => refusal(&why),
                }
            }
            (cmd, Err(why), _) | (cmd, _, Err(why)) => refused(&cmd)(why),
        };
        if let Root::Picked(picked) = &root {
            let failed = answers.iter().any(|answer| !answer.faults().is_empty());
            if failed && !permission(&picked.handle, false).await.unwrap_or(true) {
                heard.borrow_mut().denied = true;
            }
        }
        if let Some(why) = worker.as_ref().ok().and_then(|worker| worker.stopped()) {
            heard.borrow_mut().stopped.get_or_insert(why);
            inbox.borrow_mut().writing -= usize::from(writes);
            ctx.request_repaint();
            continue;
        }
        if let Ok(folder) = &folder {
            folder.forget();
            folder.follow(&moved(&answers)).await;
        }
        if answers
            .iter()
            .any(|answer| matches!(answer, Event::Opened(Ok(_))))
        {
            heard.borrow_mut().rescues = rescues(&local).await;
        }
        for answer in answers {
            if let (
                Event::Committed {
                    made: Some(made), ..
                },
                Some(hints),
            ) = (&answer, &hints)
            {
                hints.announce(*made);
            }
            let answer = match &folder {
                Ok(folder) => exec::rest_landed(folder, answer).await,
                Err(_) => answer,
            };
            heard.borrow_mut().events.push_back(answer);
        }
        inbox.borrow_mut().writing -= usize::from(writes);
        if writes && !persisting {
            persisting = true;
            // ⚠️ Firefox waits for the user to answer, so nothing waits for it.
            spawn_local(persist());
        }
        ctx.request_repaint();
    }
    if let Ok(worker) = &worker {
        worker.close().await;
    }
}

/// Every file and folder `answers` say moved, from and to, in order.
fn moved(answers: &[Event]) -> Vec<(LibPath, LibPath)> {
    let viewed = |viewed: &super::Viewed| {
        let undone = viewed.undone.iter().flatten();
        let undone = undone.filter_map(|(from, to)| Some((from.clone()?, to.clone()?)));
        viewed
            .moves
            .iter()
            .cloned()
            .chain(undone)
            .collect::<Vec<_>>()
    };
    answers
        .iter()
        .flat_map(|answer| match answer {
            Event::Moved {
                from,
                to,
                result: Ok(()),
            } => vec![(from.clone(), to.clone())],
            Event::Committed {
                result: Ok(seen), ..
            }
            | Event::Scanned(Ok(seen)) => viewed(seen),
            Event::Opened(Ok(opened)) => viewed(&opened.viewed),
            _ => Vec::new(),
        })
        .collect()
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

enum Either<A, B> {
    Left(A),
    Right(B),
}

/// The output of whichever of `a` and `b` finishes first. The other is dropped.
async fn either<A: Future, B: Future>(a: A, b: B) -> Either<A::Output, B::Output> {
    let (mut a, mut b) = (std::pin::pin!(a), std::pin::pin!(b));
    std::future::poll_fn(|cx| {
        if let Poll::Ready(left) = a.as_mut().poll(cx) {
            return Poll::Ready(Either::Left(left));
        }
        b.as_mut().poll(cx).map(Either::Right)
    })
    .await
}

/// The answers to `cmd` where it could not run, for `why`: each change to a file says it
/// did not land, so nothing waits on it.
fn refused(cmd: &Cmd) -> impl FnOnce(&str) -> Vec<Event> + 'static {
    let refusing = match cmd {
        Cmd::Open => Refusing::Open,
        Cmd::Rescan | Cmd::Refresh => Refusing::Scan,
        Cmd::Intent(intent) => Refusing::Intent(intent.label().to_string(), parts(intent)),
        Cmd::Undo | Cmd::Redo | Cmd::Settle { .. } => Refusing::Intent(
            match cmd {
                Cmd::Undo => "Undo",
                Cmd::Redo => "Redo",
                _ => "Settle",
            }
            .to_string(),
            Vec::new(),
        ),
        Cmd::Draft { .. } | Cmd::Discard(_) | Cmd::KeepSlot { .. } | Cmd::DiscardSlot(_) => {
            Refusing::Kept
        }
        Cmd::ReadSlot(slot) => Refusing::Slot(slot.clone()),
        Cmd::Read { files, .. } => Refusing::Read(files.iter().map(|(id, ..)| *id).collect()),
    };
    move |why| refusing.answers(why)
}

/// What a command that could not run is answered with.
enum Refusing {
    Open,
    Scan,
    /// The intent's label, and the answer each change to a file in it owes.
    Intent(String, Vec<Owing>),
    Kept,
    Slot(super::Slot),
    Read(Vec<u64>),
}

/// The answer one change to a file owes.
enum Owing {
    Saved(u64, LibPath),
    Rewritten(u64, LibPath),
    Imported(u64, LibPath),
    Moved(LibPath, LibPath),
}

/// What each change to a file `intent` makes owes an answer.
fn parts(intent: &Intent) -> Vec<Owing> {
    match intent {
        Intent::Save { id, path, .. } => vec![Owing::Saved(*id, path.clone())],
        Intent::Rewrite { id, path, .. } => vec![Owing::Rewritten(*id, path.clone())],
        Intent::Import { id, path, .. } => vec![Owing::Imported(*id, path.clone())],
        Intent::Move { moves } => moves
            .iter()
            .map(|moving| Owing::Moved(moving.from.clone(), moving.to.clone()))
            .collect(),
        Intent::MoveFolder { from, to } => vec![Owing::Moved(from.clone(), to.clone())],
        Intent::Batch(parts) => parts.iter().flat_map(self::parts).collect(),
        _ => Vec::new(),
    }
}

impl Refusing {
    fn answers(self, why: &str) -> Vec<Event> {
        let failed = || Failure::Io(why.to_string());
        match self {
            Refusing::Open => vec![Event::Opened(Err(why.to_string()))],
            Refusing::Scan => vec![Event::Scanned(Err(why.to_string()))],
            Refusing::Kept => vec![Event::Unkept(why.to_string())],
            Refusing::Slot(slot) => vec![Event::SlotKept {
                slot,
                kept: Err(why.to_string()),
            }],
            Refusing::Read(ids) => vec![Event::Read(
                ids.into_iter().map(|id| (id, Err(failed()))).collect(),
            )],
            Refusing::Intent(label, owing) => {
                let owed = owing.into_iter().map(|owing| match owing {
                    Owing::Saved(id, path) => Event::Saved {
                        id,
                        path,
                        result: Err(failed()),
                    },
                    Owing::Rewritten(id, path) => Event::Rewritten {
                        id,
                        path,
                        result: Err(failed()),
                    },
                    Owing::Imported(id, path) => Event::Imported {
                        id,
                        path,
                        result: Err(failed()),
                    },
                    Owing::Moved(from, to) => Event::Moved {
                        from,
                        to,
                        result: Err(why.to_string()),
                    },
                });
                let committed = Event::Committed {
                    label,
                    made: None,
                    result: Err(why.to_string()),
                };
                owed.chain(std::iter::once(committed)).collect()
            }
        }
    }
}

/// The slots' former occupants kept in the `rescued` folder of the local root at
/// `local`, by name. One still partial is never a slot's only copy, and is deleted.
async fn rescues(local: &RelPath) -> Vec<Rescue> {
    let Ok(at) = local.join(super::RESCUED) else {
        return Vec::new();
    };
    let Ok(dir) = toshokan::web::private_dir(&at).await else {
        return Vec::new();
    };
    let mut found = Vec::new();
    for (name, handle) in entries(&dir).await.unwrap_or_default() {
        if handle.kind() != FileSystemHandleKind::File
            || !name.starts_with(nord_usb::envelope::RESCUED)
        {
            continue;
        }
        if !nord_usb::envelope::is_rescue(&name) {
            let _ = JsFuture::from(dir.remove_entry(&name)).await;
            continue;
        }
        let Ok(file) = snapshot(handle.unchecked_ref()).await else {
            continue;
        };
        let source = Ok(crate::ondisk::Sliced::new(file.clone()));
        found.push(Rescue {
            slot: Rescue::slot_of(&name),
            at: format!("{at}/{name}").into(),
            broken: super::unwhole(source).await,
            file,
            name,
        });
    }
    found.sort_by(|a, b| a.name.cmp(&b.name));
    found
}

/// The entries of a folder.
async fn entries(dir: &FileSystemDirectoryHandle) -> io::Result<Vec<(String, FileSystemHandle)>> {
    let iterator = dir.entries();
    let mut found = Vec::new();
    loop {
        let step = JsFuture::from(iterator.next().map_err(failed)?)
            .await
            .map_err(failed)?;
        if field(&step, "done").and_then(|done| done.as_bool()) == Some(true) {
            return Ok(found);
        }
        let pair: js_sys::Array = field(&step, "value")
            .ok_or_else(|| io::Error::other("a folder listed an empty entry"))?
            .unchecked_into();
        if let (Some(name), Ok(handle)) = (pair.get(0).as_string(), pair.get(1).dyn_into()) {
            found.push((name, handle));
        }
    }
}

/// Delete the file at `path` of the browser's private storage, after this returns.
pub(crate) fn remove_private(path: std::path::PathBuf) {
    spawn_local(async move {
        let text = path.to_string_lossy();
        let Some((dir, name)) = text.rsplit_once('/') else {
            return;
        };
        let Ok(dir) = RelPath::new(dir) else {
            return;
        };
        if let Ok(dir) = toshokan::web::private_dir(&dir).await {
            let _ = JsFuture::from(dir.remove_entry(name)).await;
        }
    });
}

fn storage() -> Option<StorageManager> {
    let navigator = web_sys::window()?.navigator();
    Some(field(&navigator, "storage")?.unchecked_into())
}

/// Ask the browser to keep the origin's storage when space runs low: Firefox asks the
/// user, and other browsers decide for themselves.
async fn persist() {
    if let Some(asked) = storage().and_then(|storage| storage.persist().ok()) {
        let _ = JsFuture::from(asked).await;
    }
}

/// How often to read again how much of the browser's storage drawbar takes.
const MEASURE_EVERY_MS: i32 = 30_000;

/// Read how much of the browser's storage drawbar takes now and every
/// [`MEASURE_EVERY_MS`], for as long as the backend holding `room` lasts.
async fn measuring(room: std::rc::Weak<RefCell<Room>>, ctx: egui::Context) {
    while let Some(held) = room.upgrade() {
        measure(&held).await;
        drop(held);
        ctx.request_repaint();
        let waited = Promise::new(&mut |resolve, _| {
            let global = js_sys::global();
            let set = field(&global, "setTimeout").map(|set| set.unchecked_into::<Function>());
            if let Some(set) = set {
                let _ = set.call2(&global, &resolve, &MEASURE_EVERY_MS.into());
            }
        });
        let _ = JsFuture::from(waited).await;
    }
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

/// A library's folder as the page reads it: the origin private file system's root, or a
/// folder the user picked.
///
/// Neither reaches outside itself through a link. The private file system holds none,
/// and Chromium hides a link at any depth below a picked folder, so a handle is never
/// given through one.
struct Folder {
    root: FileSystemDirectoryHandle,
    /// The folders found so far, by path, so a path is not looked up again from the root
    /// one name at a time.
    ///
    /// ⚠️ Safari and Firefox keep a handle on its folder wherever it moves, so the handles
    /// are let go around each read, since the worker may have moved a folder since.
    dirs: RefCell<HashMap<String, FileSystemDirectoryHandle>>,
    /// The files left resting, each at the path it is at now, so a move takes their
    /// snapshots again where they went.
    resting: RefCell<Vec<(Weak<OnDisk>, LibPath)>>,
}

impl Folder {
    /// Let go of the folder handles, which the worker's moves and removals may have
    /// left stale.
    fn forget(&self) {
        self.dirs.borrow_mut().clear();
    }

    /// Follow the files left resting through `moves`, each a file or a folder from and
    /// to: each moved takes its snapshot again where it went.
    async fn follow(&self, moves: &[(LibPath, LibPath)]) {
        let mut moved = Vec::new();
        for (file, at) in self.resting.borrow_mut().iter_mut() {
            let Some(to) = moves.iter().find_map(|(from, to)| at.moved(from, to)) else {
                continue;
            };
            *at = to.clone();
            moved.push((file.clone(), to));
        }
        for (file, at) in moved {
            let (Some(file), Ok(handle)) = (file.upgrade(), self.file(at.as_str()).await) else {
                continue;
            };
            if let Ok(snapshot) = snapshot(&handle).await {
                file.resnapshot(snapshot);
            }
        }
    }
    /// The folder at `root`, or why this browser gives the page no storage. Some private
    /// windows refuse it.
    async fn open(root: Root) -> Result<Folder, String> {
        let root = match root {
            Root::Private => Folder::private().await?,
            Root::Picked(picked) => picked.handle,
        };
        Ok(Folder {
            root,
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

    /// The folder at `path`.
    async fn dir(&self, path: &str) -> io::Result<FileSystemDirectoryHandle> {
        let options = FileSystemGetDirectoryOptions::new();
        let mut dir = self.root.clone();
        let mut at = String::new();
        for name in path.split('/').filter(|name| !name.is_empty()) {
            at = match at.is_empty() {
                true => name.to_string(),
                false => format!("{at}/{name}"),
            };
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
    async fn spot(&self, path: &str) -> io::Result<(FileSystemDirectoryHandle, String)> {
        let (parent, leaf) = path.rsplit_once('/').unwrap_or(("", path));
        Ok((self.dir(parent).await?, leaf.to_string()))
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
/// is set and the browser has not said. Always, where the browser asks nothing, as
/// Firefox and Safari ask nothing of a folder of the private file system.
///
/// ⚠️ Asking needs a click the user has just made.
pub async fn permission(dir: &FileSystemDirectoryHandle, ask: bool) -> io::Result<bool> {
    if field(dir, "queryPermission").is_none() {
        return Ok(true);
    }
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

    /// The file is indexed through a snapshot taken now, and followed through the moves
    /// the library makes.
    async fn rest(
        &self,
        path: &str,
        known: Option<Fingerprint>,
    ) -> io::Result<Option<Arc<OnDisk>>> {
        let snapshot = snapshot(&self.file(path).await?).await?;
        let Some(file) = OnDisk::open(snapshot, known.and_then(|print| print.crc)).await? else {
            return Ok(None);
        };
        let (file, at) = (Arc::new(file), LibPath::parse(path));
        if let Some(at) = at {
            let mut resting = self.resting.borrow_mut();
            resting.retain(|(file, _)| file.strong_count() > 0);
            resting.push((Arc::downgrade(&file), at));
        }
        Ok(Some(file))
    }
}
