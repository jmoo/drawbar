//! Every backend behind one interface, driven by the drivers; the steps of a
//! commit as the library runs them; and instances writing through the log.

#![allow(dead_code, reason = "each test binary uses its own part")]

use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::rc::Rc;

use toshokan::asynch;
use toshokan::binding::Bindings;
#[cfg(not(target_arch = "wasm32"))]
use toshokan::blocking::Native;
use toshokan::blocking::{self, run, Backend};
use toshokan::compaction::compact;
use toshokan::effects::{self, Applied, EffectPlan};
use toshokan::env::{ExactNames, PrefixIdentity, SeededRandom, TestClock};
use toshokan::io::{Capabilities, Kind, Range};
use toshokan::line::Line;
use toshokan::log::{Entry, EntryKind, Logged};
use toshokan::pending::PendingRecord;
use toshokan::plan::{FileChange, Splice};
use toshokan::reader::{CachedView, Reader};
use toshokan::report::{Compacted, Start};
use toshokan::simulator::Machine;
use toshokan::writer::{Claimed, Writer};
use toshokan::{
    EntityId, EntryHash, Env, Error, Hlc, Identify, Io, IoResult, Layout, MemDisk, Nonce,
    Operation, Random, Refusal, RelPath, Reply, Root, SegmentName, Step, WriterId,
};

pub fn path(text: &str) -> RelPath {
    RelPath::new(text).unwrap()
}

/// What a test saves, made into each driver's source.
#[derive(Clone, Debug)]
pub enum Fill {
    Bytes(Vec<u8>),
    Splice(Splice),
}

impl Fill {
    pub fn blocking<'s, B: Backend + 's>(self) -> Box<dyn blocking::Source<B> + 's> {
        match self {
            Self::Bytes(bytes) => Box::new(bytes),
            Self::Splice(splice) => Box::new(splice),
        }
    }

    fn asynch<'s, F: asynch::Fs + 's>(self) -> Box<dyn asynch::Source<F> + 's> {
        match self {
            Self::Bytes(bytes) => Box::new(bytes),
            Self::Splice(splice) => Box::new(splice),
        }
    }
}

/// One process's handle on a backend, driven by one of the drivers.
pub trait Driven: Sized {
    fn capabilities(&self, root: Root) -> Capabilities;
    /// Runs `operation`, filling its `n`th content from `contents[n]`.
    fn run_filled<O: Operation>(&mut self, operation: O, contents: Vec<Fill>) -> O::Output;
    /// A handle of another process on the same storage.
    fn other_process(&self) -> Self;

    fn run<O: Operation>(&mut self, operation: O) -> O::Output {
        self.run_filled(operation, Vec::new())
    }

    fn requests(&mut self, requests: Vec<Io>) -> Vec<IoResult> {
        self.run(Script {
            requests: requests.into(),
            results: Vec::new(),
        })
    }

    fn one(&mut self, io: Io) -> IoResult {
        self.requests(vec![io]).remove(0)
    }

    fn ok(&mut self, io: Io) -> Reply {
        let shown = format!("{io:?}");
        self.one(io).unwrap_or_else(|e| panic!("{shown}: {e}"))
    }
}

/// Makes its requests in order and returns every result.
struct Script {
    requests: VecDeque<Io>,
    results: Vec<IoResult>,
}

impl Operation for Script {
    type Output = Vec<IoResult>;

    fn resume(&mut self, result: Option<IoResult>) -> Step<Vec<IoResult>> {
        self.results.extend(result);
        match self.requests.pop_front() {
            Some(io) => Step::Io(io),
            None => Step::Done(std::mem::take(&mut self.results)),
        }
    }
}

pub struct BlockingMem(pub MemDisk);

impl Driven for BlockingMem {
    fn capabilities(&self, root: Root) -> Capabilities {
        Backend::capabilities(&self.0, root)
    }

    fn run_filled<O: Operation>(&mut self, operation: O, contents: Vec<Fill>) -> O::Output {
        let sources = contents.into_iter().map(Fill::blocking).collect();
        blocking::run_with(&mut self.0, sources, operation)
    }

    fn other_process(&self) -> Self {
        Self(self.0.process())
    }
}

pub struct AsyncMem(pub MemDisk);

impl Driven for AsyncMem {
    fn capabilities(&self, root: Root) -> Capabilities {
        asynch::Fs::capabilities(&self.0, root)
    }

