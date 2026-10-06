//! Every backend behind one interface, driven by the drivers; the steps of a
//! commit as the library runs them; and instances writing through the log.

#![allow(dead_code, reason = "each test binary uses its own part")]

use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::rc::Rc;

use toshokan::asynch;
use toshokan::binding::Bindings;
use toshokan::blocking::{self, run, Backend, Native};
use toshokan::compaction::compact;
use toshokan::effects::{self, Applied, EffectPlan};
use toshokan::env::{ExactNames, PrefixIdentity, SeededRandom, TestClock};
use toshokan::io::{Capabilities, Kind, Range};
use toshokan::line::Line;
use toshokan::log::{Entry, EntryKind, Logged};
use toshokan::reader::{CachedView, Reader};
use toshokan::report::{Compacted, Start};
use toshokan::simulator::Machine;
use toshokan::writer::{Claimed, Writer};
use toshokan::pending::PendingRecord;
use toshokan::plan::FileChange;
use toshokan::{
    EntityId, EntryHash, Env, Error, Hlc, Identify, Io, IoResult, Layout, MemDisk, Nonce, Operation, Random,
    Refusal, RelPath, Reply, Root, SegmentName, Step, WriterId,
};

pub fn path(text: &str) -> RelPath {
    RelPath::new(text).unwrap()
}

/// One process's handle on a backend, driven by one of the drivers.
pub trait Driven: Sized {
    fn capabilities(&self, root: Root) -> Capabilities;
    fn run<O: Operation>(&mut self, operation: O) -> O::Output;
    /// A handle of another process on the same storage.
    fn other_process(&self) -> Self;

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

    fn run<O: Operation>(&mut self, operation: O) -> O::Output {
        blocking::run(&mut self.0, operation)
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

    fn run<O: Operation>(&mut self, operation: O) -> O::Output {
        pollster::block_on(asynch::run(&self.0, operation))
    }

    fn other_process(&self) -> Self {
        Self(self.0.process())
    }
}

pub struct NativeDirs {
    backend: Native,
    dirs: std::rc::Rc<[tempfile::TempDir; 2]>,
}

impl NativeDirs {
    pub fn new() -> Self {
        let dirs = std::rc::Rc::new([tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap()]);
        Self {
            backend: Native::new(dirs[0].path(), dirs[1].path()),
            dirs,
        }
    }
}

impl Driven for NativeDirs {
    fn capabilities(&self, root: Root) -> Capabilities {
        self.backend.capabilities(root)
    }

    fn run<O: Operation>(&mut self, operation: O) -> O::Output {
        blocking::run(&mut self.backend, operation)
    }

    fn other_process(&self) -> Self {
        Self {
            backend: Native::new(self.dirs[0].path(), self.dirs[1].path()),
            dirs: self.dirs.clone(),
        }
    }
}

/// Runs each behavior on fresh, empty roots of every backend.
#[macro_export]
macro_rules! for_every_backend {
    ($suite:ident: $($behavior:ident),* $(,)?) => {
        mod blocking_mem {
            $(#[test] fn $behavior() { super::$suite::$behavior(&mut $crate::common::BlockingMem(toshokan::MemDisk::new())); })*
        }
        mod async_mem {
            $(#[test] fn $behavior() { super::$suite::$behavior(&mut $crate::common::AsyncMem(toshokan::MemDisk::new())); })*
        }
        mod native {
            $(#[test] fn $behavior() { super::$suite::$behavior(&mut $crate::common::NativeDirs::new()); })*
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

/// Resolves `changes` and writes the pending record: what a commit does before its
/// first file effect.
pub fn prepare(
    d: &mut impl Driven,
    writer: WriterId,
    changes: &[FileChange],
    bindings: &Bindings,
    env: &mut Env,
) -> Result<Result<Run, Refusal>, Error> {
    let layout = layout();
    let capabilities = d.capabilities(Root::Folder);
    let plan = match effects::resolve(changes, &[], bindings, &layout, capabilities, env) {
        Ok(plan) => plan,
        Err(refusal) => return Ok(Err(refusal)),
    };
    let plan = Rc::new(plan);
    let record = Rc::new(PendingRecord::new(writer, "label", entry(HEAD), &plan));
    match d.run(effects::prepare(
        &layout,
        Rc::clone(&plan),
        Rc::clone(&record),
        identify(),
    ))? {
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
    d.run(effects::finish(&layout, run.record.writer, run.plan.record))?;
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

/// Commits `changes` whole, or refuses.
pub fn commit(
    d: &mut impl Driven,
    changes: &[FileChange],
    bindings: &Bindings,
    env: &mut Env,
) -> Result<Applied, Refusal> {
    let run = prepare(d, WRITER, changes, bindings, env).unwrap()?;
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
            run(&mut machine, Writer::claim(&layout(), reader.logs())).unwrap();
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
                Writer::create(layout(), id, segment, "instance".into(), at),
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
        self.writer = Some(writer);
        made.extend(appended?.iter().map(Entry::hash));
        Ok(made)
    }

    pub fn compact(&mut self) -> toshokan::Result<Compacted> {
        let name = Nonce::from_u128(self.random.next_u128());
        let writer = self.writer.take().expect("a writer");
        run(&mut self.machine, self.reader.read_writer(writer.id()))?;
        let own = &self.reader.logs()[&writer.id()];
        let (writer, compacted) = run(&mut self.machine, compact(writer, own, name));
        self.writer = Some(writer);
        compacted
    }

    pub fn close(mut self) -> Machine {
        if let Some(writer) = &mut self.writer {
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
            let placed = log.entries().iter().map(Entry::hash);
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