    fn run_filled<O: Operation>(&mut self, operation: O, contents: Vec<Fill>) -> O::Output {
        let sources = contents.into_iter().map(Fill::asynch).collect();
        pollster::block_on(asynch::run_with(&self.0, sources, operation))
    }

    fn other_process(&self) -> Self {
        Self(self.0.process())
    }
}

/// A [`MemDisk`] behind an async backend that takes single requests only, each
/// finished on a later poll, and answers batched ones by [`asynch::fan_out`].
pub struct Unbatched(pub MemDisk);

impl asynch::Fs for Unbatched {
    fn capabilities(&self, root: Root) -> Capabilities {
        asynch::Fs::capabilities(&self.0, root)
    }

    async fn perform(&self, io: Io) -> IoResult {
        asynch::fan_out(io, |io| single(&self.0, io)).await
    }
}

async fn single(disk: &MemDisk, io: Io) -> IoResult {
    assert!(
        !matches!(io, Io::ListStat { .. } | Io::ReadMany { .. }),
        "{io:?} is batched"
    );
    Later(false).await;
    disk.perform(io)
}

/// Ready on its second poll.
pub struct Later(pub bool);

impl std::future::Future for Later {
    type Output = ();

    fn poll(
        mut self: std::pin::Pin<&mut Self>,
        context: &mut std::task::Context<'_>,
    ) -> std::task::Poll<()> {
        if self.0 {
            return std::task::Poll::Ready(());
        }
        self.0 = true;
        context.waker().wake_by_ref();
        std::task::Poll::Pending
    }
}

pub struct AsyncUnbatched(pub MemDisk);

impl Driven for AsyncUnbatched {
    fn capabilities(&self, root: Root) -> Capabilities {
        asynch::Fs::capabilities(&self.0, root)
    }

    fn run_filled<O: Operation>(&mut self, operation: O, contents: Vec<Fill>) -> O::Output {
        let sources = contents.into_iter().map(Fill::asynch).collect();
        pollster::block_on(asynch::run_with(
            &Unbatched(self.0.clone()),
            sources,
            operation,
        ))
    }

    fn other_process(&self) -> Self {
        Self(self.0.process())
    }
}

/// A [`MemDisk`] that keeps every request it is asked, in order.
#[derive(Clone)]
pub struct Recorded {
    pub disk: MemDisk,
    pub requests: Rc<std::cell::RefCell<Vec<Io>>>,
}

impl Recorded {
    pub fn new(disk: MemDisk) -> Self {
        Self {
            disk,
            requests: Rc::default(),
        }
    }

    /// The requests asked since the last call.
    pub fn take(&self) -> Vec<Io> {
        std::mem::take(&mut self.requests.borrow_mut())
    }
}

impl Backend for Recorded {
    fn capabilities(&self, root: Root) -> Capabilities {
        self.disk.capabilities(root)
    }

    fn perform(&mut self, io: Io) -> IoResult {
        self.requests.borrow_mut().push(io.clone());
        self.disk.perform(io)
    }
}

impl Driven for Recorded {
    fn capabilities(&self, root: Root) -> Capabilities {
        self.disk.capabilities(root)
    }

    fn run_filled<O: Operation>(&mut self, operation: O, contents: Vec<Fill>) -> O::Output {
        let sources = contents.into_iter().map(Fill::blocking).collect();
        blocking::run_with(self, sources, operation)
    }

    fn other_process(&self) -> Self {
        Self::new(self.disk.process())
    }
}

#[cfg(all(target_arch = "wasm32", feature = "web"))]
pub mod web;

#[cfg(not(target_arch = "wasm32"))]
pub struct NativeDirs {
    backend: Native,
    dirs: std::rc::Rc<[tempfile::TempDir; 2]>,
}

#[cfg(not(target_arch = "wasm32"))]
impl NativeDirs {
    pub fn new() -> Self {
        let dirs = std::rc::Rc::new([tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap()]);
        Self {
            backend: Native::new(dirs[0].path(), dirs[1].path()),
            dirs,
        }
    }
}

#[cfg(not(target_arch = "wasm32"))]
impl Driven for NativeDirs {
    fn capabilities(&self, root: Root) -> Capabilities {
        self.backend.capabilities(root)
    }

    fn run_filled<O: Operation>(&mut self, operation: O, contents: Vec<Fill>) -> O::Output {
        let sources = contents.into_iter().map(Fill::blocking).collect();
        blocking::run_with(&mut self.backend, sources, operation)
    }

    fn other_process(&self) -> Self {
        Self {
            backend: Native::new(self.dirs[0].path(), self.dirs[1].path()),
            dirs: self.dirs.clone(),
        }
    }
}

/// Runs each behavior on fresh, empty roots of every backend: natively, or in a
/// browser with the `web` feature.
#[macro_export]
macro_rules! for_every_backend {
    ($suite:ident: $($behavior:ident),* $(,)?) => {
        mod blocking_mem {
            $(#[test] fn $behavior() { super::$suite::$behavior(&mut $crate::common::BlockingMem(toshokan::MemDisk::new())); })*
        }
        mod async_mem {
            $(#[test] fn $behavior() { super::$suite::$behavior(&mut $crate::common::AsyncMem(toshokan::MemDisk::new())); })*
        }
        mod async_unbatched {
            $(#[test] fn $behavior() { super::$suite::$behavior(&mut $crate::common::AsyncUnbatched(toshokan::MemDisk::new())); })*
        }
        #[cfg(not(target_arch = "wasm32"))]
        mod native {
            $(#[test] fn $behavior() { super::$suite::$behavior(&mut $crate::common::NativeDirs::new()); })*
        }
        $crate::for_web!(private_web, Kind::Private, $suite: $($behavior),*);
        $crate::for_web!(picked_web, Kind::Picked { rename: true }, $suite: $($behavior),*);
        $crate::for_web!(picked_web_without_rename, Kind::Picked { rename: false }, $suite: $($behavior),*);
    };
}

/// Runs each behavior in a browser on fresh roots of one kind of folder.
#[macro_export]
macro_rules! for_web {
    ($module:ident, $kind:expr, $suite:ident: $($behavior:ident),*) => {
        #[cfg(all(target_arch = "wasm32", feature = "web"))]
        mod $module {
            use $crate::common::web::{Kind, WebDirs};
            $(
                #[wasm_bindgen_test::wasm_bindgen_test]
                async fn $behavior() {
                    let mut dirs = WebDirs::new($kind).await;
                    super::$suite::$behavior(&mut dirs);
                }
            )*
        }
    };
}

/// Every file of a root, by path, read through requests.
pub fn files(d: &mut impl Driven, root: Root) -> BTreeMap<RelPath, Vec<u8>> {
    let mut files = BTreeMap::new();
    let mut dirs = vec![RelPath::ROOT];
    while let Some(dir) = dirs.pop() {
        let Reply::Listed(entries) = d.ok(Io::List {
            root,
            dir: dir.clone(),
        }) else {
            panic!("not a listing");
        };
        for entry in entries {
            let path = dir.join(&entry.name).unwrap();
            match entry.kind {
                Kind::Directory => dirs.push(path),
                Kind::File => {
                    let Reply::Bytes(bytes) = d.ok(Io::Read {
                        root,
                        path: path.clone(),
                        range: Range {
                            offset: 0,
                            len: u64::MAX,
                        },
                    }) else {
                        panic!("not bytes");
                    };
                    files.insert(path, bytes);
                }
            }
        }
    }
    files
}

/// Creates `files` in the folder, durably.
pub fn put(d: &mut impl Driven, files: &[(&str, &[u8])]) {
    for (text, bytes) in files {
        let file = path(text);
        let parent = file.parent().unwrap();
        d.ok(Io::MakeDir {
            root: Root::Folder,
            path: parent.clone(),
        });
        d.ok(Io::Create {
            root: Root::Folder,
            path: file.clone(),
            bytes: bytes.to_vec(),
        });
        d.ok(Io::Sync {
            root: Root::Folder,
            path: file,
        });
        let mut dir = Some(parent);
        while let Some(synced) = dir {
            d.ok(Io::Sync {
                root: Root::Folder,
                path: synced.clone(),
            });
            dir = synced.parent();
        }
    }
}

pub const IDENTIFY: PrefixIdentity = PrefixIdentity { prefix: 4 };

pub fn identity(bytes: &[u8]) -> toshokan::Identity {
    let len = bytes.len() as u64;
    let prefix = bytes[..bytes.len().min(4)].to_vec();
    IDENTIFY.identify(len, &[prefix])
}

pub fn identify() -> Rc<dyn Identify> {
    Rc::new(IDENTIFY)
}

pub fn env(seed: u64) -> Env {
    Env {
        clock: Box::new(TestClock::at(1)),
        random: Box::new(SeededRandom::new(seed)),
        identify: identify(),
        names: Box::new(ExactNames),
        label: "test".into(),
    }
}

/// A folder that cannot rename, and whose completed requests are as durable as
/// they will get, as a browser's picked folder without `move()`.
pub const COPYING: Capabilities = Capabilities {
    append: true,
    rename_file: false,
    no_replace: false,
    rename_dir: false,
    fsync: false,
};

/// An empty disk whose folder is [`COPYING`].
pub fn copying() -> MemDisk {
    MemDisk::with_capabilities(COPYING, Capabilities::ALL)
}

pub fn layout() -> Layout {
    Layout::new(".t").unwrap()
}

pub const WRITER: WriterId = WriterId::from_u128(0x77);
pub const HEAD: EntryHash = EntryHash::from_u128(0x11);

/// The entry a record carries; its contents are the library's business.
pub fn entry(after: EntryHash) -> Line {
    Line::seal(format!(r#"{{"prev":"{after}","kind":"intent"}}"#)).unwrap()
}

pub fn bound(files: &[(EntityId, &str)]) -> Bindings {
    let mut bindings = Bindings::default();
    for &(entity, text) in files {
        bindings.bound.insert(
            entity,
            toshokan::FileRef {
                path: path(text),
                state: toshokan::FileState::InSync,
            },
        );
    }
    bindings
}

/// What committing produced, short of appending the entry.
pub struct Run {
    pub plan: Rc<EffectPlan>,
    pub record: Rc<PendingRecord>,
}

/// Resolves `changes` and writes the pending record, filling saved files from
/// `contents`: what a commit does before its first file effect.
pub fn prepare(
    d: &mut impl Driven,
    writer: WriterId,
    changes: &[FileChange],
    contents: Vec<Fill>,
    bindings: &Bindings,
    env: &mut Env,
) -> Result<Result<Run, Refusal>, Error> {
    let layout = layout();
    let capabilities = d.capabilities(Root::Folder);
    let plan = match effects::resolve(changes, bindings, &layout, capabilities, env) {
        Ok(plan) => plan,
        Err(refusal) => return Ok(Err(refusal)),
    };
    let plan = Rc::new(plan);
    let record = Rc::new(PendingRecord::new(writer, "label", entry(HEAD), &plan));
    let prepared = effects::prepare(&layout, Rc::clone(&plan), Rc::clone(&record), identify());
    match d.run_filled(prepared, contents)? {
        Ok(()) => Ok(Ok(Run { plan, record })),
        Err(refusal) => Ok(Err(refusal)),
    }
}

/// The marker standing for the entry that closes a writer's record `name`.
pub fn closed(writer: WriterId, name: Nonce) -> RelPath {
    layout()
        .writer(writer)
        .join(&format!("closed-{name}"))
        .unwrap()
}

/// Runs the steps, appends the closing entry (a marker here) and removes the record.
pub fn complete(d: &mut impl Driven, run: &Run) -> Result<Applied, Error> {
    let layout = layout();
    let applied = d.run(effects::apply(
        &layout,
        run.plan.record,
        Rc::clone(&run.record),
        0,
        identify(),
    ))?;
    close(d, run.record.writer, run.plan.record)?;
    d.run(effects::finish(&layout, run.plan.record, &run.record))?;
    Ok(applied)
}

pub fn close(d: &mut impl Driven, writer: WriterId, name: Nonce) -> Result<(), Error> {
    let marker = closed(writer, name);
    let io = Io::Create {
        root: Root::Folder,
        path: marker.clone(),
        bytes: Vec::new(),
    };
    for io in [
        io,
        Io::Sync {
            root: Root::Folder,
            path: marker.parent().unwrap(),
        },
    ] {
        let failed = io.clone();
        d.one(io).map_err(|error| failed.failed(error))?;
    }
    Ok(())
}

/// Commits `changes` whole, saving `contents`, or refuses.
pub fn commit(
    d: &mut impl Driven,
    changes: &[FileChange],
    contents: Vec<Fill>,
    bindings: &Bindings,
    env: &mut Env,
) -> Result<Applied, Refusal> {
    let run = prepare(d, WRITER, changes, contents, bindings, env).unwrap()?;
    Ok(complete(d, &run).unwrap())
}

pub fn machine() -> Machine {
    Machine {
        folder: MemDisk::new(),
        local: MemDisk::new(),
    }
}

/// One running instance: it reads the folder, claims a writer, and creates one at
/// its first write when the pool gave none.
pub struct Instance {
    pub machine: Machine,
    pub reader: Reader,
    pub writer: Option<Writer>,
    pub start: Start,
    random: SeededRandom,
    clock: u64,
}

impl Instance {
    pub fn open(mut machine: Machine, seed: u64) -> Self {
        let mut reader = Reader::new(layout(), CachedView::default());
        run(&mut machine, reader.read()).unwrap();
        let Claimed { writer, start } =
            run(&mut machine, Writer::claim(&layout(), &reader)).unwrap();
        Self {
            machine,
            reader,
            writer,
            start,
            random: SeededRandom::new(seed),
            clock: 0,
        }
    }

    pub fn id(&self) -> WriterId {
        self.writer.as_ref().expect("a writer").id()
    }

    fn tick(&mut self) -> Hlc {
        self.clock += 1;
        Hlc {
            wall_ms: self.clock,
            counter: 0,
        }
    }

    /// Appends one intent, creating the writer first when there is none. Returns
    /// every entry it made durable, the genesis entry included.
    pub fn write(&mut self, label: &str) -> toshokan::Result<Vec<EntryHash>> {
        let mut made = Vec::new();
        if self.writer.is_none() {
            let id = WriterId::from_u128(self.random.next_u128());
            let segment = SegmentName::from_u128(self.random.next_u128());
            let at = self.tick();
            let writer = run(
                &mut self.machine,
                Writer::create(layout(), id, segment, "instance".into(), at, |genesis| {
                    CachedView::default().save(genesis)
                }),
            )?;
            made.push(writer.genesis());
            self.writer = Some(writer);
        }
        let kind = EntryKind::Intent(Logged {
            label: label.into(),
            ops: Vec::new(),
            displaced: Vec::new(),
            reverses: None,
        });
        let at = self.tick();
        let fresh = SegmentName::from_u128(self.random.next_u128());
        let writer = self.writer.take().expect("created above");
        let (writer, appended) = run(&mut self.machine, writer.append(vec![(at, kind)], fresh));
        let recorded = appended.and_then(|appended| {
            run(&mut self.machine, writer.record())?;
            Ok(appended)
        });
        self.writer = Some(writer);
        made.extend(recorded?.iter().map(Entry::hash));
        Ok(made)
    }

    pub fn compact(&mut self) -> toshokan::Result<Compacted> {
        let name = Nonce::from_u128(self.random.next_u128());
        let writer = self.writer.take().expect("a writer");
        run(&mut self.machine, self.reader.read_writer(writer.id()))?;
        let (writer, compacted) = run(&mut self.machine, compact(writer, &self.reader, name));
        self.writer = Some(writer);
        compacted
    }

    pub fn close(mut self) -> Machine {
        if let Some(writer) = self.writer.take() {
            run(&mut self.machine, writer.close()).unwrap();
        }
        self.machine
    }

    pub fn crash(mut self) -> Machine {
        self.machine.crash();
        self.machine
    }
}

/// What a reader without a cached view places from `folder`.
pub fn fresh(folder: &MemDisk) -> CachedView {
    let mut disk = folder.clone();
    let mut reader = Reader::new(layout(), CachedView::default());
    run(&mut disk, reader.read()).unwrap();
    reader.cached().clone()
}

/// Every entry `view` holds, of every writer.
pub fn held(view: &CachedView) -> BTreeSet<EntryHash> {
    view.writers()
        .values()
        .flat_map(|log| {
            let folded = log.snapshots().iter().flat_map(|s| s.folded.clone());
            let placed = log.entries().iter().map(|entry| entry.hash());
            folded.chain(placed).collect::<Vec<_>>()
        })
        .collect()
}

/// The files under `dir` in the folder of `disk`.
pub fn files_under(disk: &MemDisk, dir: &toshokan::RelPath) -> Vec<toshokan::RelPath> {
    disk.files(toshokan::Root::Folder)
        .into_keys()
        .filter(|path| path.starts_with(dir))
        .collect()
}
