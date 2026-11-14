//! The library end to end, through the blocking and the async driver.
//!
//! Every instance runs on a probe that panics on a write in another writer's
//! directory and counts the writes it passes to the folder.

use std::cell::{Cell, RefCell};
use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::rc::Rc;

use toshokan::asynch;
use toshokan::binding::Bindings;
use toshokan::blocking::{self, Backend};
use toshokan::crash;
use toshokan::env::{ExactNames, PrefixIdentity, SeededRandom, TestClock};
use toshokan::error::Why;
use toshokan::intent::{Driver, Intent};
use toshokan::io::{Capabilities, Capability, IoError, Meta, Range};
use toshokan::layout::SEGMENT_EXTENSION;
use toshokan::library::COMPACT_AFTER;
use toshokan::line::MAX_LINE;
use toshokan::log::{Entry, EntryKind, FileFact, Genesis, Logged, Op, Run, Settlement};
use toshokan::plan::FactChange;
use toshokan::reader::MAX_FILE;
use toshokan::report::{
    By, Change, Committed, Compacted, Emptied, HistoryItem, Item, ItemKind, Joined, Opened,
    Presence, Refreshed, Rekey, Start, TrashItem, What, WriterInfo,
};
use toshokan::rows::{DirWhat, How, RowWhat, Who};
use toshokan::simulator::{Machine, Simulator};
use toshokan::snapshot::Snapshot;
use toshokan::view::Conflicted;
use toshokan::{
    EntityId, EntryHash, Env, Error, Expect, Field, FileId, FileRef, FileState, Hlc, Identify,
    Identity, Invalid, Io, IoResult, Lag, Layout, Local, MemDisk, Nonce, Random, Refusal, Register,
    RelPath, Reply, Root, Row, RowChanges, RowState, Schema, Set, Unbindable, View, WriterId,
};

const ORIGIN: Register<String> = Register::new("origin");
const TAGS: Set<String> = Set::new("tags");
const PLAYS: Register<EntityId> = Register::new("plays");

fn schema() -> Schema {
    Schema::of(&[ORIGIN.key(), TAGS.key(), PLAYS.key()]).unwrap()
}

fn layout() -> Layout {
    Layout::new(".t").unwrap()
}

fn path(text: &str) -> RelPath {
    RelPath::new(text).unwrap()
}

const IDENTIFY: PrefixIdentity = PrefixIdentity { prefix: 1 << 16 };

fn identity(bytes: &[u8]) -> Identity {
    IDENTIFY.of_bytes(&RelPath::ROOT, bytes)
}

fn env(label: &str, seed: u64, clock: &TestClock) -> Env {
    Env {
        clock: Box::new(clock.clone()),
        random: Box::new(SeededRandom::new(seed)),
        identify: Rc::new(IDENTIFY),
        names: Box::new(ExactNames),
        label: label.into(),
    }
}

/// What the probe saw pass to the folder.
#[derive(Default)]
struct Seen {
    /// The writer whose directory this instance writes; a new one only once its
    /// directory is created.
    own: Option<String>,
    folder_writes: u64,
    local_writes: u64,
    /// Lists, stats and reads of library paths.
    library_reads: u64,
    /// Reads of library files' bytes.
    file_reads: u64,
    /// The longest read of a pending record.
    longest_pending_read: u64,
    /// Moves to this path fail, as a backend's might.
    refused: Option<RelPath>,
    /// Reads of library paths fail once the head is next recorded.
    blind_after_record: bool,
    blind: bool,
    /// Requests this picks fail.
    failing: Option<Box<Picks>>,
    /// Reads of files at or under this path fail, each alone among the reads of a
    /// batch.
    unreadable: Option<RelPath>,
    /// Writes in the folder fail as a full volume's do.
    full: bool,
    /// Requests this picks are carried out, and then fail, as a reply a backend
    /// loses.
    unanswered: Option<Box<Picks>>,
    /// Bytes written in the local root.
    local_written: u64,
    /// The longest read of a file directly in a writer's directory.
    log_read: u64,
    /// Renames from one library path to another.
    library_renames: u64,
    /// The one modification time the folder reports for every file, as a volume
    /// whose times are coarser than the writes between them.
    coarse: Option<u64>,
    /// How often each library directory was listed.
    listed: BTreeMap<RelPath, u64>,
    /// Each library file whose bytes were read.
    read_files: BTreeSet<RelPath>,
    /// Files the next listings of their directory leave out, the first by the next
    /// one, as while a tool replaces them.
    hidden: VecDeque<RelPath>,
    /// How often each library directory was listed with its files' metadata.
    list_stats: BTreeMap<RelPath, u64>,
    /// The folder keeps each `é` of a name decomposed, as HFS+ does, and finds a
    /// name by either form.
    decomposing: bool,
}

type Picks = dyn Fn(&Io) -> bool;

/// Each file `io` reads, with the range it asks for.
fn reads(io: &Io) -> impl Iterator<Item = (&RelPath, Range)> {
    let reads: Vec<(&RelPath, Range)> = match io {
        Io::Read { path, range, .. } => vec![(path, *range)],
        Io::ReadMany { reads, .. } => reads.iter().map(|(path, range)| (path, *range)).collect(),
        _ => Vec::new(),
    };
    reads.into_iter()
}

/// One instance's storage: a machine, checked and counted.
#[derive(Clone)]
struct Probe {
    machine: Machine,
    seen: Rc<RefCell<Seen>>,
}

impl Probe {
    fn new(machine: &Machine) -> Self {
        Self {
            machine: Machine {
                folder: machine.folder.process(),
                local: machine.local.process(),
            },
            seen: Rc::default(),
        }
    }

    fn folder_writes(&self) -> u64 {
        self.seen.borrow().folder_writes
    }

    fn local_writes(&self) -> u64 {
        self.seen.borrow().local_writes
    }

    fn library_reads(&self) -> u64 {
        self.seen.borrow().library_reads
    }

    fn file_reads(&self) -> u64 {
        self.seen.borrow().file_reads
    }

    fn library_renames(&self) -> u64 {
        self.seen.borrow().library_renames
    }

    fn read_files(&self) -> BTreeSet<RelPath> {
        self.seen.borrow().read_files.clone()
    }

    fn listed(&self) -> BTreeMap<RelPath, u64> {
        self.seen.borrow().listed.clone()
    }

    /// Panics on a write under toshokan's root outside the directory of the writer
    /// this instance writes as. The directories holding every writer's may be
    /// made and synced.
    fn check(&self, io: &Io) {
        self.count(io);
        for (path, range) in reads(io) {
            if path.components().any(|name| name == "pending") {
                let mut seen = self.seen.borrow_mut();
                let len = range.len.min(self.len(path));
                seen.longest_pending_read = seen.longest_pending_read.max(len);
            }
        }
        if io.root() == Root::Folder && !io.mutates() && !layout().owns(io.path()) {
            let mut seen = self.seen.borrow_mut();
            seen.library_reads += 1;
            let files = reads(io).filter(|(path, _)| !layout().owns(path));
            seen.file_reads += files.count() as u64;
        }
        if io.mutates() && io.root() == Root::Local {
            self.seen.borrow_mut().local_writes += 1;
        }
        if !io.mutates() || io.root() != Root::Folder {
            return;
        }
        self.seen.borrow_mut().folder_writes += 1;
        let writers = layout().writers();
        let mut paths = vec![io.path()];
        if let Io::Rename { from, .. } = io {
            paths.push(from);
        }
        for path in paths {
            let shared = writers.starts_with(path);
            if !layout().owns(path) || shared && matches!(io, Io::MakeDir { .. } | Io::Sync { .. })
            {
                continue;
            }
            let Some(writer) = path
                .strip_prefix(&writers)
                .and_then(|rest| rest.split('/').next().map(str::to_owned))
            else {
                panic!("{io:?} writes outside every writer's directory");
            };
            let dir = writers.join(&writer).unwrap();
            let new = self
                .machine
                .folder
                .files(Root::Folder)
                .keys()
                .all(|p| !p.starts_with(&dir))
                && !self.machine.folder.directories(Root::Folder).contains(&dir);
            let mut seen = self.seen.borrow_mut();
            match &seen.own {
                Some(own) if *own == writer => {}
                _ if new => seen.own = Some(writer),
                own => panic!("{io:?} writes in {writer}'s directory as {own:?}"),
            }
        }
    }

    fn count(&self, io: &Io) {
        let mut seen = self.seen.borrow_mut();
        if let Io::Create { root, bytes, .. }
        | Io::Append { root, bytes, .. }
        | Io::Write { root, bytes, .. } = io
        {
            if *root == Root::Local {
                seen.local_written += bytes.len() as u64;
            }
        }
        if io.root() != Root::Folder {
            return;
        }
        match io {
            Io::Rename { from, to, .. } if !layout().owns(from) && !layout().owns(to) => {
                seen.library_renames += 1;
            }
            Io::List { dir, .. } if !layout().owns(dir) => {
                *seen.listed.entry(dir.clone()).or_default() += 1;
            }
            Io::ListStat { dir, .. } if !layout().owns(dir) => {
                *seen.list_stats.entry(dir.clone()).or_default() += 1;
            }
            _ => {}
        }
        for (path, _) in reads(io).filter(|(path, _)| !layout().owns(path)) {
            seen.read_files.insert(path.clone());
        }
        for (path, range) in reads(io) {
            if path.parent().and_then(|dir| dir.parent()) == Some(layout().writers()) {
                seen.log_read = seen.log_read.max(range.len.min(self.len(path)));
            }
        }
    }

    fn refuse(&self, io: &Io) -> Result<(), toshokan::IoError> {
        let mut seen = self.seen.borrow_mut();
        if io.root() == Root::Local && io.mutates() && io.path().name() == Some("head.json") {
            seen.blind |= seen.blind_after_record;
        }
        if seen.blind && io.root() == Root::Folder && !io.mutates() && !layout().owns(io.path()) {
            return Err(toshokan::IoError::Other("blinded by the test".into()));
        }
        if seen.failing.as_ref().is_some_and(|failing| failing(io)) {
            return Err(toshokan::IoError::Other("failed by the test".into()));
        }
        let unreadable = |path: &RelPath| {
            seen.unreadable
                .as_ref()
                .is_some_and(|at| path.starts_with(at))
        };
        if matches!(io, Io::Read { path, .. } if unreadable(path)) {
            return Err(toshokan::IoError::Denied);
        }
        if seen.full && io.root() == Root::Folder && io.mutates() {
            return Err(toshokan::IoError::NoSpace);
        }
        match (io, &seen.refused) {
            (Io::Rename { to, .. } | Io::Create { path: to, .. }, Some(refused))
                if to == refused =>
            {
                Err(toshokan::IoError::Other("refused by the test".into()))
            }
            _ => Ok(()),
        }
    }

    /// `done`, or a failure when the probe loses the reply to `io`.
    fn answer(&self, io: &Io, done: IoResult) -> IoResult {
        let done = self.hide_once(io, done);
        let seen = self.seen.borrow();
        let coarse = seen.coarse.filter(|_| io.root() == Root::Folder);
        let done = match (coarse, done) {
            (Some(at), Ok(Reply::Stat(meta))) => Ok(Reply::Stat(meta.map(|m| coarse_at(m, at)))),
            (Some(at), Ok(Reply::ListedStat(listed))) => Ok(Reply::ListedStat(
                listed
                    .into_iter()
                    .map(|(name, meta)| (name, coarse_at(meta, at)))
                    .collect(),
            )),
            (_, done) => done,
        };
        let done = match (io, done, &seen.unreadable) {
            (Io::ReadMany { reads, .. }, Ok(Reply::ReadMany(read)), Some(unreadable)) => {
                let read = reads.iter().zip(read).map(|((path, _), read)| match path {
                    _ if path.starts_with(unreadable) => Err(toshokan::IoError::Denied),
                    _ => read,
                });
                Ok(Reply::ReadMany(read.collect()))
            }
            (_, done, _) => done,
        };
        match seen.unanswered.as_ref().is_some_and(|picks| picks(io)) {
            true => done.and(Err(toshokan::IoError::Other(
                "unanswered by the test".into(),
            ))),
            false => done,
        }
    }

    /// `done` without the first file [`Seen::hidden`] names, where `io` lists its
    /// directory.
    fn hide_once(&self, io: &Io, done: IoResult) -> IoResult {
        let mut seen = self.seen.borrow_mut();
        let (Io::ListStat { dir, .. }, Some(hidden)) = (io, seen.hidden.front()) else {
            return done;
        };
        if hidden.parent().as_ref() != Some(dir) {
            return done;
        }
        let name = hidden.name().map(str::to_owned);
        seen.hidden.pop_front();
        match done {
            Ok(Reply::ListedStat(listed)) => Ok(Reply::ListedStat(
                listed
                    .into_iter()
                    .filter(|(entry, _)| Some(entry) != name.as_ref())
                    .collect(),
            )),
            done => done,
        }
    }

    fn len(&self, path: &RelPath) -> u64 {
        self.machine
            .folder
            .files(Root::Folder)
            .get(path)
            .map_or(0, |bytes| bytes.len() as u64)
    }
}

/// `meta` at `at`, the one time a coarse volume reports for every file.
fn coarse_at(meta: Meta, at: u64) -> Meta {
    Meta {
        modified: meta.modified.map(|_| at),
        ..meta
    }
}

trait StripPrefix {
    fn strip_prefix(&self, prefix: &RelPath) -> Option<&str>;
}

impl StripPrefix for RelPath {
    fn strip_prefix(&self, prefix: &RelPath) -> Option<&str> {
        self.as_str()
            .strip_prefix(prefix.as_str())?
            .strip_prefix('/')
    }
}

impl Backend for Probe {
    fn capabilities(&self, root: Root) -> Capabilities {
        Backend::capabilities(&self.machine, root)
    }

    fn perform(&mut self, io: Io) -> IoResult {
        self.check(&io);
        self.refuse(&io)?;
        let done = self.machine.perform(self.stored(io.clone()));
        self.answer(&io, done)
    }
}

impl asynch::Fs for Probe {
    fn capabilities(&self, root: Root) -> Capabilities {
        Backend::capabilities(&self.machine, root)
    }

    async fn perform(&self, io: Io) -> IoResult {
        self.check(&io);
        self.refuse(&io)?;
        let done = asynch::Fs::perform(&self.machine, self.stored(io.clone())).await;
        self.answer(&io, done)
    }
}

impl Probe {
    /// `io` with its paths as the folder keeps them.
    fn stored(&self, io: Io) -> Io {
        match self.seen.borrow().decomposing {
            true => decomposed(io),
            false => io,
        }
    }
}

/// `io` with each `é` of its paths decomposed.
fn decomposed(io: Io) -> Io {
    let d = |path: RelPath| RelPath::new(&path.as_str().replace('\u{e9}', "e\u{301}")).unwrap();
    match io {
        Io::List { root, dir } => Io::List { root, dir: d(dir) },
        Io::Stat { root, path } => Io::Stat {
            root,
            path: d(path),
        },
        Io::ListStat { root, dir } => Io::ListStat { root, dir: d(dir) },
        Io::Read { root, path, range } => Io::Read {
            root,
            path: d(path),
            range,
        },
        Io::ReadMany { root, reads } => Io::ReadMany {
            root,
            reads: reads
                .into_iter()
                .map(|(path, range)| (d(path), range))
                .collect(),
        },
        Io::Create { root, path, bytes } => Io::Create {
            root,
            path: d(path),
            bytes,
        },
        Io::Append { root, path, bytes } => Io::Append {
            root,
            path: d(path),
            bytes,
        },
        Io::Write {
            root,
            path,
            offset,
            bytes,
        } => Io::Write {
            root,
            path: d(path),
            offset,
            bytes,
        },
        Io::Fill {
            root,
            path,
            content,
        } => Io::Fill {
            root,
            path: d(path),
            content,
        },
        Io::Rename { root, from, to } => Io::Rename {
            root,
            from: d(from),
            to: d(to),
        },
        Io::Remove { root, path } => Io::Remove {
            root,
            path: d(path),
        },
        Io::RemoveDir { root, path } => Io::RemoveDir {
            root,
            path: d(path),
        },
        Io::MakeDir { root, path } => Io::MakeDir {
            root,
            path: d(path),
        },
        Io::Sync { root, path } => Io::Sync {
            root,
            path: d(path),
        },
        Io::Lock { name } => Io::Lock { name: d(name) },
        Io::Unlock { name } => Io::Unlock { name: d(name) },
    }
}

/// Names as a volume that composes each `é` takes them.
struct Composing;

impl toshokan::Names for Composing {
    fn key(&self, path: &str) -> String {
        path.replace("e\u{301}", "\u{e9}")
    }
}

/// Tells the probe which writer an instance resumed.
fn resumed(seen: &RefCell<Seen>, opened: &Opened) {
    if let Start::Resumed(writer) = opened.start {
        seen.borrow_mut().own = Some(writer.to_string());
    }
}

/// Both drivers' libraries behind one interface.
trait Facade: Sized {
    /// The driver's library, as an intent borrows it.
    type Lib<'a>: Driver
    where
        Self: 'a;

    fn open(probe: Probe, env: Env) -> Result<(Self, Opened), Error>;
    fn view(&self) -> View;
    /// What the view would show folded again from scratch.
    fn refolded(&self) -> toshokan::merge::Folded;
    fn history(&self) -> Vec<HistoryItem>;
    fn commit<'a>(
        &'a mut self,
        label: &str,
        build: impl FnOnce(Intent<Self::Lib<'a>>) -> Intent<Self::Lib<'a>>,
    ) -> Result<Committed, Error>;
    fn undo(&mut self) -> Result<Committed, Error>;
    fn redo(&mut self) -> Result<Committed, Error>;
    fn forget_undo(&mut self);
    fn refresh(&mut self) -> Result<Refreshed, Error>;
    fn rescan(&mut self) -> Result<Refreshed, Error>;
    fn rescan_paths(&mut self, paths: Vec<RelPath>) -> Result<Refreshed, Error>;
    /// What the library binds, and what binding every fact afresh binds.
    fn rebound(&mut self) -> [(Bindings, Vec<Op>); 2];
    fn others(&mut self) -> Result<Vec<WriterInfo>, Error>;
    fn trash(&mut self) -> Result<Vec<TrashItem>, Error>;
    fn empty_trash(&mut self, kept: BTreeSet<Nonce>) -> Result<Emptied, Error>;
    fn compact(&mut self) -> Result<Compacted, Error>;
    fn settle(
        &mut self,
        orphan: toshokan::report::Orphan,
        how: Settlement,
    ) -> Result<Committed, Error>;
    fn close(self) -> Result<(), Error>;
}

struct Blocking(blocking::Library<Probe>);
struct Async(asynch::Library<Probe>);

impl Facade for Blocking {
    type Lib<'a> = &'a mut blocking::Library<Probe>;

    fn open(probe: Probe, env: Env) -> Result<(Self, Opened), Error> {
        let seen = Rc::clone(&probe.seen);
        let (library, opened) = blocking::Library::open(probe, layout(), &schema(), env)?;
        resumed(&seen, &opened);
        Ok((Self(library), opened))
    }
    fn view(&self) -> View {
        self.0.view()
    }
    fn refolded(&self) -> toshokan::merge::Folded {
        self.0.refolded()
    }
    fn history(&self) -> Vec<HistoryItem> {
        self.0.history().to_vec()
    }
    fn commit<'a>(
        &'a mut self,
        label: &str,
        build: impl FnOnce(Intent<Self::Lib<'a>>) -> Intent<Self::Lib<'a>>,
    ) -> Result<Committed, Error> {
        build(self.0.intent(label)).commit()
    }
    fn undo(&mut self) -> Result<Committed, Error> {
        self.0.undo()
    }
    fn forget_undo(&mut self) {
        self.0.forget_undo();
    }
    fn redo(&mut self) -> Result<Committed, Error> {
        self.0.redo()
    }
    fn refresh(&mut self) -> Result<Refreshed, Error> {
        self.0.refresh()
    }
    fn rescan(&mut self) -> Result<Refreshed, Error> {
        self.0.rescan()
    }
    fn rescan_paths(&mut self, paths: Vec<RelPath>) -> Result<Refreshed, Error> {
        self.0.rescan_paths(paths)
    }
    fn rebound(&mut self) -> [(Bindings, Vec<Op>); 2] {
        self.0.rebound()
    }
    fn others(&mut self) -> Result<Vec<WriterInfo>, Error> {
        self.0.others()
    }
    fn trash(&mut self) -> Result<Vec<TrashItem>, Error> {
        self.0.trash()
    }
    fn empty_trash(&mut self, kept: BTreeSet<Nonce>) -> Result<Emptied, Error> {
        self.0.empty_trash(kept)
    }
    fn compact(&mut self) -> Result<Compacted, Error> {
        self.0.compact()
    }
    fn settle(
        &mut self,
        orphan: toshokan::report::Orphan,
        how: Settlement,
    ) -> Result<Committed, Error> {
        self.0.settle(orphan, how)
    }
    fn close(self) -> Result<(), Error> {
        self.0.close().map(drop)
    }
}

impl Facade for Async {
    type Lib<'a> = &'a mut asynch::Library<Probe>;

    fn open(probe: Probe, env: Env) -> Result<(Self, Opened), Error> {
        let seen = Rc::clone(&probe.seen);
        let schema = schema();
        let opened = asynch::Library::open(probe, layout(), &schema, env);
        let (library, opened) = pollster::block_on(opened)?;
        resumed(&seen, &opened);
        Ok((Self(library), opened))
    }
    fn view(&self) -> View {
        self.0.view()
    }
    fn refolded(&self) -> toshokan::merge::Folded {
        self.0.refolded()
    }
    fn history(&self) -> Vec<HistoryItem> {
        self.0.history().to_vec()
    }
    fn commit<'a>(
        &'a mut self,
        label: &str,
        build: impl FnOnce(Intent<Self::Lib<'a>>) -> Intent<Self::Lib<'a>>,
    ) -> Result<Committed, Error> {
        pollster::block_on(build(self.0.intent(label)).commit())
    }
    fn undo(&mut self) -> Result<Committed, Error> {
        pollster::block_on(self.0.undo())
    }
    fn forget_undo(&mut self) {
        self.0.forget_undo();
    }
    fn redo(&mut self) -> Result<Committed, Error> {
        pollster::block_on(self.0.redo())
    }
    fn refresh(&mut self) -> Result<Refreshed, Error> {
        pollster::block_on(self.0.refresh())
    }
    fn rescan(&mut self) -> Result<Refreshed, Error> {
        pollster::block_on(self.0.rescan())
    }
    fn rescan_paths(&mut self, paths: Vec<RelPath>) -> Result<Refreshed, Error> {
        pollster::block_on(self.0.rescan_paths(paths))
    }
    fn rebound(&mut self) -> [(Bindings, Vec<Op>); 2] {
        self.0.rebound()
    }
    fn others(&mut self) -> Result<Vec<WriterInfo>, Error> {
        pollster::block_on(self.0.others())
    }
    fn trash(&mut self) -> Result<Vec<TrashItem>, Error> {
        pollster::block_on(self.0.trash())
    }
    fn empty_trash(&mut self, kept: BTreeSet<Nonce>) -> Result<Emptied, Error> {
        pollster::block_on(self.0.empty_trash(kept))
    }
    fn compact(&mut self) -> Result<Compacted, Error> {
        pollster::block_on(self.0.compact())
    }
    fn settle(
        &mut self,
        orphan: toshokan::report::Orphan,
        how: Settlement,
    ) -> Result<Committed, Error> {
        pollster::block_on(self.0.settle(orphan, how))
    }
    fn close(self) -> Result<(), Error> {
        pollster::block_on(self.0.close()).map(drop)
    }
}

thread_local! {
    /// What the folder of each disk a scenario makes can do.
    static FOLDER: Cell<Capabilities> = const { Cell::new(Capabilities::ALL) };
}

/// A folder that can neither rename nor sync, whose moves copy.
const COPYING: Capabilities = Capabilities {
    append: true,
    rename_file: false,
    no_replace: false,
    rename_dir: false,
    fsync: false,
};

/// An empty disk whose folder can do what the scenario runs with.
fn disk() -> MemDisk {
    MemDisk::with_capabilities(FOLDER.get(), Capabilities::ALL)
}

/// Runs `scenario` on folders that can do only what `folder` says, then on
/// folders that can do everything again, however the scenario ends.
fn on(folder: Capabilities, scenario: fn()) {
    struct Restore;
    impl Drop for Restore {
        fn drop(&mut self) {
            FOLDER.set(Capabilities::ALL);
        }
    }
    let _restore = Restore;
    FOLDER.set(folder);
    scenario();
}

/// Runs each scenario through both drivers, on folders that can do everything and
/// on folders whose moves copy.
macro_rules! through_both {
    ($($scenario:ident),* $(,)?) => {
        mod blocking_driver {
            $(#[test] fn $scenario() { super::$scenario::<super::Blocking>(); })*
        }
        mod async_driver {
            $(#[test] fn $scenario() { super::$scenario::<super::Async>(); })*
        }
        mod blocking_copying {
            $(#[test] fn $scenario() { super::on(super::COPYING, super::$scenario::<super::Blocking>); })*
        }
        mod async_copying {
            $(#[test] fn $scenario() { super::on(super::COPYING, super::$scenario::<super::Async>); })*
        }
    };
}

through_both!(
    two_writers_tag_one_library_and_converge,
    what_refreshes_fold_in_is_what_a_fresh_open_shows,
    a_refresh_that_finds_nothing_new_writes_nothing,
    a_refresh_scans_only_the_files_other_writers_moved,
    a_conflict_is_shown_and_resolved,
    a_clone_forks_into_items_taken_or_dismissed,
    a_rename_on_a_branch_kept_apart_is_that_branchs_not_an_orphan,
    a_settlement_on_a_branch_kept_apart_is_not_offered_again,
    undo_after_a_dismissal_undoes_the_change_before_it,
    a_move_of_thousands_of_files_is_one_entry_and_undoes_as_one_change,
    an_intent_wider_than_a_line_is_refused_and_writes_nothing,
    a_retired_writers_trash_is_still_listed,
    a_tree_moved_file_by_file_names_the_directories_a_file_it_did_not_know_keeps,
    an_entry_of_a_copied_instance_does_not_close_another_instances_record,
    an_unfinished_effect_of_a_writer_that_retires_stays_this_installs,
    a_commit_whose_entry_did_not_sync_keeps_its_record,
    thousands_of_entities_deleted_beside_their_files_come_back_with_undo,
    a_fork_compaction_finds_retires_the_writer,
    one_intent_takes_from_runs_of_two_forked_writers,
    a_change_made_as_ones_own_names_no_run,
    a_file_change_is_taken_only_while_the_folder_holds_that_file,
    a_run_counts_and_spans_its_entries_whether_or_not_its_branch_compacted,
    a_run_that_would_change_nothing_is_shown_until_dismissed,
    a_restored_folder_makes_the_writer_rekey,
    two_saves_undo_and_redo_one_after_the_other,
    saves_of_one_length_within_the_volumes_time_resolution_stay_in_sync,
    another_writers_rewrite_at_one_length_and_time_shows_at_the_next_refresh,
    two_other_writers_rewrites_at_one_length_and_time_show_at_one_refresh,
    a_rewrite_at_one_length_and_time_a_commit_read_first_shows_at_the_next_refresh,
    an_undo_is_not_refused_for_a_delete_older_than_its_change,
    an_undo_is_not_refused_for_another_writers_pin_alone,
    another_writers_rewrite_at_one_length_and_time_shows_where_it_then_moved_the_file,
    another_writers_move_of_a_file_this_view_rewrote_since_shows_where_it_lands,
    an_undo_is_not_refused_for_a_pin_another_writer_compacted,
    an_undo_after_another_writer_compacted_keeps_the_others_bytes,
    a_tree_move_undone_after_another_writer_compacted_carries_its_file_along,
    another_writers_move_onto_a_file_of_one_length_and_time_shows_where_it_lands,
    a_rewrite_moved_onto_a_file_of_one_length_and_time_binds_as_a_fresh_open_does,
    another_writers_swap_through_a_spare_name_binds_as_a_fresh_open_does,
    a_file_another_writer_moved_away_and_sync_kept_shows_a_rewrite_logged_before,
    a_file_two_writers_saved_at_one_length_and_time_is_the_entitys_whose_bytes_it_holds,
    a_write_back_because_a_later_entry_is_cut_short_is_no_ones_change,
    another_writers_write_back_because_a_later_entry_is_cut_short_is_no_ones_change,
    another_writers_rewrite_under_another_spelling_shows_at_the_next_refresh,
    another_writers_rewrite_whose_log_arrives_before_its_file_is_never_saved_over,
    an_undo_is_refused_over_another_writers_change_under_another_spelling,
    another_writers_rewrite_read_only_through_its_snapshot_shows_at_the_next_refresh,
    a_save_another_file_lands_over_before_it_is_seen_names_its_own_bytes,
    a_refused_save_over_a_rewrite_shows_it_at_the_next_refresh,
    a_save_whose_placed_file_goes_before_it_is_seen_names_its_own_bytes,
    a_save_resumed_after_another_file_landed_over_its_placed_file_names_its_own_bytes,
    a_refused_save_takes_no_identity_from_a_file_since_put_back,
    a_refused_save_under_another_spelling_reports_the_listed_file_rewritten,
    a_save_within_the_time_of_an_identity_read_stays_in_sync_after_reopening,
    a_compacted_writer_is_shown_and_reported_and_goes_with_its_files,
    an_interrupted_adoption_is_settled_as_an_adoption,
    the_first_write_after_a_crash_follows_the_settled_intent,
    the_first_undo_after_a_crash_undoes_the_settled_intent,
    undoing_a_save_restores_the_displaced_bytes,
    a_copy_has_no_entity_until_one_is_said,
    a_file_two_writers_adopted_unseen_is_the_lower_entitys,
    an_outside_edit_of_a_file_two_writers_adopted_gives_it_to_the_lower_entity,
    an_outside_delete_of_a_file_two_writers_adopted_leaves_one_missing_row,
    a_save_of_a_file_two_writers_adopted_and_edited_outside_is_why_the_other_is_lost,
    a_copy_seen_before_its_originals_rename_is_never_joined_into_it,
    a_join_gives_the_lower_entity_what_both_held_and_no_undo_takes_it_back,
    writers_joining_one_entity_at_once_converge,
    a_move_keeps_its_tags_and_is_pinned_by_the_next_commit,
    undoing_a_delete_binds_the_file_moved_while_deleted,
    undoing_a_delete_puts_a_file_moved_outside_back_where_it_was,
    undoing_a_delete_puts_a_file_back_under_the_spelling_it_was_found_by,
    undoing_a_rename_puts_a_file_moved_outside_back_where_it_was,
    undoing_a_rename_whose_file_was_moved_back_outside_leaves_it_there,
    a_file_written_holding_a_missing_entitys_file_is_named_by_that_entitys_row,
    a_new_entitys_file_at_a_missing_entitys_path_is_named_by_its_own_row,
    an_undo_is_committed_when_its_identity_read_fails,
    a_rename_is_committed_when_its_rescan_fails_and_binds_nothing_stale,
    a_move_read_by_a_rescan_that_failed_is_not_pinned_back_by_the_next_commit,
    a_settlement_is_committed_when_its_rescan_fails,
    a_failed_rescan_after_a_rename_binds_no_copy_in_its_place,
    a_commit_whose_head_cannot_be_recorded_is_kept_and_continued,
    a_refresh_while_the_head_cannot_be_recorded_shows_other_writers_changes,
    a_commit_removes_its_record_once_its_entries_are_durable,
    a_commit_whose_compaction_retires_its_writer_leaves_the_record_in_place,
    a_record_whose_removal_fails_is_removed_before_the_next_write,
    closing_removes_a_record_whose_removal_failed,
    closing_while_the_head_cannot_be_recorded_lets_the_writer_go,
    a_record_a_crash_left_spent_is_not_settled_again_and_goes_at_the_next_write,
    a_record_whose_writing_seems_to_fail_is_settled_before_the_next_write,
    a_record_that_never_landed_is_not_carried_out_by_the_next_write,
    a_copied_move_whose_source_removal_fails_is_finished_by_the_next_write,
    an_append_whose_reply_was_lost_stands_and_the_next_write_takes_a_new_writer,
    a_new_segment_whose_reply_was_lost_stands_and_the_next_write_takes_a_new_writer,
    writers_that_retire_over_lost_writes_leave_their_trash_empty,
    a_folder_that_never_syncs_a_directory_turns_read_only_after_one_new_writer,
    a_new_library_whose_folder_never_syncs_a_directory_leaves_at_most_two_writers,
    a_full_folder_after_a_writer_stopped_is_said_as_full,
    a_spent_record_is_reported_only_where_its_entry_has_not_arrived,
    a_settled_record_is_removed_and_logged_once,
    identities_a_failed_write_lost_are_kept_by_the_next_scan,
    a_commit_scans_only_what_it_moved_and_a_rescan_finds_the_rest,
    an_identity_read_for_a_new_time_is_read_once_per_install,
    opening_viewing_and_refreshing_write_nothing_in_the_folder,
    opening_scans_no_library_file_and_the_first_refresh_scans_them_all,
    swap_files_a_browser_left_are_neither_scanned_nor_read,
    a_view_lists_each_library_files_length_once_scanned,
    a_file_expected_as_listed_is_refused_once_its_length_or_time_moved,
    a_directory_that_cannot_be_listed_leaves_its_files_unknown_and_the_rest_scanned,
    a_file_that_cannot_be_read_leaves_its_entity_unknown_and_the_rest_listed,
    another_writers_file_that_cannot_be_read_is_taken_in_once_it_can_be,
    a_writer_whose_own_file_cannot_be_read_does_not_open,
    orphans_are_held_back_while_a_writers_file_cannot_be_read,
    orphans_are_held_back_while_a_pending_record_cannot_be_read,
    a_writer_whose_own_pending_record_cannot_be_read_does_not_open,
    a_file_no_entity_is_bound_to_is_saved_renamed_and_trashed_and_each_undone,
    a_refresh_scans_what_another_writer_did_to_files_no_entity_is_bound_to,
    a_refresh_scans_the_file_of_an_entity_another_writer_made_and_deleted_since,
    a_rename_a_failed_rescan_read_is_scanned_by_the_refresh_after_a_commit,
    an_undo_of_a_save_is_refused_as_changed_since_once_another_writer_renamed_the_file,
    an_undo_of_a_save_is_refused_as_changed_since_once_a_writer_it_did_not_see_adopted_the_file,
    an_undo_of_a_save_acts_on_the_entity_this_writer_since_made_of_the_file,
    an_undo_of_a_save_over_another_writers_entity_found_in_it_is_refused_as_changed_since,
    an_undo_of_a_new_file_this_writer_since_made_an_entitys_leaves_that_entity_lost,
    an_undo_of_a_new_file_a_lost_entity_was_found_in_leaves_that_entity_lost,
    an_undo_of_a_save_over_a_file_a_lost_entity_was_found_in_leaves_that_entity_lost,
    an_undo_of_a_new_file_a_pin_gave_a_lost_entity_leaves_that_entity_lost,
    an_undo_of_a_save_over_a_file_a_pin_gave_a_lost_entity_brings_bytes_back_as_its_file,
    an_undo_of_a_save_is_not_refused_over_another_writers_pin_of_the_entity_found_in_it,
    an_undo_of_a_rename_moves_back_the_file_another_writers_entity_was_found_in,
    an_undo_of_a_rename_is_refused_as_changed_since_once_another_writer_adopted_the_file,
    a_redo_of_a_rename_is_refused_as_changed_since_once_another_writer_trashed_the_file,
    an_undo_of_a_rename_is_refused_as_changed_once_the_file_moved_outside,
    an_undo_of_a_tree_move_is_refused_as_changed_since_once_another_writer_moved_the_tree,
    a_redo_of_a_tree_move_is_refused_as_changed_since_once_another_writer_moved_the_tree,
    an_undo_of_a_tree_move_is_refused_as_changed_since_once_another_writer_removed_the_tree,
    a_redo_of_a_tree_move_is_refused_as_changed_since_once_another_writer_removed_the_tree,
    an_undo_of_a_tree_move_is_refused_as_changed_once_the_tree_was_removed_outside,
    an_undo_of_a_new_folder_is_refused_as_changed_since_once_another_writer_put_a_file_in_it,
    an_undo_of_a_folder_removal_is_refused_as_changed_since_once_another_writer_made_it_again,
    an_undo_of_a_new_folder_is_refused_as_not_empty_once_a_file_was_put_in_it_outside,
    an_undo_of_a_move_is_refused_as_changed_since_once_another_writer_deleted_the_entity,
    an_undo_of_a_move_made_after_another_writers_delete_it_saw_is_not_refused_by_it,
    an_undo_of_a_tag_a_delete_saw_is_refused_though_the_deleters_clock_lags,
    an_undo_beside_a_concurrent_delete_goes_ahead_whichever_clock_leads,
    an_undo_of_a_save_where_the_file_was_missing_leaves_it_missing_again,
    a_redo_of_a_save_of_a_missing_file_under_another_name_puts_it_back_there,
    a_tag_on_an_entity_whose_making_entry_is_away_is_undone,
    writes_whose_entries_go_away_and_come_back_are_no_writers_change,
    the_row_of_an_entity_whose_entries_go_away_and_come_back_names_no_writer,
    a_write_whose_entry_goes_away_beside_a_real_change_names_no_writer_on_its_return,
    an_undo_pins_no_move_the_intent_it_reverses_made_plain,
    an_undo_of_a_delete_is_refused_as_changed_since_once_another_writer_made_a_file_at_its_path,
    an_effect_by_path_on_an_entitys_file_is_refused,
    a_tree_moved_with_files_no_entity_is_bound_to_is_moved_back_by_undo,
    a_folder_holding_no_file_is_renamed_and_named_back_by_undo,
    empty_folders_dragged_into_a_new_folder_and_out_of_a_removed_one_undo_and_redo,
    a_tree_move_is_undone_by_one_rename_where_directories_rename,
    a_tree_move_is_not_undone_once_an_entitys_file_it_moved_changed,
    a_tree_move_from_a_folder_moved_since_the_last_scan_is_refused_and_writing_goes_on,
    a_file_no_entity_is_bound_to_goes_back_with_its_tree_as_it_is,
    files_rescanned_together_list_their_directory_once,
    adopting_a_file_reads_no_other_file_of_its_length,
    two_windows_session_after_session_keep_a_pool_of_two,
    asking_who_else_writes_keeps_the_idle_writers_taken_up,
    an_idle_writer_whose_records_its_log_closed_is_let_go,
    an_effect_a_live_window_left_open_is_reported_live,
    an_effect_a_window_left_is_the_installs_once_the_window_closes,
    a_change_of_case_another_install_did_not_start_is_finished_by_another,
    a_change_of_case_another_install_made_is_rolled_back_by_another,
    settling_another_installs_save_reads_no_staged_file_no_op_logs,
    a_folder_another_left_in_its_trash_is_finished_by_another,
    a_folder_another_left_in_its_trash_is_rolled_back_by_another,
    finishing_a_folder_case_change_where_moves_copy_is_refused_and_changes_nothing,
    a_refresh_reports_another_installs_unfinished_change_as_it_stands,
    a_settlement_after_a_refused_undo_is_no_undo_of_it,
    settling_another_writers_change_is_no_step_of_undo,
    a_settlement_resumed_in_a_later_session_is_no_step_of_undo,
    a_scan_of_every_file_reports_the_directories_it_listed,
    a_directory_made_and_filled_in_one_change_is_undone_in_one,
    directories_made_and_removed_around_tree_moves_undo_and_redo,
    removing_an_empty_directory_is_undone_and_one_with_files_is_refused,
    an_intent_that_stops_partway_logs_nothing_until_the_next_write_finishes_it,
    a_view_lists_no_file_before_a_scan_of_every_file_though_identities_are_kept,
    losing_the_local_root_at_any_step_loses_nothing_of_the_library,
    a_crash_at_any_step_is_settled_whole_after_emptying_the_trash,
    an_empty_record_of_this_writer_is_removed_by_its_next_write,
    an_untrusted_folder_is_read_within_bounds_and_never_acted_on,
    the_trash_keeps_displaced_bytes_until_emptied,
    the_items_an_app_names_leave_the_trash,
    compaction_keeps_the_view_and_this_sessions_undo,
    emptied_trash_items_leave_the_next_snapshot,
    a_record_no_log_explains_is_reported_on_a_warm_open,
    others_on_this_machine_are_told_apart_from_others_elsewhere,
    a_refused_intent_changes_nothing,
    facts_a_restore_removed_go_at_once,
    a_partial_scan_after_a_restore_reads_files_of_the_lengths_the_restore_left,
    what_is_shown_is_what_its_logs_fold_to,
    what_is_bound_is_what_binding_afresh_binds,
    one_intent_creates_entities_that_name_each_other,
    effects_that_stop_partway_fail_the_commit_and_keep_both_versions,
    an_undo_that_stops_partway_fails_and_the_next_write_finishes_it,
    an_own_effect_that_cannot_be_finished_leaves_the_library_read_only_until_reopened,
    an_own_effect_whose_source_is_gone_is_left_to_settle_with_consent,
    a_resumed_save_leaves_a_newer_save_made_meanwhile,
    an_orphan_save_is_not_finished_over_a_newer_save,
    an_orphan_save_is_not_finished_onto_a_file_put_there_since,
    an_own_save_whose_staged_bytes_are_gone_rolls_back_to_the_old_bytes,
    an_own_save_over_a_file_deleted_since_is_finished,
    staged_files_stay_while_an_own_record_does_not_read,
    an_own_record_with_a_member_this_build_does_not_know_is_settled,
    a_newer_layout_beside_this_one_opens_the_library_read_only,
    a_settlement_that_stops_partway_leaves_the_record_open_until_settled_again,
    a_folder_whose_renames_may_replace_is_written,
    a_view_is_read_on_other_threads,
    a_commit_writes_to_the_local_root_only_what_it_adds,
    a_writers_directory_stays_bounded_across_sessions,
    a_crash_in_compaction_tears_a_snapshot_only_where_the_folder_cannot_rename,
    compaction_deletes_only_a_snapshot_cut_short_where_the_folder_cannot_rename,
    own_effects_keep_each_rows_file_id_and_name_it,
    a_commit_names_what_each_file_it_wrote_holds,
    a_row_an_own_effect_moved_continues_when_logging_it_fails,
    a_row_an_own_effect_moved_continues_when_the_effects_stop_part_way,
    own_effects_name_the_rows_of_files_the_folder_lists_respelled,
    an_own_effects_file_the_scan_misses_keeps_the_row_its_commit_named,
    a_tree_move_carries_only_the_rows_of_files_it_moved,
    a_tagged_row_follows_its_file_and_an_untagged_row_ends_where_its_file_left,
    the_binding_comes_before_the_same_path,
    a_row_under_a_folder_the_scan_cannot_list_is_unknown_and_never_moves_to_a_copy,
    an_entity_whose_file_is_nowhere_keeps_its_row_as_missing,
    a_row_follows_its_entity_into_the_entity_it_joined,
    an_untagged_copy_that_becomes_an_entitys_file_merges_into_the_entitys_row,
    a_move_found_by_identity_is_pinned_only_for_a_file_fact_this_install_wrote,
    a_move_another_install_logged_before_its_file_arrived_is_not_pinned,
    a_pin_and_another_installs_move_at_another_time_are_one_fact_a_pin_replaces,
    another_writers_open_record_hides_no_row,
    the_listing_holds_directories_and_partial_looks_update_them,
    changes_name_who_changed_an_entitys_row_and_no_one_for_an_untagged_row,
    a_row_another_writer_deleted_names_that_writer_though_this_one_made_it,
    a_row_whose_entity_another_writer_deleted_and_whose_file_it_moved_is_not_said_trashed,
    a_row_taken_for_a_file_another_writer_trashed_elsewhere_is_not_said_trashed,
    changes_since_an_earlier_view_reach_across_a_look_that_failed,
    a_full_walk_lists_again_the_folder_of_a_file_it_missed_before_ending_its_row,
    a_full_walk_listing_a_folder_again_keeps_what_it_listed_first,
);

/// One machine of a shared folder.
fn machine(folder: &MemDisk) -> Machine {
    Machine {
        folder: folder.clone(),
        local: MemDisk::new(),
    }
}

/// Puts `bytes` at `path` in the folder, durably, as another program would.
fn put(folder: &MemDisk, text: &str, bytes: &[u8]) {
    let file = path(text);
    let parent = file.parent().unwrap();
    let ok = |io| {
        folder.perform(io).unwrap();
    };
    ok(Io::MakeDir {
        root: Root::Folder,
        path: parent.clone(),
    });
    let _ = folder.perform(Io::Remove {
        root: Root::Folder,
        path: file.clone(),
    });
    ok(Io::Create {
        root: Root::Folder,
        path: file.clone(),
        bytes: bytes.to_vec(),
    });
    ok(Io::Sync {
        root: Root::Folder,
        path: file,
    });
    let mut dir = Some(parent);
    while let Some(synced) = dir {
        ok(Io::Sync {
            root: Root::Folder,
            path: synced.clone(),
        });
        dir = synced.parent();
    }
}

/// Moves a file in the folder as another program would, which renames even where
/// toshokan cannot.
fn move_outside(folder: &MemDisk, from: &str, to: &str) {
    if folder.capabilities(Root::Folder).rename_file {
        let parent = path(to).parent().unwrap();
        folder
            .perform(Io::MakeDir {
                root: Root::Folder,
                path: parent,
            })
            .unwrap();
        folder
            .perform(Io::Rename {
                root: Root::Folder,
                from: path(from),
                to: path(to),
            })
            .unwrap();
        return;
    }
    put(folder, to, &read(folder, from).unwrap());
    folder
        .perform(Io::Remove {
            root: Root::Folder,
            path: path(from),
        })
        .unwrap();
}

fn read(folder: &MemDisk, text: &str) -> Option<Vec<u8>> {
    match folder.perform(Io::Read {
        root: Root::Folder,
        path: path(text),
        range: Range {
            offset: 0,
            len: u64::MAX,
        },
    }) {
        Ok(Reply::Bytes(bytes)) => Some(bytes),
        _ => None,
    }
}

/// Every library file outside toshokan's root.
fn library_files(folder: &MemDisk) -> BTreeMap<RelPath, Vec<u8>> {
    folder
        .files(Root::Folder)
        .into_iter()
        .filter(|(path, _)| !layout().owns(path))
        .collect()
}

/// What a view says of each entity: its origin, tags and file.
type Facts = BTreeMap<EntityId, (Option<String>, Vec<String>, Option<(RelPath, FileState)>)>;

fn facts(view: &View) -> Facts {
    view.entities()
        .into_iter()
        .map(|entity| {
            let origin = entity.get(ORIGIN).shown().cloned();
            let tags = entity.members(TAGS).values;
            let file = entity.file().map(|file| (file.path, file.state));
            (entity.id(), (origin, tags, file))
        })
        .collect()
}

fn tags(view: &View, entity: EntityId) -> Vec<String> {
    view.entity(entity).unwrap().members(TAGS).values
}

fn tag(text: &str) -> String {
    text.to_owned()
}

/// Creates one entity with a file at `at`.
fn create<F: Facade>(library: &mut F, at: &str, bytes: &[u8]) -> EntityId {
    let committed = library
        .commit("Import", |intent| {
            let (intent, _) = intent.create(|e| {
                e.save(&path(at), bytes.to_vec(), Expect::Absent)
                    .add(TAGS, tag("new"));
            });
            intent
        })
        .unwrap();
    committed.created[0]
}

fn two_writers_tag_one_library_and_converge<F: Facade>() {
    let folder = disk();
    let clock = TestClock::at(1_000);
    let (mut a, _) = F::open(Probe::new(&machine(&folder)), env("a", 1, &clock)).unwrap();
    let song = create(&mut a, "song.npno", b"song");
    let (mut b, opened) = F::open(Probe::new(&machine(&folder)), env("b", 2, &clock)).unwrap();
    assert_eq!(opened.start, Start::New);
    assert_eq!(tags(&b.view(), song), ["new"]);

    clock.advance(10);
    b.commit("Tag", |i| i.add(song, TAGS, tag("live"))).unwrap();
    a.commit("Tag", |i| i.add(song, TAGS, tag("piano")))
        .unwrap();
    let from_b = a.refresh().unwrap().changes;
    b.refresh().unwrap();

    assert_eq!(tags(&a.view(), song), ["live", "new", "piano"]);
    assert_eq!(facts(&a.view()), facts(&b.view()));
    let b_id = b
        .view()
        .writers()
        .iter()
        .find(|w| w.label == "b")
        .unwrap()
        .writer;
    assert!(
        from_b.contains(&Change {
            entity: song,
            what: What::Field("tags".into()),
            by: By::Writer {
                writer: b_id,
                label: "b".into(),
            },
        }),
        "{from_b:?}"
    );
    let (mut fresh, _) = F::open(Probe::new(&machine(&folder)), env("c", 3, &clock)).unwrap();
    fresh.rescan().unwrap();
    assert_eq!(
        facts(&fresh.view()),
        facts(&a.view()),
        "a new reader agrees"
    );
}

fn what_refreshes_fold_in_is_what_a_fresh_open_shows<F: Facade>() {
    let folder = disk();
    let clock = TestClock::at(1_000);
    let (mut a, _) = F::open(Probe::new(&machine(&folder)), env("a", 1, &clock)).unwrap();
    let (mut b, _) = F::open(Probe::new(&machine(&folder)), env("b", 2, &clock)).unwrap();
    let mut songs = vec![create(&mut a, "song.npno", b"song")];
    let mut random = SeededRandom::new(9);
    for step in 0..60 {
        clock.advance(1);
        let pick = random.next_u128();
        let song = songs[pick as usize / 7 % songs.len()];
        let (writer, other) = match pick % 2 {
            0 => (&mut a, &mut b),
            _ => (&mut b, &mut a),
        };
        writer.refresh().unwrap();
        let value = format!("t{}", step % 3);
        let _ = match pick / 2 % 6 {
            0 => writer.commit("Tag", |i| i.add(song, TAGS, value)).map(drop),
            1 => writer
                .commit("Untag", |i| i.remove(song, TAGS, &value))
                .map(drop),
            2 => writer
                .commit("Origin", |i| i.set(song, ORIGIN, value))
                .map(drop),
            3 => writer.commit("Delete", |i| i.delete(song)).map(drop),
            4 => writer.compact().map(drop),
            _ => {
                let at = format!("song-{step}.npno");
                songs.push(create(writer, &at, at.as_bytes()));
                Ok(())
            }
        };
        if (pick / 12).is_multiple_of(3) {
            continue;
        }
        other.refresh().unwrap();
        let (mut fresh, _) = F::open(Probe::new(&machine(&folder)), env("c", 3, &clock)).unwrap();
        fresh.rescan().unwrap();
        assert!(
            other.view().folded() == fresh.view().folded(),
            "step {step}: {:?}",
            facts(&other.view())
        );
        assert_eq!(facts(&other.view()), facts(&fresh.view()), "step {step}");
    }
}

fn a_refresh_that_finds_nothing_new_writes_nothing<F: Facade>() {
    let folder = disk();
    let clock = TestClock::at(1_000);
    let probe = Probe::new(&machine(&folder));
    let (mut a, _) = F::open(probe.clone(), env("a", 1, &clock)).unwrap();
    let song = create(&mut a, "song.npno", b"song");
    let (mut b, _) = F::open(Probe::new(&machine(&folder)), env("b", 2, &clock)).unwrap();
    b.commit("Tag", |i| i.add(song, TAGS, tag("b"))).unwrap();
    a.refresh().unwrap();
    let (writes, view) = ((probe.folder_writes(), probe.local_writes()), a.view());
    let refreshed = a.refresh().unwrap();
    assert_eq!(refreshed.changes, []);
    assert_eq!((probe.folder_writes(), probe.local_writes()), writes);
    assert_eq!(facts(&a.view()), facts(&view));

    a.commit("Tag", |i| i.add(song, TAGS, tag("a"))).unwrap();
    let writes = (probe.folder_writes(), probe.local_writes());
    probe.seen.borrow_mut().log_read = 0;
    let refreshed = a.refresh().unwrap();
    assert_eq!(refreshed.changes, [], "after its own commit");
    assert_eq!((probe.folder_writes(), probe.local_writes()), writes);
    let read = probe.seen.borrow().log_read;
    assert!(
        read <= toshokan::line::ENDING,
        "read {read} bytes of one file"
    );
}

fn a_refresh_scans_only_the_files_other_writers_moved<F: Facade>() {
    let folder = disk();
    let clock = TestClock::at(1_000);
    let probe = Probe::new(&machine(&folder));
    let (mut a, _) = F::open(probe.clone(), env("a", 1, &clock)).unwrap();
    a.rescan().unwrap();
    let song = create(&mut a, "song.npno", b"song");
    let others = 50;
    for i in 0..others {
        put(&folder, &format!("old/{i}.npno"), i.to_string().as_bytes());
    }
    let before = probe.library_reads();
    a.refresh().unwrap();
    assert_eq!(
        probe.library_reads(),
        before,
        "nothing new, nothing scanned"
    );
    assert_eq!(
        a.view().unbound().cloned().collect::<Vec<_>>(),
        [] as [RelPath; 0]
    );

    let (mut b, _) = F::open(Probe::new(&machine(&folder)), env("b", 2, &clock)).unwrap();
    let tune = create(&mut b, "new/tune.npno", b"tune");
    let moved = Expect::Holds(identity(b"song"));
    b.commit("Move", |i| i.rename(song, &path("shelf/song.npno"), moved))
        .unwrap();
    let before = probe.library_reads();
    let changes = a.refresh().unwrap().changes;
    let reads = probe.library_reads() - before;
    assert!(
        reads < others,
        "{reads} lists, stats and reads of library paths"
    );
    for (entity, at) in [(tune, "new/tune.npno"), (song, "shelf/song.npno")] {
        let file = a.view().entity(entity).unwrap().file().unwrap();
        assert_eq!((file.path, file.state), (path(at), FileState::InSync));
        let filed = Change {
            entity,
            what: What::File,
            by: By::Writer {
                writer: b
                    .view()
                    .writers()
                    .iter()
                    .find(|w| w.label == "b")
                    .unwrap()
                    .writer,
                label: "b".into(),
            },
        };
        assert!(changes.contains(&filed), "{changes:?}");
    }
    assert_eq!(
        a.view().unbound().cloned().collect::<Vec<_>>(),
        [] as [RelPath; 0],
        "outside files wait"
    );
    a.rescan().unwrap();
    assert_eq!(
        a.view().unbound().cloned().collect::<Vec<_>>().len() as u64,
        others
    );
}

fn a_view_is_read_on_other_threads<F: Facade>() {
    let folder = disk();
    let clock = TestClock::at(1_000);
    let (mut a, _) = F::open(Probe::new(&machine(&folder)), env("a", 1, &clock)).unwrap();
    let song = create(&mut a, "song.npno", b"song");
    let view = a.view();
    let sent = view.clone();
    let worker = std::thread::spawn(move || tags(&sent, song));
    std::thread::scope(|scope| {
        let shared = &view;
        let readers = [(); 2].map(|()| scope.spawn(move || tags(shared, song)));
        for reader in readers {
            assert_eq!(reader.join().unwrap(), ["new"]);
        }
    });
    assert_eq!(worker.join().unwrap(), ["new"]);
}

fn a_conflict_is_shown_and_resolved<F: Facade>() {
    let folder = disk();
    let clock = TestClock::at(1_000);
    let (mut a, _) = F::open(Probe::new(&machine(&folder)), env("a", 1, &clock)).unwrap();
    let song = create(&mut a, "song.npno", b"song");
    let (mut b, _) = F::open(Probe::new(&machine(&folder)), env("b", 2, &clock)).unwrap();
    a.commit("Origin", |i| i.set(song, ORIGIN, "from a".into()))
        .unwrap();
    clock.advance(5);
    b.commit("Origin", |i| i.set(song, ORIGIN, "from b".into()))
        .unwrap();
    a.refresh().unwrap();
    b.refresh().unwrap();
    for view in [a.view(), b.view()] {
        let Field::Conflict { shown, all } = view.entity(song).unwrap().get(ORIGIN) else {
            panic!("a conflict");
        };
        assert_eq!(shown, "from b", "the later write is shown");
        assert_eq!(all.len(), 2);
        assert_eq!(
            view.conflicts(),
            [Conflicted::Field {
                entity: song,
                key: "origin".into()
            }]
        );
    }
    a.commit("Origin", |i| i.set(song, ORIGIN, "agreed".into()))
        .unwrap();
    b.refresh().unwrap();
    for view in [a.view(), b.view()] {
        assert_eq!(
            view.entity(song).unwrap().get(ORIGIN),
            Field::Value("agreed".into())
        );
        assert_eq!(view.conflicts(), []);
    }
}

fn a_fork_compaction_finds_retires_the_writer<F: Facade>() {
    let folder = disk();
    let clock = TestClock::at(1_000);
    let here = machine(&folder);
    let (mut a, _) = F::open(Probe::new(&here), env("a", 1, &clock)).unwrap();
    let song = create(&mut a, "song.npno", b"song");
    a.close().unwrap();
    let there = here.cloned();
    let (mut original, opened) = F::open(Probe::new(&here), env("a", 2, &clock)).unwrap();
    let Start::Resumed(writer) = opened.start else {
        panic!("{:?}", opened.start);
    };
    let (mut clone, _) = F::open(Probe::new(&there), env("a", 3, &clock)).unwrap();
    clone
        .commit("Tag", |i| i.add(song, TAGS, tag("there")))
        .unwrap();
    original
        .commit("Tag", |i| i.add(song, TAGS, tag("here")))
        .unwrap();
    let writers = || -> BTreeSet<String> {
        let paths = folder.files(Root::Folder).into_keys();
        let dirs = paths.filter_map(|path| {
            let rest = path.as_str().strip_prefix(".t/writers.2/")?;
            Some(rest.split('/').next()?.to_owned())
        });
        dirs.collect()
    };
    let refused = original.compact();
    assert!(
        matches!(refused, Err(Error::Rekey { writer: w, why: Rekey::Forked }) if w == writer),
        "{refused:?}"
    );
    assert_eq!(original.view().forks().len(), 1, "the fork is shown");
    assert_eq!(original.refresh().unwrap().retired, None, "retired already");
    let before = writers();
    original
        .commit("Tag", |i| i.add(song, TAGS, tag("later")))
        .unwrap();
    let after = writers();
    assert_eq!(after.len(), before.len() + 1, "{before:?} then {after:?}");
}

/// `count` files named by `name`, each adopted by an entity of its own, in
/// commits of 500.
fn adopted<F: Facade>(
    a: &mut F,
    folder: &MemDisk,
    count: usize,
    name: impl Fn(usize) -> String,
) -> Vec<EntityId> {
    for n in 0..count {
        put(folder, &name(n), name(n).as_bytes());
    }
    a.rescan().unwrap();
    let mut entities = Vec::new();
    let all: Vec<usize> = (0..count).collect();
    for some in all.chunks(500) {
        let committed = a
            .commit("Adopt", |mut i| {
                for &n in some {
                    let expect = Expect::Holds(identity(name(n).as_bytes()));
                    (i, _) = i.create(|e| {
                        e.adopt(&path(&name(n)), expect).add(TAGS, tag("kept"));
                    });
                }
                i
            })
            .unwrap();
        entities.extend(committed.created);
    }
    entities
}

fn one_disk(disk: &MemDisk) -> Machine {
    Machine {
        folder: disk.clone(),
        local: disk.clone(),
    }
}

fn a_move_of_thousands_of_files_is_one_entry_and_undoes_as_one_change<F: Facade>() {
    const FILES: usize = 3_000;
    let folder = disk();
    let clock = TestClock::at(1_000);
    let (mut a, _) = F::open(Probe::new(&machine(&folder)), env("a", 1, &clock)).unwrap();
    let name = |n: usize| format!("{n:04}.wav");
    let entities = adopted(&mut a, &folder, FILES, name);
    a.commit("Move", |mut i| {
        for (n, entity) in entities.iter().enumerate() {
            let expect = Expect::Holds(identity(name(n).as_bytes()));
            i = i.rename(*entity, &path(&format!("Set/{}", name(n))), expect);
        }
        i
    })
    .unwrap();
    let moves = labels_logged(&folder)
        .into_iter()
        .filter(|label| label == "Move");
    assert_eq!(moves.count(), 1, "the move is one entry");
    let under = |view: &View, dir: &str| {
        let entities = view.entities();
        let files: Vec<_> = entities.iter().filter_map(|e| e.file()).collect();
        files
            .iter()
            .filter(|file| file.path.as_str().starts_with(dir))
            .count()
    };
    assert_eq!(under(&a.view(), "Set/"), FILES);
    let (fresh, _) = F::open(Probe::new(&machine(&folder)), env("b", 2, &clock)).unwrap();
    assert_eq!(
        under(&fresh.view(), "Set/"),
        FILES,
        "another reader merges it whole"
    );
    a.undo().unwrap();
    assert_eq!(under(&a.view(), "Set/"), 0, "undone whole");
    assert_eq!(
        folder
            .files(Root::Folder)
            .keys()
            .filter(|p| p.as_str().starts_with("Set/"))
            .count(),
        0
    );
}

#[test]
fn an_intent_whose_record_would_outgrow_its_bound_is_refused_and_writes_nothing() {
    for (len, fits) in [(7 << 20, true), (9 << 20, false)] {
        let folder = disk();
        let clock = TestClock::at(1_000);
        let (mut a, _) =
            Blocking::open(Probe::new(&machine(&folder)), env("a", 1, &clock)).unwrap();
        create(&mut a, "song.npno", b"song");
        let before = folder.files(Root::Folder);
        let long = path(&format!("{}.wav", "x".repeat(len)));
        let saved = a.commit("Save", |i| {
            i.create(|e| _ = e.save(&long, b"x".to_vec(), Expect::Absent))
                .0
        });
        match fits {
            true => assert!(saved.is_ok(), "{len}: {saved:?}"),
            false => {
                assert!(
                    matches!(
                        saved,
                        Err(Error::Refused(Refusal::Invalid(Invalid::TooLong)))
                    ),
                    "{len}: {saved:?}"
                );
                assert_eq!(
                    folder.files(Root::Folder),
                    before,
                    "{len}: nothing is written"
                );
            }
        }
    }
}

#[test]
#[cfg_attr(debug_assertions, ignore = "minutes unoptimized; runs in release")]
fn a_compaction_whose_snapshot_would_outgrow_what_readers_hold_is_refused_and_keeps_the_log() {
    const ORIGIN_LEN: usize = 15 << 20;
    let folder = disk();
    let clock = TestClock::at(1_000);
    let (mut a, _) = Blocking::open(Probe::new(&machine(&folder)), env("a", 1, &clock)).unwrap();
    for _ in 0..=MAX_FILE as usize / ORIGIN_LEN {
        a.commit("Origin", |i| {
            i.create(|e| _ = e.set(ORIGIN, "x".repeat(ORIGIN_LEN))).0
        })
        .unwrap();
    }
    let before = folder.files(Root::Folder);
    let compacted = a.compact();
    assert!(
        matches!(compacted, Err(Error::Refused(Refusal::SnapshotTooLong))),
        "{compacted:?}"
    );
    assert!(folder.files(Root::Folder) == before, "nothing is written");
    let fresh = Blocking::open(Probe::new(&machine(&folder)), env("b", 2, &clock))
        .unwrap()
        .0;
    assert!(
        facts(&fresh.view()) == facts(&a.view()),
        "a fresh open reads every origin"
    );
}

/// The line, with its newline, of the intent labeled `label` in `folder`'s segments.
fn line_of(folder: &MemDisk, label: &str) -> String {
    logged(folder, label).expect("the intent is logged") + "\n"
}

/// The first line of a segment that logs an intent labeled `label`.
fn logged(folder: &MemDisk, label: &str) -> Option<String> {
    let segments = folder.files(Root::Folder).into_iter();
    let segments = segments.filter(|(path, _)| path.as_str().ends_with(SEGMENT_EXTENSION));
    let named = format!(r#""label":"{label}""#);
    let lines = segments.flat_map(|(_, bytes)| {
        let text = String::from_utf8(bytes).unwrap();
        text.lines().map(str::to_owned).collect::<Vec<_>>()
    });
    lines.into_iter().find(|line| line.contains(&named))
}

const FORKED: [&str; 3] = ["c", "d", "e"];

/// Creates `song.npno` in `folder` as writer `a`.
fn a_song(folder: &MemDisk, clock: &TestClock) -> EntityId {
    let (mut a, _) = Blocking::open(Probe::new(&machine(folder)), env("a", 1, clock)).unwrap();
    let song = create(&mut a, "song.npno", b"song");
    a.close().unwrap();
    song
}

/// Has each writer of `labels`, none aware of the others, move `song`'s file away
/// and back, then forks each, so the file register holds a live write of each.
fn forked_file_writes(folder: &MemDisk, clock: &TestClock, song: EntityId, labels: &[&str]) {
    let roots: Vec<Machine> = labels.iter().map(|_| machine(folder)).collect();
    let opened = (0..labels.len()).map(|n| {
        let env = env(labels[n], 3 + n as u64, clock);
        Blocking::open(Probe::new(&roots[n]), env).unwrap().0
    });
    for (mut writer, label) in opened.collect::<Vec<_>>().into_iter().zip(labels) {
        let held = Expect::Holds(identity(b"song"));
        let away = path(&format!("{label}.npno"));
        writer
            .commit("Rename", |i| i.rename(song, &away, held))
            .unwrap();
        writer
            .commit("Rename", |i| i.rename(song, &path("song.npno"), held))
            .unwrap();
        writer.close().unwrap();
    }
    for (n, root) in roots.iter().enumerate() {
        let seed = 10 * (n as u64 + 1);
        close_all(fork::<Blocking>(
            root,
            labels[n],
            seed,
            clock,
            |library, side| {
                library
                    .commit("Tag", |i| i.add(song, TAGS, tag(side)))
                    .unwrap();
            },
        ));
    }
}

/// How many fork points `line` names.
fn forks_in(line: &str) -> usize {
    let forks = line.split(r#""forks":"#).nth(1).unwrap_or_default();
    forks
        .split("]]")
        .next()
        .unwrap_or_default()
        .matches("[\"")
        .count()
}

/// A line length well inside every widest entry's slack.
const WELL_UNDER: usize = MAX_LINE - 1024;

/// A value that makes a line, `logged` bytes long with a one-byte value, `len`
/// bytes long.
fn value_for(len: usize, logged: usize) -> String {
    "x".repeat(1 + len - logged)
}

fn assert_too_long<T: std::fmt::Debug>(result: Result<T, Error>, folder: &MemDisk, before: Files) {
    assert!(
        matches!(
            result,
            Err(Error::Refused(Refusal::Invalid(Invalid::TooLong)))
        ),
        "{result:?}"
    );
    assert_eq!(folder.files(Root::Folder), before, "nothing is written");
}

type Files = BTreeMap<RelPath, Vec<u8>>;

// An under-counted widest entry shows first where the line is one byte too
// long, so each of these tests is refused there and logs whole well below it.

#[test]
fn an_intent_naming_forked_writers_one_byte_too_long_is_refused_and_writes_nothing() {
    let setup = || {
        let folder = disk();
        let clock = TestClock::at(1_000);
        let song = a_song(&folder, &clock);
        forked_file_writes(&folder, &clock, song, &FORKED);
        let (mut b, _) =
            Blocking::open(Probe::new(&machine(&folder)), env("b", 2, &clock)).unwrap();
        create(&mut b, "other.npno", b"other");
        (folder, b, song)
    };
    let move_song = |b: &mut Blocking, song: EntityId, origin: String| {
        b.commit("Move", |i| {
            let held = Expect::Holds(identity(b"song"));
            i.rename(song, &path("moved.npno"), held)
                .set(song, ORIGIN, origin)
        })
    };
    let (folder, mut b, song) = setup();
    move_song(&mut b, song, "x".into()).unwrap();
    let logged = line_of(&folder, "Move");
    assert_eq!(
        forks_in(&logged),
        FORKED.len(),
        "the move names each forked writer: {logged}"
    );
    let logged = logged.len();
    let (folder, mut b, song) = setup();
    let before = folder.files(Root::Folder);
    assert_too_long(
        move_song(&mut b, song, value_for(MAX_LINE + 1, logged)),
        &folder,
        before,
    );
    let (folder, mut b, song) = setup();
    move_song(&mut b, song, value_for(WELL_UNDER, logged)).unwrap();
    assert_eq!(line_of(&folder, "Move").len(), WELL_UNDER);
}

#[test]
fn a_save_finished_after_its_place_failed_is_bounded_by_the_line_it_was_planned_in() {
    let name = format!("{}.npno", "s".repeat(200));
    let setup = || {
        let folder = disk();
        let clock = TestClock::at(1_000);
        let probe = Probe::new(&machine(&folder));
        let (mut a, _) = Blocking::open(probe.clone(), env("a", 1, &clock)).unwrap();
        let song = create(&mut a, &name, b"song");
        (folder, probe, a, song)
    };
    let save = |(probe, a, song): (&Probe, &mut Blocking, EntityId), origin: String| {
        let placing = {
            let name = name.clone();
            move |io: &Io| matches!(io, Io::Rename { to, .. } if to.as_str() == name)
        };
        fail(probe, placing);
        let saved = a.commit("Save", |i| {
            let held = Expect::Holds(identity(b"song"));
            i.save(song, &path(&name), b"saved".to_vec(), held)
                .set(song, ORIGIN, origin)
        });
        heal(probe);
        saved
    };
    let (folder, probe, mut a, song) = setup();
    let stopped = save((&probe, &mut a, song), "x".into());
    assert!(matches!(stopped, Err(Error::Io { .. })), "{stopped:?}");
    a.0.settle_interrupted().unwrap();
    let logged = line_of(&folder, "Save").len();
    let (folder, probe, mut a, song) = setup();
    let before = folder.files(Root::Folder);
    let refused = save((&probe, &mut a, song), value_for(MAX_LINE + 1, logged));
    assert_too_long(refused, &folder, before);
    let (folder, probe, mut a, song) = setup();
    let stopped = save((&probe, &mut a, song), value_for(WELL_UNDER, logged));
    assert!(matches!(stopped, Err(Error::Io { .. })), "{stopped:?}");
    a.0.settle_interrupted().unwrap();
    assert_eq!(line_of(&folder, "Save").len(), WELL_UNDER);
}

#[test]
fn an_orphan_finishes_with_its_records_refs_in_the_line_its_writer_bounded() {
    let setup = |origin: String| {
        let folder = disk();
        let clock = TestClock::at(1_000);
        let song = a_song(&folder, &clock);
        let probe = Probe::new(&machine(&folder));
        let (mut w, _) = Blocking::open(probe.clone(), env("w", 2, &clock)).unwrap();
        create(&mut w, "w.npno", b"w");
        forked_file_writes(&folder, &clock, song, &FORKED);
        let moving = |io: &Io| matches!(io, Io::Rename { to, .. } if to.as_str() == "moved.npno");
        let logging = |io: &Io| io.root() == Root::Folder && io.path().as_str().ends_with(".txt");
        fail(&probe, move |io| moving(io) || io.mutates() && logging(io));
        let stopped = w.commit("Move", |i| {
            let held = Expect::Holds(identity(b"song"));
            i.rename(song, &path("moved.npno"), held)
                .set(song, ORIGIN, origin)
        });
        heal(&probe);
        w.close().unwrap();
        let (mut b, _) =
            Blocking::open(Probe::new(&machine(&folder)), env("b", 7, &clock)).unwrap();
        create(&mut b, "other.npno", b"other");
        let orphans = b.refresh().unwrap().orphaned;
        (folder, b, stopped, orphans)
    };
    let (folder, mut b, _, mut orphans) = setup("x".into());
    b.settle(orphans.remove(0), Settlement::Finished).unwrap();
    let logged = line_of(&folder, "Move");
    assert_eq!(
        forks_in(&logged),
        0,
        "finishing replaces what the record named: {logged}"
    );
    let logged = logged.len();
    let (folder, _, stopped, orphans) = setup(value_for(MAX_LINE + 1, logged));
    assert!(
        matches!(
            stopped,
            Err(Error::Refused(Refusal::Invalid(Invalid::TooLong)))
        ),
        "its writer refuses it: {stopped:?}"
    );
    assert_eq!((orphans.len(), pending_files(&folder)), (0, Vec::new()));
    let (folder, mut b, _, mut orphans) = setup(value_for(WELL_UNDER, logged));
    b.settle(orphans.remove(0), Settlement::Finished).unwrap();
    assert_eq!(line_of(&folder, "Move").len(), WELL_UNDER);
}

#[test]
fn a_move_accepted_with_less_room_than_writes_it_did_not_know_of_would_take_settles() {
    const WRITERS: [&str; 8] = ["c", "d", "e", "f", "g", "h", "i", "j"];
    // Each reference is at least `["` 32 hex `",` a digit `,"` 32 hex `"]`.
    const REF: usize = 73;
    // The most room in the line that the refs of those writes would not fit.
    const ROOM: usize = WRITERS.len() * REF - 1;
    let commit = |origin: String, since: &[&str]| {
        let folder = disk();
        let clock = TestClock::at(1_000);
        let song = a_song(&folder, &clock);
        let probe = Probe::new(&machine(&folder));
        let (mut w, _) = Blocking::open(probe.clone(), env("w", 2, &clock)).unwrap();
        create(&mut w, "w.npno", b"w");
        forked_file_writes(&folder, &clock, song, since);
        let moving = |io: &Io| matches!(io, Io::Rename { to, .. } if to.as_str() == "moved.npno");
        let logging = |io: &Io| io.root() == Root::Folder && io.path().as_str().ends_with(".txt");
        fail(&probe, move |io| moving(io) || io.mutates() && logging(io));
        let stopped = w.commit("Move", |i| {
            let held = Expect::Holds(identity(b"song"));
            i.rename(song, &path("moved.npno"), held)
                .set(song, ORIGIN, origin)
        });
        heal(&probe);
        assert!(matches!(stopped, Err(Error::Io { .. })), "{stopped:?}");
        (folder, w, song)
    };
    let settle = |folder: &MemDisk, w: &mut Blocking, song: EntityId| {
        w.refresh().unwrap();
        w.commit("Tag", |i| i.add(song, TAGS, tag("later")))
            .unwrap();
        line_of(folder, "Move").len()
    };
    let (folder, mut w, song) = commit("x".into(), &[]);
    let logged = settle(&folder, &mut w, song);
    let (folder, mut w, song) = commit(value_for(MAX_LINE - ROOM, logged), &WRITERS);
    let settled = settle(&folder, &mut w, song);
    assert!((MAX_LINE - ROOM..=MAX_LINE).contains(&settled), "{settled}");
}

fn an_intent_wider_than_a_line_is_refused_and_writes_nothing<F: Facade>() {
    let folder = disk();
    let clock = TestClock::at(1_000);
    let (mut a, _) = F::open(Probe::new(&machine(&folder)), env("a", 1, &clock)).unwrap();
    create(&mut a, "song.npno", b"song");
    let before = folder.files(Root::Folder);
    let long = path(&format!("{}.wav", "x".repeat(MAX_LINE)));
    let saved = a.commit("Save", |i| {
        i.create(|e| _ = e.save(&long, b"x".to_vec(), Expect::Absent))
            .0
    });
    assert!(
        matches!(
            saved,
            Err(Error::Refused(Refusal::Invalid(Invalid::TooLong)))
        ),
        "{saved:?}"
    );
    assert_eq!(folder.files(Root::Folder), before, "nothing is written");
}

fn thousands_of_entities_deleted_beside_their_files_come_back_with_undo<F: Facade>() {
    const FILES: usize = 1_500;
    let folder = disk();
    let clock = TestClock::at(1_000);
    let (mut a, _) = F::open(Probe::new(&machine(&folder)), env("a", 1, &clock)).unwrap();
    let name = |n: usize| format!("{n:04}.wav");
    let entities = adopted(&mut a, &folder, FILES, name);
    a.commit("Delete", |mut i| {
        for (n, entity) in entities.iter().enumerate() {
            let expect = Expect::Holds(identity(name(n).as_bytes()));
            i = i.trash(*entity, expect).delete(*entity);
        }
        i
    })
    .unwrap();
    let shown = |view: &View| view.entities().len();
    assert_eq!(shown(&a.view()), 0);
    let (fresh, _) = F::open(Probe::new(&machine(&folder)), env("b", 2, &clock)).unwrap();
    assert_eq!(shown(&fresh.view()), 0, "another reader shows none");
    a.undo().unwrap();
    let view = a.view();
    let entities = view.entities();
    let files: Vec<_> = entities.iter().filter_map(|e| e.file()).collect();
    assert_eq!(files.len(), FILES, "each comes back with its file");
}

/// The labels of the intents of every segment of the folder, in no order.
fn labels_logged(folder: &MemDisk) -> Vec<String> {
    let segments = folder.files(Root::Folder).into_iter();
    let segments = segments.filter(|(path, _)| path.as_str().ends_with(SEGMENT_EXTENSION));
    let mut labels = Vec::new();
    for (_, bytes) in segments {
        for line in String::from_utf8(bytes).unwrap().lines() {
            let json = line.split('\t').next().unwrap_or_default();
            let Ok(value) = serde_json::from_str::<serde_json::Value>(json) else {
                continue;
            };
            if let Some(label) = value["label"]
                .as_str()
                .filter(|_| value["kind"] == "intent")
            {
                labels.push(label.to_owned());
            }
        }
    }
    labels
}

fn a_commit_whose_entry_did_not_sync_keeps_its_record<F: Facade>() {
    if !FOLDER.get().fsync {
        return;
    }
    let disk = disk();
    let clock = TestClock::at(1_000);
    let probe = Probe::new(&one_disk(&disk));
    let (mut a, _) = F::open(probe.clone(), env("a", 1, &clock)).unwrap();
    let song = create(&mut a, "song.npno", b"one");
    fail(
        &probe,
        |io| matches!(io, Io::Sync { root: Root::Folder, path } if path.as_str().ends_with(SEGMENT_EXTENSION)),
    );
    let saved = a.commit("Save", |i| {
        i.save(
            song,
            &path("song.npno"),
            b"two".to_vec(),
            Expect::Holds(identity(b"one")),
        )
    });
    assert!(saved.is_err(), "the entry did not sync");
    heal(&probe);
    drop(a);
    let disk = disk.restart();
    let (mut again, opened) = F::open(Probe::new(&one_disk(&disk)), env("a", 2, &clock)).unwrap();
    let labels: Vec<&str> = opened.settled.iter().map(|s| s.label.as_str()).collect();
    assert_eq!(labels, ["Save"], "the record outlived the lost entry");
    again
        .commit("Next", |i| i.create(|e| _ = e.add(TAGS, tag("next"))).0)
        .unwrap();
    let trash = again.trash().unwrap();
    let from: Vec<_> = trash.iter().map(|item| item.from.clone()).collect();
    assert_eq!(
        from,
        [path("song.npno")],
        "an entry names the displaced bytes"
    );
}

fn an_unfinished_effect_of_a_writer_that_retires_stays_this_installs<F: Facade>() {
    let folder = disk();
    let clock = TestClock::at(1_000);
    put(&folder, "a.wav", b"a");
    let here = machine(&folder);
    let probe = Probe::new(&here);
    let (mut original, _) = F::open(probe.clone(), env("a", 1, &clock)).unwrap();
    original.rescan().unwrap();
    create(&mut original, "song.npno", b"song");
    let there = here.cloned();
    let (mut clone, _) = F::open(Probe::new(&there), env("a", 3, &clock)).unwrap();
    original
        .commit("Tag", |i| i.create(|e| _ = e.add(TAGS, tag("here"))).0)
        .unwrap();
    let logging = |io: &Io| io.root() == Root::Folder && io.path().as_str().ends_with(".txt");
    fail(&probe, move |io| io.mutates() && logging(io));
    let stopped = original.commit("Rename", |i| {
        i.rename_file(
            &path("a.wav"),
            &path("b.wav"),
            Expect::Holds(identity(b"a")),
        )
    });
    heal(&probe);
    assert!(stopped.is_err(), "{stopped:?}");
    clone
        .commit("Tag", |i| i.create(|e| _ = e.add(TAGS, tag("x"))).0)
        .unwrap();
    let refreshed = original.refresh().unwrap();
    assert!(refreshed.retired.is_some(), "the fork retires the writer");
    let orphans: Vec<(&str, bool)> = refreshed
        .orphaned
        .iter()
        .map(|o| (o.label.as_str(), o.ours))
        .collect();
    assert_eq!(orphans, [("Rename", true)]);
}

fn an_entry_of_a_copied_instance_does_not_close_another_instances_record<F: Facade>() {
    let folder = disk();
    let clock = TestClock::at(1_000);
    let mut here = machine(&folder);
    let (mut a, _) = F::open(Probe::new(&here), env("a", 1, &clock)).unwrap();
    let song = create(&mut a, "song.npno", b"one");
    a.close().unwrap();
    let there = here.cloned();
    let probe = Probe::new(&here);
    let (mut original, _) = F::open(probe.clone(), env("a", 2, &clock)).unwrap();
    let (mut copy, _) = F::open(Probe::new(&there), env("a", 3, &clock)).unwrap();
    let logging =
        |io: &Io| io.root() == Root::Folder && io.path().as_str().ends_with(SEGMENT_EXTENSION);
    fail(&probe, move |io| io.mutates() && logging(io));
    let saved = original.commit("Save", |i| {
        i.save(
            song,
            &path("song.npno"),
            b"two".to_vec(),
            Expect::Holds(identity(b"one")),
        )
    });
    assert!(
        saved.is_err(),
        "the effect is made and its entry not logged"
    );
    drop(original);
    here.crash();
    copy.commit("Tag", |i| i.add(song, TAGS, tag("there")))
        .unwrap();
    drop(copy);
    let (mut again, opened) = F::open(Probe::new(&here), env("a", 4, &clock)).unwrap();
    let labels: Vec<&str> = opened.settled.iter().map(|s| s.label.as_str()).collect();
    assert_eq!(labels, ["Save"], "no entry names the record");
    again
        .commit("Next", |i| i.add(song, TAGS, tag("next")))
        .unwrap();
    let trash = again.trash().unwrap();
    let from: Vec<_> = trash.iter().map(|item| item.from.clone()).collect();
    assert_eq!(
        from,
        [path("song.npno")],
        "an entry names the displaced bytes"
    );
}

fn a_tree_moved_file_by_file_names_the_directories_a_file_it_did_not_know_keeps<F: Facade>() {
    let folder = MemDisk::with_capabilities(
        Capabilities {
            rename_dir: false,
            ..Capabilities::ALL
        },
        Capabilities::ALL,
    );
    put(&folder, "old/a.wav", b"a");
    put(&folder, "old/sub/b.wav", b"b");
    let clock = TestClock::at(1_000);
    let (mut a, _) = F::open(Probe::new(&machine(&folder)), env("a", 1, &clock)).unwrap();
    a.rescan().unwrap();
    put(&folder, "old/late.wav", b"late");
    let moved = a
        .commit("Rename", |i| i.move_tree(&path("old"), &path("new")))
        .unwrap();
    assert_eq!(
        moved.left,
        [path("old")],
        "old/sub went, old keeps late.wav"
    );
}

fn a_retired_writers_trash_is_still_listed<F: Facade>() {
    let folder = disk();
    let clock = TestClock::at(1_000);
    let here = machine(&folder);
    let (mut original, _) = F::open(Probe::new(&here), env("a", 1, &clock)).unwrap();
    let song = create(&mut original, "song.npno", b"one");
    original
        .commit("Save", |i| {
            i.save(
                song,
                &path("song.npno"),
                b"two".to_vec(),
                Expect::Holds(identity(b"one")),
            )
        })
        .unwrap();
    let there = here.cloned();
    let (mut clone, _) = F::open(Probe::new(&there), env("a", 2, &clock)).unwrap();
    for (library, text) in [(&mut original, "here"), (&mut clone, "there")] {
        library
            .commit("Tag", |i| i.add(song, TAGS, tag(text)))
            .unwrap();
    }
    let retired = original
        .refresh()
        .unwrap()
        .retired
        .expect("the fork retires it");
    let trash = original.trash().unwrap();
    let listed: Vec<_> = trash
        .iter()
        .map(|item| (item.writer, item.from.clone()))
        .collect();
    assert_eq!(listed, [(retired, path("song.npno"))]);
}

fn undo_after_a_dismissal_undoes_the_change_before_it<F: Facade>() {
    let folder = disk();
    let clock = TestClock::at(1_000);
    let here = machine(&folder);
    let (mut a, _) = F::open(Probe::new(&here), env("a", 1, &clock)).unwrap();
    let song = create(&mut a, "song.npno", b"song");
    a.close().unwrap();
    let there = here.cloned();
    let (mut original, _) = F::open(Probe::new(&here), env("a", 2, &clock)).unwrap();
    let (mut clone, _) = F::open(Probe::new(&there), env("a", 3, &clock)).unwrap();
    for (library, text) in [(&mut original, "here"), (&mut clone, "there")] {
        library
            .commit("Tag", |i| i.add(song, TAGS, tag(text)))
            .unwrap();
    }
    original.refresh().unwrap();
    let later = original
        .commit("Tag", |i| i.add(song, TAGS, tag("later")))
        .unwrap();
    assert!(later.undoable, "a tag joins the undo history");
    let runs: Vec<Run> = original
        .view()
        .items()
        .iter()
        .map(|item| item.run)
        .collect();
    assert_eq!(runs.len(), 2, "one item per branch");
    let dismissed = original
        .commit("Dismiss", |i| runs.iter().fold(i, |i, run| i.dismiss(*run)))
        .unwrap();
    assert!(!dismissed.undoable, "a dismissal is final");
    let undone = original.undo().unwrap();
    assert!(undone.undoable, "an undo is redone");
    assert_eq!(
        tags(&original.view(), song),
        ["new"],
        "the tag before is undone"
    );
    assert_eq!(original.view().items(), [], "the dismissal stands");
}

fn a_rename_on_a_branch_kept_apart_is_that_branchs_not_an_orphan<F: Facade>() {
    let folder = disk();
    let clock = TestClock::at(1_000);
    let here = machine(&folder);
    let (mut a, _) = F::open(Probe::new(&here), env("a", 1, &clock)).unwrap();
    let song = create(&mut a, "song.npno", b"song");
    a.close().unwrap();
    let there = here.cloned();
    let (mut original, _) = F::open(Probe::new(&here), env("a", 2, &clock)).unwrap();
    let (mut clone, _) = F::open(Probe::new(&there), env("a", 3, &clock)).unwrap();
    original
        .commit("Tag", |i| i.add(song, TAGS, tag("here")))
        .unwrap();
    clone
        .commit("Rename", |i| {
            i.rename(song, &path("moved.npno"), Expect::Holds(identity(b"song")))
        })
        .unwrap();
    drop(clone);
    let refreshed = original.refresh().unwrap();
    assert!(refreshed.retired.is_some(), "the fork retires the writer");
    assert_eq!(refreshed.orphaned, [], "the branch's own intent closed it");
    let view = original.view();
    let moved = view.items().iter().find(|item| {
        item.changes
            .iter()
            .any(|change| change.entity == song && change.what == What::File)
    });
    let Some(moved) = moved else {
        panic!("the rename is the branch's item: {:?}", view.items());
    };
    let run = moved.run;
    original.commit("Dismiss", |i| i.dismiss(run)).unwrap();
    original.close().unwrap();
    let (_, reopened) = F::open(Probe::new(&here), env("a", 4, &clock)).unwrap();
    assert_eq!(
        reopened.orphaned,
        [],
        "the dismissed branch still closed it"
    );
}

fn a_settlement_on_a_branch_kept_apart_is_not_offered_again<F: Facade>() {
    let folder = disk();
    let clock = TestClock::at(1_000);
    put(&folder, "a.wav", b"a");
    let probe = Probe::new(&machine(&folder));
    let (mut theirs, _) = F::open(probe.clone(), env("a", 1, &clock)).unwrap();
    theirs.rescan().unwrap();
    theirs
        .commit("Tag", |i| i.create(|e| _ = e.add(TAGS, tag("there"))).0)
        .unwrap();
    let first_step = |io: &Io| matches!(io, Io::Rename { from, .. } if from.as_str() == "a.wav");
    let logging = |io: &Io| io.root() == Root::Folder && io.path().as_str().ends_with(".txt");
    fail(&probe, move |io| {
        io.mutates() && (first_step(io) || logging(io))
    });
    let stopped = theirs.commit("Rename", |i| {
        i.rename_file(
            &path("a.wav"),
            &path("b.wav"),
            Expect::Holds(identity(b"a")),
        )
    });
    heal(&probe);
    assert!(stopped.is_err(), "{stopped:?}");
    drop(theirs);

    let here = machine(&folder);
    let (mut b, _) = F::open(Probe::new(&here), env("b", 2, &clock)).unwrap();
    b.commit("Tag", |i| i.create(|e| _ = e.add(TAGS, tag("b"))).0)
        .unwrap();
    b.close().unwrap();
    let there = here.cloned();
    let (mut original, _) = F::open(Probe::new(&here), env("b", 3, &clock)).unwrap();
    let (mut clone, opened) = F::open(Probe::new(&there), env("b", 4, &clock)).unwrap();
    let [orphan] = opened.orphaned.as_slice() else {
        panic!("one orphan: {:?}", opened.orphaned);
    };
    clone.settle(orphan.clone(), Settlement::Finished).unwrap();
    drop(clone);
    original
        .commit("Tag", |i| i.create(|e| _ = e.add(TAGS, tag("here"))).0)
        .unwrap();
    let refreshed = original.refresh().unwrap();
    assert!(refreshed.retired.is_some(), "the fork retires the writer");
    assert_eq!(refreshed.orphaned, [], "the branch's settle closed it");
}

fn a_clone_forks_into_items_taken_or_dismissed<F: Facade>() {
    let folder = disk();
    let clock = TestClock::at(1_000);
    let here = machine(&folder);
    let (mut a, _) = F::open(Probe::new(&here), env("a", 1, &clock)).unwrap();
    let song = create(&mut a, "song.npno", b"song");
    a.close().unwrap();
    let there = here.cloned();

    let (mut original, opened) = F::open(Probe::new(&here), env("a", 2, &clock)).unwrap();
    let Start::Resumed(writer) = opened.start else {
        panic!("{:?}", opened.start);
    };
    let (mut clone, opened) = F::open(Probe::new(&there), env("a", 3, &clock)).unwrap();
    assert_eq!(opened.start, Start::Resumed(writer), "a copied local root");
    original
        .commit("Tag", |i| i.add(song, TAGS, tag("here")))
        .unwrap();
    clone
        .commit("Tag", |i| i.add(song, TAGS, tag("there")))
        .unwrap();
    for library in [&mut original, &mut clone] {
        let refreshed = library.refresh().unwrap();
        assert_eq!(refreshed.retired, Some(writer), "each instance retires it");
    }
    let item = |view: &View, text: &str| item_making(view, &added(song, text));
    for view in [original.view(), clone.view()] {
        assert_eq!(tags(&view, song), ["new"], "neither branch is merged");
        assert_eq!(view.forks().len(), 1);
        assert_eq!(view.forks()[0].writer, writer);
        assert_eq!(view.items().len(), 2, "one item per branch");
        for text in ["here", "there"] {
            let item = item(&view, text);
            assert_eq!((item.kind, item.run.writer), (ItemKind::Run, writer));
        }
    }
    let view = original.view();
    let (kept, dropped) = (item(&view, "here"), item(&view, "there"));
    original
        .commit("Keep here", |i| {
            i.take(kept.run, added(song, "here")).dismiss(dropped.run)
        })
        .unwrap();
    assert_eq!(clone.refresh().unwrap().retired, None, "retired already");
    for view in [original.view(), clone.view()] {
        assert_eq!(tags(&view, song), ["here", "new"]);
        let [left] = view.items() else {
            panic!("{:?}", view.items());
        };
        assert_eq!(left.run, kept.run, "the taken run is shown until dismissed");
        assert_eq!(left.changes, [], "with nothing left to review");
    }
    original.commit("Close", |i| i.dismiss(kept.run)).unwrap();
    clone.refresh().unwrap();
    for view in [original.view(), clone.view()] {
        assert_eq!(view.items(), [], "dismissed");
    }
    clone
        .commit("Tag", |i| i.add(song, TAGS, tag("after")))
        .unwrap();
    original.refresh().unwrap();
    assert_eq!(tags(&original.view(), song), ["after", "here", "new"]);
    assert_eq!(original.view().forks().len(), 1, "one fork, reported once");
    let writers: BTreeSet<WriterId> = original.view().writers().iter().map(|w| w.writer).collect();
    assert_eq!(
        writers.len(),
        3,
        "the forked writer, and each instance continued as a new writer"
    );
    original.close().unwrap();
    let (_, opened) = F::open(Probe::new(&here), env("a", 4, &clock)).unwrap();
    assert_ne!(
        opened.start,
        Start::Resumed(writer),
        "the forked writer is never written again"
    );
}

/// Forks the writer whose local root `here` holds: it resumes on `here` and on a
/// copy of that root, and each instance commits what `branch` makes of its
/// library and the name of its side, `"here"` or `"there"`. Both stay open.
fn fork<F: Facade>(
    here: &Machine,
    label: &str,
    seed: u64,
    clock: &TestClock,
    mut branch: impl FnMut(&mut F, &str),
) -> [F; 2] {
    let there = here.cloned();
    let (mut original, _) = F::open(Probe::new(here), env(label, seed, clock)).unwrap();
    let (mut copy, _) = F::open(Probe::new(&there), env(label, seed + 1, clock)).unwrap();
    branch(&mut original, "here");
    branch(&mut copy, "there");
    [original, copy]
}

/// Closes each of `libraries`, which compacts what it wrote.
fn close_all<F: Facade>(libraries: impl IntoIterator<Item = F>) {
    for library in libraries {
        library.close().unwrap();
    }
}

/// The item that would make `change`.
fn item_making(view: &View, change: &FactChange) -> Item {
    let mut items = view.items().iter();
    items
        .find(|item| {
            item.changes
                .iter()
                .any(|c| c.change.as_ref() == Some(change))
        })
        .cloned()
        .unwrap_or_else(|| panic!("{change:?} in {:?}", view.items()))
}

fn added(entity: EntityId, text: &str) -> FactChange {
    FactChange::Add {
        entity,
        key: "tags".into(),
        value: toshokan::Raw::of(&text).unwrap(),
    }
}

/// The ops of the intent `committed` logged.
fn logged_ops(folder: &MemDisk, view: &View, committed: &Committed) -> Vec<Op> {
    let own = view.writers().iter().find(|w| w.here == Presence::This);
    let entries = segment_entries(folder, own.unwrap().writer);
    let entry = entries
        .iter()
        .find(|entry| entry.hash() == committed.intent);
    match entry.map(Entry::kind) {
        Some(EntryKind::Intent(logged)) => logged.ops.clone(),
        kind => panic!("{kind:?}"),
    }
}

fn a_change_made_as_ones_own_names_no_run<F: Facade>() {
    let folder = disk();
    let clock = TestClock::at(1_000);
    let (mut a, _) = F::open(Probe::new(&machine(&folder)), env("a", 1, &clock)).unwrap();
    let song = create(&mut a, "song.npno", b"song");
    let committed = a.commit("Tag", |i| i.change(added(song, "own"))).unwrap();
    assert_eq!(tags(&a.view(), song), ["new", "own"]);
    let ops = logged_ops(&folder, &a.view(), &committed);
    let takes: Vec<_> = ops
        .iter()
        .map(|op| (op.entity(), op.take().copied()))
        .collect();
    assert_eq!(takes, [(Some(song), None)]);
}

fn one_intent_takes_from_runs_of_two_forked_writers<F: Facade>() {
    let folder = disk();
    let clock = TestClock::at(1_000);
    let (a_root, b_root) = (machine(&folder), machine(&folder));
    let (mut a, _) = F::open(Probe::new(&a_root), env("a", 1, &clock)).unwrap();
    let song = create(&mut a, "song.npno", b"song");
    a.close().unwrap();
    let (mut b, _) = F::open(Probe::new(&b_root), env("b", 2, &clock)).unwrap();
    b.commit("Tag", |i| i.add(song, TAGS, tag("b"))).unwrap();
    b.close().unwrap();
    for (root, label, seed) in [(&a_root, "a", 10), (&b_root, "b", 20)] {
        close_all(fork::<F>(root, label, seed, &clock, |library, side| {
            let text = format!("{label} {side}");
            library.commit("Tag", |i| i.add(song, TAGS, text)).unwrap();
        }));
    }

    let (mut c, _) = F::open(Probe::new(&machine(&folder)), env("c", 3, &clock)).unwrap();
    let view = c.view();
    assert_eq!(view.forks().len(), 2, "{:?}", view.forks());
    let (from_a, from_b) = (added(song, "a there"), added(song, "b here"));
    let (run_a, run_b) = (
        item_making(&view, &from_a).run,
        item_making(&view, &from_b).run,
    );
    assert_ne!(run_a.writer, run_b.writer);
    let committed = c
        .commit("Keep", |i| {
            i.take(run_a, from_a.clone())
                .take(run_b, from_b.clone())
                .set(song, ORIGIN, "kept".into())
        })
        .unwrap();
    assert_eq!(tags(&c.view(), song), ["a there", "b", "b here", "new"]);
    let takes: Vec<_> = logged_ops(&folder, &c.view(), &committed)
        .iter()
        .map(|op| (op.entity(), op.take().copied()))
        .collect();
    assert_eq!(
        takes,
        [
            (Some(song), Some(run_a)),
            (Some(song), Some(run_b)),
            (Some(song), None)
        ],
        "each op names the run it takes from, if any"
    );
}

fn a_file_change_is_taken_only_while_the_folder_holds_that_file<F: Facade>() {
    let folder = disk();
    let clock = TestClock::at(1_000);
    let root = machine(&folder);
    let (mut a, _) = F::open(Probe::new(&root), env("a", 1, &clock)).unwrap();
    let song = create(&mut a, "song.npno", b"song");
    a.close().unwrap();
    close_all(fork::<F>(&root, "a", 10, &clock, |library, side| {
        let committed = match side {
            "here" => library.commit("Tag", |i| i.add(song, TAGS, tag("here"))),
            _ => library.commit("Rename", |i| {
                let held = Expect::Holds(identity(b"song"));
                i.rename(song, &path("moved.npno"), held)
            }),
        };
        committed.unwrap();
    }));

    let (mut b, _) = F::open(Probe::new(&machine(&folder)), env("b", 2, &clock)).unwrap();
    let view = b.view();
    let (run, change) = view
        .items()
        .iter()
        .find_map(|item| {
            let file = item.changes.iter().find(|c| c.what == What::File)?;
            Some((item.run, file.change.clone().unwrap()))
        })
        .unwrap_or_else(|| panic!("{:?}", view.items()));
    let FactChange::File { file: named, .. } = &change else {
        panic!("{change:?}");
    };
    assert_eq!(named.path, path("moved.npno"));
    let take = |b: &mut F| b.commit("Keep", |i| i.take(run, change.clone()));
    let refused = |taken: Result<Committed, Error>| match taken {
        Err(Error::Refused(Refusal::Invalid(Invalid::Unbindable { entity, path, why }))) => {
            assert_eq!((entity, path.as_str()), (song, "moved.npno"));
            why
        }
        taken => panic!("{taken:?}"),
    };
    assert_eq!(refused(take(&mut b)), Unbindable::Unscanned);
    folder
        .perform(Io::Remove {
            root: Root::Folder,
            path: path("moved.npno"),
        })
        .unwrap();
    b.rescan().unwrap();
    assert_eq!(refused(take(&mut b)), Unbindable::Absent);
    put(&folder, "moved.npno", b"other");
    b.rescan().unwrap();
    assert_eq!(refused(take(&mut b)), Unbindable::Changed);

    put(&folder, "moved.npno", b"song");
    b.rescan().unwrap();
    let committed = take(&mut b).unwrap();
    assert_eq!(
        b.view().entity(song).unwrap().file(),
        Some(FileRef {
            path: path("moved.npno"),
            state: FileState::InSync
        })
    );
    let ops = logged_ops(&folder, &b.view(), &committed);
    let [Op::Pin {
        entity,
        file,
        take: Some(taken),
        ..
    }] = &ops[..]
    else {
        panic!("{ops:?}");
    };
    assert_eq!((*entity, file, *taken), (song, named, run));
    let view = b.view();
    let mut changes = view.items().iter().flat_map(|item| &item.changes);
    assert!(changes.all(|c| c.what != What::File), "{:?}", view.items());
}

fn a_run_counts_and_spans_its_entries_whether_or_not_its_branch_compacted<F: Facade>() {
    for compacted in [false, true] {
        let folder = disk();
        let clock = TestClock::at(1_000);
        let root = machine(&folder);
        let (mut a, _) = F::open(Probe::new(&root), env("a", 1, &clock)).unwrap();
        let song = create(&mut a, "song.npno", b"song");
        a.close().unwrap();
        let branches = fork::<F>(&root, "a", 10, &clock, |library, side| {
            let tags = if side == "here" { 3 } else { 1 };
            for k in 0..tags {
                clock.advance(10);
                let text = format!("{side} {k}");
                library.commit("Tag", |i| i.add(song, TAGS, text)).unwrap();
            }
            if side == "here" && compacted {
                library.compact().unwrap();
            }
        });
        let readings: Vec<Hlc> = branches[0].history().iter().map(|h| h.at).collect();
        let (reader, _) = F::open(Probe::new(&machine(&folder)), env("b", 2, &clock)).unwrap();
        let item = item_making(&reader.view(), &added(song, "here 0"));
        assert_eq!(
            item.run.tip.is_some(),
            compacted,
            "a snapshot holds the run"
        );
        assert_eq!(
            (item.entries, item.span),
            (3, (readings[0], readings[2])),
            "compacted: {compacted}"
        );
    }
}

fn a_run_that_would_change_nothing_is_shown_until_dismissed<F: Facade>() {
    let folder = disk();
    let clock = TestClock::at(1_000);
    let root = machine(&folder);
    let (mut a, _) = F::open(Probe::new(&root), env("a", 1, &clock)).unwrap();
    let song = create(&mut a, "song.npno", b"song");
    a.close().unwrap();
    close_all(fork::<F>(&root, "a", 10, &clock, |library, side| {
        let text = if side == "here" { "here" } else { "new" };
        library
            .commit("Tag", |i| i.add(song, TAGS, tag(text)))
            .unwrap();
    }));

    let (mut b, _) = F::open(Probe::new(&machine(&folder)), env("b", 2, &clock)).unwrap();
    let view = b.view();
    assert_eq!(tags(&view, song), ["new"]);
    let empty: Vec<&Item> = view
        .items()
        .iter()
        .filter(|i| i.changes.is_empty())
        .collect();
    let [empty] = empty[..] else {
        panic!("{:?}", view.items());
    };
    assert_eq!((empty.kind, empty.entries), (ItemKind::Run, 1));
    let run = empty.run;
    b.commit("Close", |i| i.dismiss(run)).unwrap();
    let view = b.view();
    let [left] = view.items() else {
        panic!("{:?}", view.items());
    };
    assert_ne!(left.run, run);
    assert_eq!(left.changes.len(), 1, "the other branch is still to review");
}

fn copy_folder(folder: &MemDisk) -> MemDisk {
    let copy = disk();
    for dir in folder.directories(Root::Folder) {
        copy.perform(Io::MakeDir {
            root: Root::Folder,
            path: dir,
        })
        .unwrap();
    }
    for (path, bytes) in folder.files(Root::Folder) {
        copy.perform(Io::Create {
            root: Root::Folder,
            path,
            bytes,
        })
        .unwrap();
    }
    copy
}

fn a_restored_folder_makes_the_writer_rekey<F: Facade>() {
    let folder = disk();
    let clock = TestClock::at(1_000);
    let here = machine(&folder);
    let (mut a, _) = F::open(Probe::new(&here), env("a", 1, &clock)).unwrap();
    let song = create(&mut a, "song.npno", b"song");
    a.close().unwrap();
    let backup = copy_folder(&folder);
    let (mut a, opened) = F::open(Probe::new(&here), env("a", 2, &clock)).unwrap();
    let Start::Resumed(writer) = opened.start else {
        panic!("{:?}", opened.start);
    };
    a.commit("Tag", |i| i.add(song, TAGS, tag("lost"))).unwrap();
    a.close().unwrap();

    let restored = Machine {
        folder: backup,
        local: here.local.clone(),
    };
    let (mut a, opened) = F::open(Probe::new(&restored), env("a", 3, &clock)).unwrap();
    assert_eq!(
        opened.start,
        Start::Rekeyed {
            old: writer,
            why: Rekey::Restored
        }
    );
    assert_eq!(tags(&a.view(), song), ["new"], "the folder wins at once");
    a.commit("Tag", |i| i.add(song, TAGS, tag("kept"))).unwrap();
    let writers: Vec<WriterInfo> = a.others().unwrap();
    let this: Vec<_> = writers
        .iter()
        .filter(|w| w.here == Presence::This)
        .collect();
    assert_eq!(this.len(), 1);
    assert_ne!(this[0].writer, writer, "a fresh writer");
    let elsewhere = machine(&restored.folder);
    let (fresh, _) = F::open(Probe::new(&elsewhere), env("b", 4, &clock)).unwrap();
    assert_eq!(tags(&fresh.view(), song), ["kept", "new"]);
}

fn a_compacted_writer_is_shown_and_reported_and_goes_with_its_files<F: Facade>() {
    let folder = disk();
    let clock = TestClock::at(1_000);
    let mut here = machine(&folder);
    let (mut a, _) = F::open(Probe::new(&here), env("a", 1, &clock)).unwrap();
    let song = create(&mut a, "song.npno", b"song");
    let (mut b, _) = F::open(Probe::new(&machine(&folder)), env("b", 2, &clock)).unwrap();
    b.commit("Tag", |i| i.add(song, TAGS, tag("from-b")))
        .unwrap();
    b.compact().unwrap();
    let b_id = b
        .view()
        .writers()
        .iter()
        .find(|w| w.label == "b")
        .unwrap()
        .writer;
    let theirs: Vec<RelPath> = folder
        .files(Root::Folder)
        .into_keys()
        .filter(|p| p.starts_with(&layout().writer(b_id)))
        .collect();
    assert!(
        theirs.iter().all(|p| p.as_str().contains("snapshot-")),
        "{theirs:?}"
    );

    let changes = a.refresh().unwrap().changes;
    assert_eq!(tags(&a.view(), song), ["from-b", "new"]);
    assert!(
        changes.contains(&Change {
            entity: song,
            what: What::Field("tags".into()),
            by: By::Writer {
                writer: b_id,
                label: "b".into(),
            },
        }),
        "a change that arrives in a snapshot is reported: {changes:?}"
    );
    drop(a);
    for path in theirs {
        folder
            .perform(Io::Remove {
                root: Root::Folder,
                path,
            })
            .unwrap();
    }
    here.crash();
    let (a, _) = F::open(Probe::new(&here), env("a", 3, &clock)).unwrap();
    assert_eq!(tags(&a.view(), song), ["new"], "the folder wins at once");
}

/// Adopts `copy.npno` and saves `new.npno` in one intent.
fn adopt_and_save<F: Facade>(library: &mut F) -> Result<Committed, Error> {
    library.commit("Adopt", |i| {
        let (i, _) = i.create(|e| {
            e.adopt(&path("copy.npno"), Expect::Holds(identity(b"copy")))
                .add(TAGS, tag("copy"));
        });
        let (i, _) = i.create(|e| {
            e.save(&path("new.npno"), b"fresh".to_vec(), Expect::Absent)
                .add(TAGS, tag("fresh"));
        });
        i
    })
}

fn an_interrupted_adoption_is_settled_as_an_adoption<F: Facade>() {
    let one_disk = |disk: &MemDisk| Machine {
        folder: disk.clone(),
        local: disk.clone(),
    };
    let setup = |disk: &MemDisk| {
        let clock = TestClock::at(1_000);
        let (mut library, _) = F::open(Probe::new(&one_disk(disk)), env("a", 1, &clock)).unwrap();
        create(&mut library, "song.npno", b"song");
        put(disk, "copy.npno", b"copy");
        library
    };
    let total = {
        let disk = disk();
        let mut library = setup(&disk);
        let before = disk.mutations();
        adopt_and_save(&mut library).unwrap();
        disk.mutations() - before
    };
    let mut settled = 0;
    for crash in 0..total {
        let disk = disk();
        let mut library = setup(&disk);
        disk.crash_after(crash);
        let _ = adopt_and_save(&mut library);
        drop(library);
        let disk = disk.restart();
        let shown = format!("crash after {crash}");
        let clock = TestClock::at(2_000);
        let (mut again, opened) = F::open(Probe::new(&one_disk(&disk)), env("a", 2, &clock))
            .unwrap_or_else(|e| panic!("{shown}: {e}"));
        settled += opened.settled.len();
        again
            .commit("Next", |i| {
                let (i, _) = i.create(|e| {
                    e.add(TAGS, tag("next"));
                });
                i
            })
            .unwrap_or_else(|e| panic!("{shown}: {e}"));
        for entity in again.view().entities() {
            let Some(file) = entity.file() else {
                continue;
            };
            let present = read(&disk, file.path.as_str()).is_some();
            assert!(
                !present || file.state != FileState::Missing,
                "{shown}: {} is there but shown missing",
                file.path
            );
        }
        let undone = (0..10)
            .find_map(|_| again.undo().err())
            .expect("history ends");
        assert!(
            matches!(undone, Error::Refused(Refusal::Nothing)),
            "{shown}: every intent undoes: {undone:?}"
        );
        assert_eq!(
            read(&disk, "copy.npno").as_deref(),
            Some(b"copy".as_slice()),
            "{shown}: undoing an adoption leaves the file"
        );
    }
    assert!(settled > 0, "some crash interrupted the effect");
}

/// Runs `interrupted` on a fresh one-disk library holding `song.npno` and
/// `copy.npno`, crashing after each of its mutations in turn, and hands every
/// reopened library that settled an interrupted record to `check`.
fn after_each_settled_crash<F: Facade>(
    interrupted: impl Fn(&mut F, EntityId) -> Result<Committed, Error>,
    check: impl Fn(&mut F, &MemDisk, EntityId, &str),
) {
    let one_disk = |disk: &MemDisk| Machine {
        folder: disk.clone(),
        local: disk.clone(),
    };
    let setup = |disk: &MemDisk| {
        let clock = TestClock::at(1_000);
        let (mut library, _) = F::open(Probe::new(&one_disk(disk)), env("a", 1, &clock)).unwrap();
        let song = create(&mut library, "song.npno", b"song");
        put(disk, "copy.npno", b"copy");
        (library, song)
    };
    let total = {
        let disk = disk();
        let (mut library, song) = setup(&disk);
        let before = disk.mutations();
        interrupted(&mut library, song).unwrap();
        disk.mutations() - before
    };
    let mut settled = 0;
    for crash in 0..total {
        let disk = disk();
        let (mut library, song) = setup(&disk);
        disk.crash_after(crash);
        let _ = interrupted(&mut library, song);
        drop(library);
        let disk = disk.restart();
        let shown = format!("crash after {crash}");
        let clock = TestClock::at(2_000);
        let (mut again, opened) = F::open(Probe::new(&one_disk(&disk)), env("a", 2, &clock))
            .unwrap_or_else(|e| panic!("{shown}: {e}"));
        if opened.settled.is_empty() {
            continue;
        }
        settled += 1;
        check(&mut again, &disk, song, &shown);
    }
    assert!(settled > 0, "some crash interrupted the effect");
}

fn the_first_write_after_a_crash_follows_the_settled_intent<F: Facade>() {
    after_each_settled_crash::<F>(
        |library, song| {
            library.commit("Save", |i| {
                i.save(
                    song,
                    &path("song.npno"),
                    b"saved".to_vec(),
                    Expect::Holds(identity(b"song")),
                )
                .set(song, ORIGIN, "saved".into())
            })
        },
        |again, _, song, shown| {
            again
                .commit("Origin", |i| i.set(song, ORIGIN, "final".into()))
                .unwrap_or_else(|e| panic!("{shown}: {e}"));
            assert_eq!(
                again.view().entity(song).unwrap().get(ORIGIN),
                Field::Value("final".into()),
                "{shown}: the write replaces the one its writer settled"
            );
        },
    );
}

fn the_first_undo_after_a_crash_undoes_the_settled_intent<F: Facade>() {
    after_each_settled_crash::<F>(
        |library, _| adopt_and_save(library),
        |again, disk, _, shown| {
            again.undo().unwrap_or_else(|e| panic!("{shown}: {e}"));
            let history: Vec<(String, bool)> = again
                .history()
                .into_iter()
                .map(|item| (item.label, item.undone))
                .collect();
            assert_eq!(history, [("Adopt".into(), true)], "{shown}");
            assert_eq!(read(disk, "new.npno"), None, "{shown}: the saved file left");
            assert_eq!(
                read(disk, "copy.npno").as_deref(),
                Some(b"copy".as_slice()),
                "{shown}: the adopted file stayed"
            );
        },
    );
}

fn two_saves_undo_and_redo_one_after_the_other<F: Facade>() {
    let folder = disk();
    let clock = TestClock::at(1_000);
    let (mut a, _) = F::open(Probe::new(&machine(&folder)), env("a", 1, &clock)).unwrap();
    let song = create(&mut a, "song.npno", b"one");
    for (from, to) in [(&b"one"[..], &b"two"[..]), (b"two", b"three")] {
        clock.advance(1_000);
        a.commit("Save", |i| {
            i.save(
                song,
                &path("song.npno"),
                to.to_vec(),
                Expect::Holds(identity(from)),
            )
        })
        .unwrap();
    }
    for bytes in [&b"two"[..], b"one"] {
        clock.advance(1_000);
        a.undo().unwrap();
        assert_eq!(read(&folder, "song.npno").as_deref(), Some(bytes));
    }
    for bytes in [&b"two"[..], b"three"] {
        clock.advance(1_000);
        a.redo().unwrap();
        assert_eq!(read(&folder, "song.npno").as_deref(), Some(bytes));
    }
}

fn saves_of_one_length_within_the_volumes_time_resolution_stay_in_sync<F: Facade>() {
    let folder = disk();
    let clock = TestClock::at(1_000);
    let here = machine(&folder);
    let probe = Probe::new(&here);
    probe.seen.borrow_mut().coarse = Some(0);
    let (mut a, _) = F::open(probe.clone(), env("a", 1, &clock)).unwrap();
    let song = create(&mut a, "song.npno", b"one");
    a.rescan().unwrap();
    let state = |a: &F| a.view().entity(song).unwrap().file().unwrap().state;
    for (from, to) in [(&b"one"[..], &b"two"[..]), (b"two", b"six")] {
        clock.advance(1_000);
        a.commit("Save", |i| {
            i.save(
                song,
                &path("song.npno"),
                to.to_vec(),
                Expect::Holds(identity(from)),
            )
        })
        .unwrap();
        assert_eq!(state(&a), FileState::InSync, "after saving {to:?}");
        a.rescan().unwrap();
        assert_eq!(
            state(&a),
            FileState::InSync,
            "rescanned after saving {to:?}"
        );
    }
    a.close().unwrap();
    let probe = Probe::new(&here);
    probe.seen.borrow_mut().coarse = Some(0);
    let (mut a, _) = F::open(probe, env("a", 1, &clock)).unwrap();
    a.rescan().unwrap();
    assert_eq!(state(&a), FileState::InSync, "reopened");
}

fn another_writers_rewrite_at_one_length_and_time_shows_at_the_next_refresh<F: Facade>() {
    let folder = disk();
    let clock = TestClock::at(1_000);
    put(&folder, "a.wav", b"wave");
    let open = |label, seed| {
        let probe = Probe::new(&machine(&folder));
        probe.seen.borrow_mut().coarse = Some(0);
        let (mut writer, _) = F::open(probe, env(label, seed, &clock)).unwrap();
        writer.rescan().unwrap();
        writer
    };
    let (mut a, mut b) = (open("a", 1), open("b", 2));
    save_untagged(&mut b);

    let refreshed = a.refresh().unwrap();
    assert_eq!(refreshed.rewritten, [path("a.wav")], "b's save");
    assert_eq!(a.refresh().unwrap().rewritten, [], "seen once");
}

fn two_other_writers_rewrites_at_one_length_and_time_show_at_one_refresh<F: Facade>() {
    for (seed_b, seed_c) in [(2, 3), (3, 2), (4, 5), (5, 4), (6, 7), (7, 6)] {
        let folder = disk();
        let clock = TestClock::at(1_000);
        put(&folder, "a.wav", b"wave");
        let open = |label, seed| {
            let probe = Probe::new(&machine(&folder));
            probe.seen.borrow_mut().coarse = Some(0);
            let (mut writer, _) = F::open(probe, env(label, seed, &clock)).unwrap();
            writer.rescan().unwrap();
            writer
        };
        let (mut a, mut b, mut c) = (open("a", 1), open("b", seed_b), open("c", seed_c));
        save_untagged(&mut b);
        c.refresh().unwrap();
        let held = Expect::Holds(identity(b"wavy"));
        c.commit("Save", |i| {
            i.save_file(&path("a.wav"), b"wavx".to_vec(), held)
        })
        .unwrap();
        let rewritten = a.refresh().unwrap().rewritten;
        assert_eq!(rewritten, [path("a.wav")], "seeds {seed_b} and {seed_c}");
    }
}

fn another_writers_rewrite_whose_log_arrives_before_its_file_is_never_saved_over<F: Facade>() {
    for wrote_before in [false, true] {
        let (theirs, ours) = (disk(), disk());
        let clock = TestClock::at(1_000);
        put(&theirs, "a.wav", b"wave");
        let open = |folder: &MemDisk, label, seed| {
            let probe = Probe::new(&machine(folder));
            probe.seen.borrow_mut().coarse = Some(0);
            let (mut writer, _) = F::open(probe, env(label, seed, &clock)).unwrap();
            writer.rescan().unwrap();
            writer
        };
        let deliver = |path: &RelPath, bytes: &[u8]| {
            let _ = ours.perform(Io::Remove {
                root: Root::Folder,
                path: path.clone(),
            });
            put(&ours, path.as_str(), bytes);
        };
        let mut b = open(&theirs, "b", 2);
        for (path, bytes) in theirs.files(Root::Folder) {
            deliver(&path, &bytes);
        }
        let mut a = open(&ours, "a", 1);
        if wrote_before {
            create(&mut a, "x.npno", b"x");
        }
        save_untagged(&mut b);
        // Sync brings b's log before the file it names.
        for (path, bytes) in theirs.files(Root::Folder) {
            if path.as_str().starts_with('.') {
                deliver(&path, &bytes);
            }
        }
        a.refresh().unwrap();
        deliver(&path("a.wav"), b"wavy");
        a.rescan().unwrap();

        let stale = Expect::Holds(identity(b"wave"));
        let saved = a.commit("Save", |i| {
            i.save_file(&path("a.wav"), b"mine".to_vec(), stale)
        });
        assert!(
            matches!(saved, Err(Error::Refused(Refusal::Changed(_)))),
            "a save over what a read before held, wrote before {wrote_before}: {saved:?}"
        );
        assert_eq!(ours.files(Root::Folder)[&path("a.wav")], b"wavy");
        assert_eq!(a.rescan().unwrap().rewritten, [path("a.wav")]);
    }
}

fn another_writers_rewrite_under_another_spelling_shows_at_the_next_refresh<F: Facade>() {
    let folder = disk();
    let clock = TestClock::at(1_000);
    put(&folder, "cafe\u{301}.wav", b"wave");
    let open = |env: Env| {
        let probe = decomposing(&folder);
        probe.seen.borrow_mut().coarse = Some(0);
        let (mut writer, _) = F::open(probe, env).unwrap();
        writer.rescan().unwrap();
        writer
    };
    let composing = Env {
        names: Box::new(Composing),
        ..env("a", 1, &clock)
    };
    let (mut a, mut b) = (open(composing), open(env("b", 2, &clock)));
    let held = Expect::Holds(identity(b"wave"));
    b.commit("Save", |i| {
        i.save_file(&path("caf\u{e9}.wav"), b"wavy".to_vec(), held)
    })
    .unwrap();

    let refreshed = a.refresh().unwrap();
    assert_eq!(refreshed.rewritten, [path("cafe\u{301}.wav")], "as listed");
}

fn an_undo_is_refused_over_another_writers_change_under_another_spelling<F: Facade>() {
    let folder = disk();
    let clock = TestClock::at(1_000);
    put(&folder, "cafe\u{301}.wav", b"wave");
    let open = |env: Env| {
        let (mut writer, _) = F::open(decomposing(&folder), env).unwrap();
        writer.rescan().unwrap();
        writer
    };
    let composing = Env {
        names: Box::new(Composing),
        ..env("a", 1, &clock)
    };
    let (mut a, mut b) = (open(composing), open(env("b", 2, &clock)));
    let held = Expect::Holds(identity(b"wave"));
    a.commit("Save", |i| {
        i.save_file(&path("cafe\u{301}.wav"), b"wavy".to_vec(), held)
    })
    .unwrap();
    b.refresh().unwrap();
    let held = Expect::Holds(identity(b"wavy"));
    let renamed = b
        .commit("Rename", |i| {
            i.rename_file(&path("caf\u{e9}.wav"), &path("moved.wav"), held)
        })
        .unwrap();
    a.refresh().unwrap();

    changed_since_by(a.undo(), &b, &renamed);
}

fn a_rewrite_at_one_length_and_time_a_commit_read_first_shows_at_the_next_refresh<F: Facade>() {
    let (theirs, ours) = (disk(), disk());
    let clock = TestClock::at(1_000);
    put(&theirs, "a.wav", b"wave");
    let open = |folder: &MemDisk, label, seed| {
        let probe = Probe::new(&machine(folder));
        probe.seen.borrow_mut().coarse = Some(0);
        let (mut writer, _) = F::open(probe, env(label, seed, &clock)).unwrap();
        writer.rescan().unwrap();
        writer
    };
    let mut b = open(&theirs, "b", 2);
    let deliver = |path: &RelPath, bytes: &[u8]| {
        let _ = ours.perform(Io::Remove {
            root: Root::Folder,
            path: path.clone(),
        });
        put(&ours, path.as_str(), bytes);
    };
    for (path, bytes) in theirs.files(Root::Folder) {
        deliver(&path, &bytes);
    }
    let mut a = open(&ours, "a", 1);
    save_untagged(&mut b);
    let held = Expect::Holds(identity(b"wavy"));
    b.commit("Save", |i| {
        i.save_file(&path("a.wav"), b"wavx".to_vec(), held)
    })
    .unwrap();
    // Sync brings the file here before the log entries that name it.
    deliver(&path("a.wav"), b"wavx");
    a.commit("Tag", |i| {
        let (i, _) = i.create(|e| {
            e.adopt(&path("a.wav"), Expect::File).add(TAGS, tag("Live"));
        });
        i
    })
    .unwrap();
    for (path, bytes) in theirs.files(Root::Folder) {
        if path.as_str().starts_with(".") {
            deliver(&path, &bytes);
        }
    }
    let rewritten = a.refresh().unwrap().rewritten;
    assert_eq!(
        rewritten,
        [path("a.wav")],
        "b's saves, read first by a's commit"
    );
}

fn another_writers_rewrite_at_one_length_and_time_shows_where_it_then_moved_the_file<F: Facade>() {
    for tree in [false, true] {
        let folder = disk();
        let clock = TestClock::at(1_000);
        put(&folder, "d/a.wav", b"wave");
        let open = |label, seed| {
            let probe = Probe::new(&machine(&folder));
            probe.seen.borrow_mut().coarse = Some(0);
            let (mut writer, _) = F::open(probe, env(label, seed, &clock)).unwrap();
            writer.rescan().unwrap();
            writer
        };
        let (mut a, mut b) = (open("a", 1), open("b", 2));
        let saved = Expect::Holds(identity(b"wave"));
        b.commit("Save", |i| {
            i.save_file(&path("d/a.wav"), b"wavy".to_vec(), saved)
        })
        .unwrap();
        let held = Expect::Holds(identity(b"wavy"));
        b.commit("Move", |i| match tree {
            false => i.rename_file(&path("d/a.wav"), &path("e/a.wav"), held),
            true => i.move_tree(&path("d"), &path("e")),
        })
        .unwrap();
        let rewritten = a.refresh().unwrap().rewritten;
        assert_eq!(rewritten, [path("e/a.wav")], "moved by a tree: {tree}");
    }
}

fn another_writers_move_of_a_file_this_view_rewrote_since_shows_where_it_lands<F: Facade>() {
    let (theirs, ours) = (disk(), disk());
    let clock = TestClock::at(1_000);
    let deliver = |path: &RelPath, bytes: &[u8]| {
        let _ = ours.perform(Io::Remove {
            root: Root::Folder,
            path: path.clone(),
        });
        put(&ours, path.as_str(), bytes);
    };
    let open = |folder: &MemDisk, label, seed| {
        let probe = Probe::new(&machine(folder));
        probe.seen.borrow_mut().coarse = Some(0);
        let (mut writer, _) = F::open(probe, env(label, seed, &clock)).unwrap();
        writer.rescan().unwrap();
        writer
    };
    let mut a = open(&theirs, "a", 1);
    let song = create(&mut a, "song.npno", b"song");
    for (path, bytes) in theirs.files(Root::Folder) {
        deliver(&path, &bytes);
    }
    let mut b = open(&ours, "b", 2);
    let held = Expect::Holds(identity(b"song"));
    b.commit("Save", |i| {
        i.save(song, &path("song.npno"), b"sonh".to_vec(), held)
    })
    .unwrap();
    b.rescan().unwrap();
    let held = Expect::Holds(identity(b"song"));
    a.commit("Rename", |i| i.rename(song, &path("tune.npno"), held))
        .unwrap();
    // Sync brings a's file, at the time b's save kept, and its log, and keeps b's.
    for (path, bytes) in theirs.files(Root::Folder) {
        if path.as_str() == "tune.npno" || path.as_str().starts_with(".") {
            deliver(&path, &bytes);
        }
    }
    let rewritten = b.refresh().unwrap().rewritten;
    assert_eq!(
        rewritten,
        [path("tune.npno")],
        "a's move of what b replaced"
    );
}

fn a_write_back_because_a_later_entry_is_cut_short_is_no_ones_change<F: Facade>() {
    let folder = disk();
    let clock = TestClock::at(1_000);
    let (mut a, _) = F::open(Probe::new(&machine(&folder)), env("a", 1, &clock)).unwrap();
    let (mut b, _) = F::open(Probe::new(&machine(&folder)), env("b", 2, &clock)).unwrap();
    let song = create(&mut b, "song.npno", b"song");
    a.refresh().unwrap();
    let held = Expect::Holds(identity(b"song"));
    b.commit("Move", |i| i.rename(song, &path("moved.npno"), held))
        .unwrap();
    a.refresh().unwrap();
    let b_writer = own_writer(&b.view()).unwrap().to_string();
    let logs = folder.files(Root::Folder).into_iter();
    let (log, bytes) = logs
        .filter(|(path, _)| path.as_str().contains(&b_writer) && path.as_str().ends_with(".txt"))
        .max_by_key(|(_, bytes)| bytes.len())
        .unwrap();
    // A sync client cuts b's log short of the move.
    let lines: Vec<&[u8]> = bytes.split_inclusive(|byte| *byte == b'\n').collect();
    let kept: Vec<u8> = lines[..lines.len() - 1].concat();
    folder
        .perform(Io::Remove {
            root: Root::Folder,
            path: log.clone(),
        })
        .unwrap();
    folder
        .perform(Io::Create {
            root: Root::Folder,
            path: log,
            bytes: kept,
        })
        .unwrap();
    let changes = a.rescan().unwrap().changes;
    let by_b = changes
        .iter()
        .filter(|change| matches!(change.by, By::Writer { .. }));
    assert_eq!(by_b.collect::<Vec<_>>(), [] as [&Change; 0], "{changes:?}");
}

fn another_writers_write_back_because_a_later_entry_is_cut_short_is_no_ones_change<F: Facade>() {
    let folder = disk();
    let clock = TestClock::at(1_000);
    let open = |label, seed| {
        let (writer, _) = F::open(Probe::new(&machine(&folder)), env(label, seed, &clock)).unwrap();
        writer
    };
    let (mut a, mut b, mut c) = (open("a", 1), open("b", 2), open("c", 3));
    let song = create(&mut b, "song.npno", b"song");
    c.refresh().unwrap();
    let held = Expect::Holds(identity(b"song"));
    c.commit("Move", |i| i.rename(song, &path("moved.npno"), held))
        .unwrap();
    a.rescan().unwrap();
    let before = a.view();
    let c_writer = own_writer(&c.view()).unwrap().to_string();
    let logs = folder.files(Root::Folder).into_iter();
    let (log, bytes) = logs
        .filter(|(path, _)| path.as_str().contains(&c_writer) && path.as_str().ends_with(".txt"))
        .max_by_key(|(_, bytes)| bytes.len())
        .unwrap();
    // A sync client cuts c's log short of the move, which gives back b's write.
    let lines: Vec<&[u8]> = bytes.split_inclusive(|byte| *byte == b'\n').collect();
    let kept: Vec<u8> = lines[..lines.len() - 1].concat();
    folder
        .perform(Io::Remove {
            root: Root::Folder,
            path: log.clone(),
        })
        .unwrap();
    folder
        .perform(Io::Create {
            root: Root::Folder,
            path: log,
            bytes: kept,
        })
        .unwrap();
    let changes = a.rescan().unwrap().changes;
    let by_writer = changes
        .iter()
        .filter(|change| matches!(change.by, By::Writer { .. }));
    assert_eq!(
        by_writer.collect::<Vec<_>>(),
        [] as [&Change; 0],
        "{changes:?}"
    );
    let rows = a.view().changes_since(&before);
    let tagged = row_of(&before, song).file;
    let changed = changes_of(&rows, tagged);
    assert!(
        changed
            .iter()
            .all(|(_, by, _)| !matches!(by, Some(Who::Writer(_)))),
        "the row's change is no writer's: {changed:?}"
    );
}

fn an_undo_is_not_refused_for_another_writers_pin_alone<F: Facade>() {
    let (theirs, ours) = (disk(), disk());
    let clock = TestClock::at(1_000);
    let deliver = |path: &RelPath, bytes: &[u8]| {
        let _ = ours.perform(Io::Remove {
            root: Root::Folder,
            path: path.clone(),
        });
        put(&ours, path.as_str(), bytes);
    };
    let (mut a, _) = F::open(Probe::new(&machine(&theirs)), env("a", 1, &clock)).unwrap();
    let song = create(&mut a, "p.npno", b"song");
    for (path, bytes) in theirs.files(Root::Folder) {
        deliver(&path, &bytes);
    }
    let (mut c, _) = F::open(Probe::new(&machine(&ours)), env("c", 3, &clock)).unwrap();
    c.rescan().unwrap();
    // On a's copy the file moves outside, and a's next commit pins it there.
    remove_outside(&theirs, "p.npno");
    put(&theirs, "q.npno", b"song");
    a.rescan().unwrap();
    create(&mut a, "other.npno", b"other");
    clock.advance(1_000);
    let held = Expect::Holds(identity(b"song"));
    c.commit("Save", |i| {
        i.save(song, &path("p.npno"), b"sung".to_vec(), held)
    })
    .unwrap();
    for (path, bytes) in theirs.files(Root::Folder) {
        if path.as_str().starts_with('.') {
            deliver(&path, &bytes);
        }
    }
    c.refresh().unwrap();

    c.undo().unwrap();
    let bytes = ours.files(Root::Folder).remove(&path("p.npno"));
    assert_eq!(bytes.as_deref(), Some(&b"song"[..]));
}

fn an_undo_is_not_refused_for_a_pin_another_writer_compacted<F: Facade>() {
    for again in [false, true] {
        let (theirs, ours) = (disk(), disk());
        let clock = TestClock::at(1_000);
        let deliver = |path: &RelPath, bytes: &[u8]| {
            let _ = ours.perform(Io::Remove {
                root: Root::Folder,
                path: path.clone(),
            });
            put(&ours, path.as_str(), bytes);
        };
        let (mut a, _) = F::open(Probe::new(&machine(&theirs)), env("a", 1, &clock)).unwrap();
        let song = create(&mut a, "p.npno", b"song");
        for (path, bytes) in theirs.files(Root::Folder) {
            deliver(&path, &bytes);
        }
        let (mut c, _) = F::open(Probe::new(&machine(&ours)), env("c", 3, &clock)).unwrap();
        c.rescan().unwrap();
        // On a's copy the file moves outside, a's next commit pins it there, and a
        // compacts the pin into a snapshot.
        remove_outside(&theirs, "p.npno");
        put(&theirs, "q.npno", b"song");
        a.rescan().unwrap();
        create(&mut a, "other.npno", b"other");
        a.compact().unwrap();
        if again {
            // A second compaction carries the pin over from the first.
            create(&mut a, "third.npno", b"third");
            a.compact().unwrap();
        }
        clock.advance(1_000);
        let held = Expect::Holds(identity(b"song"));
        c.commit("Save", |i| {
            i.save(song, &path("p.npno"), b"sung".to_vec(), held)
        })
        .unwrap();
        // Sync brings a's snapshot and takes away the segments it folded.
        let a_writer = own_writer(&a.view()).unwrap().to_string();
        let a_files = |folder: &MemDisk| {
            let files = folder.files(Root::Folder).into_iter();
            let files = files.filter(|(path, _)| path.as_str().contains(&a_writer));
            files.map(|(path, _)| path).collect::<BTreeSet<RelPath>>()
        };
        for gone in a_files(&ours).difference(&a_files(&theirs)) {
            remove_outside(&ours, gone.as_str());
        }
        for (path, bytes) in theirs.files(Root::Folder) {
            if path.as_str().starts_with('.') {
                deliver(&path, &bytes);
            }
        }
        c.refresh().unwrap();

        let undone = c.undo();
        assert!(undone.is_ok(), "compacted again: {again}, {undone:?}");
        let bytes = ours.files(Root::Folder).remove(&path("p.npno"));
        assert_eq!(
            bytes.as_deref(),
            Some(&b"song"[..]),
            "compacted again: {again}"
        );
    }
}

fn an_undo_after_another_writer_compacted_keeps_the_others_bytes<F: Facade>() {
    for (made_dir, compacts) in [(true, false), (true, true), (false, false), (false, true)] {
        let folder = disk();
        let clock = TestClock::at(1_000);
        put(&folder, "p.txt", b"ppp");
        let open = |label, seed| {
            let (mut writer, _) =
                F::open(Probe::new(&machine(&folder)), env(label, seed, &clock)).unwrap();
            writer.rescan().unwrap();
            writer
        };
        let (mut a, mut b) = (open("a", 1), open("b", 2));
        clock.advance(1_000);
        let theirs = match made_dir {
            true => {
                a.commit("Folder", |i| i.make_dir(&path("d"))).unwrap();
                "d/x.txt"
            }
            false => {
                let held = Expect::Holds(identity(b"ppp"));
                a.commit("Rename", |i| {
                    i.rename_file(&path("p.txt"), &path("q.txt"), held)
                })
                .unwrap();
                "q.txt"
            }
        };
        b.refresh().unwrap();
        b.rescan().unwrap();
        clock.advance(1_000);
        let expect = match made_dir {
            true => Expect::Absent,
            false => Expect::Holds(identity(b"ppp")),
        };
        b.commit("Save", |i| {
            i.save_file(&path(theirs), b"bbb".to_vec(), expect)
        })
        .unwrap();
        if compacts {
            b.compact().unwrap();
        }
        a.refresh().unwrap();
        a.rescan().unwrap();
        let undone = a.undo();
        let files = library_files(&folder);
        let kept = files.values().any(|bytes| bytes == b"bbb");
        assert!(
            kept,
            "made dir {made_dir}, compacted {compacts}: {undone:?}, {:?}",
            files.keys().collect::<Vec<_>>()
        );
    }
}

fn a_tree_move_undone_after_another_writer_compacted_carries_its_file_along<F: Facade>() {
    let folder = disk();
    let clock = TestClock::at(1_000);
    put(&folder, "t/one.txt", b"one");
    put(&folder, "x.txt", b"xxx");
    let open = |label, seed| {
        let (mut writer, _) =
            F::open(Probe::new(&machine(&folder)), env(label, seed, &clock)).unwrap();
        writer.rescan().unwrap();
        writer
    };
    let (mut a, mut b) = (open("a", 1), open("b", 2));
    clock.advance(1_000);
    a.commit("Tree", |i| i.move_tree(&path("t"), &path("u")))
        .unwrap();
    b.refresh().unwrap();
    b.rescan().unwrap();
    clock.advance(1_000);
    let held = Expect::Holds(identity(b"xxx"));
    b.commit("Into", |i| {
        i.rename_file(&path("x.txt"), &path("u/x.txt"), held)
    })
    .unwrap();
    b.compact().unwrap();
    a.refresh().unwrap();
    a.rescan().unwrap();
    // The folded move into the tree no longer refuses the undo, which takes b's file
    // back with the tree.
    a.undo().unwrap();
    assert_eq!(read(&folder, "t/x.txt").as_deref(), Some(&b"xxx"[..]));
}

fn another_writers_move_onto_a_file_of_one_length_and_time_shows_where_it_lands<F: Facade>() {
    let (theirs, ours) = (disk(), disk());
    let clock = TestClock::at(1_000);
    let open = |folder: &MemDisk, label, seed| {
        let probe = Probe::new(&machine(folder));
        probe.seen.borrow_mut().coarse = Some(0);
        let (mut writer, _) = F::open(probe, env(label, seed, &clock)).unwrap();
        writer.rescan().unwrap();
        writer
    };
    put(&theirs, "a.wav", b"wave");
    put(&ours, "a.wav", b"wave");
    put(&ours, "b.wav", b"wavy");
    let (mut a, mut b) = (open(&theirs, "a", 1), open(&ours, "b", 2));
    let held = Expect::Holds(identity(b"wave"));
    a.commit("Rename", |i| {
        i.rename_file(&path("a.wav"), &path("b.wav"), held)
    })
    .unwrap();
    // Sync brings a's log and its move, which replaces b's file of one length.
    for (path, bytes) in theirs.files(Root::Folder) {
        let _ = ours.perform(Io::Remove {
            root: Root::Folder,
            path: path.clone(),
        });
        put(&ours, path.as_str(), &bytes);
    }
    remove_outside(&ours, "a.wav");
    let rewritten = b.refresh().unwrap().rewritten;
    assert_eq!(rewritten, [path("b.wav")], "a's move onto what b held");
}

fn a_rewrite_moved_onto_a_file_of_one_length_and_time_binds_as_a_fresh_open_does<F: Facade>() {
    let cases = [
        (true, &b"sonh"[..], &b"tunq"[..]),
        (false, b"sonh", b"tunq"),
        (true, b"sonhx", b"tunqx"),
    ];
    for (trash, saved, other) in cases {
        let folder = disk();
        let clock = TestClock::at(1_000);
        let open = |label, seed| {
            let probe = Probe::new(&machine(&folder));
            probe.seen.borrow_mut().coarse = Some(0);
            let (mut writer, _) = F::open(probe, env(label, seed, &clock)).unwrap();
            writer.rescan().unwrap();
            writer
        };
        let mut a = open("a", 1);
        let song = create(&mut a, "a.npno", b"song");
        let tune = create(&mut a, "b.npno", other);
        a.rescan().unwrap();
        let mut b = open("b", 2);
        let held = Expect::Holds(identity(b"song"));
        b.commit("Save", |i| {
            i.save(song, &path("a.npno"), saved.to_vec(), held)
        })
        .unwrap();
        b.commit("Delete", |i| i.delete(tune)).unwrap();
        match trash {
            true => {
                let held = Expect::Holds(identity(other));
                b.commit("Trash", |i| i.trash_file(&path("b.npno"), held))
                    .unwrap();
            }
            false => remove_outside(&folder, "b.npno"),
        }
        let held = Expect::Holds(identity(saved));
        b.commit("Move", |i| i.rename(song, &path("b.npno"), held))
            .unwrap();
        a.refresh().unwrap();
        let state = |w: &F| {
            let file = w.view().entity(song).and_then(|e| e.file());
            file.map(|f| (f.path.clone(), f.state))
        };
        let fresh = open("c", 3);
        assert_eq!(state(&a), state(&fresh), "trashed {trash}, saved {saved:?}");
        assert_eq!(state(&a), Some((path("b.npno"), FileState::InSync)));
    }
}

fn another_writers_swap_through_a_spare_name_binds_as_a_fresh_open_does<F: Facade>() {
    let folder = disk();
    let clock = TestClock::at(1_000);
    let open = |label, seed| {
        let probe = Probe::new(&machine(&folder));
        probe.seen.borrow_mut().coarse = Some(0);
        let (mut writer, _) = F::open(probe, env(label, seed, &clock)).unwrap();
        writer.rescan().unwrap();
        writer
    };
    let mut a = open("a", 1);
    let song = create(&mut a, "a.npno", b"song");
    let tune = create(&mut a, "b.npno", b"tune");
    a.rescan().unwrap();
    let mut b = open("b", 2);
    let moves = [
        (song, "t.npno", b"song"),
        (tune, "a.npno", b"tune"),
        (song, "b.npno", b"song"),
    ];
    for (entity, to, holds) in moves {
        let held = Expect::Holds(identity(holds));
        b.commit("Swap", |i| i.rename(entity, &path(to), held))
            .unwrap();
    }
    a.refresh().unwrap();
    let fresh = open("c", 3);
    let files = |w: &F| {
        let view = w.view();
        let file = |e| view.entity(e).and_then(|e| e.file());
        [song, tune].map(|e| file(e).map(|f| (f.path, f.state)))
    };
    assert_eq!(files(&a), files(&fresh));
    assert_eq!(files(&a)[0], Some((path("b.npno"), FileState::InSync)));
}

fn a_file_another_writer_moved_away_and_sync_kept_shows_a_rewrite_logged_before<F: Facade>() {
    for seeds in [(2, 3), (3, 2)] {
        let (ours, theirs, others) = (disk(), disk(), disk());
        let clock = TestClock::at(1_000);
        let open = |folder: &MemDisk, label, seed| {
            let probe = Probe::new(&machine(folder));
            probe.seen.borrow_mut().coarse = Some(0);
            let (mut writer, _) = F::open(probe, env(label, seed, &clock)).unwrap();
            writer.rescan().unwrap();
            writer
        };
        put(&ours, "a.wav", b"wave");
        put(&ours, "b.wav", b"wavy");
        put(&theirs, "b.wav", b"wavy");
        put(&others, "a.wav", b"wave");
        let mut a = open(&ours, "a", 1);
        let (mut b, mut c) = (open(&theirs, "b", seeds.0), open(&others, "c", seeds.1));
        let held = Expect::Holds(identity(b"wavy"));
        b.commit("Rename", |i| {
            i.rename_file(&path("b.wav"), &path("a.wav"), held)
        })
        .unwrap();
        let held = Expect::Holds(identity(b"wave"));
        c.commit("Rename", |i| {
            i.rename_file(&path("a.wav"), &path("c.wav"), held)
        })
        .unwrap();
        // Sync brings both logs and keeps b's file at a.wav over c's move of a's.
        for folder in [&theirs, &others] {
            for (path, bytes) in folder.files(Root::Folder) {
                let _ = ours.perform(Io::Remove {
                    root: Root::Folder,
                    path: path.clone(),
                });
                put(&ours, path.as_str(), &bytes);
            }
        }
        put(&ours, "c.wav", b"wave");
        remove_outside(&ours, "b.wav");
        let rewritten = a.refresh().unwrap().rewritten;
        assert!(
            rewritten.contains(&path("a.wav")),
            "b's file kept where c moved a's from, writers {seeds:?}: {rewritten:?}"
        );
    }
}

fn a_file_two_writers_saved_at_one_length_and_time_is_the_entitys_whose_bytes_it_holds<
    F: Facade,
>() {
    for seeds in [(1, 2), (2, 1)] {
        let (ours, theirs, held) = (disk(), disk(), disk());
        let clock = TestClock::at(1_000);
        let open = |folder: &MemDisk, label, seed| {
            let probe = Probe::new(&machine(folder));
            probe.seen.borrow_mut().coarse = Some(0);
            let (mut writer, _) = F::open(probe, env(label, seed, &clock)).unwrap();
            writer.rescan().unwrap();
            writer
        };
        let mut a = open(&ours, "a", seeds.0);
        let mut b = open(&theirs, "b", seeds.1);
        create(&mut a, "song.npno", b"song");
        let sonh = create(&mut b, "song.npno", b"sonh");
        // Sync brings both logs and keeps b's file.
        for folder in [&ours, &theirs] {
            for (path, bytes) in folder.files(Root::Folder) {
                put(&held, path.as_str(), &bytes);
            }
        }
        put(&held, "song.npno", b"sonh");
        let fresh = open(&held, "c", 3);
        let row = fresh.view().row_at(&path("song.npno")).cloned();
        assert_eq!(
            row.and_then(|row| row.entity),
            Some(sonh),
            "the file holds b's bytes, writers {seeds:?}"
        );
    }
}

fn an_undo_is_not_refused_for_a_delete_older_than_its_change<F: Facade>() {
    let (theirs, ours) = (disk(), disk());
    let clock = TestClock::at(1_000);
    let deliver = |path: &RelPath, bytes: &[u8]| {
        let _ = ours.perform(Io::Remove {
            root: Root::Folder,
            path: path.clone(),
        });
        put(&ours, path.as_str(), bytes);
    };
    let (mut a, _) = F::open(Probe::new(&machine(&theirs)), env("a", 1, &clock)).unwrap();
    let song = create(&mut a, "song.npno", b"song");
    for (path, bytes) in theirs.files(Root::Folder) {
        deliver(&path, &bytes);
    }
    let (mut b, _) = F::open(Probe::new(&machine(&ours)), env("b", 2, &clock)).unwrap();
    b.rescan().unwrap();
    a.undo().unwrap();
    clock.advance(1_000);
    let rename = |b: &mut F, to: &str| {
        let held = Expect::Holds(identity(b"song"));
        b.commit("Rename", |i| i.rename(song, &path(to), held))
            .unwrap();
    };
    rename(&mut b, "x.npno");
    // a's delete, which never saw the rename, arrives after it.
    for (path, bytes) in theirs.files(Root::Folder) {
        if path.as_str().starts_with(".") {
            deliver(&path, &bytes);
        }
    }
    b.refresh().unwrap();
    rename(&mut b, "y.npno");
    b.undo().unwrap();
    b.undo().unwrap();
    assert_eq!(row_of(&b.view(), song).path, path("song.npno"));
}

fn another_writers_rewrite_read_only_through_its_snapshot_shows_at_the_next_refresh<F: Facade>() {
    let folder = disk();
    let clock = TestClock::at(1_000);
    let open = |label, seed| {
        let probe = Probe::new(&machine(&folder));
        probe.seen.borrow_mut().coarse = Some(0);
        let (mut writer, _) = F::open(probe, env(label, seed, &clock)).unwrap();
        writer.rescan().unwrap();
        writer
    };
    let (mut a, mut b) = (open("a", 1), open("b", 2));
    let song = create(&mut b, "song.npno", b"song");
    a.refresh().unwrap();
    let held = Expect::Holds(identity(b"song"));
    b.commit("Save", |i| {
        i.save(song, &path("song.npno"), b"sonh".to_vec(), held)
    })
    .unwrap();
    b.compact().unwrap();
    assert_eq!(a.refresh().unwrap().rewritten, [path("song.npno")]);
}

fn a_save_another_file_lands_over_before_it_is_seen_names_its_own_bytes<F: Facade>() {
    let folder = disk();
    let clock = TestClock::at(1_000);
    let probe = Probe::new(&machine(&folder));
    let seen = Rc::clone(&probe.seen);
    let (mut a, _) = F::open(probe, env("a", 1, &clock)).unwrap();
    a.rescan().unwrap();
    let song = create(&mut a, "song.npno", b"song");
    // Once the save's file is in place, renamed there or copied and its stage removed,
    // another program's lands over it.
    let placed = Rc::new(Cell::new(false));
    let (theirs, landed) = (folder.clone(), Rc::clone(&placed));
    let staged = |at: &RelPath| layout().owns(at) && at.as_str().ends_with(".npno");
    seen.borrow_mut().failing = Some(Box::new(move |io: &Io| {
        match io {
            Io::Rename { to, .. } if *to == path("song.npno") => landed.set(true),
            Io::Remove { path: at, .. } if staged(at) => landed.set(true),
            Io::Stat { path: at, .. } if landed.get() && *at == path("song.npno") => {
                landed.set(false);
                remove_outside(&theirs, "song.npno");
                put(&theirs, "song.npno", b"theirs");
            }
            _ => {}
        }
        false
    }));
    let held = Expect::Holds(identity(b"song"));
    let committed = a
        .commit("Save", |i| {
            i.save(song, &path("song.npno"), b"sonh".to_vec(), held)
        })
        .unwrap();
    seen.borrow_mut().failing = None;
    let wrote: Vec<Identity> = committed.wrote.iter().map(|fact| fact.identity).collect();
    assert_eq!(wrote, [identity(b"sonh")], "the save's own bytes");
    assert_eq!(a.rescan().unwrap().rewritten, [path("song.npno")]);
    let mine = Expect::Holds(identity(b"sonh"));
    let again = a.commit("Save", |i| {
        i.save(song, &path("song.npno"), b"sonj".to_vec(), mine)
    });
    assert!(again.is_err(), "a save over theirs: {again:?}");
    assert_eq!(read(&folder, "song.npno").as_deref(), Some(&b"theirs"[..]));
}

fn a_refused_save_takes_no_identity_from_a_file_since_put_back<F: Facade>() {
    let folder = disk();
    let clock = TestClock::at(1_000);
    let open = |label, seed| {
        let probe = Probe::new(&machine(&folder));
        probe.seen.borrow_mut().coarse = Some(0);
        let (mut writer, _) = F::open(probe, env(label, seed, &clock)).unwrap();
        writer.rescan().unwrap();
        writer
    };
    let mut a = open("a", 1);
    let song = create(&mut a, "song.npno", b"song");
    a.rescan().unwrap();
    put(&folder, "song.npno", b"longer!");
    let held = Expect::Holds(identity(b"song"));
    let saved = a.commit("Save", |i| {
        i.save(song, &path("song.npno"), b"sonh".to_vec(), held)
    });
    assert!(saved.is_err(), "{saved:?}");
    // The longer file goes back to what it was, at its length and time.
    put(&folder, "song.npno", b"song");
    a.rescan().unwrap();
    let state = |w: &F| {
        w.view()
            .entity(song)
            .and_then(|e| e.file())
            .map(|f| f.state)
    };
    assert_eq!(state(&a), Some(FileState::InSync));
    assert_eq!(state(&a), state(&open("c", 3)));
}

fn a_refused_save_under_another_spelling_reports_the_listed_file_rewritten<F: Facade>() {
    let folder = disk();
    let clock = TestClock::at(1_000);
    let probe = decomposing(&folder);
    probe.seen.borrow_mut().coarse = Some(0);
    let composing = Env {
        names: Box::new(Composing),
        ..env("a", 1, &clock)
    };
    let (mut a, _) = F::open(probe, composing).unwrap();
    a.rescan().unwrap();
    let song = create(&mut a, "caf\u{e9}.wav", b"wave");
    a.rescan().unwrap();
    // Another program rewrites it at the same length and time.
    remove_outside(&folder, "cafe\u{301}.wav");
    put(&folder, "cafe\u{301}.wav", b"wavz");
    let held = Expect::Holds(identity(b"wave"));
    let saved = a.commit("Save", |i| {
        i.save(song, &path("caf\u{e9}.wav"), b"mine".to_vec(), held)
    });
    assert!(saved.is_err(), "{saved:?}");
    let rewritten = a.refresh().unwrap().rewritten;
    assert_eq!(rewritten, [path("cafe\u{301}.wav")]);
}

fn a_save_resumed_after_another_file_landed_over_its_placed_file_names_its_own_bytes<F: Facade>() {
    let folder = disk();
    let clock = TestClock::at(1_000);
    let probe = Probe::new(&machine(&folder));
    let seen = Rc::clone(&probe.seen);
    let (mut a, _) = F::open(probe, env("a", 1, &clock)).unwrap();
    a.rescan().unwrap();
    let x = create(&mut a, "x/x.npno", b"xxxx");
    let y = create(&mut a, "y/y.npno", b"yyyy");
    // x's file goes into place, and the move of y's old file to the trash fails.
    seen.borrow_mut().failing = Some(Box::new(|io: &Io| {
        matches!(io, Io::Rename { from, .. } if *from == path("y/y.npno"))
            || matches!(io, Io::Remove { path: at, .. } if *at == path("y/y.npno"))
    }));
    let saved = a.commit("Save", |i| {
        let (xs, ys) = (
            Expect::Holds(identity(b"xxxx")),
            Expect::Holds(identity(b"yyyy")),
        );
        i.save(x, &path("x/x.npno"), b"xxx2".to_vec(), xs).save(
            y,
            &path("y/y.npno"),
            b"yyy2".to_vec(),
            ys,
        )
    });
    seen.borrow_mut().failing = None;
    assert!(saved.is_err(), "{saved:?}");
    remove_outside(&folder, "x/x.npno");
    put(&folder, "x/x.npno", b"theirs!");
    let next = a
        .commit("Origin", |i| i.set(y, ORIGIN, "o".to_string()))
        .unwrap();
    let x_wrote = next.wrote.iter().find(|fact| fact.path == path("x/x.npno"));
    let x_wrote = x_wrote.map(|fact| fact.identity);
    assert_eq!(x_wrote, Some(identity(b"xxx2")), "{:?}", next.wrote);
    assert_eq!(a.refresh().unwrap().rewritten, [path("x/x.npno")]);
}

fn a_refused_save_over_a_rewrite_shows_it_at_the_next_refresh<F: Facade>() {
    for rewrite in [Some(&b"longer!"[..]), None] {
        let folder = disk();
        let clock = TestClock::at(1_000);
        let open = |label, seed| {
            let probe = Probe::new(&machine(&folder));
            let (mut writer, _) = F::open(probe, env(label, seed, &clock)).unwrap();
            writer.rescan().unwrap();
            writer
        };
        let mut a = open("a", 1);
        let song = create(&mut a, "song.npno", b"song");
        a.rescan().unwrap();
        clock.advance(1_000);
        match rewrite {
            Some(bytes) => put(&folder, "song.npno", bytes),
            None => remove_outside(&folder, "song.npno"),
        }
        let held = Expect::Holds(identity(b"song"));
        let saved = a.commit("Save", |i| {
            i.save(song, &path("song.npno"), b"sonh".to_vec(), held)
        });
        assert!(saved.is_err(), "{saved:?}");
        a.refresh().unwrap();
        let state = |w: &F| {
            w.view()
                .entity(song)
                .and_then(|e| e.file())
                .map(|f| f.state)
        };
        let fresh = state(&open("c", 3));
        assert_eq!(state(&a), fresh, "{rewrite:?}");
    }
}

fn a_save_whose_placed_file_goes_before_it_is_seen_names_its_own_bytes<F: Facade>() {
    for directory in [true, false] {
        let folder = disk();
        let clock = TestClock::at(1_000);
        let probe = Probe::new(&machine(&folder));
        let seen = Rc::clone(&probe.seen);
        let (mut a, _) = F::open(probe, env("a", 1, &clock)).unwrap();
        a.rescan().unwrap();
        let song = create(&mut a, "song.npno", b"song");
        // Once the save's file is in place, another program removes it, and may put a
        // directory there.
        let placed = Rc::new(Cell::new(false));
        let (theirs, landed) = (folder.clone(), Rc::clone(&placed));
        let staged = |at: &RelPath| layout().owns(at) && at.as_str().ends_with(".npno");
        seen.borrow_mut().failing = Some(Box::new(move |io: &Io| {
            match io {
                Io::Rename { to, .. } if *to == path("song.npno") => landed.set(true),
                Io::Remove { path: at, .. } if staged(at) => landed.set(true),
                Io::Stat { path: at, .. } if landed.get() && *at == path("song.npno") => {
                    landed.set(false);
                    remove_outside(&theirs, "song.npno");
                    if directory {
                        let dir = Io::MakeDir {
                            root: Root::Folder,
                            path: path("song.npno"),
                        };
                        theirs.perform(dir).unwrap();
                    }
                }
                _ => {}
            }
            false
        }));
        let held = Expect::Holds(identity(b"song"));
        let committed = a
            .commit("Save", |i| {
                i.save(song, &path("song.npno"), b"sonh".to_vec(), held)
            })
            .unwrap();
        seen.borrow_mut().failing = None;
        let wrote: Vec<Identity> = committed.wrote.iter().map(|fact| fact.identity).collect();
        assert_eq!(wrote, [identity(b"sonh")], "directory: {directory}");
        assert_eq!(a.refresh().unwrap().rewritten, [path("song.npno")]);
        if directory {
            // Nothing is left to finish, so the next change goes ahead.
            create(&mut a, "other.npno", b"other");
            continue;
        }
        // The path is free: undoing the save puts the old bytes back, or is refused and
        // stays.
        match a.undo() {
            Ok(_) => assert_eq!(read(&folder, "song.npno").as_deref(), Some(&b"song"[..])),
            Err(refused) => {
                assert!(matches!(refused, Error::Refused(_)), "{refused:?}");
                let labels: Vec<String> = a.history().iter().map(|i| i.label.clone()).collect();
                assert!(labels.contains(&"Save".to_string()), "{labels:?}");
            }
        }
    }
}

fn a_save_within_the_time_of_an_identity_read_stays_in_sync_after_reopening<F: Facade>() {
    let folder = disk();
    let clock = TestClock::at(1_000);
    let here = machine(&folder);
    let probe = Probe::new(&here);
    let (mut a, _) = F::open(probe.clone(), env("a", 1, &clock)).unwrap();
    let song = create(&mut a, "song.npno", b"one");
    put(&folder, "song.npno", b"one");
    let touched = match folder.perform(Io::Stat {
        root: Root::Folder,
        path: path("song.npno"),
    }) {
        Ok(Reply::Stat(Some(meta))) => meta.modified,
        other => panic!("{other:?}"),
    };
    probe.seen.borrow_mut().coarse = touched;
    a.rescan().unwrap();
    assert!(
        probe.read_files().contains(&path("song.npno")),
        "a rescan reads the identity of a file at its fact's length and another time"
    );
    fail(
        &probe,
        local_writes(&["identities.json", "identities.json.next"]),
    );
    a.commit("Save", |i| {
        i.save(
            song,
            &path("song.npno"),
            b"two".to_vec(),
            Expect::Holds(identity(b"one")),
        )
    })
    .unwrap();
    let state = |a: &F| a.view().entity(song).unwrap().file().unwrap().state;
    assert_eq!(state(&a), FileState::InSync, "saved");
    a.close().unwrap();
    let probe = Probe::new(&here);
    probe.seen.borrow_mut().coarse = touched;
    let (mut a, _) = F::open(probe, env("a", 1, &clock)).unwrap();
    a.rescan().unwrap();
    assert_eq!(state(&a), FileState::InSync, "reopened");
}

fn undoing_a_save_restores_the_displaced_bytes<F: Facade>() {
    let folder = disk();
    let clock = TestClock::at(1_000);
    let (mut a, _) = F::open(Probe::new(&machine(&folder)), env("a", 1, &clock)).unwrap();
    let song = create(&mut a, "song.npno", b"one");
    a.commit("Save", |i| {
        i.save(
            song,
            &path("song.npno"),
            b"two".to_vec(),
            Expect::Holds(identity(b"one")),
        )
    })
    .unwrap();
    assert_eq!(read(&folder, "song.npno").unwrap(), b"two");
    assert_eq!(
        a.history()
            .iter()
            .map(|h| h.label.as_str())
            .collect::<Vec<_>>(),
        ["Import", "Save"]
    );

    a.undo().unwrap();
    assert_eq!(read(&folder, "song.npno").unwrap(), b"one");
    assert!(a.history()[1].undone);
    a.redo().unwrap();
    assert_eq!(read(&folder, "song.npno").unwrap(), b"two");
    assert!(!a.history()[1].undone);

    let (mut b, _) = F::open(Probe::new(&machine(&folder)), env("b", 2, &clock)).unwrap();
    b.commit("Save", |i| {
        i.save(
            song,
            &path("song.npno"),
            b"three".to_vec(),
            Expect::Holds(identity(b"two")),
        )
    })
    .unwrap();
    a.refresh().unwrap();
    assert!(
        matches!(a.undo(), Err(Error::Refused(Refusal::ChangedSince { .. }))),
        "another writer saved since"
    );
    assert_eq!(read(&folder, "song.npno").unwrap(), b"three");
}

const NOTES: &str = "Set/notes.txt";

/// Two writers, `a` and `b`, that each made an entity of [`NOTES`] before seeing the
/// other's: `a` tagged it `Live` and `b` `Studio`, each with its origin. Returns them
/// with the entities they made.
fn adopted_by_both<F: Facade>(
    folder: &MemDisk,
    clock: &TestClock,
    origins: [Option<&str>; 2],
) -> (F, F, [EntityId; 2]) {
    put(folder, NOTES, b"notes");
    let open = |label, seed| {
        let (mut writer, _) =
            F::open(Probe::new(&machine(folder)), env(label, seed, clock)).unwrap();
        writer.rescan().unwrap();
        writer
    };
    let (mut a, mut b) = (open("a", 1), open("b", 2));
    fn adopt<F: Facade>(writer: &mut F, name: &str, origin: Option<&str>) -> EntityId {
        let committed = writer.commit("Tag", |i| {
            let (i, _) = i.create(|e| {
                e.adopt(&path(NOTES), Expect::Holds(identity(b"notes")))
                    .add(TAGS, tag(name));
                if let Some(origin) = origin {
                    e.set(ORIGIN, origin.to_owned());
                }
            });
            i
        });
        committed.unwrap().created[0]
    }
    let made = [
        adopt(&mut a, "Live", origins[0]),
        adopt(&mut b, "Studio", origins[1]),
    ];
    (a, b, made)
}

fn a_file_two_writers_adopted_unseen_is_the_lower_entitys<F: Facade>() {
    let folder = disk();
    let clock = TestClock::at(1_000);
    let (mut a, mut b, made) = adopted_by_both::<F>(&folder, &clock, [None, None]);
    let (low, high) = (made[0].min(made[1]), made[0].max(made[1]));
    let joined = Joined {
        entity: high,
        into: low,
    };
    for writer in [&mut a, &mut b] {
        let scan = writer.refresh().unwrap().scan;
        assert_eq!(
            (scan.joined, scan.departed, scan.ambiguous),
            (vec![joined], vec![], vec![])
        );
        let view = writer.view();
        assert_eq!(view.at(&path(NOTES)).map(|e| e.id()), Some(low));
        assert_eq!(view.joined(), [joined]);
        let high = view.entity(high).unwrap();
        assert_eq!(high.file(), None, "neither bound nor missing");
        assert_eq!(high.members(TAGS).values.len(), 1, "its tag is kept");
    }
}

fn an_outside_edit_of_a_file_two_writers_adopted_gives_it_to_the_lower_entity<F: Facade>() {
    let folder = disk();
    let clock = TestClock::at(1_000);
    let (mut a, _b, made) = adopted_by_both::<F>(&folder, &clock, [None, None]);
    let (low, high) = (made[0].min(made[1]), made[0].max(made[1]));
    put(&folder, NOTES, b"changed");
    let scan = a.rescan().unwrap().scan;
    assert_eq!((scan.changed, scan.joined), (vec![low], vec![]));
    let view = a.view();
    assert_eq!(view.entity(high).and_then(|e| e.file()), None);
    let file = view.entity(low).and_then(|e| e.file());
    assert_eq!(
        file.map(|file| (file.path, file.state)),
        Some((path(NOTES), FileState::ChangedOutside))
    );
    let rows: Vec<&Row> = view.rows().filter(|row| row.path == path(NOTES)).collect();
    let [row] = rows[..] else {
        panic!("one file, one row: {rows:?}");
    };
    assert_eq!(row.entity, Some(low));
    let row = row.file;
    let held = Expect::Holds(identity(b"changed"));
    a.commit("Save", |i| {
        i.save(low, &path(NOTES), b"saved".to_vec(), held)
    })
    .unwrap();
    let saved = a.view().row_at(&path(NOTES)).cloned();
    assert_eq!(
        saved.map(|saved| (saved.file, saved.entity)),
        Some((row, Some(low)))
    );
}

fn an_outside_delete_of_a_file_two_writers_adopted_leaves_one_missing_row<F: Facade>() {
    let folder = disk();
    let clock = TestClock::at(1_000);
    let (mut a, _b, made) = adopted_by_both::<F>(&folder, &clock, [None, None]);
    let (low, high) = (made[0].min(made[1]), made[0].max(made[1]));
    remove_outside(&folder, NOTES);
    let scan = a.rescan().unwrap().scan;
    assert_eq!((scan.departed, scan.joined), (vec![low], vec![]));
    let view = a.view();
    assert_eq!(view.entity(high).and_then(|e| e.file()), None);
    let rows: Vec<&Row> = view.rows().filter(|row| row.path == path(NOTES)).collect();
    let [row] = rows[..] else {
        panic!("one file, one row: {rows:?}");
    };
    assert_eq!((row.state, row.entity), (RowState::Missing, Some(low)));
}

fn a_save_of_a_file_two_writers_adopted_and_edited_outside_is_why_the_other_is_lost<F: Facade>() {
    let folder = disk();
    let clock = TestClock::at(1_000);
    let (mut a, _b, made) = adopted_by_both::<F>(&folder, &clock, [None, None]);
    let (low, high) = (made[0].min(made[1]), made[0].max(made[1]));
    put(&folder, NOTES, b"changed");
    a.rescan().unwrap();
    let before = a.view();
    let held = Expect::Holds(identity(b"changed"));
    a.commit("Save", |i| {
        i.save(low, &path(NOTES), b"saved".to_vec(), held)
    })
    .unwrap();
    a.rescan().unwrap();
    let view = a.view();
    let lost = row_of(&view, high);
    assert_eq!((lost.state, &lost.path), (RowState::Missing, &path(NOTES)));
    assert_eq!(
        changes_of(&view.changes_since(&before), lost.file),
        [(RowWhat::Appeared, Some(Who::This), Some(How::Saved))]
    );
}

fn a_copy_seen_before_its_originals_rename_is_never_joined_into_it<F: Facade>() {
    for left_at_p in [None, Some(&b"edited"[..])] {
        let (theirs, ours) = (disk(), disk());
        let clock = TestClock::at(1_000);
        let deliver = |path: &RelPath, bytes: &[u8]| {
            let _ = ours.perform(Io::Remove {
                root: Root::Folder,
                path: path.clone(),
            });
            put(&ours, path.as_str(), bytes);
        };
        let (mut x, _) = F::open(Probe::new(&machine(&theirs)), env("x", 1, &clock)).unwrap();
        let song = create(&mut x, "p.npno", b"song");
        for (path, bytes) in theirs.files(Root::Folder) {
            deliver(&path, &bytes);
        }
        let (mut z, _) = F::open(Probe::new(&machine(&ours)), env("z", 3, &clock)).unwrap();
        z.rescan().unwrap();
        let before: BTreeSet<RelPath> = theirs.files(Root::Folder).into_keys().collect();
        clock.advance(1_000);
        let held = Expect::Holds(identity(b"song"));
        x.commit("Rename", |i| i.rename(song, &path("q.npno"), held))
            .unwrap();
        let (mut y, _) = F::open(Probe::new(&machine(&theirs)), env("y", 2, &clock)).unwrap();
        y.rescan().unwrap();
        clock.advance(1_000);
        let copy = create(&mut y, "p.npno", b"song");
        // Sync brings y's log and x's move of the file to z, but not yet x's log.
        for (path, bytes) in theirs.files(Root::Folder) {
            if path.as_str().starts_with('.') && !before.contains(&path) {
                deliver(&path, &bytes);
            }
        }
        remove_outside(&ours, "p.npno");
        if let Some(bytes) = left_at_p {
            put(&ours, "p.npno", bytes);
        }
        put(&ours, "q.npno", b"song");

        let scan = z.rescan().unwrap().scan;
        assert_eq!(
            scan.joined,
            [],
            "z's facts say p.npno for both, {left_at_p:?} there"
        );
        create(&mut z, "other.npno", b"other");
        for (path, bytes) in theirs.files(Root::Folder) {
            deliver(&path, &bytes);
        }
        z.rescan().unwrap();
        let view = z.view();
        let at = |at: &str| view.row_at(&path(at)).and_then(|row| row.entity);
        let rows = (at("q.npno"), at("p.npno"));
        assert_eq!(rows, (Some(song), Some(copy)), "{left_at_p:?} at p.npno");
    }
}

fn a_join_gives_the_lower_entity_what_both_held_and_no_undo_takes_it_back<F: Facade>() {
    let folder = disk();
    let clock = TestClock::at(1_000);
    let (mut a, mut b, made) = adopted_by_both::<F>(&folder, &clock, [Some("a"), Some("b")]);
    a.refresh().unwrap();
    let [joined] = a.view().joined().try_into().unwrap();
    let history = a.history();
    let committed = a
        .commit("Join", |i| i.join(joined.entity, joined.into))
        .unwrap();
    assert!(!committed.undoable);
    assert_eq!(a.history(), history, "no step of undo");
    b.refresh().unwrap();
    let origin = match joined.into == made[0] {
        true => "a",
        false => "b",
    };
    for writer in [&a, &b] {
        let view = writer.view();
        let into = view.at(&path(NOTES)).unwrap();
        assert_eq!(into.id(), joined.into);
        assert_eq!(into.members(TAGS).values, ["Live", "Studio"]);
        assert_eq!(into.get(ORIGIN), Field::Value(origin.to_owned()), "its own");
        assert!(view.entity(joined.entity).is_none());
        assert_eq!((view.joined(), view.conflicts()), (&[][..], vec![]));
    }
    for writer in [&mut a, &mut b] {
        let undone = writer.undo();
        assert!(
            matches!(undone, Err(Error::Refused(Refusal::ChangedSince { .. }))),
            "{undone:?}"
        );
        let view = writer.view();
        let into = view.at(&path(NOTES)).unwrap();
        assert_eq!(into.members(TAGS).values, ["Live", "Studio"]);
    }
}

fn writers_joining_one_entity_at_once_converge<F: Facade>() {
    let folder = disk();
    let clock = TestClock::at(1_000);
    let (mut a, mut b, _) = adopted_by_both::<F>(&folder, &clock, [Some("a"), None]);
    a.refresh().unwrap();
    b.refresh().unwrap();
    for writer in [&mut a, &mut b] {
        let [joined] = writer.view().joined().try_into().unwrap();
        writer
            .commit("Join", |i| i.join(joined.entity, joined.into))
            .unwrap();
    }
    a.refresh().unwrap();
    b.refresh().unwrap();
    assert_eq!(facts(&a.view()), facts(&b.view()));
    let view = a.view();
    let [(_, (origin, tags, file))] = facts(&view)
        .into_iter()
        .collect::<Vec<_>>()
        .try_into()
        .unwrap();
    assert_eq!(
        (origin.as_deref(), &tags[..], file),
        (
            Some("a"),
            &["Live".to_owned(), "Studio".to_owned()][..],
            Some((path(NOTES), FileState::InSync))
        )
    );
    assert_eq!((view.joined(), view.conflicts()), (&[][..], vec![]));
}

fn a_copy_has_no_entity_until_one_is_said<F: Facade>() {
    let folder = disk();
    let clock = TestClock::at(1_000);
    let (mut a, _) = F::open(Probe::new(&machine(&folder)), env("a", 1, &clock)).unwrap();
    let song = create(&mut a, "song.npno", b"song");
    put(&folder, "copy.npno", b"song");
    a.rescan().unwrap();
    let view = a.view();
    assert_eq!(
        view.unbound().cloned().collect::<Vec<_>>(),
        [path("copy.npno")]
    );
    assert_eq!(view.entities().len(), 1);
    assert_eq!(
        view.entity(song).unwrap().file().unwrap().path,
        path("song.npno")
    );
    a.close().unwrap();

    let probe = Probe::new(&machine(&folder));
    let (mut a, _) = F::open(probe.clone(), env("a", 2, &clock)).unwrap();
    let before = probe.file_reads();
    a.rescan().unwrap();
    assert_eq!(
        probe.file_reads() - before,
        0,
        "a file of the length of one in place is not read"
    );
    assert_eq!(
        a.view().unbound().cloned().collect::<Vec<_>>(),
        [path("copy.npno")]
    );
    let committed = a
        .commit("Tag", |i| {
            let (i, _) = i.create(|e| {
                e.adopt(&path("copy.npno"), Expect::Holds(identity(b"song")))
                    .add(TAGS, tag("copy"));
            });
            i
        })
        .unwrap();
    let copy = committed.created[0];
    let view = a.view();
    assert_eq!(
        view.unbound().cloned().collect::<Vec<_>>(),
        [] as [RelPath; 0]
    );
    assert_eq!(tags(&view, copy), ["copy"]);
    assert_eq!(
        view.entity(copy).unwrap().file().unwrap().path,
        path("copy.npno")
    );
    a.undo().unwrap();
    assert_eq!(
        read(&folder, "copy.npno").unwrap(),
        b"song",
        "undoing an adoption leaves the file"
    );
    assert_eq!(
        a.view().unbound().cloned().collect::<Vec<_>>(),
        [path("copy.npno")]
    );
}

fn a_move_keeps_its_tags_and_is_pinned_by_the_next_commit<F: Facade>() {
    let folder = disk();
    let clock = TestClock::at(1_000);
    let (mut a, _) = F::open(Probe::new(&machine(&folder)), env("a", 1, &clock)).unwrap();
    let song = create(&mut a, "song.npno", b"song");
    folder
        .perform(Io::MakeDir {
            root: Root::Folder,
            path: path("moved"),
        })
        .unwrap();
    move_outside(&folder, "song.npno", "moved/song.npno");
    let changes = a.rescan().unwrap().changes;
    assert_eq!(
        changes,
        [Change {
            entity: song,
            what: What::File,
            by: By::Outside
        }]
    );
    let file = a.view().entity(song).unwrap().file().unwrap();
    assert_eq!(
        (file.path, file.state),
        (path("moved/song.npno"), FileState::InSync)
    );
    assert_eq!(tags(&a.view(), song), ["new"]);
    let committed = a
        .commit("Tag", |i| i.add(song, TAGS, tag("moved")))
        .unwrap();
    assert!(committed.changes.contains(&Change {
        entity: song,
        what: What::File,
        by: By::This
    }));
    put(&folder, "song.npno", b"song");
    let (mut b, _) = F::open(Probe::new(&machine(&folder)), env("b", 2, &clock)).unwrap();
    b.rescan().unwrap();
    let file = b.view().entity(song).unwrap().file().unwrap();
    assert_eq!(
        file.path,
        path("moved/song.npno"),
        "the pinned path wins over a copy at the old one"
    );
    assert_eq!(
        b.view().unbound().cloned().collect::<Vec<_>>(),
        [path("song.npno")]
    );
}

fn undoing_a_delete_binds_the_file_moved_while_deleted<F: Facade>() {
    let folder = disk();
    let clock = TestClock::at(1_000);
    let (mut a, _) = F::open(Probe::new(&machine(&folder)), env("a", 1, &clock)).unwrap();
    let song = create(&mut a, "song.npno", b"song");
    a.commit("Delete", |i| i.delete(song)).unwrap();
    folder
        .perform(Io::MakeDir {
            root: Root::Folder,
            path: path("moved"),
        })
        .unwrap();
    move_outside(&folder, "song.npno", "moved/song.npno");
    a.rescan().unwrap();
    a.undo().unwrap();
    let file = a.view().entity(song).unwrap().file().unwrap();
    assert_eq!(
        (file.path, file.state),
        (path("moved/song.npno"), FileState::InSync)
    );
    let committed = a.commit("Tag", |i| i.add(song, TAGS, tag("back"))).unwrap();
    assert!(
        committed.changes.contains(&Change {
            entity: song,
            what: What::File,
            by: By::This
        }),
        "{:?}",
        committed.changes
    );
}

/// Opens a library holding `song.npno` as an entity, then moves it to
/// `moved/song.npno` outside the app, which the rescan finds.
fn moved_outside<F: Facade>(folder: &MemDisk, clock: &TestClock) -> (F, EntityId) {
    let (mut a, _) = F::open(Probe::new(&machine(folder)), env("a", 1, clock)).unwrap();
    let song = create(&mut a, "song.npno", b"song");
    folder
        .perform(Io::MakeDir {
            root: Root::Folder,
            path: path("moved"),
        })
        .unwrap();
    move_outside(folder, "song.npno", "moved/song.npno");
    a.rescan().unwrap();
    (a, song)
}

fn undoing_a_delete_puts_a_file_moved_outside_back_where_it_was<F: Facade>() {
    let folder = disk();
    let clock = TestClock::at(1_000);
    let (mut a, song) = moved_outside::<F>(&folder, &clock);
    a.commit("Delete", |i| {
        i.trash(song, Expect::Holds(identity(b"song"))).delete(song)
    })
    .unwrap();
    assert_eq!(read(&folder, "moved/song.npno"), None);
    a.undo().unwrap();
    assert_eq!(
        read(&folder, "moved/song.npno").as_deref(),
        Some(&b"song"[..])
    );
    let file = a.view().entity(song).unwrap().file().unwrap();
    assert_eq!(
        (file.path, file.state),
        (path("moved/song.npno"), FileState::InSync)
    );
    a.redo().unwrap();
    assert_eq!(read(&folder, "moved/song.npno"), None);
    assert!(a.view().entity(song).is_none());
}

fn undoing_a_delete_puts_a_file_back_under_the_spelling_it_was_found_by<F: Facade>() {
    let folder = disk();
    let clock = TestClock::at(1_000);
    let folding = Env {
        names: Box::new(Folding),
        ..env("a", 1, &clock)
    };
    let (mut a, _) = F::open(Probe::new(&machine(&folder)), folding).unwrap();
    let song = create(&mut a, "Song.npno", b"song");
    move_outside(&folder, "Song.npno", "song.npno");
    a.rescan().unwrap();
    a.commit("Delete", |i| {
        i.trash(song, Expect::Holds(identity(b"song"))).delete(song)
    })
    .unwrap();
    a.undo().unwrap();
    let files: Vec<RelPath> = library_files(&folder).into_keys().collect();
    assert_eq!(files, [path("song.npno")]);
}

fn undoing_a_rename_puts_a_file_moved_outside_back_where_it_was<F: Facade>() {
    let folder = disk();
    let clock = TestClock::at(1_000);
    let (mut a, song) = moved_outside::<F>(&folder, &clock);
    a.commit("Rename", |i| {
        i.rename(
            song,
            &path("moved/renamed.npno"),
            Expect::Holds(identity(b"song")),
        )
    })
    .unwrap();
    a.undo().unwrap();
    assert_eq!(
        read(&folder, "moved/song.npno").as_deref(),
        Some(&b"song"[..])
    );
    assert_eq!(read(&folder, "song.npno"), None);
    let file = a.view().entity(song).unwrap().file().unwrap();
    assert_eq!(
        (file.path, file.state),
        (path("moved/song.npno"), FileState::InSync)
    );
}

fn a_file_written_holding_a_missing_entitys_file_is_named_by_that_entitys_row<F: Facade>() {
    let folder = disk();
    let clock = TestClock::at(1_000);
    let (mut a, _) = F::open(Probe::new(&machine(&folder)), env("a", 1, &clock)).unwrap();
    create(&mut a, "song.npno", b"song");
    let removed = Io::Remove {
        root: Root::Folder,
        path: path("song.npno"),
    };
    folder.perform(removed).unwrap();
    a.rescan().unwrap();
    let missing = a.view().row_at(&path("song.npno")).unwrap().file;

    let saved = a.commit("Save", |i| {
        i.save_file(&path("found.npno"), b"song".to_vec(), Expect::Absent)
    });
    let files = saved.unwrap().files;
    assert_eq!(files, [(path("found.npno"), missing)], "the entity's row");
    assert_eq!(a.view().row_at(&path("found.npno")).unwrap().file, missing);
}

fn a_new_entitys_file_at_a_missing_entitys_path_is_named_by_its_own_row<F: Facade>() {
    let folder = disk();
    let clock = TestClock::at(1_000);
    let (mut a, _) = F::open(Probe::new(&machine(&folder)), env("a", 1, &clock)).unwrap();
    let old = create(&mut a, "song.npno", b"song");
    remove_outside(&folder, "song.npno");
    a.rescan().unwrap();
    let missing = row_of(&a.view(), old).file;

    let committed = a
        .commit("Import", |intent| {
            let (intent, _) = intent.create(|e| {
                e.save(&path("song.npno"), b"another".to_vec(), Expect::Absent)
                    .add(TAGS, tag("new"));
            });
            intent
        })
        .unwrap();
    let new = committed.created[0];
    let view = a.view();
    let [(at, named)] = &committed.files[..] else {
        panic!("{:?}", committed.files);
    };
    let named = *named;
    assert_eq!(*at, path("song.npno"));
    assert_eq!(view.row(named).and_then(|row| row.entity), Some(new));
    assert_ne!(named, missing, "the missing entity's row is its own");
    let old = row_of(&view, old);
    assert_eq!((old.file, old.state), (missing, RowState::Missing));
}

fn undoing_a_rename_whose_file_was_moved_back_outside_leaves_it_there<F: Facade>() {
    let folder = disk();
    let clock = TestClock::at(1_000);
    let (mut a, _) = F::open(Probe::new(&machine(&folder)), env("a", 1, &clock)).unwrap();
    let song = create(&mut a, "song.npno", b"song");
    let held = Expect::Holds(identity(b"song"));
    a.commit("Rename", |i| i.rename(song, &path("renamed.npno"), held))
        .unwrap();
    move_outside(&folder, "renamed.npno", "song.npno");
    a.rescan().unwrap();

    a.undo().unwrap();
    assert_eq!(read(&folder, "song.npno").as_deref(), Some(&b"song"[..]));
    let file = a.view().entity(song).unwrap().file().unwrap();
    assert_eq!(
        (file.path, file.state),
        (path("song.npno"), FileState::InSync)
    );
}

/// Makes reads of library paths fail once `probe` next records the head, after
/// the entries are durable.
fn blind_after_record(probe: &Probe) {
    let mut seen = probe.seen.borrow_mut();
    (seen.blind_after_record, seen.blind) = (true, false);
}

fn see(probe: &Probe) {
    let mut seen = probe.seen.borrow_mut();
    (seen.blind_after_record, seen.blind) = (false, false);
}

fn an_undo_is_committed_when_its_identity_read_fails<F: Facade>() {
    let folder = disk();
    let clock = TestClock::at(1_000);
    let probe = Probe::new(&machine(&folder));
    let (mut a, _) = F::open(probe.clone(), env("a", 1, &clock)).unwrap();
    let song = create(&mut a, "song.npno", b"song");
    a.commit("Delete", |i| i.delete(song)).unwrap();
    folder
        .perform(Io::MakeDir {
            root: Root::Folder,
            path: path("moved"),
        })
        .unwrap();
    move_outside(&folder, "song.npno", "moved/song.npno");
    a.rescan().unwrap();
    blind_after_record(&probe);
    a.undo().unwrap();
    assert!(
        probe.seen.borrow().blind,
        "the identity read came after the append"
    );
    see(&probe);
    assert!(a.view().entity(song).is_some(), "the undo is shown");
    let redone = a.redo().unwrap();
    assert!(
        redone.changes.iter().any(|change| change.entity == song),
        "redo redoes the delete the undo reversed: {:?}",
        redone.changes
    );
    a.undo().unwrap();
    let file = a.view().entity(song).unwrap().file().unwrap();
    assert_eq!(
        (file.path, file.state),
        (path("moved/song.npno"), FileState::InSync)
    );
}

fn a_rename_is_committed_when_its_rescan_fails_and_binds_nothing_stale<F: Facade>() {
    let folder = disk();
    let clock = TestClock::at(1_000);
    let probe = Probe::new(&machine(&folder));
    let (mut a, _) = F::open(probe.clone(), env("a", 1, &clock)).unwrap();
    let song = create(&mut a, "song.npno", b"song");
    blind_after_record(&probe);
    a.commit("Rename", |i| {
        i.rename(song, &path("b.npno"), Expect::Holds(identity(b"song")))
    })
    .unwrap();
    assert!(
        probe.seen.borrow().blind,
        "the rescan came after the append"
    );
    see(&probe);
    assert_ne!(
        a.view().entity(song).unwrap().file().unwrap().path,
        path("song.npno"),
        "the file the rename moved is not bound where it was"
    );
    let committed = a.commit("Tag", |i| i.add(song, TAGS, tag("x"))).unwrap();
    assert!(
        committed
            .changes
            .iter()
            .all(|change| change.what != What::File),
        "no stale move is pinned: {:?}",
        committed.changes
    );
    a.refresh().unwrap();
    let file = a.view().entity(song).unwrap().file().unwrap();
    assert_eq!((file.path, file.state), (path("b.npno"), FileState::InSync));
}

fn a_move_read_by_a_rescan_that_failed_is_not_pinned_back_by_the_next_commit<F: Facade>() {
    let folder = disk();
    let clock = TestClock::at(1_000);
    let probe = Probe::new(&machine(&folder));
    let (mut a, _) = F::open(probe.clone(), env("a", 1, &clock)).unwrap();
    let song = create(&mut a, "song.npno", b"song");
    create(&mut a, "tune.npno", b"tune");
    a.rescan().unwrap();
    let (mut b, _) = F::open(Probe::new(&machine(&folder)), env("b", 2, &clock)).unwrap();
    b.rescan().unwrap();
    let moved = Expect::Holds(identity(b"song"));
    b.commit("Move", |i| i.rename(song, &path("shelf/song.npno"), moved))
        .unwrap();
    move_outside(&folder, "tune.npno", "tune2.npno");
    fail(&probe, |io| {
        matches!(io, Io::ReadMany { .. }) && !layout().owns(io.path())
    });
    assert!(a.rescan().is_err(), "the moved tune cannot be identified");
    heal(&probe);
    let committed = a.commit("Tag", |i| i.add(song, TAGS, tag("x"))).unwrap();
    assert!(
        !committed
            .changes
            .iter()
            .any(|change| change.what == What::File),
        "no move back is pinned: {:?}",
        committed.changes
    );
    a.refresh().unwrap();
    let file = a.view().entity(song).unwrap().file().unwrap();
    assert_eq!(
        (file.path, file.state),
        (path("shelf/song.npno"), FileState::InSync)
    );
    let (c, _) = F::open(Probe::new(&machine(&folder)), env("c", 3, &clock)).unwrap();
    let file = c.view().entity(song).unwrap().file().unwrap();
    assert_eq!(file.path, path("shelf/song.npno"), "the log holds the move");
}

fn a_settlement_is_committed_when_its_rescan_fails<F: Facade>() {
    interrupted_renames::<F>(|disk, shown| {
        let machine = Machine {
            folder: disk.clone(),
            local: disk.clone(),
        };
        let clock = TestClock::at(2_000);
        let probe = Probe::new(&machine);
        let (mut again, opened) = F::open(probe.clone(), env("a", 2, &clock)).unwrap();
        assert_eq!(opened.settled.len(), 1, "{shown}");
        let song = again.view().entities()[0].id();
        blind_after_record(&probe);
        let committed = again
            .commit("Tag", |i| i.add(song, TAGS, tag("next")))
            .unwrap_or_else(|e| panic!("{shown}: {e}"));
        assert!(
            probe.seen.borrow().blind,
            "{shown}: the rescan came after the append"
        );
        see(&probe);
        assert!(
            committed
                .changes
                .iter()
                .all(|change| change.what != What::File),
            "{shown}: no stale move is pinned: {:?}",
            committed.changes
        );
        assert_eq!(tags(&again.view(), song), ["new", "next"], "{shown}");
        again.refresh().unwrap();
        let file = again.view().entity(song).unwrap().file().unwrap();
        assert_eq!(
            (file.path, file.state),
            (path("b.npno"), FileState::InSync),
            "{shown}"
        );
    });
}

fn a_failed_rescan_after_a_rename_binds_no_copy_in_its_place<F: Facade>() {
    let folder = disk();
    let clock = TestClock::at(1_000);
    let probe = Probe::new(&machine(&folder));
    let (mut a, _) = F::open(probe.clone(), env("a", 1, &clock)).unwrap();
    let song = create(&mut a, "song.npno", b"song");
    put(&folder, "copy.npno", b"song");
    a.rescan().unwrap();
    assert_eq!(
        a.view().unbound().cloned().collect::<Vec<_>>(),
        [path("copy.npno")]
    );
    blind_after_record(&probe);
    let renamed = a
        .commit("Rename", |i| {
            i.rename(song, &path("b.npno"), Expect::Holds(identity(b"song")))
        })
        .unwrap();
    assert!(
        probe.seen.borrow().blind,
        "the rescan came after the append"
    );
    see(&probe);
    assert_eq!(behind(&renamed), [Lag::Scan]);
    let file = a.view().entity(song).unwrap().file().unwrap();
    assert_eq!(
        (file.path, file.state),
        (path("b.npno"), FileState::Unscanned),
        "the copy is not taken for the renamed file"
    );
    let tagged = a.commit("Tag", |i| i.add(song, TAGS, tag("x"))).unwrap();
    assert!(
        tagged
            .changes
            .iter()
            .all(|change| change.what != What::File),
        "nothing is pinned: {:?}",
        tagged.changes
    );
    assert_eq!(behind(&tagged), [Lag::Scan], "until a refresh scans");
    let refreshed = a.refresh().unwrap();
    assert!(
        refreshed
            .changes
            .iter()
            .all(|change| change.by != By::Outside),
        "the rename was this writer's: {:?}",
        refreshed.changes
    );
    let file = a.view().entity(song).unwrap().file().unwrap();
    assert_eq!((file.path, file.state), (path("b.npno"), FileState::InSync));
    assert_eq!(
        a.view().unbound().cloned().collect::<Vec<_>>(),
        [path("copy.npno")]
    );
    let next = a.commit("Tag", |i| i.add(song, TAGS, tag("y"))).unwrap();
    assert_eq!(next.local, Local::Current);
}

/// The parts `committed` reports behind.
fn behind(committed: &Committed) -> Vec<Lag> {
    match &committed.local {
        Local::Current => Vec::new(),
        Local::Behind(parts) => parts.keys().copied().collect(),
    }
}

/// Makes the requests `which` picks fail until [`heal`].
fn fail(probe: &Probe, which: impl Fn(&Io) -> bool + 'static) {
    probe.seen.borrow_mut().failing = Some(Box::new(which));
}

fn heal(probe: &Probe) {
    let mut seen = probe.seen.borrow_mut();
    (seen.failing, seen.unanswered, seen.unreadable) = (None, None, None);
}

fn unanswered(probe: &Probe, which: impl Fn(&Io) -> bool + 'static) {
    probe.seen.borrow_mut().unanswered = Some(Box::new(which));
}

/// Picks writes in the local root to a file named one of `names`.
fn local_writes(names: &'static [&'static str]) -> impl Fn(&Io) -> bool {
    move |io| {
        io.root() == Root::Local
            && io.mutates()
            && io.path().name().is_some_and(|name| names.contains(&name))
    }
}

/// A file of the local root named `name`, of any writer.
fn local_file(here: &Machine, name: &str) -> Option<Vec<u8>> {
    let files = here.local.files(Root::Local).into_iter();
    files
        .filter(|(path, _)| path.name() == Some(name))
        .map(|(_, bytes)| bytes)
        .next()
}

/// `a` opened on `here`, with `song` created, and its tag "x" committed while the
/// local root's files `names` cannot be written. Returns the commit.
fn committed_while_failing<F: Facade>(
    here: &Machine,
    clock: &TestClock,
    names: &'static [&'static str],
) -> (F, EntityId, Committed) {
    let probe = Probe::new(here);
    let (mut a, _) = F::open(probe.clone(), env("a", 1, clock)).unwrap();
    let song = create(&mut a, "song.npno", b"song");
    fail(&probe, local_writes(names));
    let tagged = a.commit("Tag", |i| i.add(song, TAGS, tag("x")));
    heal(&probe);
    let tagged = tagged.expect("the entries are durable");
    (a, song, tagged)
}

fn a_commit_whose_head_cannot_be_recorded_is_kept_and_continued<F: Facade>() {
    const HEAD: &[&str] = &["head.json", "head.json.next"];
    let clock = TestClock::at(1_000);
    let folder = disk();
    let here = machine(&folder);
    let (a, song, tagged) = committed_while_failing::<F>(&here, &clock, HEAD);
    assert_eq!(behind(&tagged), [Lag::Head]);
    let writer = label_of(&a.view(), "a");
    drop(a);
    let crashed = Machine {
        folder: folder.clone(),
        local: here.local.restart(),
    };
    let (mut reopened, opened) = F::open(Probe::new(&crashed), env("a", 2, &clock)).unwrap();
    assert_eq!(
        opened.start,
        Start::Resumed(writer),
        "the stale head is followed to the last entry of the chain"
    );
    assert_eq!(tags(&reopened.view(), song), ["new", "x"]);
    let next = reopened
        .commit("Tag", |i| i.add(song, TAGS, tag("y")))
        .unwrap();
    assert_eq!(next.local, Local::Current);
    assert_eq!(
        reopened.view().forks(),
        [],
        "the next entry follows the tag"
    );

    let folder = disk();
    let here = machine(&folder);
    let (mut a, _, tagged) = committed_while_failing::<F>(&here, &clock, HEAD);
    a.refresh().unwrap();
    let head = local_file(&here, "head.json").unwrap();
    assert!(
        String::from_utf8(head)
            .unwrap()
            .contains(&tagged.intent.to_string()),
        "a refresh records the head"
    );
}

fn a_refresh_while_the_head_cannot_be_recorded_shows_other_writers_changes<F: Facade>() {
    let folder = disk();
    let clock = TestClock::at(1_000);
    let probe = Probe::new(&machine(&folder));
    let (mut a, _) = F::open(probe.clone(), env("a", 1, &clock)).unwrap();
    let song = create(&mut a, "song.npno", b"song");
    fail(&probe, local_writes(&["head.json", "head.json.next"]));
    let tagged = a.commit("Tag", |i| i.add(song, TAGS, tag("x"))).unwrap();
    assert_eq!(behind(&tagged), [Lag::Head]);
    let (mut b, _) = F::open(Probe::new(&machine(&folder)), env("b", 2, &clock)).unwrap();
    b.commit("Tag", |i| i.add(song, TAGS, tag("y"))).unwrap();
    a.refresh().expect("a lagging head fails no refresh");
    assert_eq!(tags(&a.view(), song), ["new", "x", "y"]);
    a.rescan().expect("a lagging head fails no rescan");
}

/// Every file in a writer's `pending/`: records, and markers where moves copy.
fn pending_files(folder: &MemDisk) -> Vec<RelPath> {
    let files = folder.files(Root::Folder).into_keys();
    files
        .filter(|path| path.components().any(|c| c == "pending"))
        .collect()
}

/// Picks removals in a writer's `pending/`.
fn pending_removal(io: &Io) -> bool {
    matches!(io, Io::Remove { root: Root::Folder, path } if path.components().any(|c| c == "pending"))
}

fn a_commit_removes_its_record_once_its_entries_are_durable<F: Facade>() {
    let folder = disk();
    let clock = TestClock::at(1_000);
    let (mut a, _) = F::open(Probe::new(&machine(&folder)), env("a", 1, &clock)).unwrap();
    let song = create(&mut a, "song.npno", b"song");
    assert_eq!(pending_files(&folder), [], "the import's record is removed");
    let renamed = a
        .commit("Rename", |i| {
            i.rename(song, &path("b.npno"), Expect::Holds(identity(b"song")))
        })
        .unwrap();
    assert_eq!(renamed.local, Local::Current);
    assert_eq!(pending_files(&folder), [], "the rename's record is removed");
    let labels: Vec<String> = a.history().into_iter().map(|item| item.label).collect();
    assert_eq!(labels, ["Import", "Rename"]);
}

fn a_commit_whose_compaction_retires_its_writer_leaves_the_record_in_place<F: Facade>() {
    let folder = disk();
    let clock = TestClock::at(1_000);
    let here = machine(&folder);
    let (mut a, _) = F::open(Probe::new(&here), env("a", 1, &clock)).unwrap();
    let song = create(&mut a, "song.npno", b"song");
    a.close().unwrap();
    let there = here.cloned();
    let (mut original, _) = F::open(Probe::new(&here), env("a", 2, &clock)).unwrap();
    let (mut clone, _) = F::open(Probe::new(&there), env("a", 3, &clock)).unwrap();
    clone
        .commit("Tag", |i| i.add(song, TAGS, tag("there")))
        .unwrap();
    for n in 1..COMPACT_AFTER {
        original
            .commit("Tag", |i| i.add(song, TAGS, n.to_string()))
            .unwrap();
    }
    let records = || -> Vec<RelPath> {
        let files = pending_files(&folder).into_iter();
        files.filter(|at| at.as_str().ends_with(".json")).collect()
    };
    assert_eq!(records(), []);
    original
        .commit("Rename", |i| {
            i.rename(song, &path("b.npno"), Expect::Holds(identity(b"song")))
        })
        .unwrap();
    assert_eq!(
        original.view().forks().len(),
        1,
        "the compaction found the fork"
    );
    assert_eq!(records().len(), 1, "the retired writer's record stays");
}

/// Opens a library on `folder` and commits an import whose record removal fails,
/// which still succeeds; returns the library and the record left.
fn with_a_record_left<F: Facade>(folder: &MemDisk, clock: &TestClock) -> (F, Probe, RelPath) {
    let probe = Probe::new(&machine(folder));
    let (mut a, _) = F::open(probe.clone(), env("a", 1, clock)).unwrap();
    fail(&probe, pending_removal);
    create(&mut a, "one.npno", b"one");
    heal(&probe);
    let left = pending_files(folder);
    let records: Vec<&RelPath> = left
        .iter()
        .filter(|at| at.as_str().ends_with(".json"))
        .collect();
    assert_eq!(records.len(), 1, "{left:?}");
    let record = records[0].clone();
    (a, probe, record)
}

fn a_record_whose_removal_fails_is_removed_before_the_next_write<F: Facade>() {
    let folder = disk();
    let clock = TestClock::at(1_000);
    let (mut a, probe, record) = with_a_record_left::<F>(&folder, &clock);
    let song = a.view().entities()[0].id();
    fail(&probe, pending_removal);
    let refused = a.commit("Tag", |i| i.add(song, TAGS, tag("x")));
    heal(&probe);
    assert!(
        refused.is_err(),
        "no write while the record stays: {refused:?}"
    );
    assert!(pending_files(&folder).contains(&record));
    a.commit("Tag", |i| i.add(song, TAGS, tag("x"))).unwrap();
    assert_eq!(pending_files(&folder), []);
}

fn closing_while_the_head_cannot_be_recorded_lets_the_writer_go<F: Facade>() {
    let folder = disk();
    let clock = TestClock::at(1_000);
    let here = machine(&folder);
    let probe = Probe::new(&here);
    let (mut a, _) = F::open(probe.clone(), env("a", 1, &clock)).unwrap();
    let song = create(&mut a, "song.npno", b"song");
    let writer = label_of(&a.view(), "a");
    fail(&probe, local_writes(&["head.json", "head.json.next"]));
    a.commit("Tag", |i| i.add(song, TAGS, tag("x"))).unwrap();
    a.close().expect("a lagging head fails no close");
    let (again, opened) = F::open(Probe::new(&here), env("a", 2, &clock)).unwrap();
    assert!(
        matches!(
            opened.start,
            Start::Resumed(old) | Start::Rekeyed { old, .. } if old == writer
        ),
        "the next instance takes up the closed writer: {:?}",
        opened.start
    );
    assert_eq!(tags(&again.view(), song), ["new", "x"]);
}

fn closing_removes_a_record_whose_removal_failed<F: Facade>() {
    let folder = disk();
    let clock = TestClock::at(1_000);
    let (a, _, _) = with_a_record_left::<F>(&folder, &clock);
    a.close().unwrap();
    assert_eq!(pending_files(&folder), []);
}

fn an_append_whose_reply_was_lost_stands_and_the_next_write_takes_a_new_writer<F: Facade>() {
    a_lost_segment_write_stands::<F>(false);
}

fn a_new_segment_whose_reply_was_lost_stands_and_the_next_write_takes_a_new_writer<F: Facade>() {
    a_lost_segment_write_stands::<F>(true);
}

/// Loses the reply to the write of a segment's first line when `fresh`, and to an
/// append otherwise, then writes again.
fn a_lost_segment_write_stands<F: Facade>(fresh: bool) {
    let folder = disk();
    let clock = TestClock::at(1_000);
    let probe = Probe::new(&machine(&folder));
    let (mut a, _) = F::open(probe.clone(), env("a", 1, &clock)).unwrap();
    let song = create(&mut a, "song.npno", b"song");
    if fresh {
        a.close().unwrap();
        a = F::open(probe.clone(), env("a", 2, &clock)).unwrap().0;
    }
    let once = Cell::new(true);
    unanswered(&probe, move |io| {
        let segment = io.path().as_str().ends_with(SEGMENT_EXTENSION);
        let write = match fresh {
            true => matches!(io, Io::Create { .. }),
            false => matches!(io, Io::Append { .. }),
        };
        write && segment && once.replace(false)
    });
    let lost = a.commit("Tag", |i| i.add(song, TAGS, tag("landed")));
    heal(&probe);
    assert!(lost.is_err(), "the append's reply is lost: {lost:?}");
    let stopped = a.commit("Tag", |i| i.add(song, TAGS, tag("next")));
    assert!(
        matches!(
            stopped,
            Err(Error::Rekey {
                why: Rekey::Landed,
                ..
            })
        ),
        "the writer finds its write landed and stops: {stopped:?}"
    );
    a.commit("Tag", |i| i.add(song, TAGS, tag("next"))).unwrap();
    assert_eq!(a.view().forks(), [], "no fork");
    let holding = |text: &str| -> Vec<String> {
        let files = folder.files(Root::Folder).into_iter();
        let segments = files.filter(|(at, _)| at.as_str().ends_with(SEGMENT_EXTENSION));
        let holding = segments.filter(|(_, bytes)| {
            String::from_utf8_lossy(bytes)
                .matches(&format!(r#""value":"{text}""#))
                .count()
                > 0
        });
        holding
            .map(|(at, _)| at.parent().unwrap().as_str().to_owned())
            .collect()
    };
    let (landed, next) = (holding("landed"), holding("next"));
    assert_eq!(
        landed.len(),
        1,
        "the landed entry is there once: {landed:?}"
    );
    assert_eq!(next.len(), 1, "{next:?}");
    assert_ne!(landed, next, "the next entry is another writer's");
    a.refresh().unwrap();
    let mut tags = tags(&a.view(), song);
    tags.sort();
    assert_eq!(
        tags,
        ["landed", "new", "next"],
        "a refresh shows the landed entry"
    );
}

/// Picks a write of a segment's lines, whether it starts the segment or appends.
fn segment_write(io: &Io) -> bool {
    let write = matches!(io, Io::Create { .. } | Io::Append { .. });
    write && io.root() == Root::Folder && io.path().as_str().ends_with(SEGMENT_EXTENSION)
}

fn segment_sync(io: &Io) -> bool {
    let sync = matches!(io, Io::Sync { .. });
    sync && io.root() == Root::Folder && io.path().as_str().ends_with(SEGMENT_EXTENSION)
}

/// The bytes in each writer's trash in `folder`, by the writer's directory name.
fn trash_bytes(folder: &MemDisk) -> BTreeMap<String, usize> {
    let mut by_writer = BTreeMap::new();
    for (at, bytes) in folder.files(Root::Folder) {
        let Some(rest) = at.strip_prefix(&layout().writers()) else {
            continue;
        };
        if let [writer, "trash", _] = rest.split('/').collect::<Vec<_>>()[..] {
            *by_writer.entry(writer.to_owned()).or_default() += bytes.len();
        }
    }
    by_writer
}

/// Session after session, a save or a tag whose segment write loses its reply, or
/// whose sync fails, lands all the same, and the next save retires the writer
/// before it moves a file. A writer empties its trash as it retires, so the trash
/// holds only what the session's latest writer displaced, however many writers
/// retired before it.
fn writers_that_retire_over_lost_writes_leave_their_trash_empty<F: Facade>() {
    const SESSIONS: u64 = 6;
    const SAVES: usize = 3;
    const LEN: usize = 8;
    let folder = disk();
    let clock = TestClock::at(1_000);
    let here = machine(&folder);
    let (mut a, _) = F::open(Probe::new(&here), env("a", 1, &clock)).unwrap();
    let song = create(&mut a, "song.npno", &[b'0'; LEN]);
    a.close().unwrap();
    let reading = folder.clone();
    let mut saved = 0;
    let mut save = move |a: &mut F| {
        saved += 1;
        let was = read(&reading, "song.npno").unwrap();
        a.commit("Save", |i| {
            i.save(
                song,
                &path("song.npno"),
                format!("{saved:0LEN$}").into_bytes(),
                Expect::Holds(identity(&was)),
            )
        })
    };
    let mut retired = BTreeSet::new();
    for session in 0..SESSIONS {
        let probe = Probe::new(&here);
        let (mut a, _) = F::open(probe.clone(), env("a", 10 + session, &clock)).unwrap();
        for _ in 0..SAVES {
            save(&mut a).unwrap();
        }
        match session % 2 == 1 && FOLDER.get().fsync {
            true => fail(&probe, segment_sync),
            false => unanswered(&probe, segment_write),
        }
        let lost = match session / 2 % 2 {
            0 => save(&mut a),
            _ => a.commit("Tag", |i| i.add(song, TAGS, session.to_string())),
        };
        heal(&probe);
        assert!(lost.is_err(), "session {session}: {lost:?}");
        match save(&mut a) {
            Err(Error::Rekey {
                writer,
                why: Rekey::Landed,
            }) => retired.insert(writer.to_string()),
            stopped => panic!("session {session}: {stopped:?}"),
        };
        for _ in 0..SAVES {
            save(&mut a).unwrap();
        }
        let held = trash_bytes(&folder);
        let stranded: Vec<_> = held
            .iter()
            .filter(|(writer, bytes)| retired.contains(*writer) && **bytes > 0)
            .collect();
        assert_eq!(stranded, [] as [(&String, &usize); 0], "session {session}");
        let total: usize = held.values().sum();
        assert!(
            total <= SAVES * LEN,
            "session {session}: the trash holds {total} B: {held:?}"
        );
        let listed = a.trash().unwrap();
        let this = a.others().unwrap();
        let this = this.iter().find(|w| w.here == Presence::This).unwrap();
        assert!(
            listed.iter().all(|item| item.writer == this.writer),
            "session {session}: {listed:?}"
        );
        a.empty_trash(BTreeSet::new()).unwrap();
        a.close().unwrap();
    }
    assert_eq!(retired.len(), SESSIONS as usize);
}

/// Where the folder never makes a directory durable, the writer whose new segment
/// could not be synced retires, and the writer that takes over cannot be made
/// durable either: the library turns read-only rather than start a writer at every
/// commit.
fn a_folder_that_never_syncs_a_directory_turns_read_only_after_one_new_writer<F: Facade>() {
    if !FOLDER.get().fsync {
        return;
    }
    let folder = disk();
    let clock = TestClock::at(1_000);
    let here = machine(&folder);
    let (mut a, _) = F::open(Probe::new(&here), env("a", 1, &clock)).unwrap();
    let song = create(&mut a, "song.npno", b"song");
    a.close().unwrap();
    let writers = || {
        let dirs = folder.directories(Root::Folder).into_iter();
        dirs.filter(|dir| dir.parent() == Some(layout().writers()))
            .count()
    };
    let probe = Probe::new(&here);
    let (mut a, _) = F::open(probe.clone(), env("a", 2, &clock)).unwrap();
    let dirs = folder.clone();
    fail(&probe, move |io| {
        let directory = dirs.directories(Root::Folder).contains(io.path());
        matches!(io, Io::Sync { .. }) && io.root() == Root::Folder && directory
    });
    let mut results = Vec::new();
    for n in 0..8 {
        results.push(a.commit("Tag", |i| i.add(song, TAGS, n.to_string())));
    }
    assert_eq!(writers(), 2, "one new writer: {results:?}");
    assert!(
        matches!(results.last(), Some(Err(Error::ReadOnly(Why::NotDurable)))),
        "{results:?}"
    );
}

/// A folder that never syncs a directory from the first write: a writer whose
/// making failed may leave its directory, so the next failure turns the library
/// read-only rather than leave another at every commit.
fn a_new_library_whose_folder_never_syncs_a_directory_leaves_at_most_two_writers<F: Facade>() {
    if !FOLDER.get().fsync {
        return;
    }
    let folder = disk();
    let clock = TestClock::at(1_000);
    let here = machine(&folder);
    let probe = Probe::new(&here);
    let dirs = folder.clone();
    fail(&probe, move |io| {
        let directory = dirs.directories(Root::Folder).contains(io.path());
        matches!(io, Io::Sync { .. }) && io.root() == Root::Folder && directory
    });
    let (mut a, _) = F::open(probe, env("a", 1, &clock)).unwrap();
    let mut results = Vec::new();
    for n in 0..8 {
        results.push(a.commit("Make", |i| {
            let (i, _) = i.create(|e| {
                e.add(TAGS, n.to_string());
            });
            i
        }));
    }
    let dirs = folder.directories(Root::Folder).into_iter();
    let writers = dirs.filter(|dir| dir.parent() == Some(layout().writers()));
    assert!(writers.count() <= 2, "{results:?}");
    assert!(
        matches!(results.last(), Some(Err(Error::ReadOnly(Why::NotDurable)))),
        "{results:?}"
    );
}

fn a_full_folder_after_a_writer_stopped_is_said_as_full<F: Facade>() {
    if !FOLDER.get().fsync {
        return;
    }
    let folder = disk();
    let clock = TestClock::at(1_000);
    let here = machine(&folder);
    let (mut a, _) = F::open(Probe::new(&here), env("a", 1, &clock)).unwrap();
    let song = create(&mut a, "song.npno", b"song");
    a.close().unwrap();
    let probe = Probe::new(&here);
    let (mut a, _) = F::open(probe.clone(), env("a", 2, &clock)).unwrap();
    let dirs = folder.clone();
    let once = Cell::new(true);
    fail(&probe, move |io| {
        let directory = dirs.directories(Root::Folder).contains(io.path());
        let sync = matches!(io, Io::Sync { .. }) && io.root() == Root::Folder && directory;
        sync && once.replace(false)
    });
    let _ = a.commit("Tag", |i| i.add(song, TAGS, "Live".to_string()));
    let _ = a.commit("Tag", |i| i.add(song, TAGS, "Gig".to_string()));
    heal(&probe);
    probe.seen.borrow_mut().full = true;
    let full = a.commit("Tag", |i| i.add(song, TAGS, "Studio".to_string()));
    assert!(
        matches!(
            full,
            Err(Error::Io {
                error: IoError::NoSpace,
                ..
            })
        ),
        "{full:?}"
    );
}

fn a_copied_move_whose_source_removal_fails_is_finished_by_the_next_write<F: Facade>() {
    let folder = disk();
    let clock = TestClock::at(1_000);
    let probe = Probe::new(&machine(&folder));
    let (mut a, _) = F::open(probe.clone(), env("a", 1, &clock)).unwrap();
    let song = create(&mut a, "song.npno", b"song");
    let once = Cell::new(true);
    fail(&probe, move |io| {
        let source = matches!(io, Io::Remove { path, .. } if path.as_str() == "song.npno");
        source && once.replace(false)
    });
    let held = Expect::Holds(identity(b"song"));
    let _ = a.commit("Rename", |i| i.rename(song, &path("b.npno"), held));
    heal(&probe);
    create(&mut a, "next.npno", b"next");
    let files = folder.files(Root::Folder);
    let library: Vec<&RelPath> = files
        .keys()
        .filter(|at| !at.as_str().starts_with(".t"))
        .collect();
    assert_eq!(library, [&path("b.npno"), &path("next.npno")]);
}

fn a_record_that_never_landed_is_not_carried_out_by_the_next_write<F: Facade>() {
    let folder = disk();
    let clock = TestClock::at(1_000);
    let probe = Probe::new(&machine(&folder));
    let (mut a, _) = F::open(probe.clone(), env("a", 1, &clock)).unwrap();
    let song = create(&mut a, "song.npno", b"song");
    let journal = |path: &RelPath| {
        let text = path.as_str();
        text.ends_with(".json") && (text.contains("/pending/") || text.contains("/tmp/"))
    };
    fail(&probe, move |io| match io {
        Io::Create { path, .. } => journal(path),
        Io::Stat { path, .. } => path.as_str().contains("/pending/"),
        _ => false,
    });
    let held = Expect::Holds(identity(b"song"));
    let moved = a.commit("Rename", |i| i.rename(song, &path("b.npno"), held));
    heal(&probe);
    assert!(moved.is_err(), "the record is not written: {moved:?}");
    let before = folder.files(Root::Folder);
    let library = |files: &BTreeMap<RelPath, Vec<u8>>| -> Vec<(RelPath, Vec<u8>)> {
        let outside = files
            .iter()
            .filter(|(at, _)| !at.as_str().starts_with(".t"));
        outside
            .map(|(at, bytes)| (at.clone(), bytes.clone()))
            .collect()
    };
    a.commit("Tag", |i| i.add(song, TAGS, tag("later")))
        .unwrap();
    let after = folder.files(Root::Folder);
    assert_eq!(
        library(&after),
        library(&before),
        "the rename is not carried out"
    );
    let file = a.view().entity(song).unwrap().file().unwrap();
    assert_eq!(file.path, path("song.npno"));
    let undone: Vec<String> = a.history().into_iter().map(|item| item.label).collect();
    assert_eq!(undone, ["Import", "Tag"], "the rename is not logged");
}

fn a_record_whose_writing_seems_to_fail_is_settled_before_the_next_write<F: Facade>() {
    let folder = disk();
    let clock = TestClock::at(1_000);
    let probe = Probe::new(&machine(&folder));
    let (mut a, _) = F::open(probe.clone(), env("a", 1, &clock)).unwrap();
    let song = create(&mut a, "song.npno", b"song");
    let record = |path: &RelPath| path.as_str().contains("/pending/");
    unanswered(&probe, move |io| match io {
        Io::Rename { to, .. } | Io::Create { path: to, .. } => record(to),
        _ => false,
    });
    let placed = folder.clone();
    let written =
        move |path: &RelPath| record(path) && placed.files(Root::Folder).contains_key(path);
    fail(
        &probe,
        move |io| matches!(io, Io::Stat { path, .. } if written(path)),
    );
    let held = Expect::Holds(identity(b"song"));
    let moved = a.commit("Rename", |i| i.rename(song, &path("b.npno"), held));
    heal(&probe);
    assert!(
        moved.is_err(),
        "the record's write seems to fail: {moved:?}"
    );
    let records = |folder: &MemDisk| -> Vec<RelPath> {
        let files = pending_files(folder).into_iter();
        files.filter(|at| at.as_str().ends_with(".json")).collect()
    };
    assert_eq!(records(&folder).len(), 1, "the record is in place");
    let at = |a: &F, entity| a.view().entity(entity).unwrap().file().unwrap().path;
    let from = at(&a, song);
    let held = Expect::Holds(identity(b"song"));
    let renamed = a.commit("Rename", |i| i.rename(song, &path("c.npno"), held));
    assert!(renamed.is_ok(), "from {from}: {renamed:?}");
    let other = create(&mut a, "song.npno", b"other");
    a.close().unwrap();
    let (mut a, _) = F::open(probe, env("a", 2, &clock)).unwrap();
    a.commit("Tag", |i| i.add(song, TAGS, tag("later")))
        .unwrap();
    let files = folder.files(Root::Folder);
    for (entity, bytes) in [(song, &b"song"[..]), (other, &b"other"[..])] {
        let file = at(&a, entity);
        assert_eq!(files.get(&file).map(Vec::as_slice), Some(bytes), "{file}");
    }
    assert_eq!(records(&folder), [], "the record is removed");
}

fn a_record_a_crash_left_spent_is_not_settled_again_and_goes_at_the_next_write<F: Facade>() {
    let one_disk = |disk: &MemDisk| Machine {
        folder: disk.clone(),
        local: disk.clone(),
    };
    let folder = disk();
    let clock = TestClock::at(1_000);
    let probe = Probe::new(&one_disk(&folder));
    let (mut a, _) = F::open(probe.clone(), env("a", 1, &clock)).unwrap();
    let song = create(&mut a, "one.npno", b"one");
    fail(&probe, pending_removal);
    create(&mut a, "two.npno", b"two");
    drop(a);
    let folder = folder.restart();
    assert_ne!(pending_files(&folder), [], "the crash left the record");
    let (mut again, opened) = F::open(Probe::new(&one_disk(&folder)), env("a", 2, &clock)).unwrap();
    assert!(
        matches!(opened.start, Start::Resumed(_)),
        "{:?}",
        opened.start
    );
    assert_eq!(opened.settled, [], "none is open");
    assert_eq!(opened.orphaned, []);
    again
        .commit("Tag", |i| i.add(song, TAGS, tag("x")))
        .unwrap();
    assert_eq!(pending_files(&folder), [], "the next write removes it");
    let labels: Vec<String> = again.history().into_iter().map(|i| i.label).collect();
    assert_eq!(labels, ["Tag"], "the history holds this session's intents");
}

fn a_spent_record_is_reported_only_where_its_entry_has_not_arrived<F: Facade>() {
    let folder = disk();
    let clock = TestClock::at(1_000);
    let probe = Probe::new(&machine(&folder));
    let (mut a, _) = F::open(probe.clone(), env("a", 1, &clock)).unwrap();
    create(&mut a, "one.npno", b"one");
    let segments = |folder: &MemDisk| -> BTreeMap<RelPath, Vec<u8>> {
        let files = folder.files(Root::Folder).into_iter();
        files
            .filter(|(path, _)| path.as_str().ends_with(SEGMENT_EXTENSION))
            .collect()
    };
    let before = segments(&folder);
    fail(&probe, pending_removal);
    create(&mut a, "two.npno", b"two");
    heal(&probe);

    let (_, opened) = F::open(Probe::new(&machine(&folder)), env("b", 2, &clock)).unwrap();
    assert_eq!(opened.orphaned, [], "every record's entry arrived");

    let behind = disk();
    for (at, bytes) in folder.files(Root::Folder) {
        put(&behind, at.as_str(), before.get(&at).unwrap_or(&bytes));
    }
    let (_, opened) = F::open(Probe::new(&machine(&behind)), env("c", 3, &clock)).unwrap();
    let labels: Vec<&str> = opened.orphaned.iter().map(|o| o.label.as_str()).collect();
    assert_eq!(
        labels,
        ["Import"],
        "the second import's entry has not arrived"
    );
    assert_eq!(opened.orphaned[0].paths, [path("two.npno")]);
}

fn a_settled_record_is_removed_and_logged_once<F: Facade>() {
    interrupted_renames::<F>(|disk, shown| {
        let machine = Machine {
            folder: disk.clone(),
            local: disk.clone(),
        };
        let clock = TestClock::at(2_000);
        let (mut again, opened) = F::open(Probe::new(&machine), env("a", 2, &clock)).unwrap();
        assert_eq!(opened.settled.len(), 1, "{shown}");
        let song = again.view().entities()[0].id();
        let tagged = again
            .commit("Tag", |i| i.add(song, TAGS, tag("next")))
            .unwrap_or_else(|e| panic!("{shown}: {e}"));
        assert_eq!(tagged.local, Local::Current, "{shown}");
        assert_eq!(pending_files(&disk), [], "{shown}: the record is removed");
        again
            .commit("Tag", |i| i.add(song, TAGS, tag("last")))
            .unwrap_or_else(|e| panic!("{shown}: {e}"));
        let labels: Vec<String> = again.history().into_iter().map(|i| i.label).collect();
        assert_eq!(
            labels.iter().filter(|label| *label == "Rename").count(),
            1,
            "{shown}: the settlement is logged once: {labels:?}"
        );
        assert_eq!(
            tags(&again.view(), song),
            ["last", "new", "next"],
            "{shown}"
        );
    });
}

fn identities_a_failed_write_lost_are_kept_by_the_next_scan<F: Facade>() {
    let folder = disk();
    let clock = TestClock::at(1_000);
    let here = machine(&folder);
    let probe = Probe::new(&here);
    let (mut a, _) = F::open(probe.clone(), env("a", 1, &clock)).unwrap();
    create(&mut a, "song.npno", b"song");
    move_outside(&folder, "song.npno", "tune.npno");
    fail(&probe, local_writes(&["identities.json.next"]));
    a.rescan()
        .expect("identities the local root cannot keep fail no scan");
    heal(&probe);
    a.rescan().unwrap();
    let kept = local_file(&here, "identities.json").expect("kept by the next scan");
    assert!(
        String::from_utf8(kept).unwrap().contains("tune.npno"),
        "the identity read for a file that may be an entity's, moved"
    );
}

fn a_commit_scans_only_what_it_moved_and_a_rescan_finds_the_rest<F: Facade>() {
    let folder = disk();
    let clock = TestClock::at(1_000);
    let probe = Probe::new(&machine(&folder));
    let (mut a, _) = F::open(probe.clone(), env("a", 1, &clock)).unwrap();
    let song = create(&mut a, "song.npno", b"song");
    let others = 100;
    for i in 0..others {
        put(&folder, &format!("old/{i}.npno"), i.to_string().as_bytes());
    }
    a.rescan().unwrap();
    move_outside(&folder, "song.npno", "old/song.npno");
    let before = probe.library_reads();
    let new = create(&mut a, "new/one.npno", b"one");
    let reads = probe.library_reads() - before;
    assert!(
        reads < others,
        "{reads} lists, stats and reads of library paths"
    );
    let file = a.view().entity(new).unwrap().file().unwrap();
    assert_eq!(
        (file.path, file.state),
        (path("new/one.npno"), FileState::InSync)
    );
    a.commit("Move", |i| i.move_tree(&path("new"), &path("shelf/new")))
        .unwrap();
    let file = a.view().entity(new).unwrap().file().unwrap();
    assert_eq!(
        (file.path, file.state),
        (path("shelf/new/one.npno"), FileState::InSync)
    );

    let changes = a.rescan().unwrap().changes;
    assert_eq!(
        changes,
        [Change {
            entity: song,
            what: What::File,
            by: By::Outside
        }]
    );
    let file = a.view().entity(song).unwrap().file().unwrap();
    assert_eq!(file.path, path("old/song.npno"));
}

/// A writer's log that bound `count` files under `lib/` to as many entities, at
/// the modification times they have now, written straight into the folder.
fn shelved(folder: &MemDisk, count: usize) -> Vec<(EntityId, RelPath)> {
    let mut files = Vec::with_capacity(count);
    let mut ops = Vec::with_capacity(2 * count);
    for i in 0..count {
        let at = path(&format!("lib/{:03}/{i}.npno", i / 100));
        let bytes = format!("file {i}").into_bytes();
        put(folder, at.as_str(), &bytes);
        let entity = EntityId::from_u128(i as u128 + 1);
        let file = FileFact {
            path: at.clone(),
            identity: identity(&bytes),
            len: bytes.len() as u64,
            modified: Some(modified(folder, &at)),
        };
        ops.push(Op::Create {
            entity,
            replaces: Vec::new(),
            take: None,
        });
        ops.push(Op::File {
            entity,
            file: Some(file),
            replaces: Vec::new(),
            take: None,
        });
        files.push((entity, at));
    }
    let writer = SHELVED;
    let genesis = EntryKind::Genesis(Genesis {
        writer,
        label: "elsewhere".into(),
    });
    let intents = ops.chunks(2000).map(|ops| {
        EntryKind::Intent(Logged {
            label: "Import".into(),
            ops: ops.to_vec(),
            displaced: Vec::new(),
            forks: Vec::new(),
            record: None,
        })
    });
    let (mut prev, mut segment) = (EntryHash::ZERO, Vec::new());
    for (n, kind) in std::iter::once(genesis).chain(intents).enumerate() {
        let at = Hlc {
            wall_ms: n as u64 + 1,
            counter: 0,
        };
        let entry = Entry::encode(prev, n as u64, at, kind).unwrap();
        prev = entry.hash();
        segment.extend(entry.to_bytes());
    }
    let log = layout().writer(writer).join("log.jsonl").unwrap();
    put(folder, log.as_str(), &segment);
    files
}

/// The writer whose log [`shelved`] writes.
const SHELVED: WriterId = WriterId::from_u128(0xfeed);

/// Gives `machine`'s install a directory for `writer`, retired, as for a writer
/// it wrote as before: a move of a file it logged is this install's to pin.
fn retired_here(machine: &Machine, writer: WriterId) {
    let genesis = EntryHash::from_u128(writer.to_u128());
    let head = format!(r#"{{"writer":"{writer}","head":"{genesis}","retired":true}}"#);
    let local = &machine.local;
    let made = local.perform(Io::MakeDir {
        root: Root::Local,
        path: Layout::local(genesis),
    });
    made.unwrap();
    let created = local.perform(Io::Create {
        root: Root::Local,
        path: Layout::head(genesis),
        bytes: head.into_bytes(),
    });
    created.unwrap();
}

/// The writer of `machine`'s pool whose genesis sorts first: the one the next
/// instance there resumes while its lock is free.
fn first_in_pool(machine: &Machine) -> WriterId {
    let dirs = machine.local.directories(Root::Local).into_iter();
    let mut geneses = dirs.filter_map(|dir| dir.as_str().parse::<EntryHash>().ok());
    let read = machine.local.perform(Io::Read {
        root: Root::Local,
        path: Layout::head(geneses.next().unwrap()),
        range: Range {
            offset: 0,
            len: u64::MAX,
        },
    });
    let Ok(Reply::Bytes(bytes)) = read else {
        panic!("{read:?}");
    };
    let head: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    head["writer"].as_str().unwrap().parse().unwrap()
}

/// The writer the instance showing `view` writes as.
fn own_writer(view: &View) -> Option<WriterId> {
    let here = view.writers().iter().find(|w| w.here == Presence::This);
    here.map(|w| w.writer)
}

fn modified(folder: &MemDisk, at: &RelPath) -> u64 {
    let stat = Io::Stat {
        root: Root::Folder,
        path: at.clone(),
    };
    match folder.perform(stat) {
        Ok(Reply::Stat(Some(meta))) => meta.modified.unwrap(),
        reply => panic!("{at}: {reply:?}"),
    }
}

fn binds(folder: &MemDisk, writer: WriterId) -> Vec<Entry> {
    let entries = segment_entries(folder, writer).into_iter();
    entries
        .filter(|entry| matches!(entry.kind(), EntryKind::Bind(_)))
        .collect()
}

fn an_identity_read_for_a_new_time_is_read_once_per_install<F: Facade>() {
    let folder = disk();
    let files = shelved(&folder, 20);
    for (_, at) in &files {
        let later = modified(&folder, at) + 1_000_000;
        folder.set_modified(Root::Folder, at, later).unwrap();
    }
    let clock = TestClock::at(1_000);
    let here = machine(&folder);
    let scan = |seed| {
        let probe = Probe::new(&here);
        let (mut library, _) = F::open(probe.clone(), env("a", seed, &clock)).unwrap();
        let scanned = library.rescan().unwrap().scan;
        (library, scanned, probe.file_reads())
    };
    let (_, scanned, reads) = scan(1);
    assert_eq!((reads, scanned.changed.len()), (20, 0));
    let (_, scanned, reads) = scan(2);
    assert_eq!((reads, scanned.changed.len()), (0, 0));
    let (song, at) = &files[0];
    put(&folder, at.as_str(), b"FILE 0");
    let (_, scanned, reads) = scan(3);
    assert_eq!((reads, scanned.changed), (1, vec![*song]));
}

#[test]
fn a_library_whose_files_all_have_new_modification_times_commits() {
    let folder = disk();
    let files = shelved(&folder, 26_000);
    for (_, at) in &files {
        let later = modified(&folder, at) + 1_000_000;
        folder.set_modified(Root::Folder, at, later).unwrap();
    }
    let clock = TestClock::at(1_000);
    let here = machine(&folder);
    retired_here(&here, SHELVED);
    let (mut a, _) = Blocking::open(Probe::new(&here), env("a", 1, &clock)).unwrap();
    let scanned = a.rescan().unwrap().scan;
    assert_eq!((scanned.changed.len(), scanned.moved.len()), (0, 0));
    let (song, _) = files[0];
    a.commit("Tag", |i| i.add(song, TAGS, tag("kept"))).unwrap();
    let writer = label_of(&a.view(), "a");
    let pinned = binds(&folder, writer).len();
    assert_eq!(pinned, 0, "a new time alone is not logged");
}

#[test]
fn a_commit_pins_any_number_of_moves_in_entries_within_the_line_limit() {
    let folder = disk();
    let files = shelved(&folder, 26_000);
    let clock = TestClock::at(1_000);
    let here = machine(&folder);
    retired_here(&here, SHELVED);
    let (mut a, _) = Blocking::open(Probe::new(&here), env("a", 1, &clock)).unwrap();
    folder
        .perform(Io::Rename {
            root: Root::Folder,
            from: path("lib"),
            to: path("shelf"),
        })
        .unwrap();
    assert_eq!(a.rescan().unwrap().changes.len(), files.len());
    let (song, _) = files[0];
    a.commit("Tag", |i| i.add(song, TAGS, tag("moved")))
        .unwrap();

    let writer = label_of(&a.view(), "a");
    let binds = binds(&folder, writer);
    let pinned: usize = binds
        .iter()
        .map(|entry| match entry.kind() {
            EntryKind::Bind(bound) => bound.ops.len(),
            kind => panic!("{kind:?}"),
        })
        .sum();
    assert_eq!((pinned, binds.len() > 1), (files.len(), true));
    for entry in &binds {
        let len = entry.to_bytes().len();
        assert!(len <= MAX_LINE, "{len} bytes");
    }
    let probe = Probe::new(&machine(&folder));
    let (mut b, _) = Blocking::open(probe, env("b", 2, &clock)).unwrap();
    let scanned = b.rescan().unwrap().scan;
    assert_eq!(scanned.moved, [], "the pins say where each file went");
    let file = b.view().entity(song).unwrap().file().unwrap();
    assert_eq!(
        (file.path, file.state),
        (path("shelf/000/0.npno"), FileState::InSync)
    );
}

fn opening_viewing_and_refreshing_write_nothing_in_the_folder<F: Facade>() {
    let folder = disk();
    let clock = TestClock::at(1_000);
    let (mut a, _) = F::open(Probe::new(&machine(&folder)), env("a", 1, &clock)).unwrap();
    let song = create(&mut a, "song.npno", b"song");
    a.commit("Save", |i| {
        i.save(
            song,
            &path("song.npno"),
            b"two".to_vec(),
            Expect::Holds(identity(b"song")),
        )
    })
    .unwrap();
    put(&folder, "outside.npno", b"outside");
    let probe = Probe::new(&machine(&folder));
    let (mut b, _) = F::open(probe.clone(), env("b", 2, &clock)).unwrap();
    assert_eq!(b.rescan().unwrap().scan.arrived, [path("outside.npno")]);
    let _ = b.view();
    a.commit("Tag", |i| i.add(song, TAGS, tag("more"))).unwrap();
    b.refresh().unwrap();
    b.others().unwrap();
    b.trash().unwrap();
    assert_eq!(probe.folder_writes(), 0);
    b.commit("Tag", |i| i.add(song, TAGS, tag("b"))).unwrap();
    assert!(probe.folder_writes() > 0);
}

fn opening_scans_no_library_file_and_the_first_refresh_scans_them_all<F: Facade>() {
    let folder = disk();
    let clock = TestClock::at(1_000);
    let (mut a, _) = F::open(Probe::new(&machine(&folder)), env("a", 1, &clock)).unwrap();
    let song = create(&mut a, "song.npno", b"song");
    a.close().unwrap();
    move_outside(&folder, "song.npno", "moved.npno");
    put(&folder, "outside.npno", b"outside");

    let probe = Probe::new(&machine(&folder));
    let (mut b, _) = F::open(probe.clone(), env("b", 2, &clock)).unwrap();
    assert_eq!(probe.library_reads(), 0);
    let file = b.view().entity(song).unwrap().file().unwrap();
    assert_eq!(
        (file.path, file.state),
        (path("song.npno"), FileState::Unscanned),
        "presumed where its facts say"
    );
    assert_eq!(
        b.view().unbound().cloned().collect::<Vec<_>>(),
        [] as [RelPath; 0]
    );

    let refreshed = b.refresh().unwrap();
    assert!(probe.library_reads() > 0, "nothing scanned yet, so all");
    let moved = Change {
        entity: song,
        what: What::File,
        by: By::Outside,
    };
    assert_eq!(refreshed.changes, [moved]);
    assert_eq!(refreshed.scan.arrived, [path("outside.npno")]);
    let file = b.view().entity(song).unwrap().file().unwrap();
    assert_eq!(
        (file.path, file.state),
        (path("moved.npno"), FileState::InSync)
    );
    let before = probe.library_reads();
    b.refresh().unwrap();
    assert_eq!(probe.library_reads(), before, "then only what entries move");
}

fn swap_files_a_browser_left_are_neither_scanned_nor_read<F: Facade>() {
    let folder = disk();
    let clock = TestClock::at(1_000);
    let here = machine(&folder);
    let (mut a, _) = F::open(Probe::new(&here), env("a", 1, &clock)).unwrap();
    create(&mut a, "song.npno", b"song");
    a.close().unwrap();
    let writers = layout().writers();
    let own = folder
        .files(Root::Folder)
        .into_keys()
        .find(|p| p.starts_with(&writers))
        .and_then(|p| p.parent())
        .unwrap();
    let zero = "0".repeat(32);
    for (at, bytes) in [
        ("song.npno.crswap".to_owned(), &b"so"[..]),
        (format!("{own}/{zero}.txt.crswap"), b"{\"prev\""),
        (format!("{own}/pending/{zero}.json.crswap"), b"{"),
    ] {
        put(&folder, &at, bytes);
    }
    let (mut a, opened) = F::open(Probe::new(&here), env("a", 1, &clock)).unwrap();
    assert_eq!(a.rescan().unwrap().scan.arrived, [] as [RelPath; 0]);
    assert_eq!(opened.unreadable, [] as [RelPath; 0]);
    assert_eq!(opened.ignored, [] as [RelPath; 0]);
    assert_eq!(
        a.view().unbound().cloned().collect::<Vec<_>>(),
        [] as [RelPath; 0]
    );
}

fn a_view_lists_each_library_files_length_once_scanned<F: Facade>() {
    let folder = disk();
    let clock = TestClock::at(1_000);
    let here = machine(&folder);
    put(&folder, "a.wav", b"wave");
    put(&folder, "d/b.syx", b"sysex!");
    let (mut a, _) = F::open(Probe::new(&here), env("a", 1, &clock)).unwrap();
    assert_eq!(
        a.view().listed(&path("a.wav")),
        None,
        "nothing is scanned at open"
    );
    a.rescan().unwrap();
    let view = a.view();
    let len = |at: &str| view.listed(&path(at)).map(|listed| listed.len);
    assert_eq!((len("a.wav"), len("d/b.syx")), (Some(4), Some(6)));
    assert_eq!(len("d"), None, "a directory is no file");
    assert_eq!(len("gone.wav"), None);
    let listed = view.listed(&path("a.wav")).unwrap();
    assert_eq!(listed.modified, Some(modified(&folder, &path("a.wav"))));
}

fn a_file_expected_as_listed_is_refused_once_its_length_or_time_moved<F: Facade>() {
    let folder = disk();
    let clock = TestClock::at(1_000);
    put(&folder, "a.wav", b"wave");
    put(&folder, "b.wav", b"wave");
    let (mut a, _) = F::open(Probe::new(&machine(&folder)), env("a", 1, &clock)).unwrap();
    a.rescan().unwrap();
    let listed = |view: &View, at: &str| {
        let listed = view.listed(&path(at)).unwrap();
        Expect::Listed {
            len: listed.len,
            modified: listed.modified,
        }
    };
    let view = a.view();
    let adopt = |a: &mut F, at: &str, expect| {
        a.commit("Tag", |i| {
            let (i, _) = i.create(|e| {
                e.adopt(&path(at), expect).add(TAGS, tag("Sunday"));
            });
            i
        })
    };
    put(&folder, "b.wav", b"wavy");
    let refused = adopt(&mut a, "b.wav", listed(&view, "b.wav"));
    assert!(
        matches!(&refused, Err(Error::Refused(Refusal::Changed(m))) if m.path == path("b.wav")),
        "{refused:?}"
    );
    adopt(&mut a, "a.wav", listed(&view, "a.wav")).unwrap();
    assert_eq!(
        tags(&a.view(), a.view().at(&path("a.wav")).unwrap().id()),
        [tag("Sunday")]
    );
}

fn a_directory_that_cannot_be_listed_leaves_its_files_unknown_and_the_rest_scanned<F: Facade>() {
    let folder = disk();
    let clock = TestClock::at(1_000);
    let probe = Probe::new(&machine(&folder));
    let (mut a, _) = F::open(probe.clone(), env("a", 1, &clock)).unwrap();
    let kept = create(&mut a, "locked/kept.npno", b"kept");
    put(&folder, "open.wav", b"open");
    put(&folder, "locked/new.wav", b"new");
    let locked = path("locked");
    fail(&probe, move |io| {
        matches!(io, Io::List { .. } | Io::ListStat { .. }) && io.path().starts_with(&locked)
    });
    let refreshed = a.rescan().unwrap();
    assert_eq!(refreshed.unlisted, [path("locked")]);
    let view = a.view();
    assert_eq!(
        view.unbound().cloned().collect::<Vec<_>>(),
        [path("open.wav")],
        "the rest is scanned"
    );
    let file = view.entity(kept).unwrap().file().unwrap();
    assert_eq!(
        (file.path, file.state),
        (path("locked/kept.npno"), FileState::Unscanned),
        "unknown, not missing"
    );
    heal(&probe);
    let refreshed = a.rescan().unwrap();
    assert_eq!(refreshed.unlisted, [] as [RelPath; 0]);
    let view = a.view();
    assert_eq!(
        view.unbound().cloned().collect::<Vec<_>>(),
        [path("locked/new.wav"), path("open.wav")]
    );
    let file = view.entity(kept).unwrap().file().unwrap();
    assert_eq!(file.state, FileState::InSync);
}

fn a_file_that_cannot_be_read_leaves_its_entity_unknown_and_the_rest_listed<F: Facade>() {
    let folder = disk();
    let clock = TestClock::at(1_000);
    let (mut a, _) = F::open(Probe::new(&machine(&folder)), env("a", 1, &clock)).unwrap();
    let song = create(&mut a, "song.npno", b"song");
    put(&folder, "other.npno", b"other");
    put(&folder, "song.npno", b"SONG");
    let probe = Probe::new(&machine(&folder));
    probe.seen.borrow_mut().unreadable = Some(path("song.npno"));
    let (mut b, _) = F::open(probe.clone(), env("b", 2, &clock)).unwrap();
    for _ in 0..2 {
        let refreshed = b.rescan().unwrap();
        assert_eq!(refreshed.unlisted, [path("song.npno")]);
        let view = b.view();
        assert!(
            view.listed(&path("other.npno")).is_some(),
            "the rest is listed"
        );
        let file = view.entity(song).unwrap().file().unwrap();
        assert_eq!(
            (file.path, file.state),
            (path("song.npno"), FileState::Unscanned),
            "unknown, not missing"
        );
    }
    heal(&probe);
    let refreshed = b.rescan().unwrap();
    assert_eq!(refreshed.unlisted, [] as [RelPath; 0]);
    let file = b.view().entity(song).unwrap().file().unwrap();
    assert_eq!(
        (file.path, file.state),
        (path("song.npno"), FileState::ChangedOutside)
    );
    put(&folder, "song.npno", b"Song");
    probe.seen.borrow_mut().unreadable = Some(path("song.npno"));
    b.rescan_paths(vec![path("song.npno")]).unwrap();
    let file = b.view().entity(song).unwrap().file().unwrap();
    assert_eq!(file.state, FileState::Unscanned, "a scan of one path");
    let refreshed = b.refresh().unwrap();
    assert_eq!(
        refreshed.unlisted,
        [path("song.npno")],
        "the next scans every file"
    );
}

/// The directory of the only writer the folder holds.
fn only_writer(folder: &MemDisk) -> RelPath {
    let dirs = folder.directories(Root::Folder).into_iter();
    let mut writers = dirs.filter(|dir| dir.parent() == Some(layout().writers()));
    let only = writers.next().expect("a writer");
    assert_eq!(writers.next(), None);
    only
}

fn another_writers_file_that_cannot_be_read_is_taken_in_once_it_can_be<F: Facade>() {
    let folder = disk();
    let clock = TestClock::at(1_000);
    let (mut a, _) = F::open(Probe::new(&machine(&folder)), env("a", 1, &clock)).unwrap();
    let song = create(&mut a, "song.npno", b"song");
    let probe = Probe::new(&machine(&folder));
    probe.seen.borrow_mut().unreadable = Some(only_writer(&folder));
    let (mut b, opened) = F::open(probe.clone(), env("b", 2, &clock)).unwrap();
    assert!(b.view().entity(song).is_none(), "not read yet");
    assert!(
        opened
            .unreadable
            .iter()
            .all(|at| at.starts_with(&only_writer(&folder)))
            && !opened.unreadable.is_empty(),
        "{:?}",
        opened.unreadable
    );
    let tagged = b
        .commit("Tag", |i| i.create(|e| _ = e.add(TAGS, tag("b"))).0)
        .unwrap();
    assert_eq!(behind(&tagged), [Lag::Read]);
    heal(&probe);
    b.refresh().unwrap();
    assert_eq!(tags(&b.view(), song), [tag("new")]);
    let tagged = b.commit("Tag", |i| i.add(song, TAGS, tag("b"))).unwrap();
    assert_eq!(behind(&tagged), []);
}

fn a_writer_whose_own_file_cannot_be_read_does_not_open<F: Facade>() {
    let folder = disk();
    let clock = TestClock::at(1_000);
    let here = machine(&folder);
    let (mut a, _) = F::open(Probe::new(&here), env("a", 1, &clock)).unwrap();
    create(&mut a, "song.npno", b"song");
    a.close().unwrap();
    let probe = Probe::new(&here);
    probe.seen.borrow_mut().unreadable = Some(only_writer(&folder));
    let refused = F::open(probe.clone(), env("a", 1, &clock)).map(drop);
    assert!(
        matches!(
            refused,
            Err(Error::Io {
                error: IoError::Denied,
                ..
            })
        ),
        "{refused:?}"
    );
    heal(&probe);
    let (again, opened) = F::open(probe, env("a", 1, &clock)).unwrap();
    assert!(
        matches!(opened.start, Start::Resumed(_)),
        "{:?}",
        opened.start
    );
    again.close().unwrap();
}

fn orphans_are_held_back_while_a_writers_file_cannot_be_read<F: Facade>() {
    let folder = disk();
    let clock = TestClock::at(1_000);
    let probe = Probe::new(&machine(&folder));
    let (mut ours, _) = F::open(probe.clone(), env("b", 2, &clock)).unwrap();
    let orphan = a_rename_left_by_another(&folder, &clock, &mut ours);
    let late = only_writer(&folder).join("snapshot-x.json").unwrap();
    put(&folder, late.as_str(), b"late");
    probe.seen.borrow_mut().unreadable = Some(late);
    assert_eq!(ours.refresh().unwrap().orphaned, [], "held back");
    let refused = ours.settle(orphan, Settlement::Finished);
    assert!(
        matches!(refused, Err(Error::Refused(Refusal::Nothing))),
        "{refused:?}"
    );
    heal(&probe);
    assert_eq!(ours.refresh().unwrap().orphaned.len(), 1);
}

fn orphans_are_held_back_while_a_pending_record_cannot_be_read<F: Facade>() {
    let folder = disk();
    let clock = TestClock::at(1_000);
    let probe = Probe::new(&machine(&folder));
    let (mut ours, _) = F::open(probe.clone(), env("b", 2, &clock)).unwrap();
    let orphan = a_rename_left_by_another(&folder, &clock, &mut ours);
    let record = layout().pending(orphan.writer, orphan.record);
    probe.seen.borrow_mut().unreadable = Some(record.clone());
    let refreshed = ours.refresh().unwrap();
    assert_eq!(
        (refreshed.orphaned, refreshed.unread),
        (vec![], vec![record]),
        "held back"
    );
    let refused = ours.settle(orphan, Settlement::Finished);
    assert!(
        matches!(
            refused,
            Err(Error::Io {
                error: IoError::Denied,
                ..
            })
        ),
        "{refused:?}"
    );
    heal(&probe);
    let refreshed = ours.refresh().unwrap();
    assert_eq!(
        (refreshed.orphaned.len(), refreshed.unread),
        (1, vec![]),
        "read again"
    );
}

fn a_writer_whose_own_pending_record_cannot_be_read_does_not_open<F: Facade>() {
    let folder = disk();
    let clock = TestClock::at(1_000);
    put(&folder, "a.wav", b"a");
    let mut here = machine(&folder);
    let probe = Probe::new(&here);
    let (mut a, _) = F::open(probe.clone(), env("a", 1, &clock)).unwrap();
    a.rescan().unwrap();
    a.commit("Tag", |i| i.create(|e| _ = e.add(TAGS, tag("x"))).0)
        .unwrap();
    let logging = |io: &Io| io.root() == Root::Folder && io.path().as_str().ends_with(".txt");
    fail(&probe, move |io| io.mutates() && logging(io));
    let stopped = a.commit("Rename", |i| {
        i.rename_file(
            &path("a.wav"),
            &path("b.wav"),
            Expect::Holds(identity(b"a")),
        )
    });
    heal(&probe);
    assert!(stopped.is_err(), "{stopped:?}");
    drop(a);
    here.crash();
    let probe = Probe::new(&here);
    let pending = only_writer(&folder).join("pending").unwrap();
    probe.seen.borrow_mut().unreadable = Some(pending);
    let refused = F::open(probe.clone(), env("a", 1, &clock)).map(drop);
    assert!(
        matches!(
            refused,
            Err(Error::Io {
                error: IoError::Denied,
                ..
            })
        ),
        "{refused:?}"
    );
    heal(&probe);
    let (_, opened) = F::open(probe, env("a", 1, &clock)).unwrap();
    assert!(
        matches!(opened.start, Start::Resumed(_)),
        "{:?}",
        opened.start
    );
}

fn a_view_lists_no_file_before_a_scan_of_every_file_though_identities_are_kept<F: Facade>() {
    let folder = disk();
    let clock = TestClock::at(1_000);
    let here = machine(&folder);
    let (mut a, _) = F::open(Probe::new(&here), env("a", 1, &clock)).unwrap();
    create(&mut a, "song.npno", b"song");
    move_outside(&folder, "song.npno", "moved.npno");
    put(&folder, "same.npno", b"sang");
    a.rescan().unwrap();
    a.close().unwrap();
    assert!(
        local_file(&here, "identities.json").is_some(),
        "the scan kept the identities it read"
    );
    let (mut a, _) = F::open(Probe::new(&here), env("a", 1, &clock)).unwrap();
    assert_eq!(a.view().listed(&path("same.npno")), None);
    assert_eq!(a.view().listed(&path("moved.npno")), None);
    a.rescan().unwrap();
    assert_eq!(a.view().listed(&path("same.npno")).map(|l| l.len), Some(4));
}

fn a_file_no_entity_is_bound_to_is_saved_renamed_and_trashed_and_each_undone<F: Facade>() {
    let folder = disk();
    let clock = TestClock::at(1_000);
    put(&folder, "a.wav", b"wave");
    let (mut a, _) = F::open(Probe::new(&machine(&folder)), env("a", 1, &clock)).unwrap();
    a.rescan().unwrap();
    let files = || {
        let files = library_files(&folder);
        let files = files
            .into_iter()
            .map(|(path, bytes)| (path.as_str().to_owned(), bytes));
        files.collect::<Vec<(String, Vec<u8>)>>()
    };
    let held = |pairs: &[(&str, &[u8])]| -> Vec<(String, Vec<u8>)> {
        let pairs = pairs
            .iter()
            .map(|(at, bytes)| (at.to_string(), bytes.to_vec()));
        pairs.collect()
    };
    a.commit("Add", |i| {
        i.save_file(&path("new.syx"), b"new".to_vec(), Expect::Absent)
    })
    .unwrap();
    a.commit("Save", |i| {
        i.save_file(
            &path("a.wav"),
            b"wavy".to_vec(),
            Expect::Holds(identity(b"wave")),
        )
    })
    .unwrap();
    let row = a.view().row_at(&path("a.wav")).unwrap().file;
    let renamed = a
        .commit("Rename", |i| {
            i.rename_file(
                &path("a.wav"),
                &path("b.wav"),
                Expect::Holds(identity(b"wavy")),
            )
        })
        .unwrap();
    assert_eq!(renamed.files, [(path("b.wav"), row)]);
    a.commit("Delete", |i| {
        i.trash_file(&path("new.syx"), Expect::Holds(identity(b"new")))
    })
    .unwrap();
    assert_eq!(files(), held(&[("b.wav", b"wavy")]));
    assert!(a.view().entities().is_empty(), "no file became an entity");
    let view = a.view();
    let unbound: Vec<&str> = view.unbound().map(RelPath::as_str).collect();
    assert_eq!(unbound, ["b.wav"], "the view lists only the files left");
    let undone: [&[(&str, &[u8])]; 4] = [
        &[("b.wav", b"wavy"), ("new.syx", b"new")],
        &[("a.wav", b"wavy"), ("new.syx", b"new")],
        &[("a.wav", b"wave"), ("new.syx", b"new")],
        &[("a.wav", b"wave")],
    ];
    for (n, expected) in undone.iter().enumerate() {
        let undid = a.undo().unwrap();
        assert_eq!(files(), held(expected), "undo {}", n + 1);
        if n == 1 {
            assert_eq!(undid.files, [(path("a.wav"), row)], "undo of the rename");
        }
    }
    a.redo().unwrap();
    assert_eq!(files(), held(&[("a.wav", b"wave"), ("new.syx", b"new")]));
    assert!(a.view().entities().is_empty());
}

fn an_effect_by_path_on_an_entitys_file_is_refused<F: Facade>() {
    let folder = disk();
    let clock = TestClock::at(1_000);
    let (mut a, _) = F::open(Probe::new(&machine(&folder)), env("a", 1, &clock)).unwrap();
    create(&mut a, "song.npno", b"song");
    let refused = a.commit("Delete", |i| {
        i.trash_file(&path("song.npno"), Expect::Holds(identity(b"song")))
    });
    assert!(
        matches!(
            &refused,
            Err(Error::Refused(Refusal::Invalid(Invalid::Bound(at)))) if *at == path("song.npno")
        ),
        "{refused:?}"
    );
    assert_eq!(read(&folder, "song.npno").as_deref(), Some(&b"song"[..]));
}

fn a_tree_moved_with_files_no_entity_is_bound_to_is_moved_back_by_undo<F: Facade>() {
    let folder = disk();
    let clock = TestClock::at(1_000);
    put(&folder, "old/a.wav", b"a");
    put(&folder, "old/deep/b.wav", b"b");
    let (mut a, _) = F::open(Probe::new(&machine(&folder)), env("a", 1, &clock)).unwrap();
    a.rescan().unwrap();
    create(&mut a, "old/song.npno", b"song");
    let files = || {
        let files = library_files(&folder).into_iter();
        let files = files.map(|(path, _)| path.as_str().to_owned());
        files.collect::<Vec<String>>()
    };
    a.commit("Move", |i| i.move_tree(&path("old"), &path("new")))
        .unwrap();
    assert_eq!(
        files(),
        ["new/a.wav", "new/deep/b.wav", "new/song.npno"],
        "moved"
    );
    let dirs = || {
        let dirs = folder.directories(Root::Folder).into_iter();
        let dirs = dirs.filter(|dir| !dir.starts_with(&path(".t")) && !dir.is_root());
        dirs.map(|dir| dir.as_str().to_owned())
            .collect::<Vec<String>>()
    };
    a.undo().unwrap();
    assert_eq!(
        files(),
        ["old/a.wav", "old/deep/b.wav", "old/song.npno"],
        "undone"
    );
    assert_eq!(
        a.view().unbound().cloned().collect::<Vec<_>>(),
        [path("old/a.wav"), path("old/deep/b.wav")]
    );
    assert_eq!(dirs(), ["old", "old/deep"], "the tree's new name is gone");
    a.redo().unwrap();
    assert_eq!(
        files(),
        ["new/a.wav", "new/deep/b.wav", "new/song.npno"],
        "redone"
    );
    assert_eq!(dirs(), ["new", "new/deep"], "and its old name");
}

fn a_refresh_scans_what_another_writer_did_to_files_no_entity_is_bound_to<F: Facade>() {
    let folder = disk();
    let clock = TestClock::at(1_000);
    put(&folder, "a.wav", b"a");
    put(&folder, "old/b.wav", b"b");
    let (mut a, _) = F::open(Probe::new(&machine(&folder)), env("a", 1, &clock)).unwrap();
    a.rescan().unwrap();
    let (mut b, _) = F::open(Probe::new(&machine(&folder)), env("b", 2, &clock)).unwrap();
    b.rescan().unwrap();
    let renamed = Expect::Holds(identity(b"a"));
    b.commit("Rename", |i| {
        i.rename_file(&path("a.wav"), &path("c.wav"), renamed)
    })
    .unwrap();
    b.commit("Move", |i| i.move_tree(&path("old"), &path("new")))
        .unwrap();
    put(&folder, "outside.wav", b"outside");

    let refreshed = a.refresh().unwrap();
    assert_eq!(refreshed.changes, [], "no entity changed");
    let view = a.view();
    let unbound: Vec<&str> = view.unbound().map(RelPath::as_str).collect();
    assert_eq!(
        unbound,
        ["c.wav", "new/b.wav"],
        "only what b moved is scanned"
    );
}

fn a_refresh_scans_the_file_of_an_entity_another_writer_made_and_deleted_since<F: Facade>() {
    let folder = disk();
    let clock = TestClock::at(1_000);
    put(&folder, "a.wav", b"a");
    let (mut a, _) = F::open(Probe::new(&machine(&folder)), env("a", 1, &clock)).unwrap();
    a.rescan().unwrap();
    let (mut b, _) = F::open(Probe::new(&machine(&folder)), env("b", 2, &clock)).unwrap();
    b.rescan().unwrap();
    let held = Expect::Holds(identity(b"a"));
    let adopted = b.commit("Tag", |i| {
        let (i, _) = i.create(|e| {
            e.adopt(&path("a.wav"), held).add(TAGS, tag("kept"));
        });
        i
    });
    let entity = adopted.unwrap().created[0];
    b.commit("Delete", |i| i.trash(entity, held).delete(entity))
        .unwrap();

    a.refresh().unwrap();
    assert_eq!(a.view().unbound().count(), 0, "the file went to b's trash");
}

fn a_rename_a_failed_rescan_read_is_scanned_by_the_refresh_after_a_commit<F: Facade>() {
    let folder = disk();
    let clock = TestClock::at(1_000);
    let probe = Probe::new(&machine(&folder));
    put(&folder, "a.wav", b"a");
    let (mut a, _) = F::open(probe.clone(), env("a", 1, &clock)).unwrap();
    let song = create(&mut a, "song.npno", b"song");
    create(&mut a, "tune.npno", b"tune");
    a.rescan().unwrap();
    let (mut b, _) = F::open(Probe::new(&machine(&folder)), env("b", 2, &clock)).unwrap();
    b.rescan().unwrap();
    let renamed = Expect::Holds(identity(b"a"));
    b.commit("Rename", |i| {
        i.rename_file(&path("a.wav"), &path("c.wav"), renamed)
    })
    .unwrap();
    move_outside(&folder, "tune.npno", "tune2.npno");
    fail(&probe, |io| {
        matches!(io, Io::ReadMany { .. }) && !layout().owns(io.path())
    });
    assert!(a.rescan().is_err(), "the moved tune cannot be identified");
    heal(&probe);
    a.commit("Tag", |i| i.add(song, TAGS, tag("x"))).unwrap();

    a.refresh().unwrap();
    let view = a.view();
    let unbound: Vec<&str> = view.unbound().map(RelPath::as_str).collect();
    assert_eq!(unbound, ["c.wav"]);
}

/// A writer `a` that saved the file no entity is bound to `a.wav`, then `then` did
/// to it, and another writer `b` that has read what `a` did.
fn untagged_then<F: Facade>(
    folder: &MemDisk,
    clock: &TestClock,
    then: impl FnOnce(&mut F),
) -> (F, F) {
    put(folder, "a.wav", b"wave");
    let (mut a, _) = F::open(Probe::new(&machine(folder)), env("a", 1, clock)).unwrap();
    a.rescan().unwrap();
    then(&mut a);
    let (mut b, _) = F::open(Probe::new(&machine(folder)), env("b", 2, clock)).unwrap();
    b.rescan().unwrap();
    (a, b)
}

#[test]
fn a_read_owed_for_a_rewrite_sync_resolved_for_this_copy_is_settled_by_the_next_scan() {
    let (theirs, ours) = (disk(), disk());
    let clock = TestClock::at(1_000);
    put(&theirs, "a.wav", b"wave");
    let deliver = |path: &RelPath, bytes: &[u8]| {
        let _ = ours.perform(Io::Remove {
            root: Root::Folder,
            path: path.clone(),
        });
        put(&ours, path.as_str(), bytes);
    };
    let open = |folder: &MemDisk, label, seed| {
        let probe = Probe::new(&machine(folder));
        probe.seen.borrow_mut().coarse = Some(0);
        let (mut writer, _) = Blocking::open(probe, env(label, seed, &clock)).unwrap();
        writer.rescan().unwrap();
        writer
    };
    let mut b = open(&theirs, "b", 2);
    for (path, bytes) in theirs.files(Root::Folder) {
        deliver(&path, &bytes);
    }
    let mut a = open(&ours, "a", 1);
    let held = Expect::Holds(identity(b"wave"));
    a.commit("Save", |i| {
        i.save_file(&path("a.wav"), b"wavx".to_vec(), held)
    })
    .unwrap();
    save_untagged(&mut b);
    // Sync brings b's log and keeps a's own a.wav, as a conflict resolved for a.
    for (path, bytes) in theirs.files(Root::Folder) {
        if path.as_str().starts_with('.') {
            deliver(&path, &bytes);
        }
    }
    a.refresh().unwrap();
    assert_eq!(a.0.owed(), 0, "the refresh's scan read a.wav once");
}

#[test]
fn a_write_superseded_while_its_entry_was_unread_is_let_go_of() {
    let folder = disk();
    let clock = TestClock::at(1_000);
    let open = |label, seed| {
        let (writer, _) =
            Blocking::open(Probe::new(&machine(&folder)), env(label, seed, &clock)).unwrap();
        writer
    };
    let (mut a, mut b) = (open("a", 1), open("b", 2));
    let song = create(&mut b, "song.npno", b"song");
    let b_writer = own_writer(&b.view()).unwrap().to_string();
    let log = |folder: &MemDisk| {
        let logs = folder.files(Root::Folder).into_iter();
        let logs = logs.filter(|(path, _)| {
            path.as_str().contains(&b_writer) && path.as_str().ends_with(".txt")
        });
        logs.max_by_key(|(_, bytes)| bytes.len()).unwrap()
    };
    let mut away = Vec::new();
    for round in 0..3 {
        b.commit("Origin", |i| i.set(song, ORIGIN, format!("one {round}")))
            .unwrap();
        a.refresh().unwrap();
        // A sync client takes b's latest entry away for a while.
        let (path, bytes) = log(&folder);
        let lines: Vec<&[u8]> = bytes.split_inclusive(|byte| *byte == b'\n').collect();
        remove_outside(&folder, path.as_str());
        put(&folder, path.as_str(), &lines[..lines.len() - 1].concat());
        a.refresh().unwrap();
        remove_outside(&folder, path.as_str());
        put(&folder, path.as_str(), &bytes);
        b.commit("Origin", |i| i.set(song, ORIGIN, format!("two {round}")))
            .unwrap();
        a.refresh().unwrap();
        away.push(a.0.away());
    }
    assert_eq!(away, [0, 0, 0]);
}

/// The latest snapshot in `folder`.
fn latest_snapshot(folder: &MemDisk) -> Snapshot {
    let files = folder.files(Root::Folder).into_iter();
    let snapshots = files.filter(|(path, _)| path.as_str().contains("/snapshot-"));
    let snapshots = snapshots.map(|(_, bytes)| Snapshot::decode(&bytes).unwrap());
    snapshots.max_by_key(|snapshot| snapshot.head.n).unwrap()
}

#[test]
fn a_snapshot_keeps_one_binding_pin_per_file_until_a_write_replaces_it() {
    let folder = disk();
    let clock = TestClock::at(1_000);
    let (mut a, _) = Blocking::open(Probe::new(&machine(&folder)), env("a", 1, &clock)).unwrap();
    let song = create(&mut a, "p.npno", b"song");
    // The file moves outside twice, and each next commit pins it where it went.
    for (from, to, other) in [("p", "q", "x"), ("q", "r", "y")] {
        remove_outside(&folder, &format!("{from}.npno"));
        put(&folder, &format!("{to}.npno"), b"song");
        a.rescan().unwrap();
        create(&mut a, &format!("{other}.npno"), b"other");
    }
    a.compact().unwrap();
    assert_eq!(
        latest_snapshot(&folder).binds.len(),
        1,
        "one pin of one file"
    );
    let held = Expect::Holds(identity(b"song"));
    a.commit("Save", |i| {
        i.save(song, &path("r.npno"), b"sung".to_vec(), held)
    })
    .unwrap();
    a.compact().unwrap();
    assert_eq!(
        latest_snapshot(&folder).binds,
        BTreeSet::new(),
        "a write replaced it"
    );
}

#[test]
fn a_lost_write_back_behind_a_gap_is_no_ones_change_once_it_merges() {
    let folder = disk();
    let clock = TestClock::at(1_000);
    let open = |label, seed| {
        let (writer, _) =
            Blocking::open(Probe::new(&machine(&folder)), env(label, seed, &clock)).unwrap();
        writer
    };
    let (mut a, mut b) = (open("a", 1), open("b", 2));
    let song = create(&mut b, "song.npno", b"song");
    let tune = create(&mut b, "tune.npno", b"tune");
    let b_writer = own_writer(&b.view()).unwrap().to_string();
    b.commit("Origin", |i| i.set(song, ORIGIN, "s".to_string()))
        .unwrap();
    b.commit("Origin", |i| i.set(tune, ORIGIN, "t".to_string()))
        .unwrap();
    a.refresh().unwrap();
    let logs = folder.files(Root::Folder).into_iter();
    let logs =
        logs.filter(|(at, _)| at.as_str().contains(&b_writer) && at.as_str().ends_with(".txt"));
    let (log, bytes) = logs.max_by_key(|(_, bytes)| bytes.len()).unwrap();
    let lines: Vec<&[u8]> = bytes.split_inclusive(|byte| *byte == b'\n').collect();
    let n = lines.len();
    let replace = |bytes: Vec<u8>| {
        remove_outside(&folder, log.as_str());
        put(&folder, log.as_str(), &bytes);
    };
    // Sync takes both writes away, brings the second back alone, then both.
    replace(lines[..n - 2].concat());
    a.refresh().unwrap();
    replace([&lines[..n - 2], &lines[n - 1..]].concat().concat());
    a.refresh().unwrap();
    replace(bytes.clone());
    let back = a.refresh().unwrap().changes;
    let by_writer: Vec<&Change> = back
        .iter()
        .filter(|change| matches!(change.by, By::Writer { .. }))
        .collect();
    assert_eq!(by_writer, [] as [&Change; 0], "{back:?}");
    assert_eq!(a.0.away(), 0);
}

fn save_untagged<F: Facade>(a: &mut F) {
    let saved = Expect::Holds(identity(b"wave"));
    a.commit("Save", |i| {
        i.save_file(&path("a.wav"), b"wavy".to_vec(), saved)
    })
    .unwrap();
}

fn rename_untagged<F: Facade>(a: &mut F) {
    let renamed = Expect::Holds(identity(b"wave"));
    a.commit("Rename", |i| {
        i.rename_file(&path("a.wav"), &path("b.wav"), renamed)
    })
    .unwrap();
}

/// `refused` is another writer's refusal naming `b` and the entry `by` logged.
fn changed_since_by(refused: Result<Committed, Error>, b: &impl Facade, by: &Committed) {
    let expected = Refusal::ChangedSince {
        by: label_of(&b.view(), "b"),
        entry: by.intent,
    };
    assert!(
        matches!(&refused, Err(Error::Refused(refusal)) if *refusal == expected),
        "{refused:?}"
    );
}

fn an_undo_of_a_save_is_refused_as_changed_since_once_another_writer_renamed_the_file<F: Facade>() {
    let folder = disk();
    let clock = TestClock::at(1_000);
    let (mut a, mut b) = untagged_then::<F>(&folder, &clock, save_untagged);
    let renamed = Expect::Holds(identity(b"wavy"));
    let theirs = b
        .commit("Rename", |i| {
            i.rename_file(&path("a.wav"), &path("b.wav"), renamed)
        })
        .unwrap();
    a.refresh().unwrap();

    changed_since_by(a.undo(), &b, &theirs);
    assert_eq!(read(&folder, "b.wav").as_deref(), Some(&b"wavy"[..]));
    assert_eq!(read(&folder, "a.wav"), None);
}

fn an_undo_of_a_rename_is_refused_as_changed_since_once_another_writer_adopted_the_file<
    F: Facade,
>() {
    let folder = disk();
    let clock = TestClock::at(1_000);
    let (mut a, mut b) = untagged_then::<F>(&folder, &clock, rename_untagged);
    let theirs = b
        .commit("Tag", |i| {
            let (i, _) = i.create(|e| {
                e.adopt(&path("b.wav"), Expect::Holds(identity(b"wave")))
                    .add(TAGS, tag("Live"));
            });
            i
        })
        .unwrap();
    a.refresh().unwrap();

    changed_since_by(a.undo(), &b, &theirs);
    assert_eq!(read(&folder, "b.wav").as_deref(), Some(&b"wave"[..]));
    assert_eq!(read(&folder, "a.wav"), None);
}

fn an_undo_of_a_save_is_refused_as_changed_since_once_a_writer_it_did_not_see_adopted_the_file<
    F: Facade,
>() {
    let folder = disk();
    let clock = TestClock::at(1_000);
    let (mut a, mut b) = untagged_then::<F>(&folder, &clock, |_| {});
    let theirs = b
        .commit("Tag", |i| {
            let (i, _) = i.create(|e| {
                e.adopt(&path("a.wav"), Expect::Holds(identity(b"wave")))
                    .add(TAGS, tag("Live"));
            });
            i
        })
        .unwrap();
    clock.advance(10);
    save_untagged(&mut a);
    a.refresh().unwrap();

    changed_since_by(a.undo(), &b, &theirs);
    assert_eq!(read(&folder, "a.wav").as_deref(), Some(&b"wavy"[..]));
}

fn an_undo_of_a_save_acts_on_the_entity_this_writer_since_made_of_the_file<F: Facade>() {
    let folder = disk();
    let clock = TestClock::at(1_000);
    let (mut a, _b) = untagged_then::<F>(&folder, &clock, save_untagged);
    let tagged = a.commit("Tag", |i| {
        let (i, _) = i.create(|e| {
            e.adopt(&path("a.wav"), Expect::Holds(identity(b"wavy")))
                .add(TAGS, tag("Live"));
        });
        i
    });
    let entity = tagged.unwrap().created[0];
    a.forget_undo();

    let bound = |a: &F| {
        let file = a.view().entity(entity).and_then(|e| e.file());
        file.map(|file| (file.path.as_str().to_string(), file.state))
    };
    let in_sync = Some(("a.wav".to_string(), FileState::InSync));
    a.undo().unwrap();
    assert_eq!(read(&folder, "a.wav").as_deref(), Some(&b"wave"[..]));
    assert_eq!(bound(&a), in_sync, "undone");
    a.redo().unwrap();
    assert_eq!(read(&folder, "a.wav").as_deref(), Some(&b"wavy"[..]));
    assert_eq!(bound(&a), in_sync, "redone");
}

fn an_undo_of_a_save_over_another_writers_entity_found_in_it_is_refused_as_changed_since<
    F: Facade,
>() {
    let folder = disk();
    let clock = TestClock::at(1_000);
    put(&folder, "b.wav", b"wavy");
    let (mut a, mut b) = untagged_then::<F>(&folder, &clock, |_| {});
    let tagged = b.commit("Tag", |i| {
        let (i, _) = i.create(|e| {
            e.adopt(&path("b.wav"), Expect::Holds(identity(b"wavy")))
                .add(TAGS, tag("Live"));
        });
        i
    });
    let tagged = tagged.unwrap();
    let entity = tagged.created[0];
    let removed = Io::Remove {
        root: Root::Folder,
        path: path("b.wav"),
    };
    folder.perform(removed).unwrap();
    a.refresh().unwrap();
    save_untagged(&mut a);
    a.rescan().unwrap();
    let facts = |a: &F| a.refolded().file_writes(entity);
    let before = facts(&a);

    let undone = a.undo();
    let refused = |refused: &Refusal| matches!(refused, Refusal::ChangedSince { entry, .. } if *entry == tagged.intent);
    assert!(
        matches!(&undone, Err(Error::Refused(refusal)) if refused(refusal)),
        "{undone:?}"
    );
    assert_eq!(facts(&a), before, "its file fact is its writer's");
}

/// Where the binding finds `entity`'s file, and its row's state.
fn shown_at<F: Facade>(f: &F, entity: EntityId) -> (Option<(String, FileState)>, Option<RowState>) {
    let view = f.view();
    let file = view.entity(entity).and_then(|e| e.file());
    let file = file.map(|file| (file.path.as_str().to_string(), file.state));
    (file, view.row_of(entity).map(|row| row.state))
}

fn at(text: &str, state: FileState) -> Option<(String, FileState)> {
    Some((text.to_string(), state))
}

/// A writer `a` over `a.wav`, which no entity is bound to, and another writer `b`,
/// with an entity `by` tagged whose file `b.wav`, holding `wavy`, is gone since.
fn lost<F: Facade>(folder: &MemDisk, clock: &TestClock, by: Tagger) -> (F, F, EntityId) {
    put(folder, "b.wav", b"wavy");
    let (mut a, mut b) = untagged_then::<F>(folder, clock, |_| {});
    let tagger = match by {
        Tagger::A => &mut a,
        Tagger::B => &mut b,
    };
    let tagged = tagger.commit("Tag", |i| {
        let (i, _) = i.create(|e| {
            e.adopt(&path("b.wav"), Expect::Holds(identity(b"wavy")))
                .add(TAGS, tag("Live"));
        });
        i
    });
    let entity = tagged.unwrap().created[0];
    let removed = Io::Remove {
        root: Root::Folder,
        path: path("b.wav"),
    };
    folder.perform(removed).unwrap();
    for writer in [&mut a, &mut b] {
        writer.refresh().unwrap();
        writer.rescan().unwrap();
    }
    (a, b, entity)
}

#[derive(Clone, Copy)]
enum Tagger {
    A,
    B,
}

fn save_new<F: Facade>(a: &mut F, text: &str, bytes: &[u8]) {
    a.commit("Save", |i| {
        i.save_file(&path(text), bytes.to_vec(), Expect::Absent)
    })
    .unwrap();
    a.rescan().unwrap();
}

/// Commits and undoes a tag of `entity`, a commit that pins the moves `a` holds.
fn pin<F: Facade>(a: &mut F, entity: EntityId) {
    a.commit("Tag", |i| i.add(entity, TAGS, tag("Kept")))
        .unwrap();
    a.undo().unwrap();
}

/// The path the latest live write of `entity`'s file register names.
fn fact_at<F: Facade>(a: &F, entity: EntityId) -> Option<String> {
    let writes = a.refolded().file_writes(entity);
    let fact = writes.last()?.value.as_ref()?;
    Some(fact.path.as_str().to_string())
}

fn an_undo_of_a_new_file_this_writer_since_made_an_entitys_leaves_that_entity_lost<F: Facade>() {
    let folder = disk();
    let clock = TestClock::at(1_000);
    let (mut a, _b) = untagged_then::<F>(&folder, &clock, |a| save_new(a, "c.wav", b"wavy"));
    let tagged = a.commit("Tag", |i| {
        let (i, _) = i.create(|e| {
            e.adopt(&path("c.wav"), Expect::Holds(identity(b"wavy")))
                .add(TAGS, tag("Live"));
        });
        i
    });
    let entity = tagged.unwrap().created[0];
    a.forget_undo();

    a.undo().unwrap();
    assert_eq!(read(&folder, "c.wav"), None);
    let lost = (at("c.wav", FileState::Missing), Some(RowState::Missing));
    assert_eq!(shown_at(&a, entity), lost, "undone");
    a.redo().unwrap();
    assert_eq!(read(&folder, "c.wav").as_deref(), Some(&b"wavy"[..]));
    assert_eq!(
        shown_at(&a, entity).0,
        at("c.wav", FileState::InSync),
        "redone"
    );
}

fn an_undo_of_a_new_file_a_lost_entity_was_found_in_leaves_that_entity_lost<F: Facade>() {
    let folder = disk();
    let clock = TestClock::at(1_000);
    let (mut a, _b, entity) = lost::<F>(&folder, &clock, Tagger::A);
    save_new(&mut a, "c.wav", b"wavy");
    assert_eq!(
        shown_at(&a, entity).0,
        at("c.wav", FileState::InSync),
        "found"
    );

    a.undo().unwrap();
    assert_eq!(read(&folder, "c.wav"), None);
    let lost = (at("b.wav", FileState::Missing), Some(RowState::Missing));
    assert_eq!(shown_at(&a, entity), lost, "undone");
    a.redo().unwrap();
    assert_eq!(read(&folder, "c.wav").as_deref(), Some(&b"wavy"[..]));
    assert_eq!(
        shown_at(&a, entity).0,
        at("c.wav", FileState::InSync),
        "redone"
    );
}

fn an_undo_of_a_save_over_a_file_a_lost_entity_was_found_in_leaves_that_entity_lost<F: Facade>() {
    let folder = disk();
    let clock = TestClock::at(1_000);
    let (mut a, _b, entity) = lost::<F>(&folder, &clock, Tagger::A);
    save_untagged(&mut a);
    a.rescan().unwrap();
    assert_eq!(
        shown_at(&a, entity).0,
        at("a.wav", FileState::InSync),
        "found"
    );

    a.undo().unwrap();
    assert_eq!(read(&folder, "a.wav").as_deref(), Some(&b"wave"[..]));
    let lost = (at("b.wav", FileState::Missing), Some(RowState::Missing));
    assert_eq!(shown_at(&a, entity), lost, "undone");
    let view = a.view();
    let unbound: Vec<&str> = view.unbound().map(RelPath::as_str).collect();
    assert_eq!(unbound, ["a.wav"]);
    a.redo().unwrap();
    assert_eq!(read(&folder, "a.wav").as_deref(), Some(&b"wavy"[..]));
    assert_eq!(
        shown_at(&a, entity).0,
        at("a.wav", FileState::InSync),
        "redone"
    );
}

fn an_undo_of_a_new_file_a_pin_gave_a_lost_entity_leaves_that_entity_lost<F: Facade>() {
    let folder = disk();
    let clock = TestClock::at(1_000);
    let (mut a, _b, entity) = lost::<F>(&folder, &clock, Tagger::A);
    save_new(&mut a, "c.wav", b"wavy");
    pin(&mut a, entity);
    assert_eq!(fact_at(&a, entity).as_deref(), Some("c.wav"), "pinned");

    a.undo().unwrap();
    assert_eq!(read(&folder, "c.wav"), None);
    let lost = (at("c.wav", FileState::Missing), Some(RowState::Missing));
    assert_eq!(shown_at(&a, entity), lost, "undone");
    a.redo().unwrap();
    assert_eq!(read(&folder, "c.wav").as_deref(), Some(&b"wavy"[..]));
    assert_eq!(
        shown_at(&a, entity).0,
        at("c.wav", FileState::InSync),
        "redone"
    );
}

fn an_undo_of_a_save_over_a_file_a_pin_gave_a_lost_entity_brings_bytes_back_as_its_file<
    F: Facade,
>() {
    let folder = disk();
    let clock = TestClock::at(1_000);
    let (mut a, _b, entity) = lost::<F>(&folder, &clock, Tagger::A);
    save_untagged(&mut a);
    a.rescan().unwrap();
    pin(&mut a, entity);
    assert_eq!(fact_at(&a, entity).as_deref(), Some("a.wav"), "pinned");

    a.undo().unwrap();
    assert_eq!(read(&folder, "a.wav").as_deref(), Some(&b"wave"[..]));
    assert_eq!(
        shown_at(&a, entity).0,
        at("a.wav", FileState::InSync),
        "undone"
    );
    a.redo().unwrap();
    assert_eq!(read(&folder, "a.wav").as_deref(), Some(&b"wavy"[..]));
    assert_eq!(
        shown_at(&a, entity).0,
        at("a.wav", FileState::InSync),
        "redone"
    );
}

fn an_undo_of_a_save_is_not_refused_over_another_writers_pin_of_the_entity_found_in_it<
    F: Facade,
>() {
    let folder = disk();
    let clock = TestClock::at(1_000);
    let (mut a, mut b, entity) = lost::<F>(&folder, &clock, Tagger::B);
    save_untagged(&mut a);
    b.refresh().unwrap();
    b.rescan().unwrap();
    b.commit("Tag", |i| i.add(entity, TAGS, tag("Kept")))
        .unwrap();
    a.refresh().unwrap();
    assert_eq!(fact_at(&a, entity).as_deref(), Some("a.wav"), "pinned");

    a.undo().unwrap();
    assert_eq!(read(&folder, "a.wav").as_deref(), Some(&b"wave"[..]));
    assert_eq!(
        shown_at(&a, entity).0,
        at("a.wav", FileState::InSync),
        "undone"
    );
}

fn an_undo_of_a_rename_moves_back_the_file_another_writers_entity_was_found_in<F: Facade>() {
    let folder = disk();
    let clock = TestClock::at(1_000);
    put(&folder, "x.wav", b"wave");
    let (mut a, mut b) = untagged_then::<F>(&folder, &clock, |_| {});
    let tagged = b.commit("Tag", |i| {
        let (i, _) = i.create(|e| {
            e.adopt(&path("x.wav"), Expect::Holds(identity(b"wave")))
                .add(TAGS, tag("Live"));
        });
        i
    });
    let entity = tagged.unwrap().created[0];
    a.refresh().unwrap();
    rename_untagged(&mut a);
    let removed = Io::Remove {
        root: Root::Folder,
        path: path("x.wav"),
    };
    folder.perform(removed).unwrap();
    a.rescan().unwrap();
    let bound = |a: &F| {
        let file = a.view().entity(entity).and_then(|e| e.file());
        file.map(|file| (file.path.as_str().to_string(), file.state))
    };
    assert_eq!(bound(&a), Some(("b.wav".to_string(), FileState::InSync)));

    a.undo().unwrap();
    assert_eq!(read(&folder, "a.wav").as_deref(), Some(&b"wave"[..]));
    assert_eq!(bound(&a), Some(("a.wav".to_string(), FileState::InSync)));
}

fn a_redo_of_a_rename_is_refused_as_changed_since_once_another_writer_trashed_the_file<
    F: Facade,
>() {
    let folder = disk();
    let clock = TestClock::at(1_000);
    let (mut a, mut b) = untagged_then::<F>(&folder, &clock, |a| {
        rename_untagged(a);
        a.undo().unwrap();
    });
    let trashed = Expect::Holds(identity(b"wave"));
    let theirs = b
        .commit("Delete", |i| i.trash_file(&path("a.wav"), trashed))
        .unwrap();
    a.refresh().unwrap();

    changed_since_by(a.redo(), &b, &theirs);
    assert_eq!(read(&folder, "a.wav"), None);
    assert_eq!(read(&folder, "b.wav"), None);
}

/// A writer `a` that moved the tree `old`, holding `a.wav` no entity is bound to, to
/// `new`, then `then` did, and another writer `b` that has read what `a` did.
fn tree_then<F: Facade>(folder: &MemDisk, clock: &TestClock, then: impl FnOnce(&mut F)) -> (F, F) {
    put(folder, "old/a.wav", b"wave");
    let (mut a, _) = F::open(Probe::new(&machine(folder)), env("a", 1, clock)).unwrap();
    a.rescan().unwrap();
    a.commit("Move", |i| i.move_tree(&path("old"), &path("new")))
        .unwrap();
    then(&mut a);
    let (mut b, _) = F::open(Probe::new(&machine(folder)), env("b", 2, clock)).unwrap();
    b.rescan().unwrap();
    (a, b)
}

fn move_tree<F: Facade>(b: &mut F, from: &str, to: &str) -> Committed {
    b.commit("Move", |i| i.move_tree(&path(from), &path(to)))
        .unwrap()
}

/// Removes the folder `at` as an app does: its file moves up, and the folder goes.
fn remove_tree<F: Facade>(b: &mut F, at: &str) -> Committed {
    let file = path(at).join("a.wav").unwrap();
    let moved = Expect::Holds(identity(b"wave"));
    b.commit("Remove folder", |i| {
        i.rename_file(&file, &path("a.wav"), moved)
            .remove_dir(&path(at))
    })
    .unwrap()
}

fn an_undo_of_a_tree_move_is_refused_as_changed_since_once_another_writer_moved_the_tree<
    F: Facade,
>() {
    let folder = disk();
    let clock = TestClock::at(1_000);
    let (mut a, mut b) = tree_then::<F>(&folder, &clock, |_| {});
    let theirs = move_tree(&mut b, "new", "newer");
    a.refresh().unwrap();

    changed_since_by(a.undo(), &b, &theirs);
    assert_eq!(read(&folder, "newer/a.wav").as_deref(), Some(&b"wave"[..]));
}

fn a_redo_of_a_tree_move_is_refused_as_changed_since_once_another_writer_moved_the_tree<
    F: Facade,
>() {
    let folder = disk();
    let clock = TestClock::at(1_000);
    let (mut a, mut b) = tree_then::<F>(&folder, &clock, |a| {
        a.undo().unwrap();
    });
    let theirs = move_tree(&mut b, "old", "older");
    a.refresh().unwrap();

    changed_since_by(a.redo(), &b, &theirs);
    assert_eq!(read(&folder, "older/a.wav").as_deref(), Some(&b"wave"[..]));
}

fn an_undo_of_a_tree_move_is_refused_as_changed_since_once_another_writer_removed_the_tree<
    F: Facade,
>() {
    let folder = disk();
    let clock = TestClock::at(1_000);
    let (mut a, mut b) = tree_then::<F>(&folder, &clock, |_| {});
    let theirs = remove_tree(&mut b, "new");
    a.refresh().unwrap();

    changed_since_by(a.undo(), &b, &theirs);
    assert_eq!(read(&folder, "a.wav").as_deref(), Some(&b"wave"[..]));
    assert_eq!(read(&folder, "old/a.wav"), None);
}

fn a_redo_of_a_tree_move_is_refused_as_changed_since_once_another_writer_removed_the_tree<
    F: Facade,
>() {
    let folder = disk();
    let clock = TestClock::at(1_000);
    let (mut a, mut b) = tree_then::<F>(&folder, &clock, |a| {
        a.undo().unwrap();
    });
    let theirs = remove_tree(&mut b, "old");
    a.refresh().unwrap();

    changed_since_by(a.redo(), &b, &theirs);
    assert_eq!(read(&folder, "a.wav").as_deref(), Some(&b"wave"[..]));
    assert_eq!(read(&folder, "new/a.wav"), None);
}

/// Writers `a` and `b` on one folder, `b` having read what `then` did as `a`.
fn a_then<F: Facade>(folder: &MemDisk, clock: &TestClock, then: impl FnOnce(&mut F)) -> (F, F) {
    let (mut a, _) = F::open(Probe::new(&machine(folder)), env("a", 1, clock)).unwrap();
    a.rescan().unwrap();
    then(&mut a);
    let (mut b, _) = F::open(Probe::new(&machine(folder)), env("b", 2, clock)).unwrap();
    b.rescan().unwrap();
    (a, b)
}

fn make_gig<F: Facade>(a: &mut F) {
    a.commit("New folder", |i| i.make_dir(&path("Gig")))
        .unwrap();
}

fn an_undo_of_a_new_folder_is_refused_as_changed_since_once_another_writer_put_a_file_in_it<
    F: Facade,
>() {
    let folder = disk();
    let clock = TestClock::at(1_000);
    let (mut a, mut b) = a_then::<F>(&folder, &clock, make_gig);
    let theirs = b
        .commit("Add", |i| {
            i.save_file(&path("Gig/a.wav"), b"wave".to_vec(), Expect::Absent)
        })
        .unwrap();
    a.refresh().unwrap();

    changed_since_by(a.undo(), &b, &theirs);
    assert_eq!(read(&folder, "Gig/a.wav").as_deref(), Some(&b"wave"[..]));
}

fn an_undo_of_a_folder_removal_is_refused_as_changed_since_once_another_writer_made_it_again<
    F: Facade,
>() {
    let folder = disk();
    let clock = TestClock::at(1_000);
    let (mut a, mut b) = a_then::<F>(&folder, &clock, |a| {
        make_gig(a);
        a.commit("Remove folder", |i| i.remove_dir(&path("Gig")))
            .unwrap();
    });
    let theirs = b
        .commit("New folder", |i| i.make_dir(&path("Gig")))
        .unwrap();
    a.refresh().unwrap();

    changed_since_by(a.undo(), &b, &theirs);
    assert!(folder.directories(Root::Folder).contains(&path("Gig")));
}

fn an_undo_of_a_new_folder_is_refused_as_not_empty_once_a_file_was_put_in_it_outside<F: Facade>() {
    let folder = disk();
    let clock = TestClock::at(1_000);
    let (mut a, _) = a_then::<F>(&folder, &clock, make_gig);
    put(&folder, "Gig/a.wav", b"wave");
    a.rescan().unwrap();

    let refused = a.undo();
    assert!(
        matches!(
            refused,
            Err(Error::Refused(Refusal::Invalid(Invalid::NotEmpty(_))))
        ),
        "{refused:?}"
    );
    assert_eq!(read(&folder, "Gig/a.wav").as_deref(), Some(&b"wave"[..]));
}

fn an_undo_of_a_move_is_refused_as_changed_since_once_another_writer_deleted_the_entity<
    F: Facade,
>() {
    let folder = disk();
    let clock = TestClock::at(1_000);
    let mut song = None;
    let (mut a, mut b) = a_then::<F>(&folder, &clock, |a| {
        let made = create(a, "song.npno", b"song");
        let moved = Expect::Holds(identity(b"song"));
        a.commit("Move", |i| i.rename(made, &path("Set/song.npno"), moved))
            .unwrap();
        song = Some(made);
    });
    let song = song.unwrap();
    let trashed = Expect::Holds(identity(b"song"));
    let theirs = b
        .commit("Delete", |i| i.delete(song).trash(song, trashed))
        .unwrap();
    a.refresh().unwrap();

    changed_since_by(a.undo(), &b, &theirs);
    assert_eq!(read(&folder, "song.npno"), None);
    assert_eq!(read(&folder, "Set/song.npno"), None);
}

fn an_undo_of_a_tag_a_delete_saw_is_refused_though_the_deleters_clock_lags<F: Facade>() {
    let folder = disk();
    let ahead = TestClock::at(1_000 + 2 * 24 * 60 * 60 * 1000);
    let behind = TestClock::at(1_000);
    let (mut a, _) = F::open(Probe::new(&machine(&folder)), env("a", 1, &ahead)).unwrap();
    a.rescan().unwrap();
    let song = create(&mut a, "song.npno", b"song");
    a.commit("Tag", |i| i.add(song, TAGS, tag("kept"))).unwrap();
    let (mut b, _) = F::open(Probe::new(&machine(&folder)), env("b", 2, &behind)).unwrap();
    b.rescan().unwrap();
    assert_eq!(tags(&b.view(), song), ["kept", "new"]);
    let theirs = b.commit("Forget", |i| i.delete(song)).unwrap();
    a.refresh().unwrap();
    changed_since_by(a.undo(), &b, &theirs);
    changed_since_by(a.undo(), &b, &theirs);
    let last = a.history().last().cloned().unwrap();
    assert_eq!((last.label.as_str(), last.undone), ("Tag", false));
}

fn an_undo_beside_a_concurrent_delete_goes_ahead_whichever_clock_leads<F: Facade>() {
    for lead in [-5_000, 5_000] {
        let folder = disk();
        let clock = TestClock::at(10_000);
        let theirs = TestClock::at(10_000_i64.saturating_add(lead) as u64);
        let (mut a, _) = F::open(Probe::new(&machine(&folder)), env("a", 1, &clock)).unwrap();
        a.rescan().unwrap();
        let song = create(&mut a, "song.npno", b"song");
        let (mut b, _) = F::open(Probe::new(&machine(&folder)), env("b", 2, &theirs)).unwrap();
        b.rescan().unwrap();
        let held = Expect::Holds(identity(b"song"));
        a.commit("Move", |i| i.rename(song, &path("Set/song.npno"), held))
            .unwrap();
        b.commit("Forget", |i| i.delete(song)).unwrap();
        a.refresh().unwrap();
        assert!(a.view().entity(song).unwrap().deletion_conflicted());
        let undone = a.undo();
        assert!(undone.is_ok(), "lead {lead}: {undone:?}");
        assert_eq!(read(&folder, "song.npno").as_deref(), Some(&b"song"[..]));
    }
}

fn an_undo_of_a_save_where_the_file_was_missing_leaves_it_missing_again<F: Facade>() {
    let folder = disk();
    let clock = TestClock::at(1_000);
    let (mut a, _) = F::open(Probe::new(&machine(&folder)), env("a", 1, &clock)).unwrap();
    let song = create(&mut a, "song.npno", b"song");
    remove_outside(&folder, "song.npno");
    a.rescan().unwrap();
    let lost = row_of(&a.view(), song);
    assert_eq!(lost.state, RowState::Missing);
    let anew = b"anew".to_vec();
    a.commit("New", |i| {
        i.save(song, &path("song.npno"), anew, Expect::Absent)
    })
    .unwrap();
    let undone = a.undo().unwrap();
    assert!(!undone.changes.is_empty(), "{undone:?}");
    assert_eq!(
        read(&folder, "song.npno"),
        None,
        "the saved file is trashed"
    );
    let view = a.view();
    assert_eq!(row_of(&view, song), lost, "missing at its old path again");
    assert_eq!(tags(&view, song), ["new"]);
    a.rescan().unwrap();
    assert_eq!(row_of(&a.view(), song), lost);
    a.redo().unwrap();
    assert_eq!(read(&folder, "song.npno").as_deref(), Some(&b"anew"[..]));
    let row = row_of(&a.view(), song);
    assert!(
        matches!(row.state, RowState::Present { len: 4, .. }),
        "{row:?}"
    );
    a.undo().unwrap();
    assert_eq!(read(&folder, "song.npno"), None, "undone again");
    assert_eq!(row_of(&a.view(), song).state, RowState::Missing);
}

fn a_redo_of_a_save_of_a_missing_file_under_another_name_puts_it_back_there<F: Facade>() {
    let folder = disk();
    let clock = TestClock::at(1_000);
    let (mut a, _) = F::open(Probe::new(&machine(&folder)), env("a", 1, &clock)).unwrap();
    let song = create(&mut a, "song.npno", b"song");
    remove_outside(&folder, "song.npno");
    a.rescan().unwrap();
    let lost = row_of(&a.view(), song);
    a.commit("New", |i| {
        i.save(song, &path("song 2.npno"), b"anew".to_vec(), Expect::Absent)
    })
    .unwrap();
    a.undo().unwrap();
    assert_eq!(
        read(&folder, "song 2.npno"),
        None,
        "the saved file is trashed"
    );
    assert_eq!(
        row_of(&a.view(), song),
        lost,
        "missing at its old path again"
    );
    let redone = a.redo();
    assert!(redone.is_ok(), "{redone:?}");
    assert_eq!(read(&folder, "song 2.npno").as_deref(), Some(&b"anew"[..]));
    let row = row_of(&a.view(), song);
    assert_eq!(row.path.as_str(), "song 2.npno", "{row:?}");
    assert!(matches!(row.state, RowState::Present { .. }), "{row:?}");
}

fn an_undo_pins_no_move_the_intent_it_reverses_made_plain<F: Facade>() {
    let folder = disk();
    let clock = TestClock::at(1_000);
    let (mut a, _) = F::open(Probe::new(&machine(&folder)), env("a", 1, &clock)).unwrap();
    let grand = create(&mut a, "grand.npno", b"grand");
    put(&folder, "live/grand.npno", b"grand");
    put(&folder, "live/sound.npno", b"grand");
    remove_outside(&folder, "grand.npno");
    a.rescan().unwrap();
    let before = row_of(&a.view(), grand);
    assert_eq!(before.state, RowState::Missing, "two files could be it");
    let held = Expect::Holds(identity(b"grand"));
    a.commit("Save", |i| {
        i.save_file(&path("live/grand.npno"), b"edited".to_vec(), held)
    })
    .unwrap();
    assert_eq!(row_of(&a.view(), grand).path.as_str(), "live/sound.npno");
    a.undo().unwrap();
    let view = a.view();
    assert_eq!(row_of(&view, grand), before, "as before the save");
    assert_eq!(row_at(&view, "live/sound.npno").entity, None);
}

fn a_tag_on_an_entity_whose_making_entry_is_away_is_undone<F: Facade>() {
    let folder = disk();
    let clock = TestClock::at(1_000);
    put(&folder, "strings.npno", b"strings");
    let (mut a, _) = F::open(Probe::new(&machine(&folder)), env("a", 1, &clock)).unwrap();
    a.rescan().unwrap();
    let (mut b, _) = F::open(Probe::new(&machine(&folder)), env("b", 2, &clock)).unwrap();
    b.rescan().unwrap();
    let made = b.commit("Tag", |i| {
        let (i, _) = i.create(|e| {
            e.adopt(&path("strings.npno"), Expect::Holds(identity(b"strings")))
                .add(TAGS, tag("Live"));
        });
        i
    });
    let strings = made.unwrap().created[0];
    a.refresh().unwrap();
    let held = Expect::Holds(identity(b"strings"));
    a.commit("Rename", |i| i.rename(strings, &path("sound.npno"), held))
        .unwrap();
    let theirs = layout().writer(label_of(&a.view(), "b"));
    for (at, _) in folder.files(Root::Folder) {
        if at.starts_with(&theirs) && at.as_str().ends_with(SEGMENT_EXTENSION) {
            remove_outside(&folder, at.as_str());
        }
    }
    a.refresh().unwrap();
    assert!(
        a.view().entity(strings).is_some(),
        "shown by this writer's file fact"
    );
    a.commit("Tag", |i| i.add(strings, TAGS, tag("Studio")))
        .unwrap();
    let undone = a.undo();
    assert!(undone.is_ok(), "{undone:?}");
    assert!(!tags(&a.view(), strings).contains(&tag("Studio")));
}

fn writes_whose_entries_go_away_and_come_back_are_no_writers_change<F: Facade>() {
    let folder = disk();
    let clock = TestClock::at(1_000);
    put(&folder, "strings.npno", b"strings");
    let (mut a, _) = F::open(Probe::new(&machine(&folder)), env("a", 1, &clock)).unwrap();
    a.rescan().unwrap();
    let (mut b, _) = F::open(Probe::new(&machine(&folder)), env("b", 2, &clock)).unwrap();
    b.rescan().unwrap();
    let made = b.commit("Tag", |i| {
        let (i, _) = i.create(|e| {
            e.adopt(&path("strings.npno"), Expect::Holds(identity(b"strings")))
                .add(TAGS, tag("Live"));
        });
        i
    });
    let strings = made.unwrap().created[0];
    a.refresh().unwrap();
    let held = Expect::Holds(identity(b"strings"));
    a.commit("Rename", |i| i.rename(strings, &path("sound.npno"), held))
        .unwrap();
    let theirs = layout().writer(label_of(&a.view(), "b"));
    let segments: Vec<(RelPath, Vec<u8>)> = folder
        .files(Root::Folder)
        .into_iter()
        .filter(|(at, _)| at.starts_with(&theirs) && at.as_str().ends_with(SEGMENT_EXTENSION))
        .collect();
    let tags_changed = |changes: &[Change]| -> Vec<By> {
        let tagged = changes.iter().filter(|change| change.entity == strings);
        let tagged = tagged.filter(|change| change.what == What::Field("tags".into()));
        tagged.map(|change| change.by.clone()).collect()
    };
    for (at, _) in &segments {
        remove_outside(&folder, at.as_str());
    }
    let away = a.refresh().unwrap().changes;
    assert_eq!(tags(&a.view(), strings), Vec::<String>::new());
    assert_eq!(tags_changed(&away), [By::Outside], "{away:?}");
    for (at, bytes) in &segments {
        put(&folder, at.as_str(), bytes);
    }
    let back = a.refresh().unwrap().changes;
    assert_eq!(tags(&a.view(), strings), ["Live"]);
    assert_eq!(tags_changed(&back), [By::Outside], "{back:?}");
    b.refresh().unwrap();
    b.commit("Tag", |i| i.add(strings, TAGS, tag("Gig")))
        .unwrap();
    let tagged = a.refresh().unwrap().changes;
    let by_b = By::Writer {
        writer: label_of(&a.view(), "b"),
        label: "b".into(),
    };
    assert_eq!(tags_changed(&tagged), [by_b], "{tagged:?}");
}

/// The segments of the writer `label` in `folder`, with their bytes.
fn segments_of(folder: &MemDisk, view: &View, label: &str) -> Vec<(RelPath, Vec<u8>)> {
    let theirs = layout().writer(label_of(view, label));
    let files = folder.files(Root::Folder).into_iter();
    let segments = files.filter(|(at, _)| at.starts_with(&theirs));
    segments
        .filter(|(at, _)| at.as_str().ends_with(SEGMENT_EXTENSION))
        .collect()
}

fn the_row_of_an_entity_whose_entries_go_away_and_come_back_names_no_writer<F: Facade>() {
    let folder = disk();
    let clock = TestClock::at(1_000);
    let (mut a, _) = F::open(Probe::new(&machine(&folder)), env("a", 1, &clock)).unwrap();
    a.rescan().unwrap();
    let (mut b, _) = F::open(Probe::new(&machine(&folder)), env("b", 2, &clock)).unwrap();
    b.rescan().unwrap();
    let song = create(&mut b, "song.npno", b"song");
    a.refresh().unwrap();
    let file = row_of(&a.view(), song).file;
    let segments = segments_of(&folder, &a.view(), "b");
    for (at, _) in &segments {
        remove_outside(&folder, at.as_str());
    }
    let before = a.view();
    a.refresh().unwrap();
    let away = a.view();
    assert_eq!(away.row(file).unwrap().entity, None);
    assert_eq!(
        changes_of(&away.changes_since(&before), file),
        [(RowWhat::Lost(song), Some(Who::Outside), None)]
    );
    for (at, bytes) in &segments {
        put(&folder, at.as_str(), bytes);
    }
    a.refresh().unwrap();
    let back = a.view();
    assert_eq!(
        changes_of(&back.changes_since(&away), file),
        [(RowWhat::Gained(song), Some(Who::Outside), None)]
    );
    assert_eq!(
        back.changes_since(&before).files,
        [],
        "nothing changed across both"
    );
}

fn a_write_whose_entry_goes_away_beside_a_real_change_names_no_writer_on_its_return<F: Facade>() {
    let folder = disk();
    let clock = TestClock::at(1_000);
    let open = |label, seed| {
        let (mut writer, _) =
            F::open(Probe::new(&machine(&folder)), env(label, seed, &clock)).unwrap();
        writer.rescan().unwrap();
        writer
    };
    let mut a = open("a", 1);
    let song = create(&mut a, "song.npno", b"song");
    let (mut b, mut c) = (open("b", 2), open("c", 3));
    b.commit("Origin", |i| i.set(song, ORIGIN, "b".into()))
        .unwrap();
    a.refresh().unwrap();
    let segments = segments_of(&folder, &a.view(), "b");
    c.commit("Origin", |i| i.set(song, ORIGIN, "c".into()))
        .unwrap();
    for (at, _) in &segments {
        remove_outside(&folder, at.as_str());
    }
    let origin_by = |changes: Vec<Change>| -> Vec<By> {
        let origin = changes.into_iter().filter(|change| change.entity == song);
        let origin = origin.filter(|change| change.what == What::Field("origin".into()));
        origin.map(|change| change.by).collect()
    };
    let replaced = a.refresh().unwrap().changes;
    let by_c = By::Writer {
        writer: label_of(&a.view(), "c"),
        label: "c".into(),
    };
    assert_eq!(origin_by(replaced), [by_c], "the real change is c's");
    for (at, bytes) in &segments {
        put(&folder, at.as_str(), bytes);
    }
    let back = a.refresh().unwrap().changes;
    assert_eq!(origin_by(back), [By::Outside], "b's write came back");
}

fn an_undo_of_a_move_made_after_another_writers_delete_it_saw_is_not_refused_by_it<F: Facade>() {
    let folder = disk();
    let clock = TestClock::at(1_000);
    let mut song = None;
    let (mut a, mut b) = a_then::<F>(&folder, &clock, |a| {
        song = Some(create(a, "song.npno", b"song"));
    });
    let song = song.unwrap();
    b.commit("Forget", |i| i.delete(song)).unwrap();
    let held = Expect::Holds(identity(b"song"));
    a.commit("Move", |i| i.rename(song, &path("Set/song.npno"), held))
        .unwrap();
    a.refresh().unwrap();
    assert!(a.view().entity(song).unwrap().deletion_conflicted());
    a.commit("Rename", |i| i.rename(song, &path("Set/tune.npno"), held))
        .unwrap();

    a.undo().unwrap();
    assert_eq!(
        read(&folder, "Set/song.npno").as_deref(),
        Some(&b"song"[..])
    );
    assert_eq!(read(&folder, "Set/tune.npno"), None);
}

fn an_undo_of_a_delete_is_refused_as_changed_since_once_another_writer_made_a_file_at_its_path<
    F: Facade,
>() {
    let folder = disk();
    let clock = TestClock::at(1_000);
    let (mut a, mut b) = a_then::<F>(&folder, &clock, |a| {
        let song = create(a, "song.npno", b"song");
        let trashed = Expect::Holds(identity(b"song"));
        a.commit("Delete", |i| i.delete(song).trash(song, trashed))
            .unwrap();
    });
    let theirs = b
        .commit("Import", |i| {
            let (i, _) = i.create(|e| {
                e.save(&path("song.npno"), b"tune".to_vec(), Expect::Absent);
            });
            i
        })
        .unwrap();
    a.refresh().unwrap();

    changed_since_by(a.undo(), &b, &theirs);
    assert_eq!(read(&folder, "song.npno").as_deref(), Some(&b"tune"[..]));
}

fn an_undo_of_a_tree_move_is_refused_as_changed_once_the_tree_was_removed_outside<F: Facade>() {
    let folder = disk();
    let clock = TestClock::at(1_000);
    let (mut a, _) = tree_then::<F>(&folder, &clock, |_| {});
    move_outside(&folder, "new/a.wav", "a.wav");
    folder
        .perform(Io::RemoveDir {
            root: Root::Folder,
            path: path("new"),
        })
        .unwrap();
    a.rescan().unwrap();

    let refused = a.undo();
    assert!(
        matches!(refused, Err(Error::Refused(Refusal::Changed(_)))),
        "{refused:?}"
    );
    assert_eq!(read(&folder, "a.wav").as_deref(), Some(&b"wave"[..]));
}

fn an_undo_of_a_rename_is_refused_as_changed_once_the_file_moved_outside<F: Facade>() {
    let folder = disk();
    let clock = TestClock::at(1_000);
    let (mut a, _) = untagged_then::<F>(&folder, &clock, rename_untagged);
    move_outside(&folder, "b.wav", "c.wav");
    a.rescan().unwrap();

    let refused = a.undo();
    assert!(
        matches!(refused, Err(Error::Refused(Refusal::Changed(_)))),
        "{refused:?}"
    );
    assert_eq!(read(&folder, "c.wav").as_deref(), Some(&b"wave"[..]));
}

fn a_folder_holding_no_file_is_renamed_and_named_back_by_undo<F: Facade>() {
    let folder = disk();
    let clock = TestClock::at(1_000);
    let (mut a, _) = F::open(Probe::new(&machine(&folder)), env("a", 1, &clock)).unwrap();
    a.rescan().unwrap();
    a.commit("New folder", |i| i.make_dir(&path("old")))
        .unwrap();
    let dirs = || {
        let dirs = folder.directories(Root::Folder).into_iter();
        let dirs = dirs.filter(|dir| !dir.starts_with(&path(".t")) && !dir.is_root());
        dirs.map(|dir| dir.as_str().to_owned())
            .collect::<Vec<String>>()
    };
    a.commit("Rename", |i| i.move_tree(&path("old"), &path("new")))
        .unwrap();
    assert_eq!(dirs(), ["new"], "renamed");
    a.undo().unwrap();
    assert_eq!(dirs(), ["old"], "undone");
    a.redo().unwrap();
    assert_eq!(dirs(), ["new"], "redone");
}

fn empty_folders_dragged_into_a_new_folder_and_out_of_a_removed_one_undo_and_redo<F: Facade>() {
    let folder = disk();
    let clock = TestClock::at(1_000);
    for dir in ["A", "B", "Gig", "Gig/C"] {
        folder
            .perform(Io::MakeDir {
                root: Root::Folder,
                path: path(dir),
            })
            .unwrap();
    }
    put(&folder, "x.wav", b"x");
    let (mut a, _) = F::open(Probe::new(&machine(&folder)), env("a", 1, &clock)).unwrap();
    a.rescan().unwrap();
    let dirs = || {
        let dirs = folder.directories(Root::Folder).into_iter();
        let dirs = dirs.filter(|dir| !dir.starts_with(&path(".t")) && !dir.is_root());
        dirs.map(|dir| dir.as_str().to_owned())
            .collect::<Vec<String>>()
    };
    a.commit("Move", |i| {
        i.make_dir(&path("Set"))
            .move_tree(&path("A"), &path("Set/A"))
            .move_tree(&path("B"), &path("Set/B"))
    })
    .unwrap();
    assert_eq!(dirs(), ["Gig", "Gig/C", "Set", "Set/A", "Set/B"]);
    a.commit("Remove folder", |i| {
        i.move_tree(&path("Gig/C"), &path("C"))
            .remove_dir(&path("Gig"))
    })
    .unwrap();
    let moved = ["C", "Set", "Set/A", "Set/B"];
    assert_eq!(dirs(), moved);
    a.undo().unwrap();
    assert_eq!(dirs(), ["Gig", "Gig/C", "Set", "Set/A", "Set/B"]);
    a.undo().unwrap();
    assert_eq!(dirs(), ["A", "B", "Gig", "Gig/C"]);
    a.redo().unwrap();
    a.redo().unwrap();
    assert_eq!(dirs(), moved);
}

fn a_tree_move_is_undone_by_one_rename_where_directories_rename<F: Facade>() {
    let folder = MemDisk::with_capabilities(Capabilities::ALL, Capabilities::ALL);
    let clock = TestClock::at(1_000);
    for name in ["a", "b", "c"] {
        put(&folder, &format!("old/{name}.wav"), name.as_bytes());
    }
    let probe = Probe::new(&machine(&folder));
    let (mut a, _) = F::open(probe.clone(), env("a", 1, &clock)).unwrap();
    a.rescan().unwrap();
    create(&mut a, "old/song.npno", b"song");
    a.commit("Move", |i| i.move_tree(&path("old"), &path("new")))
        .unwrap();
    let before = probe.library_renames();
    a.undo().unwrap();
    assert_eq!(probe.library_renames() - before, 1, "renames of the undo");
    let files = library_files(&folder).into_keys();
    assert_eq!(
        files
            .map(|path| path.as_str().to_owned())
            .collect::<Vec<_>>(),
        ["old/a.wav", "old/b.wav", "old/c.wav", "old/song.npno"]
    );
}

fn a_tree_move_is_not_undone_once_an_entitys_file_it_moved_changed<F: Facade>() {
    let folder = disk();
    let clock = TestClock::at(1_000);
    put(&folder, "old/a.wav", b"a");
    let (mut a, _) = F::open(Probe::new(&machine(&folder)), env("a", 1, &clock)).unwrap();
    a.rescan().unwrap();
    create(&mut a, "old/song.npno", b"song");
    a.commit("Move", |i| i.move_tree(&path("old"), &path("new")))
        .unwrap();
    put(&folder, "new/song.npno", b"changed");
    let refused = a.undo();
    assert!(
        matches!(refused, Err(Error::Refused(Refusal::Changed(_)))),
        "{refused:?}"
    );
    let files = library_files(&folder).into_keys();
    assert_eq!(
        files
            .map(|path| path.as_str().to_owned())
            .collect::<Vec<_>>(),
        ["new/a.wav", "new/song.npno"],
        "nothing moved back"
    );
}

fn a_tree_move_from_a_folder_moved_since_the_last_scan_is_refused_and_writing_goes_on<F: Facade>() {
    let folder = disk();
    let clock = TestClock::at(1_000);
    put(&folder, "old/a.wav", b"a");
    let (mut a, _) = F::open(Probe::new(&machine(&folder)), env("a", 1, &clock)).unwrap();
    a.rescan().unwrap();
    let song = create(&mut a, "old/song.npno", b"song");
    for (at, bytes) in [("old/a.wav", &b"a"[..]), ("old/song.npno", b"song")] {
        let gone = Io::Remove {
            root: Root::Folder,
            path: path(at),
        };
        folder.perform(gone).unwrap();
        put(&folder, &at.replace("old", "elsewhere"), bytes);
    }
    let gone = Io::RemoveDir {
        root: Root::Folder,
        path: path("old"),
    };
    folder.perform(gone).unwrap();
    let refused = a.commit("Move", |i| i.move_tree(&path("old"), &path("new")));
    assert!(
        matches!(refused, Err(Error::Refused(Refusal::Changed(_)))),
        "{refused:?}"
    );
    assert_eq!(pending_files(&folder), [], "no record is left open");
    a.commit("Tag", |i| i.add(song, TAGS, tag("x")))
        .expect("the writer goes on writing");
    assert_eq!(a.view().writers().len(), 1, "no writer is retired");
}

fn a_file_no_entity_is_bound_to_goes_back_with_its_tree_as_it_is<F: Facade>() {
    let folder = disk();
    let clock = TestClock::at(1_000);
    put(&folder, "old/a.wav", b"a");
    let (mut a, _) = F::open(Probe::new(&machine(&folder)), env("a", 1, &clock)).unwrap();
    a.rescan().unwrap();
    a.commit("Move", |i| i.move_tree(&path("old"), &path("new")))
        .unwrap();
    put(&folder, "new/a.wav", b"changed");
    a.undo().unwrap();
    assert_eq!(read(&folder, "old/a.wav").as_deref(), Some(&b"changed"[..]));
}

fn adopting_a_file_reads_no_other_file_of_its_length<F: Facade>() {
    let folder = disk();
    let clock = TestClock::at(1_000);
    for n in 0..20 {
        put(
            &folder,
            &format!("{n:02}.wav"),
            format!("{n:02}").as_bytes(),
        );
    }
    let probe = Probe::new(&machine(&folder));
    let (mut a, _) = F::open(probe.clone(), env("a", 1, &clock)).unwrap();
    a.rescan().unwrap();
    let before = probe.read_files();
    let committed = a
        .commit("Tag", |i| {
            i.create(|e| {
                e.adopt(&path("07.wav"), Expect::Holds(identity(b"07")))
                    .add(TAGS, tag("live"));
            })
            .0
        })
        .unwrap();
    let song = committed.created[0];
    let file = a.view().entity(song).unwrap().file().unwrap();
    assert_eq!((file.path, file.state), (path("07.wav"), FileState::InSync));
    let read: BTreeSet<RelPath> = probe.read_files().difference(&before).cloned().collect();
    assert_eq!(
        read,
        BTreeSet::from([path("07.wav")]),
        "the adopted file alone is read"
    );
}

fn two_windows_session_after_session_keep_a_pool_of_two<F: Facade>() {
    let folder = disk();
    let clock = TestClock::at(1_000);
    let here = machine(&folder);
    let pool = || {
        let dirs = here.local.directories(Root::Local).into_iter();
        let writers = dirs.filter(|dir| dir.parent().is_some_and(|up| up.is_root()));
        writers.count()
    };
    for session in 0..3 {
        let mut windows = [1, 2].map(|n| {
            let seed = 10 * session + n;
            F::open(Probe::new(&here), env("w", seed, &clock))
                .unwrap()
                .0
        });
        for (n, window) in windows.iter_mut().enumerate() {
            clock.advance(10);
            let name = format!("{session}-{n}");
            window
                .commit("Tag", |i| i.create(|e| _ = e.add(TAGS, tag(&name))).0)
                .unwrap();
        }
        for window in windows {
            window.close().unwrap();
        }
        assert!(pool() <= 2, "session {session}: a pool of {}", pool());
    }
}

fn asking_who_else_writes_keeps_the_idle_writers_taken_up<F: Facade>() {
    let folder = disk();
    let clock = TestClock::at(1_000);
    let here = machine(&folder);
    put(&folder, "a.wav", b"a");
    let probes = [1, 2].map(|_| Probe::new(&here));
    let mut windows = [0, 1].map(|n| {
        let opened = F::open(probes[n].clone(), env("w", n as u64 + 1, &clock));
        opened.unwrap().0
    });
    for (n, window) in windows.iter_mut().enumerate() {
        window
            .commit("Tag", |i| {
                i.create(|e| _ = e.add(TAGS, tag(&n.to_string()))).0
            })
            .unwrap();
    }
    // The next instance resumes the first writer of the pool, so the other window
    // leaves the record.
    let first = first_in_pool(&here);
    let stuck = windows
        .iter()
        .position(|w| own_writer(&w.view()) != Some(first));
    let stuck = stuck.unwrap();
    windows[stuck].rescan().unwrap();
    // The effect is made, and its entry not logged: the record stays open.
    let logging = |io: &Io| io.root() == Root::Folder && io.path().as_str().ends_with(".txt");
    fail(&probes[stuck], move |io| io.mutates() && logging(io));
    let stopped = windows[stuck].commit("Rename", |i| {
        i.rename_file(
            &path("a.wav"),
            &path("b.wav"),
            Expect::Holds(identity(b"a")),
        )
    });
    heal(&probes[stuck]);
    assert!(stopped.is_err(), "the entry was not logged: {stopped:?}");
    for window in windows {
        window.close().unwrap();
    }

    let (mut first, opened) = F::open(Probe::new(&here), env("w", 3, &clock)).unwrap();
    let ours = opened.orphaned.iter().filter(|orphan| orphan.ours);
    assert_eq!(ours.count(), 1, "the idle writer's record is taken up");
    first.others().unwrap();
    let (_, opened) = F::open(Probe::new(&here), env("w", 4, &clock)).unwrap();
    let orphans: Vec<(bool, bool)> = opened.orphaned.iter().map(|o| (o.ours, o.live)).collect();
    assert_eq!(orphans, [(false, true)], "the first window still holds it");
}

fn an_idle_writer_whose_records_its_log_closed_is_let_go<F: Facade>() {
    let disk = disk();
    let one_disk = |disk: &MemDisk| Machine {
        folder: disk.clone(),
        local: disk.clone(),
    };
    let clock = TestClock::at(1_000);
    let probes = [1, 2].map(|_| Probe::new(&one_disk(&disk)));
    let [mut crashed, mut closed] = [1, 2].map(|seed| {
        let probe = probes[seed as usize - 1].clone();
        F::open(probe, env("w", seed, &clock)).unwrap().0
    });
    fail(&probes[0], pending_removal);
    create(&mut crashed, "song.npno", b"song");
    create(&mut closed, "kick.wav", b"kick");
    closed.close().unwrap();
    drop(crashed);
    let disk = disk.restart();
    assert!(
        !pending_files(&disk).is_empty(),
        "the crash left a spent record"
    );

    let (_third, opened) = F::open(Probe::new(&one_disk(&disk)), env("w", 3, &clock)).unwrap();
    assert!(
        matches!(opened.start, Start::Resumed(_)),
        "{:?}",
        opened.start
    );
    let (_fourth, opened) = F::open(Probe::new(&one_disk(&disk)), env("w", 4, &clock)).unwrap();
    assert!(
        matches!(opened.start, Start::Resumed(_)),
        "the other idle writer is free to resume: {:?}",
        opened.start
    );
}

fn an_effect_a_live_window_left_open_is_reported_live<F: Facade>() {
    let folder = disk();
    let clock = TestClock::at(1_000);
    let here = machine(&folder);
    put(&folder, "a.wav", b"a");
    let probe = Probe::new(&here);
    let (mut first, _) = F::open(probe.clone(), env("w", 1, &clock)).unwrap();
    first.rescan().unwrap();
    first
        .commit("Tag", |i| i.create(|e| _ = e.add(TAGS, tag("x"))).0)
        .unwrap();
    // The effect is made, and its entry not logged: the record stays open.
    let logging = |io: &Io| io.root() == Root::Folder && io.path().as_str().ends_with(".txt");
    fail(&probe, move |io| io.mutates() && logging(io));
    let stopped = first.commit("Rename", |i| {
        i.rename_file(
            &path("a.wav"),
            &path("b.wav"),
            Expect::Holds(identity(b"a")),
        )
    });
    heal(&probe);
    assert!(stopped.is_err(), "the entry was not logged: {stopped:?}");

    let (_, opened) = F::open(Probe::new(&here), env("w", 2, &clock)).unwrap();
    let orphans: Vec<(bool, bool)> = opened.orphaned.iter().map(|o| (o.ours, o.live)).collect();
    assert_eq!(
        orphans,
        [(false, true)],
        "the first window holds its writer"
    );
}

fn an_effect_a_window_left_is_the_installs_once_the_window_closes<F: Facade>() {
    let folder = disk();
    let clock = TestClock::at(1_000);
    let here = machine(&folder);
    put(&folder, "a.wav", b"a");
    let probe = Probe::new(&here);
    let (mut first, _) = F::open(probe.clone(), env("w", 1, &clock)).unwrap();
    first.rescan().unwrap();
    first
        .commit("Tag", |i| i.create(|e| _ = e.add(TAGS, tag("x"))).0)
        .unwrap();
    let (mut second, _) = F::open(Probe::new(&here), env("w", 2, &clock)).unwrap();
    let logging = |io: &Io| io.root() == Root::Folder && io.path().as_str().ends_with(".txt");
    fail(&probe, move |io| io.mutates() && logging(io));
    let stopped = first.commit("Rename", |i| {
        i.rename_file(
            &path("a.wav"),
            &path("b.wav"),
            Expect::Holds(identity(b"a")),
        )
    });
    heal(&probe);
    assert!(stopped.is_err(), "the entry was not logged: {stopped:?}");
    let orphans = |second: &mut F| {
        let refreshed = second.refresh().unwrap();
        let orphans = refreshed.orphaned.into_iter();
        orphans.map(|o| (o.ours, o.live)).collect::<Vec<_>>()
    };
    assert_eq!(
        orphans(&mut second),
        [(false, true)],
        "the first window is live"
    );

    first.close().unwrap();
    assert_eq!(
        orphans(&mut second),
        [(true, false)],
        "the install's own to finish"
    );
    let orphan = second.refresh().unwrap().orphaned.remove(0);
    second.settle(orphan, Settlement::Finished).unwrap();
    assert_eq!(orphans(&mut second), [], "finished");
}

fn a_change_of_case_another_install_did_not_start_is_finished_by_another<F: Facade>() {
    let folder = disk();
    let clock = TestClock::at(1_000);
    put(&folder, "Kick.wav", b"kick");
    let probe = Probe::new(&machine(&folder));
    let folding = |label, seed| Env {
        names: Box::new(Folding),
        ..env(label, seed, &clock)
    };
    let (mut theirs, _) = F::open(probe.clone(), folding("a", 1)).unwrap();
    theirs.rescan().unwrap();
    theirs
        .commit("Tag", |i| i.create(|e| _ = e.add(TAGS, tag("x"))).0)
        .unwrap();
    // Its first step and its entry both fail: the record stays, and nothing moved.
    let first_step = |io: &Io| matches!(io, Io::Rename { from, .. } if from.as_str() == "Kick.wav");
    let logging = |io: &Io| io.root() == Root::Folder && io.path().as_str().ends_with(".txt");
    fail(&probe, move |io| {
        io.mutates() && (first_step(io) || logging(io))
    });
    let stopped = theirs.commit("Rename", |i| {
        i.rename_file(
            &path("Kick.wav"),
            &path("kick.wav"),
            Expect::Holds(identity(b"kick")),
        )
    });
    heal(&probe);
    assert!(stopped.is_err(), "{stopped:?}");
    drop(theirs);

    let (mut ours, opened) = F::open(Probe::new(&machine(&folder)), folding("b", 2)).unwrap();
    let [orphan] = opened.orphaned.as_slice() else {
        panic!("one orphan: {:?}", opened.orphaned);
    };
    ours.settle(orphan.clone(), Settlement::Finished).unwrap();
    let files: Vec<String> = library_files(&folder)
        .into_iter()
        .map(|(path, bytes)| format!("{path:?} {}", String::from_utf8_lossy(&bytes)))
        .collect();
    assert_eq!(files, [r#"RelPath("kick.wav") kick"#]);
}

fn a_change_of_case_another_install_made_is_rolled_back_by_another<F: Facade>() {
    let folder = disk();
    let clock = TestClock::at(1_000);
    put(&folder, "Kick.wav", b"kick");
    let probe = Probe::new(&machine(&folder));
    let folding = |label, seed| Env {
        names: Box::new(Folding),
        ..env(label, seed, &clock)
    };
    let (mut theirs, _) = F::open(probe.clone(), folding("a", 1)).unwrap();
    theirs.rescan().unwrap();
    theirs
        .commit("Tag", |i| i.create(|e| _ = e.add(TAGS, tag("x"))).0)
        .unwrap();
    let logging = |io: &Io| io.root() == Root::Folder && io.path().as_str().ends_with(".txt");
    fail(&probe, move |io| io.mutates() && logging(io));
    let stopped = theirs.commit("Rename", |i| {
        i.rename_file(
            &path("Kick.wav"),
            &path("kick.wav"),
            Expect::Holds(identity(b"kick")),
        )
    });
    heal(&probe);
    assert!(stopped.is_err(), "{stopped:?}");
    assert_eq!(
        read(&folder, "kick.wav").as_deref(),
        Some(b"kick".as_slice())
    );
    drop(theirs);

    let (mut ours, opened) = F::open(Probe::new(&machine(&folder)), folding("b", 2)).unwrap();
    let [orphan] = opened.orphaned.as_slice() else {
        panic!("one orphan: {:?}", opened.orphaned);
    };
    ours.settle(orphan.clone(), Settlement::RolledBack).unwrap();
    let files = library_files(&folder).into_keys();
    assert_eq!(
        files.map(|at| at.as_str().to_owned()).collect::<Vec<_>>(),
        ["Kick.wav"]
    );
    assert_eq!(
        read(&folder, "Kick.wav").as_deref(),
        Some(b"kick".as_slice())
    );
}

fn settling_another_installs_save_reads_no_staged_file_no_op_logs<F: Facade>() {
    for how in [Settlement::Finished, Settlement::RolledBack] {
        let folder = disk();
        let clock = TestClock::at(1_000);
        put(&folder, "Kick.wav", b"kick");
        let probe = Probe::new(&machine(&folder));
        let folding = |label, seed| Env {
            names: Box::new(Folding),
            ..env(label, seed, &clock)
        };
        let (mut theirs, _) = F::open(probe.clone(), folding("a", 1)).unwrap();
        theirs.rescan().unwrap();
        theirs
            .commit("Tag", |i| i.create(|e| _ = e.add(TAGS, tag("x"))).0)
            .unwrap();
        let logging = |io: &Io| io.root() == Root::Folder && io.path().as_str().ends_with(".txt");
        fail(&probe, move |io| io.mutates() && logging(io));
        let held = Expect::Holds(identity(b"kick"));
        let stopped = theirs.commit("Save", |i| {
            i.save_file(&path("Kick.wav"), b"kicx".to_vec(), held)
        });
        heal(&probe);
        assert!(stopped.is_err(), "{stopped:?}");
        drop(theirs);

        let ours_probe = Probe::new(&machine(&folder));
        let read = Rc::new(Cell::new(0));
        let counted = Rc::clone(&read);
        // An identity is read in a batch; a copy reads its source a chunk at a time.
        fail(&ours_probe, move |io| {
            let staged = |at: &RelPath| layout().owns(at) && at.as_str().ends_with(".wav");
            if let Io::ReadMany { reads, .. } = io {
                counted.set(counted.get() + reads.iter().filter(|(at, _)| staged(at)).count());
            }
            false
        });
        let (mut ours, opened) = F::open(ours_probe, folding("b", 2)).unwrap();
        let [orphan] = opened.orphaned.as_slice() else {
            panic!("one orphan: {:?}", opened.orphaned);
        };
        ours.settle(orphan.clone(), how).unwrap();
        assert_eq!(read.get(), 0, "staged files no file op logs, {how:?}");
    }
}

/// Leaves the folder `Organ` in another install's trash, as a crash between the
/// two steps of a change of its case to `organ` does, and opens this install.
fn a_folder_left_in_anothers_trash<F: Facade>(
    folder: &MemDisk,
    clock: &TestClock,
) -> (F, toshokan::report::Orphan) {
    put(folder, "Organ/b/c.wav", b"c");
    let probe = Probe::new(&machine(folder));
    let folding = |label, seed| Env {
        names: Box::new(Folding),
        ..env(label, seed, clock)
    };
    let (mut theirs, _) = F::open(probe.clone(), folding("a", 1)).unwrap();
    theirs.rescan().unwrap();
    create(&mut theirs, "Organ/song.npno", b"song");
    let second_step = |io: &Io| matches!(io, Io::Rename { to, .. } if to.as_str() == "organ");
    let logging = |io: &Io| io.root() == Root::Folder && io.path().as_str().ends_with(".txt");
    fail(&probe, move |io| {
        io.mutates() && (second_step(io) || logging(io))
    });
    let stopped = theirs.commit("Rename", |i| i.move_tree(&path("Organ"), &path("organ")));
    heal(&probe);
    assert!(stopped.is_err(), "{stopped:?}");
    assert_eq!(library_files(folder).len(), 0, "the folder is in the trash");
    drop(theirs);
    let (ours, opened) = F::open(Probe::new(&machine(folder)), folding("b", 2)).unwrap();
    let [orphan] = opened.orphaned.as_slice() else {
        panic!("one orphan: {:?}", opened.orphaned);
    };
    (ours, orphan.clone())
}

fn a_folder_another_left_in_its_trash_is_finished_by_another<F: Facade>() {
    let folder = MemDisk::with_capabilities(Capabilities::ALL, Capabilities::ALL);
    let clock = TestClock::at(1_000);
    let (mut ours, orphan) = a_folder_left_in_anothers_trash::<F>(&folder, &clock);
    ours.settle(orphan, Settlement::Finished).unwrap();
    let files = library_files(&folder).into_keys();
    assert_eq!(
        files.map(|at| at.as_str().to_owned()).collect::<Vec<_>>(),
        ["organ/b/c.wav", "organ/song.npno"]
    );
    let trashed = folder.files(Root::Folder).into_iter();
    let trashed = trashed.filter(|(at, bytes)| at.as_str().contains("/trash/") && bytes == b"c");
    assert_eq!(
        trashed.count(),
        1,
        "their trash keeps what was copied out of it"
    );
    ours.rescan().unwrap();
    let view = ours.view();
    let song = view.entities().into_iter().next().unwrap();
    let file = song.file().unwrap();
    assert_eq!(
        (file.path, file.state),
        (path("organ/song.npno"), FileState::InSync)
    );
}

fn a_folder_another_left_in_its_trash_is_rolled_back_by_another<F: Facade>() {
    let folder = MemDisk::with_capabilities(Capabilities::ALL, Capabilities::ALL);
    let clock = TestClock::at(1_000);
    let (mut ours, orphan) = a_folder_left_in_anothers_trash::<F>(&folder, &clock);
    ours.settle(orphan, Settlement::RolledBack).unwrap();
    let files = library_files(&folder).into_keys();
    assert_eq!(
        files.map(|at| at.as_str().to_owned()).collect::<Vec<_>>(),
        ["Organ/b/c.wav", "Organ/song.npno"]
    );
}

/// `folder`'s files and directories, in a folder whose moves copy.
fn where_moves_copy(folder: &MemDisk) -> MemDisk {
    let copied = MemDisk::with_capabilities(COPYING, Capabilities::ALL);
    let dirs = folder.directories(Root::Folder).into_iter();
    for path in dirs.filter(|dir| !dir.is_root()) {
        let made = Io::MakeDir {
            root: Root::Folder,
            path,
        };
        copied.perform(made).unwrap();
    }
    for (path, bytes) in folder.files(Root::Folder) {
        let root = Root::Folder;
        copied.perform(Io::Create { root, path, bytes }).unwrap();
    }
    copied
}

fn finishing_a_folder_case_change_where_moves_copy_is_refused_and_changes_nothing<F: Facade>() {
    let folder = MemDisk::with_capabilities(Capabilities::ALL, Capabilities::ALL);
    let clock = TestClock::at(1_000);
    put(&folder, "Organ/b/c.wav", b"c");
    let probe = Probe::new(&machine(&folder));
    let folding = |label, seed| Env {
        names: Box::new(Folding),
        ..env(label, seed, &clock)
    };
    let (mut theirs, _) = F::open(probe.clone(), folding("a", 1)).unwrap();
    theirs.rescan().unwrap();
    create(&mut theirs, "Organ/song.npno", b"song");
    let first_step = |io: &Io| matches!(io, Io::Rename { from, .. } if from.as_str() == "Organ");
    let logging = |io: &Io| io.root() == Root::Folder && io.path().as_str().ends_with(".txt");
    fail(&probe, move |io| {
        io.mutates() && (first_step(io) || logging(io))
    });
    let stopped = theirs.commit("Rename", |i| i.move_tree(&path("Organ"), &path("organ")));
    heal(&probe);
    assert!(stopped.is_err(), "{stopped:?}");
    drop(theirs);

    let copied = where_moves_copy(&folder);
    let (mut ours, opened) = F::open(Probe::new(&machine(&copied)), folding("b", 2)).unwrap();
    let [orphan] = opened.orphaned.as_slice() else {
        panic!("one orphan: {:?}", opened.orphaned);
    };
    let before = library_files(&copied);
    let refused = ours.settle(orphan.clone(), Settlement::Finished);
    assert!(
        matches!(
            refused,
            Err(Error::Refused(Refusal::Unsupported(Capability::RenameDir)))
        ),
        "{refused:?}"
    );
    assert_eq!(library_files(&copied), before, "nothing moved");
    ours.commit("Tag", |i| i.create(|e| _ = e.add(TAGS, tag("x"))).0)
        .expect("the library can still be written");
}

fn a_scan_of_every_file_reports_the_directories_it_listed<F: Facade>() {
    let folder = disk();
    let clock = TestClock::at(1_000);
    put(&folder, "a/b/c.wav", b"c");
    folder
        .perform(Io::MakeDir {
            root: Root::Folder,
            path: path("empty"),
        })
        .unwrap();
    let (mut a, _) = F::open(Probe::new(&machine(&folder)), env("a", 1, &clock)).unwrap();
    let dirs = a.rescan().unwrap().dirs;
    assert_eq!(dirs, Some(vec![path("a"), path("a/b"), path("empty")]));
    assert_eq!(
        a.refresh().unwrap().dirs,
        None,
        "a refresh lists no directory"
    );
}

fn a_refresh_reports_another_installs_unfinished_change_as_it_stands<F: Facade>() {
    let folder = disk();
    let clock = TestClock::at(1_000);
    put(&folder, "a.wav", b"a");
    let (mut ours, opened) = F::open(Probe::new(&machine(&folder)), env("b", 2, &clock)).unwrap();
    assert_eq!(opened.orphaned, []);
    let probe = Probe::new(&machine(&folder));
    let (mut theirs, _) = F::open(probe.clone(), env("a", 1, &clock)).unwrap();
    theirs.rescan().unwrap();
    theirs
        .commit("Tag", |i| i.create(|e| _ = e.add(TAGS, tag("x"))).0)
        .unwrap();
    assert_eq!(ours.refresh().unwrap().orphaned, []);
    // The effect is made, and its entry not logged: the record stays open.
    let logging = |io: &Io| io.root() == Root::Folder && io.path().as_str().ends_with(".txt");
    fail(&probe, move |io| io.mutates() && logging(io));
    let stopped = theirs.commit("Rename", |i| {
        i.rename_file(
            &path("a.wav"),
            &path("b.wav"),
            Expect::Holds(identity(b"a")),
        )
    });
    heal(&probe);
    assert!(stopped.is_err(), "{stopped:?}");
    drop(theirs);

    let refreshed = ours.refresh().unwrap();
    let [orphan] = refreshed.orphaned.as_slice() else {
        panic!("one orphan once it began: {:?}", refreshed.orphaned);
    };
    assert_eq!((orphan.label.as_str(), orphan.ours), ("Rename", false));
    let progress = |ours: &mut F| ours.refresh().unwrap().orphaned[0].progress;
    assert_eq!(progress(&mut ours), orphan.progress, "nothing moved");
    put(&folder, "b.wav", b"bb");
    assert_ne!(progress(&mut ours), orphan.progress, "its path moved");
    ours.settle(orphan.clone(), Settlement::Finished).unwrap();
    assert_eq!(
        ours.refresh().unwrap().orphaned,
        [],
        "none once it is settled"
    );
}

/// Leaves another install's change, which renames `a.wav` to `b.wav` and sets an
/// origin, made but unlogged in `folder`, and returns the orphan `ours` then finds.
fn a_rename_left_by_another<F: Facade>(
    folder: &MemDisk,
    clock: &TestClock,
    ours: &mut F,
) -> toshokan::report::Orphan {
    put(folder, "a.wav", b"a");
    let probe = Probe::new(&machine(folder));
    let (mut theirs, _) = F::open(probe.clone(), env("a", 1, clock)).unwrap();
    theirs.rescan().unwrap();
    let tagged = theirs
        .commit("Tag", |i| i.create(|e| _ = e.add(TAGS, tag("x"))).0)
        .unwrap();
    let logging = |io: &Io| io.root() == Root::Folder && io.path().as_str().ends_with(".txt");
    fail(&probe, move |io| io.mutates() && logging(io));
    let stopped = theirs.commit("Rename", |i| {
        i.rename_file(
            &path("a.wav"),
            &path("b.wav"),
            Expect::Holds(identity(b"a")),
        )
        .set(tagged.created[0], ORIGIN, "renamed".into())
    });
    heal(&probe);
    assert!(stopped.is_err(), "{stopped:?}");
    drop(theirs);
    let mut orphaned = ours.refresh().unwrap().orphaned;
    assert_eq!(orphaned.len(), 1, "{orphaned:?}");
    orphaned.remove(0)
}

fn a_settlement_after_a_refused_undo_is_no_undo_of_it<F: Facade>() {
    let folder = disk();
    let clock = TestClock::at(1_000);
    let (mut ours, _) = F::open(Probe::new(&machine(&folder)), env("b", 2, &clock)).unwrap();
    let song = create(&mut ours, "song.npno", b"one");
    ours.commit("Save", |i| {
        i.save(
            song,
            &path("song.npno"),
            b"two".to_vec(),
            Expect::Holds(identity(b"one")),
        )
    })
    .unwrap();
    put(&folder, "song.npno", b"outside");
    let refused = ours.undo();
    assert!(matches!(refused, Err(Error::Refused(_))), "{refused:?}");
    let orphan = a_rename_left_by_another(&folder, &clock, &mut ours);
    ours.settle(orphan, Settlement::Finished).unwrap();
    let save = ours.history().into_iter().find(|item| item.label == "Save");
    assert_eq!(save.map(|item| item.undone), Some(false), "the save stands");
    let redone = ours.redo();
    assert!(
        matches!(redone, Err(Error::Refused(Refusal::Nothing))),
        "nothing to redo: {redone:?}"
    );
    assert_eq!(read(&folder, "b.wav").as_deref(), Some(b"a".as_slice()));
}

fn settling_another_writers_change_is_no_step_of_undo<F: Facade>() {
    for how in [Settlement::Finished, Settlement::RolledBack] {
        let folder = disk();
        let clock = TestClock::at(1_000);
        let (mut ours, _) = F::open(Probe::new(&machine(&folder)), env("b", 2, &clock)).unwrap();
        create(&mut ours, "song.npno", b"one");
        let orphan = a_rename_left_by_another(&folder, &clock, &mut ours);
        let settled = ours.settle(orphan, how).unwrap();
        assert!(!settled.undoable, "{how:?}");
        ours.undo().unwrap();
        let history: Vec<(String, bool)> = ours
            .history()
            .into_iter()
            .map(|item| (item.label, item.undone))
            .collect();
        assert_eq!(history, [("Import".into(), true)], "{how:?}");
        assert_eq!(
            read(&folder, "song.npno"),
            None,
            "{how:?}: the import is undone"
        );
    }
}

fn a_settlement_resumed_in_a_later_session_is_no_step_of_undo<F: Facade>() {
    let folder = disk();
    let clock = TestClock::at(1_000);
    let (x, y) = (machine(&folder), machine(&folder));
    let probe = Probe::new(&x);
    let (mut ours, _) = F::open(probe.clone(), env("a", 1, &clock)).unwrap();
    let song = create(&mut ours, "song.npno", b"one");
    let their_probe = Probe::new(&y);
    let (mut theirs, _) = F::open(their_probe.clone(), env("b", 2, &clock)).unwrap();
    theirs.rescan().unwrap();
    fail(&their_probe, into_trash);
    let saved = theirs.commit("Save", |i| {
        i.save(
            song,
            &path("song.npno"),
            b"two".to_vec(),
            Expect::Holds(identity(b"one")),
        )
        .add(song, TAGS, tag("saved"))
    });
    assert!(saved.is_err(), "{saved:?}");
    drop(theirs);
    let orphaned = ours.refresh().unwrap().orphaned;
    let [orphan] = &orphaned[..] else {
        panic!("their save is left to settle: {orphaned:?}");
    };
    refuse(&probe, Some("song.npno"));
    let stopped = ours.settle(orphan.clone(), Settlement::Finished);
    assert!(matches!(stopped, Err(Error::Io { .. })), "{stopped:?}");
    drop(ours);

    let x = Machine {
        folder: folder.clone(),
        local: x.local.restart(),
    };
    let (mut again, opened) = F::open(Probe::new(&x), env("a", 3, &clock)).unwrap();
    assert_eq!(
        opened.settled.len(),
        1,
        "the settlement is this writer's own"
    );
    let [again_orphaned] = &opened.orphaned[..] else {
        panic!("their record is settled again: {:?}", opened.orphaned);
    };
    assert_eq!(again_orphaned.record, orphan.record);
    again
        .settle(again_orphaned.clone(), Settlement::Finished)
        .unwrap();
    assert_eq!(read(&folder, "song.npno").as_deref(), Some(&b"two"[..]));
    let undone = again.undo();
    assert!(
        matches!(undone, Err(Error::Refused(Refusal::Nothing))),
        "nothing to undo: {undone:?}"
    );
    assert_eq!(read(&folder, "song.npno").as_deref(), Some(&b"two"[..]));
    assert_eq!(again.refresh().unwrap().orphaned, [], "settled once");
    assert_eq!(tags(&again.view(), song), ["new", "saved"]);
}

fn a_directory_made_and_filled_in_one_change_is_undone_in_one<F: Facade>() {
    let folder = disk();
    let clock = TestClock::at(1_000);
    put(&folder, "a.wav", b"a");
    let (mut a, _) = F::open(Probe::new(&machine(&folder)), env("a", 1, &clock)).unwrap();
    a.rescan().unwrap();
    let dirs = || {
        let dirs = folder.directories(Root::Folder).into_iter();
        let dirs = dirs.filter(|dir| !dir.starts_with(&path(".t")) && !dir.is_root());
        dirs.map(|dir| dir.as_str().to_owned()).collect::<Vec<_>>()
    };
    a.commit("Move", |i| {
        i.make_dir(&path("Gig")).rename_file(
            &path("a.wav"),
            &path("Gig/a.wav"),
            Expect::Holds(identity(b"a")),
        )
    })
    .unwrap();
    assert_eq!(dirs(), ["Gig"]);
    a.undo().unwrap();
    assert_eq!(
        dirs(),
        [] as [String; 0],
        "the directory went with the move"
    );
    assert!(read(&folder, "a.wav").is_some());
    a.redo().unwrap();
    assert_eq!(dirs(), ["Gig"]);
    assert!(read(&folder, "Gig/a.wav").is_some());
}

fn directories_made_and_removed_around_tree_moves_undo_and_redo<F: Facade>() {
    let folder = disk();
    let clock = TestClock::at(1_000);
    put(&folder, "A/x.wav", b"x");
    put(&folder, "Gig/B/y.wav", b"y");
    let (mut a, _) = F::open(Probe::new(&machine(&folder)), env("a", 1, &clock)).unwrap();
    a.rescan().unwrap();
    let dirs = || {
        let dirs = folder.directories(Root::Folder).into_iter();
        let dirs = dirs.filter(|dir| !dir.starts_with(&path(".t")) && !dir.is_root());
        dirs.map(|dir| dir.as_str().to_owned()).collect::<Vec<_>>()
    };
    a.commit("Move", |i| {
        i.make_dir(&path("Set"))
            .move_tree(&path("A"), &path("Set/A"))
    })
    .unwrap();
    a.commit("Remove folder", |i| {
        i.move_tree(&path("Gig/B"), &path("B"))
            .remove_dir(&path("Gig"))
    })
    .unwrap();
    let moved = ["B", "Set", "Set/A"];
    assert_eq!(dirs(), moved);
    a.undo().unwrap();
    assert_eq!(
        dirs(),
        ["Gig", "Gig/B", "Set", "Set/A"],
        "the folder made again"
    );
    a.undo().unwrap();
    let before = ["A", "Gig", "Gig/B"];
    assert_eq!(
        dirs(),
        before,
        "the folder removed after the tree moved out"
    );
    a.redo().unwrap();
    a.redo().unwrap();
    assert_eq!(dirs(), moved, "redone in the order each was made");
    assert!(read(&folder, "Set/A/x.wav").is_some());
    assert!(read(&folder, "B/y.wav").is_some());
}

fn removing_an_empty_directory_is_undone_and_one_with_files_is_refused<F: Facade>() {
    let folder = disk();
    let clock = TestClock::at(1_000);
    put(&folder, "Full/a.wav", b"a");
    folder
        .perform(Io::MakeDir {
            root: Root::Folder,
            path: path("Empty"),
        })
        .unwrap();
    let (mut a, _) = F::open(Probe::new(&machine(&folder)), env("a", 1, &clock)).unwrap();
    a.rescan().unwrap();
    let refused = a.commit("Remove", |i| i.remove_dir(&path("Full")));
    assert!(
        matches!(
            refused,
            Err(Error::Refused(Refusal::Invalid(Invalid::NotEmpty(_))))
        ),
        "{refused:?}"
    );
    let filled = a.commit("Remove", |i| {
        i.save_file(&path("Empty/new.wav"), b"new".to_vec(), Expect::Absent)
            .remove_dir(&path("Empty"))
    });
    assert!(
        matches!(
            filled,
            Err(Error::Refused(Refusal::Invalid(Invalid::NotEmpty(_))))
        ),
        "a file the intent puts there keeps it: {filled:?}"
    );
    let filled = a.commit("Remove", |i| {
        i.remove_dir(&path("Empty")).save_file(
            &path("Empty/new.wav"),
            b"new".to_vec(),
            Expect::Absent,
        )
    });
    assert!(
        matches!(
            filled,
            Err(Error::Refused(Refusal::Invalid(Invalid::NotEmpty(_))))
        ),
        "whichever it asks first: {filled:?}"
    );
    a.commit("Remove", |i| i.remove_dir(&path("Empty")))
        .unwrap();
    let there = |dir: &str| folder.directories(Root::Folder).contains(&path(dir));
    assert!(!there("Empty"));
    a.undo().unwrap();
    assert!(there("Empty"), "made again");
}

fn an_intent_that_stops_partway_logs_nothing_until_the_next_write_finishes_it<F: Facade>() {
    let folder = disk();
    let clock = TestClock::at(1_000);
    let probe = Probe::new(&machine(&folder));
    put(&folder, "old.wav", b"old");
    put(&folder, "late.wav", b"late");
    let (mut a, _) = F::open(probe.clone(), env("a", 1, &clock)).unwrap();
    a.rescan().unwrap();
    fail(&probe, |io| io.mutates() && io.path().as_str() == "b.wav");
    let stopped = a.commit("Copy in", |i| {
        i.trash_file(&path("old.wav"), Expect::Holds(identity(b"old")))
            .save_file(&path("a.wav"), b"a".to_vec(), Expect::Absent)
            .save_file(&path("b.wav"), b"b".to_vec(), Expect::Absent)
            .trash_file(&path("late.wav"), Expect::Holds(identity(b"late")))
    });
    heal(&probe);
    assert!(
        matches!(&stopped, Err(Error::Io { path, .. }) if path.as_str() == "b.wav"),
        "{stopped:?}"
    );
    assert_eq!(logged(&folder, "Copy in"), None);
    a.commit("Next", |i| {
        i.save_file(&path("c.wav"), b"c".to_vec(), Expect::Absent)
    })
    .unwrap();
    let held = |name| read(&folder, name);
    assert_eq!(
        ["old.wav", "a.wav", "b.wav", "late.wav", "c.wav"].map(held),
        [
            None,
            Some(b"a".to_vec()),
            Some(b"b".to_vec()),
            None,
            Some(b"c".to_vec())
        ]
    );
    let labels: Vec<String> = a.history().into_iter().map(|item| item.label).collect();
    assert_eq!(labels, ["Copy in", "Next"]);
}

fn files_rescanned_together_list_their_directory_once<F: Facade>() {
    let folder = disk();
    let clock = TestClock::at(1_000);
    let names = ["a", "b", "c", "d"];
    for name in names {
        put(&folder, &format!("dir/{name}.wav"), name.as_bytes());
    }
    let probe = Probe::new(&machine(&folder));
    let (mut a, _) = F::open(probe.clone(), env("a", 1, &clock)).unwrap();
    a.rescan().unwrap();
    let before = probe.listed();
    let paths = names.map(|name| path(&format!("dir/{name}.wav")));
    a.rescan_paths(paths.to_vec()).unwrap();
    let lists = |dir: &RelPath| {
        probe.listed().get(dir).copied().unwrap_or(0) - before.get(dir).copied().unwrap_or(0)
    };
    assert_eq!(
        (lists(&RelPath::ROOT), lists(&path("dir"))),
        (1, 1),
        "lists of the folder and of dir"
    );
}

/// The scenario losing the local root interrupts: every step a user might take.
fn steps<F: Facade>(library: &mut F, clock: &TestClock, done: &mut usize) -> Result<(), Error> {
    let song = library
        .commit("Import", |i| {
            let (i, _) = i.create(|e| {
                e.save(&path("a/song.npno"), b"one".to_vec(), Expect::Absent)
                    .add(TAGS, tag("new"));
            });
            i
        })?
        .created[0];
    *done += 1;
    library.commit("Save", |i| {
        i.save(
            song,
            &path("a/song.npno"),
            b"two".to_vec(),
            Expect::Holds(identity(b"one")),
        )
    })?;
    library.commit("Rename", |i| {
        i.rename(song, &path("b/song.npno"), Expect::Holds(identity(b"two")))
    })?;
    library.undo()?;
    library.compact()?;
    library.commit("Move", |i| i.move_tree(&path("a"), &path("c")))?;
    clock.advance(1);
    library.commit("Trash", |i| i.trash(song, Expect::Holds(identity(b"two"))))?;
    let loose = path("loose.syx");
    library.commit("Add", |i| {
        i.save_file(&loose, b"loose".to_vec(), Expect::Absent)
    })?;
    library.commit("Rename", |i| {
        i.rename_file(
            &loose,
            &path("d/loose.syx"),
            Expect::Holds(identity(b"loose")),
        )
    })?;
    library.undo()?;
    library.commit("Delete", |i| {
        i.trash_file(&loose, Expect::Holds(identity(b"loose")))
    })?;
    Ok(())
}

fn losing_the_local_root_at_any_step_loses_nothing_of_the_library<F: Facade>() {
    let total = {
        let disk = disk();
        let clock = TestClock::at(1_000);
        let mut library = F::open(Probe::new(&machine(&disk)), env("a", 1, &clock))
            .unwrap()
            .0;
        steps(&mut library, &clock, &mut 0).unwrap();
        disk.mutations()
    };
    let contents: [&[u8]; 2] = [b"one", b"two"];
    let mut orphaned = 0;
    for crash in 0..total {
        let folder = disk();
        let clock = TestClock::at(1_000);
        let here = machine(&folder);
        let (mut library, _) = F::open(Probe::new(&here), env("a", 1, &clock)).unwrap();
        folder.crash_after(crash);
        let mut done = 0;
        let _ = steps(&mut library, &clock, &mut done);
        drop(library);
        let folder = folder.restart();
        let here = machine(&folder);
        let shown = format!("crash after {crash}");

        let all = folder.files(Root::Folder);
        let present: BTreeSet<&[u8]> = all.values().map(Vec::as_slice).collect();
        let written: Vec<&[u8]> = contents
            .iter()
            .copied()
            .filter(|bytes| present.contains(bytes))
            .collect();
        let (mut heir, opened) = F::open(Probe::new(&here), env("heir", 2, &clock))
            .unwrap_or_else(|e| panic!("{shown}: {e}"));
        assert_eq!(opened.start, Start::New, "{shown}");
        assert!(
            opened.settled.is_empty(),
            "{shown}: nothing is this writer's"
        );
        if done > 0 {
            let view = heir.view();
            let imported = view
                .entities()
                .into_iter()
                .any(|e| e.members(TAGS).values.contains(&tag("new")));
            assert!(imported, "{shown}: a committed intent survives");
        }
        orphaned += opened.orphaned.len();
        for orphan in opened.orphaned.clone() {
            heir.settle(orphan, Settlement::Finished)
                .unwrap_or_else(|e| panic!("{shown}: {e}"));
        }
        let (again, opened) = F::open(Probe::new(&here), env("third", 3, &clock)).unwrap();
        assert!(opened.orphaned.is_empty(), "{shown}: settled once");
        let after: BTreeSet<Vec<u8>> = folder.files(Root::Folder).into_values().collect();
        for bytes in &written {
            assert!(after.contains(*bytes), "{shown}: {bytes:?} left the folder");
        }
        assert_eq!(again.view().gaps(), [], "{shown}");
        let view = again.view();
        for entity in view.entities() {
            if let Some(file) = entity.file() {
                assert!(
                    file.state != FileState::Missing || entity.members(TAGS).values.is_empty(),
                    "{shown}: {:?} lost its file",
                    entity.id()
                );
            }
        }
    }
    assert!(orphaned > 0, "some crash interrupted an effect");
}

fn a_crash_at_any_step_is_settled_whole_after_emptying_the_trash<F: Facade>() {
    let one_disk = |disk: &MemDisk| Machine {
        folder: disk.clone(),
        local: disk.clone(),
    };
    let total = {
        let disk = disk();
        let clock = TestClock::at(1_000);
        let mut library = F::open(Probe::new(&one_disk(&disk)), env("a", 1, &clock))
            .unwrap()
            .0;
        steps(&mut library, &clock, &mut 0).unwrap();
        disk.mutations()
    };
    let (mut settled, mut emptied) = (0, 0);
    for crash in 0..total {
        let disk = disk();
        let clock = TestClock::at(1_000);
        let (mut library, _) = F::open(Probe::new(&one_disk(&disk)), env("a", 1, &clock)).unwrap();
        disk.crash_after(crash);
        let _ = steps(&mut library, &clock, &mut 0);
        drop(library);
        let disk = disk.restart();
        let shown = format!("crash after {crash}");
        let present: BTreeSet<Vec<u8>> = library_files(&disk).into_values().collect();

        let (mut again, opened) = F::open(Probe::new(&one_disk(&disk)), env("a", 2, &clock))
            .unwrap_or_else(|e| panic!("{shown}: {e}"));
        assert!(opened.orphaned.is_empty(), "{shown}: {:?}", opened.orphaned);
        settled += opened.settled.len();
        let kept = again.empty_trash(BTreeSet::new());
        emptied += kept
            .unwrap_or_else(|e| panic!("{shown}: {e}"))
            .removed
            .len();
        let next = again.commit("Next", |i| {
            let (i, _) = i.create(|e| {
                e.add(TAGS, tag("next"));
            });
            i
        });
        next.unwrap_or_else(|e| panic!("{shown}: {e}"));
        again.close().unwrap();
        let (last, opened) = F::open(Probe::new(&one_disk(&disk)), env("a", 3, &clock)).unwrap();
        assert!(opened.settled.is_empty(), "{shown}: settled once");
        assert_eq!(last.view().gaps(), [], "{shown}");
        let everywhere: BTreeSet<Vec<u8>> = disk.files(Root::Folder).into_values().collect();
        // Where moves copy, a crash can leave the start of a copy, which
        // settling removes once the copy is whole.
        let copied = |part: &Vec<u8>| {
            !FOLDER.get().rename_file && everywhere.iter().any(|whole| whole.starts_with(part))
        };
        for bytes in &present {
            assert!(
                everywhere.contains(bytes) || copied(bytes),
                "{shown}: {bytes:?} left the folder"
            );
        }
    }
    assert!(settled > 0, "some crash interrupted an effect");
    assert!(emptied > 0, "some crash left what settling does not need");
}

#[test]
fn settling_a_commit_whose_append_landed_whole_writes_nothing() {
    let folder = disk();
    let clock = TestClock::at(1_000);
    let here = machine(&folder);
    let (mut a, _) = Blocking::open(Probe::new(&here), env("a", 1, &clock)).unwrap();
    create(&mut a, "song.npno", b"song");
    a.close().unwrap();
    let probe = Probe::new(&here);
    let (mut a, _) = Blocking::open(probe.clone(), env("a", 2, &clock)).unwrap();
    let writes = || (probe.folder_writes(), probe.local_writes());
    let before = writes();
    a.0.settle_interrupted().unwrap();
    assert_eq!(writes(), before, "the log holds the commit's every op");
}

fn an_empty_record_of_this_writer_is_removed_by_its_next_write<F: Facade>() {
    let folder = disk();
    let clock = TestClock::at(1_000);
    let here = machine(&folder);
    let (mut a, _) = F::open(Probe::new(&here), env("a", 1, &clock)).unwrap();
    let song = create(&mut a, "song.npno", b"song");
    let writer = label_of(&a.view(), "a");
    a.close().unwrap();
    let record = |n| layout().pending(writer, toshokan::Nonce::from_u128(n));
    let (empty, newer) = (record(1), record(2));
    put(&folder, empty.as_str(), b"");
    put(&folder, newer.as_str(), br#"{"from":"a newer version"}"#);

    let (mut a, opened) = F::open(Probe::new(&here), env("a", 2, &clock)).unwrap();
    assert_eq!(opened.ignored, [empty.clone(), newer.clone()]);
    a.commit("Tag", |i| i.add(song, TAGS, tag("x"))).unwrap();
    assert_eq!(
        read(&folder, empty.as_str()),
        None,
        "the empty record is left"
    );
    assert!(
        read(&folder, newer.as_str()).is_some(),
        "a record it cannot read is removed"
    );
    a.close().unwrap();
    let (_, opened) = F::open(Probe::new(&here), env("a", 3, &clock)).unwrap();
    assert_eq!(opened.ignored, [newer]);
}

fn an_untrusted_folder_is_read_within_bounds_and_never_acted_on<F: Facade>() {
    let folder = disk();
    let clock = TestClock::at(1_000);
    let here = machine(&folder);
    let (mut a, _) = F::open(Probe::new(&here), env("a", 1, &clock)).unwrap();
    let song = create(&mut a, "song.npno", b"song");
    a.close().unwrap();
    let writers = layout().writers();
    let real = folder
        .files(Root::Folder)
        .into_keys()
        .find(|p| p.starts_with(&writers))
        .unwrap();
    let own = real.as_str().split('/').nth(2).unwrap().to_owned();
    let stranger = WriterId::from_u128(0x5);
    let forged = |after: &str, path: &str| {
        format!(
            r#"{{"writer":"{stranger}","entry":{{"prev":"{after}","at":[1,0],"kind":"intent","label":"x","ops":[]}},"label":"forged","steps":[{{"step":"to_trash","path":"{path}","item":"00000000000000000000000000000001"}}],"files":[]}}"#
        )
    };
    let zero = "0".repeat(32);
    let far = WriterId::from_u128(0x6);
    let last_reading = Hlc {
        wall_ms: u64::MAX,
        counter: u32::MAX,
    };
    let genesis = EntryKind::Genesis(Genesis {
        writer: far,
        label: "far".into(),
    });
    let from_the_future = Entry::encode(EntryHash::ZERO, 0, last_reading, genesis)
        .unwrap()
        .line()
        .to_bytes();
    let mut random = SeededRandom::new(7);
    let mut noise = |len: usize| -> Vec<u8> {
        use toshokan::Random;
        (0..len).map(|_| random.next_u128() as u8).collect()
    };
    let w = |name: &str| format!(".t/writers.2/{name}");
    let garbage: Vec<(String, Vec<u8>)> = vec![
        (format!("{}/noise.jsonl", w(&own)), noise(4096)),
        (
            format!("{}/snapshot-x.json", w(&own)),
            br#"{"writer":1}"#.to_vec(),
        ),
        (
            format!("{}/pending/{}1.json", w(&own), &zero[1..]),
            vec![b' '; (16 << 20) + 1],
        ),
        (
            format!("{}/pending/{zero}.json", w(&stranger.to_string())),
            forged(&zero, ".t/x").into_bytes(),
        ),
        (
            format!("{}/pending/{zero}.json", w(&own)),
            forged(&zero, "song.npno").into_bytes(),
        ),
        (
            format!("{}/x.jsonl", w(&stranger.to_string())),
            b"{\"prev\":\"zz\"}\tbad\n".to_vec(),
        ),
        (
            format!("{}/zeros.jsonl", w(&stranger.to_string())),
            vec![0; 1000],
        ),
        (w("not-a-writer/seg.jsonl"), noise(100)),
        (".t/writers.2/file".into(), noise(10)),
        (
            format!("{}/far.jsonl", w(&far.to_string())),
            from_the_future,
        ),
    ];
    for (at, bytes) in &garbage {
        put(&folder, at, bytes);
    }
    let before = library_files(&folder);
    let probe = Probe::new(&here);
    let (mut a, opened) = F::open(probe.clone(), env("a", 2, &clock)).unwrap();
    assert_eq!(probe.folder_writes(), 0);
    assert!(opened.orphaned.is_empty(), "{:?}", opened.orphaned);
    assert!(opened.settled.is_empty(), "{:?}", opened.settled);
    assert!(
        probe.seen.borrow().longest_pending_read <= toshokan::pending::MAX_RECORD,
        "an oversized record is not read"
    );
    let pending = |name: &str| path(&format!("{}/pending/{name}.json", w(&own)));
    for ignored in [pending(&zero), pending(&format!("{}1", &zero[1..]))] {
        assert!(opened.ignored.contains(&ignored), "{:?}", opened.ignored);
    }
    for unreadable in ["noise.jsonl", "snapshot-x.json"] {
        let unreadable = path(&format!("{}/{unreadable}", w(&own)));
        assert!(
            opened.unreadable.contains(&unreadable),
            "{:?}",
            opened.unreadable
        );
    }
    assert_eq!(tags(&a.view(), song), ["new"]);
    a.commit("Tag", |i| i.add(song, TAGS, tag("still")))
        .unwrap();
    let ours = a.others().unwrap();
    let ours = ours.iter().find(|w| w.here == Presence::This).unwrap();
    assert_eq!(
        ours.last_entry_at.map(|at| at.wall_ms),
        Some(1_000),
        "a forged clock in the folder is not followed"
    );
    assert_eq!(library_files(&folder), before, "no forged effect ran");

    for seed in 0..20 {
        let fuzzed = disk();
        let clock = TestClock::at(1_000);
        let mut random = SeededRandom::new(seed);
        use toshokan::Random;
        for n in 0..8u128 {
            let dir = WriterId::from_u128(random.next_u128() % 3);
            let name = match n % 4 {
                0 => format!("{n}.jsonl"),
                1 => format!("snapshot-{n}.json"),
                2 => format!("pending/{:032x}.json", random.next_u128() % 2),
                _ => format!("trash/{n}"),
            };
            let len = (random.next_u128() % 300) as usize;
            let alphabet = b"{}[]\":,\t\n0123456789abcdefprevatkindopsintent";
            let bytes: Vec<u8> = (0..len)
                .map(|_| alphabet[(random.next_u128() % alphabet.len() as u128) as usize])
                .collect();
            put(&fuzzed, &format!(".t/writers.2/{dir}/{name}"), &bytes);
        }
        let probe = Probe::new(&machine(&fuzzed));
        let (_, opened) = F::open(probe.clone(), env("f", seed, &clock))
            .unwrap_or_else(|e| panic!("seed {seed}: {e}"));
        assert_eq!(probe.folder_writes(), 0, "seed {seed}");
        assert!(opened.orphaned.is_empty(), "seed {seed}");
    }
}

fn the_trash_keeps_displaced_bytes_until_emptied<F: Facade>() {
    let folder = disk();
    let clock = TestClock::at(1_000);
    let (mut a, _) = F::open(Probe::new(&machine(&folder)), env("a", 1, &clock)).unwrap();
    let song = create(&mut a, "song.npno", b"one");
    a.commit("Save", |i| {
        i.save(
            song,
            &path("song.npno"),
            b"two".to_vec(),
            Expect::Holds(identity(b"one")),
        )
    })
    .unwrap();
    let trash = a.trash().unwrap();
    assert_eq!(trash.len(), 1);
    assert_eq!(
        (trash[0].from.clone(), trash[0].len),
        (path("song.npno"), 3)
    );
    let kept = a.empty_trash([trash[0].item].into()).unwrap();
    assert_eq!(kept, Emptied::default(), "what the app keeps stays");
    let emptied = a.empty_trash(BTreeSet::new()).unwrap();
    assert_eq!((emptied.removed.len(), emptied.bytes), (1, 3));
    assert_eq!(a.trash().unwrap(), []);
    let present: BTreeSet<Vec<u8>> = folder.files(Root::Folder).into_values().collect();
    assert!(!present.contains(b"one".as_slice()));
    a.compact().unwrap();
    assert!(
        matches!(a.undo(), Err(Error::Refused(Refusal::Emptied))),
        "undo needs what was emptied, whatever the snapshot keeps"
    );
}

fn the_items_an_app_names_leave_the_trash<F: Facade>() {
    let folder = disk();
    let clock = TestClock::at(1_000);
    let (mut a, _) = F::open(Probe::new(&machine(&folder)), env("a", 1, &clock)).unwrap();
    let song = create(&mut a, "song.npno", b"one");
    for (from, to) in [(&b"one"[..], &b"two"[..]), (b"two", b"three")] {
        a.commit("Save", |i| {
            i.save(
                song,
                &path("song.npno"),
                to.to_vec(),
                Expect::Holds(identity(from)),
            )
        })
        .unwrap();
    }
    let trash = a.trash().unwrap();
    assert_eq!(trash.len(), 2);
    let emptied = a.empty_trash([trash[1].item].into()).unwrap();
    assert_eq!((emptied.removed, emptied.bytes), (vec![trash[0].item], 3));
    assert_eq!(a.trash().unwrap(), [trash[1].clone()]);
    a.undo().unwrap();
    assert_eq!(read(&folder, "song.npno").as_deref(), Some(&b"two"[..]));
    let refused = a.undo();
    assert!(
        matches!(refused, Err(Error::Refused(_))),
        "the earlier save's bytes are gone: {refused:?}"
    );
    assert_eq!(read(&folder, "song.npno").as_deref(), Some(&b"two"[..]));
}

fn a_record_no_log_explains_is_reported_on_a_warm_open<F: Facade>() {
    let folder = disk();
    let clock = TestClock::at(1_000);
    let here = machine(&folder);
    let (mut a, _) = F::open(Probe::new(&here), env("a", 1, &clock)).unwrap();
    create(&mut a, "song.npno", b"song");
    a.close().unwrap();
    let stranger = WriterId::from_u128(0x5);
    let record = format!(
        ".t/writers.2/{stranger}/pending/{}.json",
        toshokan::Nonce::from_u128(9)
    );
    put(&folder, &record, b"{\"not\":\"a record\"}");
    let (_, opened) = F::open(Probe::new(&here), env("a", 2, &clock)).unwrap();
    assert_eq!(opened.ignored, [path(&record)]);
}

fn emptied_trash_items_leave_the_next_snapshot<F: Facade>() {
    let folder = disk();
    let clock = TestClock::at(1_000);
    let (mut a, _) = F::open(Probe::new(&machine(&folder)), env("a", 1, &clock)).unwrap();
    let song = create(&mut a, "song.npno", b"one");
    for bytes in [b"two".as_slice(), b"three"] {
        a.commit("Save", |i| {
            let held = read(&folder, "song.npno").unwrap();
            i.save(
                song,
                &path("song.npno"),
                bytes.to_vec(),
                Expect::Holds(identity(&held)),
            )
        })
        .unwrap();
    }
    let trashed = |folder: &MemDisk| -> usize {
        let snapshots = folder.files(Root::Folder).into_iter();
        let snapshots = snapshots.filter(|(path, _)| path.as_str().contains("/snapshot-"));
        let states = snapshots.map(|(_, bytes)| Snapshot::decode(&bytes).unwrap().state);
        states.map(|state| state.trash.len()).sum()
    };
    a.compact().unwrap();
    assert_eq!(trashed(&folder), 2);
    let newest = a.trash().unwrap().last().unwrap().item;
    let emptied = a.empty_trash([newest].into()).unwrap();
    assert_eq!(emptied.removed.len(), 1, "the older item");
    a.commit("Tag", |i| i.add(song, TAGS, tag("x"))).unwrap();
    a.compact().unwrap();
    assert_eq!(
        trashed(&folder),
        1,
        "the emptied item is gone from the snapshot"
    );
    assert_eq!(a.trash().unwrap().len(), 1);
}

fn compaction_keeps_the_view_and_this_sessions_undo<F: Facade>() {
    let folder = disk();
    let clock = TestClock::at(1_000);
    let here = machine(&folder);
    let (mut a, _) = F::open(Probe::new(&here), env("a", 1, &clock)).unwrap();
    let song = create(&mut a, "song.npno", b"song");
    for n in 0..5 {
        a.commit("Tag", |i| i.add(song, TAGS, format!("t{n}")))
            .unwrap();
    }
    a.close().unwrap();
    let files = |folder: &MemDisk| -> Vec<String> {
        let names = folder.files(Root::Folder).into_keys();
        let names = names.filter(|path| path.as_str().contains("/writers.2/"));
        names
            .filter_map(|path| Some(path.name()?.to_owned()))
            .collect()
    };
    let closed = files(&folder);
    assert_eq!(closed.len(), 1, "closing compacted: {closed:?}");
    assert!(closed[0].starts_with("snapshot-"), "{closed:?}");
    let (mut a, _) = F::open(Probe::new(&here), env("a", 2, &clock)).unwrap();
    a.commit("Tag", |i| i.add(song, TAGS, tag("last"))).unwrap();
    let before = facts(&a.view());
    let compacted = a.compact().unwrap();
    assert_eq!(compacted.folded, 1, "the last tag");
    assert_eq!(
        compacted.removed.len(),
        1,
        "the segment compaction sealed: {:?}",
        compacted.removed
    );
    assert_eq!(facts(&a.view()), before);
    let fresh = F::open(Probe::new(&machine(&folder)), env("b", 3, &clock))
        .unwrap()
        .0;
    assert_eq!(facts(&fresh.view()), before);
    a.undo().unwrap();
    assert!(
        !tags(&a.view(), song).contains(&tag("last")),
        "this session's intents stay undoable"
    );
    assert!(matches!(a.undo(), Err(Error::Refused(Refusal::Nothing))));
}

fn others_on_this_machine_are_told_apart_from_others_elsewhere<F: Facade>() {
    let folder = disk();
    let clock = TestClock::at(1_000);
    let here = machine(&folder);
    let (mut a, _) = F::open(Probe::new(&here), env("a", 1, &clock)).unwrap();
    let song = create(&mut a, "song.npno", b"song");
    let (mut b, _) = F::open(Probe::new(&here), env("b", 2, &clock)).unwrap();
    b.commit("Tag", |i| i.add(song, TAGS, tag("b"))).unwrap();
    let (mut c, _) = F::open(Probe::new(&machine(&folder)), env("c", 3, &clock)).unwrap();
    c.commit("Tag", |i| i.add(song, TAGS, tag("c"))).unwrap();
    a.refresh().unwrap();
    let presence: BTreeMap<String, Presence> = a
        .others()
        .unwrap()
        .into_iter()
        .map(|w| (w.label, w.here))
        .collect();
    assert_eq!(
        presence,
        BTreeMap::from([
            ("a".into(), Presence::This),
            ("b".into(), Presence::SameMachine),
            ("c".into(), Presence::Elsewhere),
        ])
    );
    b.close().unwrap();
    let presence = a.others().unwrap();
    assert!(presence
        .iter()
        .all(|w| w.label != "b" || w.here == Presence::Elsewhere));
}

fn a_refused_intent_changes_nothing<F: Facade>() {
    let folder = disk();
    let clock = TestClock::at(1_000);
    let (mut a, _) = F::open(Probe::new(&machine(&folder)), env("a", 1, &clock)).unwrap();
    put(&folder, "taken.npno", b"taken");
    let refused = a.commit("Import", |i| {
        let (i, _) = i.create(|e| {
            e.save(&path("taken.npno"), b"mine".to_vec(), Expect::Absent);
        });
        i
    });
    assert!(
        matches!(refused, Err(Error::Refused(Refusal::Changed(_)))),
        "{refused:?}"
    );
    assert!(
        !folder
            .directories(Root::Folder)
            .iter()
            .any(|d| d.starts_with(&layout().writers())),
        "a refused first intent creates no writer"
    );
    put(&folder, "dir.npno/inside", b"inside");
    let refused = a.commit("Import", |i| {
        let (i, _) = i.create(|e| {
            e.save(&path("dir.npno"), b"mine".to_vec(), Expect::Absent);
        });
        i
    });
    assert!(
        matches!(&refused, Err(Error::Refused(Refusal::Directory(at))) if *at == path("dir.npno")),
        "{refused:?}"
    );
    let song = create(&mut a, "song.npno", b"song");
    let written = folder.files(Root::Folder);
    let refused = a.commit("Rename", |i| {
        i.add(song, TAGS, tag("x")).rename(
            song,
            &path("taken.npno"),
            Expect::Holds(identity(b"song")),
        )
    });
    assert!(matches!(refused, Err(Error::Refused(Refusal::Changed(_)))));
    let after = folder.files(Root::Folder);
    let changed: Vec<&RelPath> = after
        .iter()
        .filter(|(path, bytes)| written.get(*path) != Some(*bytes))
        .map(|(path, _)| path)
        .collect();
    assert!(changed.is_empty(), "{changed:?}");
    assert_eq!(tags(&a.view(), song), ["new"]);
    a.commit("Tag", |i| i.add(song, TAGS, tag("next"))).unwrap();
    assert_eq!(
        tags(&a.view(), song),
        ["new", "next"],
        "the refused intent is never logged"
    );
    let nothing = a.commit("Nothing", |i| i);
    assert!(
        matches!(
            nothing,
            Err(Error::Refused(Refusal::Invalid(toshokan::Invalid::Empty)))
        ),
        "{nothing:?}"
    );
}

/// Puts the folder back as `backup` holds it, as a restore from a backup does.
fn restore(folder: &MemDisk, backup: &MemDisk) {
    let kept = backup.files(Root::Folder);
    for path in folder.files(Root::Folder).into_keys() {
        if !kept.contains_key(&path) {
            folder
                .perform(Io::Remove {
                    root: Root::Folder,
                    path,
                })
                .unwrap();
        }
    }
    for (path, bytes) in kept {
        put(folder, path.as_str(), &bytes);
    }
}

/// `a` creates `song`, the folder is backed up, then `b` tags it "theirs" and
/// sets its origin, and `a` tags it "mine". Returns `a`, still open, with the
/// backup.
fn tagged_after_a_backup<F: Facade>(
    folder: &MemDisk,
    here: &Machine,
    clock: &TestClock,
) -> (F, MemDisk, EntityId) {
    let (mut a, _) = F::open(Probe::new(here), env("a", 1, clock)).unwrap();
    let song = create(&mut a, "song.npno", b"song");
    let backup = copy_folder(folder);
    let (mut b, _) = F::open(Probe::new(&machine(folder)), env("b", 2, clock)).unwrap();
    b.commit("Tag", |i| {
        i.add(song, TAGS, tag("theirs"))
            .set(song, ORIGIN, "from b".into())
    })
    .unwrap();
    b.close().unwrap();
    a.refresh().unwrap();
    a.commit("Tag", |i| i.add(song, TAGS, tag("mine"))).unwrap();
    assert_eq!(tags(&a.view(), song), ["mine", "new", "theirs"]);
    (a, backup, song)
}

fn label_of(view: &View, label: &str) -> WriterId {
    view.writers()
        .iter()
        .find(|w| w.label == label)
        .unwrap()
        .writer
}

/// Two instances commit, refresh, compact, close, reopen and crash at random. At
/// every step each shows what its logs fold to from scratch, however it got
/// there: folded entry by entry, or read again at a reopen.
fn what_is_shown_is_what_its_logs_fold_to<F: Facade>() {
    for seed in 0..12 {
        let folder = disk();
        let clock = TestClock::at(1_000);
        let mut random = SeededRandom::new(seed);
        let mut pick = |n: usize| (random.next_u128() % n as u128) as usize;
        let mut machines = [machine(&folder), machine(&folder)];
        let mut opened: [Option<F>; 2] = [None, None];
        let mut entities: Vec<EntityId> = Vec::new();
        let mut opens = 0;
        for step in 0..40 {
            let at = pick(2);
            let Some(library) = &mut opened[at] else {
                opens += 1;
                let probe = Probe::new(&machines[at]);
                let label = ["a", "b"][at];
                opened[at] = Some(
                    F::open(probe, env(label, seed * 100 + opens, &clock))
                        .unwrap()
                        .0,
                );
                continue;
            };
            clock.advance(1 + pick(3) as u64);
            match pick(9) {
                0 | 1 => {
                    let name = format!("f{seed}-{step}.bin");
                    entities.push(create(library, &name, name.as_bytes()));
                }
                2 | 3 if !entities.is_empty() => {
                    let entity = entities[pick(entities.len())];
                    let value = tag(["x", "y", "z"][pick(3)]);
                    let _ = library.commit("Tag", |i| i.add(entity, TAGS, value));
                }
                4 if !entities.is_empty() => {
                    let entity = entities[pick(entities.len())];
                    let _ = library.commit("Origin", |i| i.set(entity, ORIGIN, format!("o{step}")));
                }
                5 if !entities.is_empty() => {
                    let entity = entities[pick(entities.len())];
                    let _ = library.commit("Untag", |i| i.remove(entity, TAGS, &tag("x")));
                }
                6 => {
                    library.refresh().unwrap();
                }
                7 => {
                    let _ = library.compact();
                }
                _ => {
                    library.refresh().unwrap();
                    let shown = facts(&library.view());
                    let library = opened[at].take().unwrap();
                    match pick(2) {
                        0 => library.close().unwrap(),
                        _ => {
                            drop(library);
                            machines[at] = Machine {
                                folder: folder.clone(),
                                local: machines[at].local.restart(),
                            };
                        }
                    }
                    opens += 1;
                    let probe = Probe::new(&machines[at]);
                    let label = ["a", "b"][at];
                    let (mut again, _) =
                        F::open(probe, env(label, seed * 100 + opens, &clock)).unwrap();
                    again.rescan().unwrap();
                    assert_eq!(
                        facts(&again.view()),
                        shown,
                        "seed {seed} step {step}: reopened"
                    );
                    opened[at] = Some(again);
                }
            }
            for (index, library) in opened.iter().enumerate() {
                if let Some(library) = library {
                    let shown = library.view().folded().clone();
                    assert_eq!(
                        shown,
                        library.refolded(),
                        "seed {seed} step {step}: instance {index}"
                    );
                }
            }
        }
    }
}

/// Two instances save, rename, trash and undo, refresh and rescan, while other
/// programs copy, move, change and remove files. After every step each binds
/// what binding every fact it shows afresh to its last scan binds, however few
/// of its bindings the step bound again.
fn what_is_bound_is_what_binding_afresh_binds<F: Facade>() {
    const PLACES: [&str; 6] = ["x/a.bin", "x/b.bin", "y/a.bin", "y/c.bin", "z.bin", "x"];
    const CONTENTS: [&[u8]; 3] = [b"one", b"two", b"three!"];
    for seed in 0..12 {
        let folder = disk();
        let clock = TestClock::at(1_000);
        let mut random = SeededRandom::new(seed + 700);
        let mut pick = |n: usize| (random.next_u128() % n as u128) as usize;
        let machines = [machine(&folder), machine(&folder)];
        let mut opened: Vec<F> = Vec::new();
        for (at, label) in ["a", "b"].into_iter().enumerate() {
            let probe = Probe::new(&machines[at]);
            let (mut library, _) =
                F::open(probe, env(label, seed * 10 + at as u64, &clock)).unwrap();
            library.rescan().unwrap();
            opened.push(library);
        }
        for step in 0..60 {
            clock.advance(1 + pick(3) as u64);
            let library = &mut opened[pick(2)];
            let view = library.view();
            let files: Vec<(EntityId, RelPath)> = view
                .entities()
                .iter()
                .filter_map(|entity| Some((entity.id(), entity.file()?.path)))
                .collect();
            let place = path(PLACES[pick(PLACES.len() - 1)]);
            let other = PLACES[pick(PLACES.len())];
            let bytes = CONTENTS[pick(CONTENTS.len())];
            let chosen = (!files.is_empty()).then(|| files[pick(files.len())].clone());
            let held = |at: &RelPath| match read(&folder, at.as_str()) {
                Some(bytes) => Expect::Holds(identity(&bytes)),
                None => Expect::Absent,
            };
            match (pick(12), chosen) {
                (0 | 1, _) => {
                    let _ = library.commit("Import", |intent| {
                        let (intent, _) = intent.create(|e| {
                            e.save(&place, bytes.to_vec(), held(&place));
                        });
                        intent
                    });
                }
                (2 | 3, Some((entity, at))) => {
                    let expect = held(&at);
                    let _ = library.commit("Save", |i| i.save(entity, &at, bytes.to_vec(), expect));
                }
                (4, Some((entity, _))) => {
                    let expect = held(&place);
                    let _ = library.commit("Rename", |i| i.rename(entity, &place, expect));
                }
                (5, Some((entity, at))) => {
                    let expect = held(&at);
                    let _ = library.commit("Trash", |i| i.trash(entity, expect));
                }
                (6, _) => {
                    let _ = library.undo();
                }
                (7, _) => {
                    if let Some(bytes) = read(&folder, other) {
                        put(&folder, place.as_str(), &bytes);
                    }
                }
                (8, _) => {
                    if read(&folder, other).is_some() && read(&folder, place.as_str()).is_none() {
                        move_outside(&folder, other, place.as_str());
                    }
                }
                (9, _) => {
                    if read(&folder, place.as_str()).is_some() {
                        put(&folder, place.as_str(), bytes);
                    }
                }
                (10, _) => {
                    let _ = folder.perform(Io::Remove {
                        root: Root::Folder,
                        path: place.clone(),
                    });
                }
                _ => {
                    match pick(3) {
                        0 => library.refresh(),
                        1 => library.rescan(),
                        _ => library.rescan_paths(vec![path(other)]),
                    }
                    .unwrap();
                }
            }
            for (index, library) in opened.iter_mut().enumerate() {
                let [held, afresh] = library.rebound();
                assert_eq!(held, afresh, "seed {seed} step {step}: instance {index}");
            }
        }
    }
}

fn a_partial_scan_after_a_restore_reads_files_of_the_lengths_the_restore_left<F: Facade>() {
    let folder = disk();
    let clock = TestClock::at(1_000);
    let here = machine(&folder);
    let (mut a, _) = F::open(Probe::new(&here), env("a", 1, &clock)).unwrap();
    let song = create(&mut a, "song.npno", b"song");
    a.rescan().unwrap();
    let backup = copy_folder(&folder);
    clock.advance(10);
    a.commit("Save", |i| {
        i.save(
            song,
            &path("song.npno"),
            b"longer!!".to_vec(),
            Expect::Holds(identity(b"song")),
        )
    })
    .unwrap();
    restore(&folder, &backup);
    put(&folder, "copy.npno", b"song");
    folder
        .perform(Io::Remove {
            root: Root::Folder,
            path: path("song.npno"),
        })
        .unwrap();
    a.rescan_paths(vec![path("copy.npno"), path("song.npno")])
        .unwrap();
    let (mut fresh, _) = F::open(Probe::new(&machine(&folder)), env("c", 4, &clock)).unwrap();
    fresh.rescan().unwrap();
    let file = |library: &F| library.view().entity(song).unwrap().file().unwrap();
    assert_eq!(file(&a), file(&fresh));
    assert_eq!(file(&a).path, path("copy.npno"));
}

fn facts_a_restore_removed_go_at_once<F: Facade>() {
    let folder = disk();
    let clock = TestClock::at(1_000);
    let mut here = machine(&folder);
    let (a, backup, song) = tagged_after_a_backup::<F>(&folder, &here, &clock);
    a.close().unwrap();
    restore(&folder, &backup);

    let (mut a, _) = F::open(Probe::new(&here), env("a", 3, &clock)).unwrap();
    a.rescan().unwrap();
    let (mut elsewhere, _) = F::open(Probe::new(&machine(&folder)), env("c", 4, &clock)).unwrap();
    elsewhere.rescan().unwrap();
    assert_eq!(tags(&a.view(), song), ["new"]);
    assert_eq!(facts(&a.view()), facts(&elsewhere.view()));
    drop(a);
    here.crash();
    let (mut a, _) = F::open(Probe::new(&here), env("a", 5, &clock)).unwrap();
    a.rescan().unwrap();
    assert_eq!(facts(&a.view()), facts(&elsewhere.view()), "after a crash");
    a.commit("Tag", |i| i.add(song, TAGS, tag("after")))
        .unwrap();
    assert_eq!(tags(&a.view(), song), ["after", "new"]);
}

fn one_intent_creates_entities_that_name_each_other<F: Facade>() {
    let folder = disk();
    let clock = TestClock::at(1_000);
    let (mut a, _) = F::open(Probe::new(&machine(&folder)), env("a", 1, &clock)).unwrap();
    let committed = a
        .commit("Import bundle", |i| {
            let (i, piano) = i.create(|e| {
                e.save(&path("b/piano.npno"), b"piano".to_vec(), Expect::Absent);
            });
            let (i, program) = i.create(|e| {
                e.save(
                    &path("b/program.nprog"),
                    b"program".to_vec(),
                    Expect::Absent,
                )
                .set(PLAYS, piano);
            });
            i.add(program, TAGS, tag("bundle"))
        })
        .unwrap();
    let [piano, program] = committed.created[..] else {
        panic!("{:?}", committed.created);
    };
    let view = a.view();
    assert_eq!(
        view.entity(program).unwrap().get(PLAYS),
        Field::Value(piano)
    );
    assert_eq!(tags(&view, program), ["bundle"]);
    assert_eq!(
        view.entity(piano).unwrap().file().unwrap().path,
        path("b/piano.npno")
    );
}

/// Opens `folder` through a probe the test keeps, to refuse renames with.
fn open_probed<F: Facade>(folder: &MemDisk, clock: &TestClock) -> (F, Probe) {
    let probe = Probe::new(&machine(folder));
    let (library, _) = F::open(probe.clone(), env("a", 1, clock)).unwrap();
    (library, probe)
}

fn refuse(probe: &Probe, text: Option<&str>) {
    probe.seen.borrow_mut().refused = text.map(path);
}

fn effects_that_stop_partway_fail_the_commit_and_keep_both_versions<F: Facade>() {
    let folder = disk();
    let clock = TestClock::at(1_000);
    let (mut a, probe) = open_probed::<F>(&folder, &clock);
    let song = create(&mut a, "song.npno", b"song");
    refuse(&probe, Some("song.npno"));
    let saved = a.commit("Save", |i| {
        i.save(
            song,
            &path("song.npno"),
            b"saved".to_vec(),
            Expect::Holds(identity(b"song")),
        )
        .set(song, ORIGIN, "saved".into())
    });
    refuse(&probe, None);
    assert!(
        matches!(&saved, Err(Error::Io { path, .. }) if path.as_str() == "song.npno"),
        "{saved:?}"
    );
    assert_ne!(a.history().last().unwrap().label, "Save");
    assert_eq!(a.view().entity(song).unwrap().get(ORIGIN), Field::Unset);
    assert_eq!(read(&folder, "song.npno"), None);
    let kept: Vec<Vec<u8>> = folder
        .files(Root::Folder)
        .into_iter()
        .filter(|(path, _)| layout().owns(path))
        .map(|(_, bytes)| bytes)
        .filter(|bytes| [b"song".as_slice(), b"saved"].contains(&bytes.as_slice()))
        .collect();
    assert_eq!(kept.len(), 2, "the old bytes in the trash, the new staged");

    create(&mut a, "next.npno", b"next");
    assert_eq!(
        read(&folder, "song.npno").as_deref(),
        Some(b"saved".as_slice())
    );
    assert_eq!(
        a.view().entity(song).unwrap().get(ORIGIN),
        Field::Value("saved".into())
    );
}

fn an_undo_that_stops_partway_fails_and_the_next_write_finishes_it<F: Facade>() {
    let folder = disk();
    let clock = TestClock::at(1_000);
    let (mut a, probe) = open_probed::<F>(&folder, &clock);
    let song = create(&mut a, "song.npno", b"one");
    a.commit("Save", |i| {
        i.save(
            song,
            &path("song.npno"),
            b"two".to_vec(),
            Expect::Holds(identity(b"one")),
        )
    })
    .unwrap();
    refuse(&probe, Some("song.npno"));
    let undone = a.undo();
    refuse(&probe, None);
    assert!(
        matches!(&undone, Err(Error::Io { path, .. }) if path.as_str() == "song.npno"),
        "{undone:?}"
    );
    assert!(!a.history()[1].undone, "the undo is not logged");
    assert_eq!(read(&folder, "song.npno"), None);
    create(&mut a, "next.npno", b"next");
    assert!(a.history()[1].undone);
    assert_eq!(
        read(&folder, "song.npno").as_deref(),
        Some(b"one".as_slice())
    );
}

/// The disk `change` of `song.npno` leaves after each crash that leaves its
/// pending record open and `unfinished` true of the folder, reopened.
fn interrupted<F: Facade>(
    change: fn(&mut F, EntityId) -> Result<Committed, Error>,
    unfinished: fn(&MemDisk) -> bool,
    mut each: impl FnMut(MemDisk, &str),
) {
    let one_disk = |disk: &MemDisk| Machine {
        folder: disk.clone(),
        local: disk.clone(),
    };
    let run = |disk: &MemDisk, crash: Option<u64>| {
        let clock = TestClock::at(1_000);
        let (mut library, _) = F::open(Probe::new(&one_disk(disk)), env("a", 1, &clock)).unwrap();
        let song = create(&mut library, "song.npno", b"song");
        let before = disk.mutations();
        if let Some(crash) = crash {
            disk.crash_after(crash);
        }
        let changed = change(&mut library, song);
        (changed, disk.mutations() - before)
    };
    let (changed, total) = run(&disk(), None);
    changed.unwrap();
    let mut open = 0;
    for crash in 0..total {
        let disk = disk();
        let _ = run(&disk, Some(crash));
        let disk = disk.restart();
        let records = disk.files(Root::Folder).into_keys().filter(|p| {
            p.components().any(|name| name == "pending") && p.as_str().ends_with(".json")
        });
        if records.count() == 1 && unfinished(&disk) {
            open += 1;
            each(disk, &format!("crash after {crash}"));
        }
    }
    assert!(open > 0, "some crash leaves the change open");
}

/// The disk a rename of `song.npno` to `b.npno` leaves after each crash that
/// leaves its record open with the file not yet moved, reopened.
fn interrupted_renames<F: Facade>(each: impl FnMut(MemDisk, &str)) {
    interrupted::<F>(
        |library, song| {
            library.commit("Rename", |i| {
                i.rename(song, &path("b.npno"), Expect::Holds(identity(b"song")))
            })
        },
        |disk| read(disk, "song.npno").is_some() && read(disk, "b.npno").is_none(),
        each,
    );
}

fn an_own_effect_that_cannot_be_finished_leaves_the_library_read_only_until_reopened<F: Facade>() {
    interrupted_renames::<F>(|disk, shown| {
        let machine = Machine {
            folder: disk.clone(),
            local: disk.clone(),
        };
        let clock = TestClock::at(2_000);
        let (mut again, opened) = F::open(Probe::new(&machine), env("a", 2, &clock)).unwrap();
        assert_eq!(opened.settled.len(), 1, "{shown}");
        put(&disk, "b.npno", b"theirs");
        let song = again.view().entities()[0].id();
        let unfinished = Why::Unfinished {
            label: "Rename".into(),
            path: path("b.npno"),
            error: IoError::AlreadyExists,
        };
        for attempt in ["the first write", "the next"] {
            let next = again.commit("Tag", |i| i.add(song, TAGS, tag("next")));
            assert!(
                matches!(&next, Err(Error::ReadOnly(why)) if *why == unfinished),
                "{shown}, {attempt}: {next:?}"
            );
        }
        assert_eq!(tags(&again.view(), song), ["new"], "{shown}: not tried");
        assert_eq!(
            read(&disk, "song.npno").as_deref(),
            Some(b"song".as_slice()),
            "{shown}"
        );
        again
            .close()
            .unwrap_or_else(|e| panic!("{shown}: closing: {e}"));

        disk.perform(Io::Remove {
            root: Root::Folder,
            path: path("b.npno"),
        })
        .unwrap();
        let (mut reopened, opened) = F::open(Probe::new(&machine), env("a", 2, &clock)).unwrap();
        assert_eq!(opened.mode, toshokan::report::Mode::Writable, "{shown}");
        reopened
            .commit("Tag", |i| i.add(song, TAGS, tag("next")))
            .unwrap_or_else(|e| panic!("{shown}: {e}"));
        assert_eq!(read(&disk, "b.npno").as_deref(), Some(b"song".as_slice()));
        assert_eq!(read(&disk, "song.npno"), None, "{shown}");
    });
}

fn an_own_effect_whose_source_is_gone_is_left_to_settle_with_consent<F: Facade>() {
    interrupted_renames::<F>(|disk, shown| {
        let machine = Machine {
            folder: disk.clone(),
            local: disk.clone(),
        };
        let clock = TestClock::at(2_000);
        disk.perform(Io::Remove {
            root: Root::Folder,
            path: path("song.npno"),
        })
        .unwrap();
        let (mut again, _) = F::open(Probe::new(&machine), env("a", 2, &clock)).unwrap();
        let song = again.view().entities()[0].id();
        again
            .commit("Tag", |i| i.add(song, TAGS, tag("next")))
            .unwrap_or_else(|e| panic!("{shown}: the next write is made: {e}"));
        let orphaned = again.refresh().unwrap().orphaned;
        let [orphan] = &orphaned[..] else {
            panic!("{shown}: one change to settle: {orphaned:?}");
        };
        assert_eq!(orphan.label, "Rename", "{shown}");
        let finished = again.settle(orphan.clone(), Settlement::Finished);
        assert!(
            matches!(&finished, Err(Error::Refused(Refusal::Gone(at))) if at.as_str() == "b.npno"),
            "{shown}: {finished:?}"
        );
        again
            .settle(orphan.clone(), Settlement::RolledBack)
            .unwrap_or_else(|e| panic!("{shown}: rolled back: {e}"));
        assert!(again.refresh().unwrap().orphaned.is_empty(), "{shown}");
    });
}

fn into_trash(io: &Io) -> bool {
    match io {
        Io::Rename { to, .. } => to.as_str().contains("/trash/"),
        Io::Create { path, .. } => path.as_str().contains("/trash/"),
        _ => false,
    }
}

/// Saves "two" over `song`, which holds "one", failing as the old bytes go to the
/// trash, so the save's record stays open.
fn interrupted_save<F: Facade>(library: &mut F, probe: &Probe, song: EntityId) {
    fail(probe, into_trash);
    let saved = library.commit("Save", |i| {
        i.save(
            song,
            &path("song.npno"),
            b"two".to_vec(),
            Expect::Holds(identity(b"one")),
        )
    });
    assert!(saved.is_err(), "{saved:?}");
    heal(probe);
}

fn a_resumed_save_leaves_a_newer_save_made_meanwhile<F: Facade>() {
    let folder = disk();
    let clock = TestClock::at(1_000);
    let (x, y) = (machine(&folder), machine(&folder));
    let probe = Probe::new(&x);
    let (mut a, _) = F::open(probe.clone(), env("x", 1, &clock)).unwrap();
    let song = create(&mut a, "song.npno", b"one");
    interrupted_save(&mut a, &probe, song);
    drop(a);

    let (mut b, _) = F::open(Probe::new(&y), env("y", 2, &clock)).unwrap();
    b.rescan().unwrap();
    b.commit("Save", |i| {
        i.save(
            song,
            &path("song.npno"),
            b"three".to_vec(),
            Expect::Holds(identity(b"one")),
        )
    })
    .expect("y saves over the file it sees");
    b.close().unwrap();

    let x = Machine {
        folder: folder.clone(),
        local: x.local.restart(),
    };
    let (mut again, _) = F::open(Probe::new(&x), env("x", 3, &clock)).unwrap();
    again
        .commit("Tag", |i| i.add(song, TAGS, tag("next")))
        .expect("the next write is made");
    assert_eq!(read(&folder, "song.npno").as_deref(), Some(&b"three"[..]));
    let orphaned = again.refresh().unwrap().orphaned;
    let [orphan] = &orphaned[..] else {
        panic!("the stale save is left to settle: {orphaned:?}");
    };
    let finished = again.settle(orphan.clone(), Settlement::Finished);
    assert!(
        matches!(&finished, Err(Error::Refused(Refusal::Changed(m))) if m.path == path("song.npno")),
        "{finished:?}"
    );
    again.empty_trash(BTreeSet::new()).unwrap();
    assert_eq!(read(&folder, "song.npno").as_deref(), Some(&b"three"[..]));
}

fn an_orphan_save_is_not_finished_over_a_newer_save<F: Facade>() {
    let folder = disk();
    let clock = TestClock::at(1_000);
    let here = machine(&folder);
    let (mut a, _) = F::open(Probe::new(&here), env("a", 1, &clock)).unwrap();
    let song = create(&mut a, "song.npno", b"one");
    let probe = Probe::new(&here);
    let (mut b, _) = F::open(probe.clone(), env("b", 2, &clock)).unwrap();
    b.rescan().unwrap();
    interrupted_save(&mut b, &probe, song);
    b.close().unwrap();

    a.commit("Save", |i| {
        i.save(
            song,
            &path("song.npno"),
            b"three".to_vec(),
            Expect::Holds(identity(b"one")),
        )
    })
    .expect("a saves over the file it sees");
    let orphaned = a.refresh().unwrap().orphaned;
    let [orphan] = &orphaned[..] else {
        panic!("the other window's save is left to settle: {orphaned:?}");
    };
    assert!(orphan.ours, "{orphan:?}");
    let finished = a.settle(orphan.clone(), Settlement::Finished);
    assert!(
        matches!(&finished, Err(Error::Refused(Refusal::Changed(m))) if m.path == path("song.npno")),
        "{finished:?}"
    );
    a.empty_trash(BTreeSet::new()).unwrap();
    assert_eq!(read(&folder, "song.npno").as_deref(), Some(&b"three"[..]));
}

fn an_orphan_save_is_not_finished_onto_a_file_put_there_since<F: Facade>() {
    let folder = disk();
    let clock = TestClock::at(1_000);
    let here = machine(&folder);
    let (mut a, _) = F::open(Probe::new(&here), env("a", 1, &clock)).unwrap();
    let song = create(&mut a, "song.npno", b"one");
    let probe = Probe::new(&here);
    let (mut b, _) = F::open(probe.clone(), env("b", 2, &clock)).unwrap();
    b.rescan().unwrap();
    refuse(&probe, Some("song.npno"));
    let saved = b.commit("Save", |i| {
        i.save(
            song,
            &path("song.npno"),
            b"two".to_vec(),
            Expect::Holds(identity(b"one")),
        )
    });
    assert!(saved.is_err(), "{saved:?}");
    refuse(&probe, None);
    b.close().unwrap();

    put(&folder, "song.npno", b"three");
    let orphaned = a.refresh().unwrap().orphaned;
    let [orphan] = &orphaned[..] else {
        panic!("the other window's save is left to settle: {orphaned:?}");
    };
    let finished = a.settle(orphan.clone(), Settlement::Finished);
    assert!(
        matches!(&finished, Err(Error::Refused(Refusal::Changed(m))) if m.path == path("song.npno")),
        "{finished:?}"
    );
    a.commit("Tag", |i| i.add(song, TAGS, tag("next")))
        .expect("the library stays writable");
    assert_eq!(read(&folder, "song.npno").as_deref(), Some(&b"three"[..]));
}

fn an_own_save_whose_staged_bytes_are_gone_rolls_back_to_the_old_bytes<F: Facade>() {
    let folder = disk();
    let machine = machine(&folder);
    let clock = TestClock::at(1_000);
    let probe = Probe::new(&machine);
    let (mut a, _) = F::open(probe.clone(), env("a", 1, &clock)).unwrap();
    let song = create(&mut a, "song.npno", b"song");
    refuse(&probe, Some("song.npno"));
    let saved = a.commit("Save", |i| {
        i.save(
            song,
            &path("song.npno"),
            b"saved".to_vec(),
            Expect::Holds(identity(b"song")),
        )
    });
    refuse(&probe, None);
    assert!(saved.is_err(), "{saved:?}");
    let staged = folder
        .files(Root::Folder)
        .into_iter()
        .find_map(|(at, bytes)| (layout().owns(&at) && bytes == b"saved").then_some(at));
    folder
        .perform(Io::Remove {
            root: Root::Folder,
            path: staged.expect("the new bytes are staged"),
        })
        .unwrap();

    a.commit("Tag", |i| i.add(song, TAGS, tag("next")))
        .expect("the next write is made");
    let orphaned = a.refresh().unwrap().orphaned;
    let [orphan] = &orphaned[..] else {
        panic!("one change to settle: {orphaned:?}");
    };
    assert!(orphan.ours, "this install's to settle");
    a.settle(orphan.clone(), Settlement::RolledBack)
        .expect("rolled back");
    assert_eq!(
        read(&folder, "song.npno").as_deref(),
        Some(b"song".as_slice()),
        "the old bytes are back"
    );
}

fn staged_files_stay_while_an_own_record_does_not_read<F: Facade>() {
    let folder = disk();
    let clock = TestClock::at(1_000);
    let (mut a, _) = F::open(Probe::new(&machine(&folder)), env("a", 1, &clock)).unwrap();
    let song = create(&mut a, "song.npno", b"song");
    let writers = layout().writers();
    let real = folder
        .files(Root::Folder)
        .into_keys()
        .find(|p| p.starts_with(&writers))
        .unwrap();
    let own = real.as_str().split('/').nth(2).unwrap().to_owned();
    let at = |name: &str| format!(".t/writers.2/{own}/{name}");
    let newer = format!(
        r#"{{"writer":"{own}","entry":{{}},"label":"Save","steps":[{{"step":"clone","staged":"{}","path":"song.npno"}}],"files":[]}}"#,
        "0".repeat(31) + "b"
    );
    put(
        &folder,
        &at(&format!("pending/{}a.json", "0".repeat(31))),
        newer.as_bytes(),
    );
    let staged = at(&format!("tmp/{}b.npno", "0".repeat(31)));
    put(&folder, &staged, b"saved");
    a.commit("Tag", |i| i.add(song, TAGS, tag("next"))).unwrap();
    assert_eq!(
        read(&folder, &staged).as_deref(),
        Some(&b"saved"[..]),
        "a record this build cannot read may place it"
    );
}

fn an_own_record_with_a_member_this_build_does_not_know_is_settled<F: Facade>() {
    let one_disk = |disk: &MemDisk| Machine {
        folder: disk.clone(),
        local: disk.clone(),
    };
    let folder = disk();
    let clock = TestClock::at(1_000);
    let probe = Probe::new(&one_disk(&folder));
    let (mut a, _) = F::open(probe.clone(), env("a", 1, &clock)).unwrap();
    let song = create(&mut a, "song.npno", b"song");
    refuse(&probe, Some("song.npno"));
    let saved = a.commit("Save", |i| {
        i.save(
            song,
            &path("song.npno"),
            b"saved".to_vec(),
            Expect::Holds(identity(b"song")),
        )
    });
    assert!(saved.is_err(), "{saved:?}");
    drop(a);
    let folder = folder.restart();
    let records = pending_files(&folder).into_iter();
    let [record] = &records
        .filter(|at| at.as_str().ends_with(".json"))
        .collect::<Vec<_>>()[..]
    else {
        panic!("one record");
    };
    let bytes = read(&folder, record.as_str()).unwrap();
    let newer = [&br#"{"newer":true,"#[..], &bytes[1..]].concat();
    put(&folder, record.as_str(), &newer);

    let (mut a, _) = F::open(Probe::new(&one_disk(&folder)), env("a", 2, &clock)).unwrap();
    a.commit("Tag", |i| i.add(song, TAGS, tag("next"))).unwrap();
    assert_eq!(
        read(&folder, "song.npno").as_deref(),
        Some(&b"saved"[..]),
        "the save is finished"
    );
}

fn an_own_save_over_a_file_deleted_since_is_finished<F: Facade>() {
    interrupted::<F>(
        |library, song| {
            library.commit("Save", |i| {
                i.save(
                    song,
                    &path("song.npno"),
                    b"saved".to_vec(),
                    Expect::Holds(identity(b"song")),
                )
            })
        },
        |disk| read(disk, "song.npno").as_deref() == Some(b"song".as_slice()),
        |disk, shown| {
            let machine = Machine {
                folder: disk.clone(),
                local: disk.clone(),
            };
            disk.perform(Io::Remove {
                root: Root::Folder,
                path: path("song.npno"),
            })
            .unwrap();
            let clock = TestClock::at(2_000);
            let (mut again, _) = F::open(Probe::new(&machine), env("a", 2, &clock)).unwrap();
            let song = again.view().entities()[0].id();
            again
                .commit("Tag", |i| i.add(song, TAGS, tag("next")))
                .unwrap_or_else(|e| panic!("{shown}: {e}"));
            assert_eq!(
                read(&disk, "song.npno").as_deref(),
                Some(b"saved".as_slice()),
                "{shown}: the save is finished"
            );
            let orphaned = again.refresh().unwrap().orphaned;
            assert!(
                orphaned.is_empty(),
                "{shown}: nothing is left: {orphaned:?}"
            );
        },
    );
}

fn a_newer_layout_beside_this_one_opens_the_library_read_only<F: Facade>() {
    let clock = TestClock::at(1_000);
    for (dir, newer) in [
        ("writers", false),
        ("writers.1", false),
        ("writers.3", true),
    ] {
        let folder = disk();
        folder
            .perform(Io::MakeDir {
                root: Root::Folder,
                path: path(&format!(".t/{dir}")),
            })
            .unwrap();
        let (_, opened) = F::open(Probe::new(&machine(&folder)), env("a", 1, &clock)).unwrap();
        let expected = match newer {
            true => toshokan::report::Mode::ReadOnly(Why::NewerLayout { dir: dir.into() }),
            false => toshokan::report::Mode::Writable,
        };
        assert_eq!(opened.mode, expected, "{dir}");
    }
}

/// Every entry in `writer`'s segments in the folder.
fn segment_entries(folder: &MemDisk, writer: WriterId) -> Vec<Entry> {
    let dir = layout().writer(writer);
    folder
        .files(Root::Folder)
        .into_iter()
        .filter(|(p, _)| {
            p.parent().as_ref() == Some(&dir) && p.as_str().ends_with(SEGMENT_EXTENSION)
        })
        .flat_map(|(_, bytes)| toshokan::line::lines(&bytes).collect::<Vec<_>>())
        .map(|line| Entry::decode(line).unwrap())
        .collect()
}

fn a_settlement_that_stops_partway_leaves_the_record_open_until_settled_again<F: Facade>() {
    for how in [Settlement::Finished, Settlement::RolledBack] {
        let mut stopped = 0;
        interrupted::<F>(
            |library, song| {
                library.commit("Save", |i| {
                    i.save(
                        song,
                        &path("song.npno"),
                        b"saved".to_vec(),
                        Expect::Holds(identity(b"song")),
                    )
                    .add(song, TAGS, tag("saved"))
                })
            },
            |disk| read(disk, "song.npno").as_deref() != Some(b"saved".as_slice()),
            |disk, crash| {
                let shown = format!("{how:?}, {crash}");
                disk.lose_local();
                let here = Machine {
                    folder: disk.clone(),
                    local: disk.clone(),
                };
                let clock = TestClock::at(2_000);
                let probe = Probe::new(&here);
                let (mut heir, opened) = F::open(probe.clone(), env("b", 2, &clock)).unwrap();
                let [orphan] = &opened.orphaned[..] else {
                    panic!("{shown}: {:?}", opened.orphaned);
                };
                refuse(&probe, Some("song.npno"));
                let settled = heir.settle(orphan.clone(), how);
                refuse(&probe, None);
                let Err(Error::Io { path: at, .. }) = settled else {
                    assert!(
                        how == Settlement::RolledBack && settled.is_ok(),
                        "{shown}: {settled:?}"
                    );
                    return;
                };
                stopped += 1;
                assert_eq!(at, path("song.npno"), "{shown}");

                let (between, opened) =
                    F::open(Probe::new(&machine(&disk)), env("c", 3, &clock)).unwrap();
                assert!(
                    opened.orphaned.contains(orphan),
                    "{shown}: still open: {:?}",
                    opened.orphaned
                );
                let song = between.view().entities()[0].id();
                assert_eq!(tags(&between.view(), song), ["new"], "{shown}");

                heir.settle(orphan.clone(), how)
                    .unwrap_or_else(|e| panic!("{shown}: settled again: {e}"));
                let (after, opened) =
                    F::open(Probe::new(&machine(&disk)), env("d", 4, &clock)).unwrap();
                assert_eq!(opened.orphaned, [], "{shown}: settled");
                let (bytes, tagged): (&[u8], &[&str]) = match how {
                    Settlement::Finished => (b"saved", &["new", "saved"]),
                    _ => (b"song", &["new"]),
                };
                assert_eq!(read(&disk, "song.npno").as_deref(), Some(bytes), "{shown}");
                assert_eq!(tags(&after.view(), song), tagged, "{shown}");

                let entries = segment_entries(&disk, label_of(&after.view(), "b"));
                let settles = entries
                    .iter()
                    .filter(
                        |e| matches!(e.kind(), EntryKind::Settle(s) if s.record == orphan.record),
                    )
                    .count();
                let saved = toshokan::Raw::of(&"saved").unwrap();
                let adds = entries
                    .iter()
                    .filter_map(|e| match e.kind() {
                        EntryKind::Intent(logged) => Some(&logged.ops),
                        _ => None,
                    })
                    .flatten()
                    .filter(|op| matches!(op, Op::Add { value, .. } if *value == saved))
                    .count();
                let planned = usize::from(how == Settlement::Finished);
                assert_eq!((settles, adds), (1, planned), "{shown}: settled once");
            },
        );
        assert!(stopped > 0, "{how:?}: some settlement stops partway");
    }
}

fn a_folder_whose_renames_may_replace_is_written<F: Facade>() {
    let racy = Capabilities {
        no_replace: false,
        ..Capabilities::ALL
    };
    let folder = MemDisk::with_capabilities(racy, Capabilities::ALL);
    let clock = TestClock::at(1_000);
    let (mut a, opened) = F::open(Probe::new(&machine(&folder)), env("a", 1, &clock)).unwrap();
    assert_eq!(opened.mode, toshokan::report::Mode::Writable);
    let song = create(&mut a, "song.npno", b"song");
    put(&folder, "taken.npno", b"taken");
    let renamed = a.commit("Rename", |i| {
        i.rename(song, &path("taken.npno"), Expect::Holds(identity(b"song")))
    });
    assert!(
        matches!(renamed, Err(Error::Refused(Refusal::Changed(_)))),
        "{renamed:?}"
    );
    assert_eq!(
        read(&folder, "taken.npno").as_deref(),
        Some(b"taken".as_slice())
    );
}

fn a_commit_writes_to_the_local_root_only_what_it_adds<F: Facade>() {
    let folder = disk();
    let clock = TestClock::at(1_000);
    let probe = Probe::new(&machine(&folder));
    let seen = Rc::clone(&probe.seen);
    let (mut a, _) = F::open(probe, env("a", 1, &clock)).unwrap();
    let song = create(&mut a, "song.npno", b"song");
    let long = "x".repeat(4096);
    for n in 0..20 {
        a.commit("Tag", |i| i.add(song, TAGS, format!("{n}{long}")))
            .unwrap();
    }
    let before = seen.borrow().local_written;
    a.commit("Tag", |i| i.add(song, TAGS, tag("last"))).unwrap();
    let written = seen.borrow().local_written - before;
    assert!(written < 2048, "{written} bytes for one short entry");
}

fn a_writers_directory_stays_bounded_across_sessions<F: Facade>() {
    let folder = disk();
    let clock = TestClock::at(1_000);
    let here = machine(&folder);
    let (mut a, _) = F::open(Probe::new(&here), env("a", 1, &clock)).unwrap();
    let song = create(&mut a, "song.npno", b"song");
    a.close().unwrap();
    for n in 0..8 {
        let (mut a, _) = F::open(Probe::new(&here), env("a", 10 + n, &clock)).unwrap();
        a.commit("Tag", |i| i.add(song, TAGS, format!("t{n}")))
            .unwrap();
        if n % 2 == 1 {
            a.compact().unwrap();
        }
        a.close().unwrap();
    }
    let logs: Vec<RelPath> = folder
        .files(Root::Folder)
        .into_keys()
        .filter(|path| path.parent().and_then(|dir| dir.parent()) == Some(layout().writers()))
        .collect();
    assert_eq!(logs.len(), 1, "one snapshot: {logs:?}");
    let (fresh, _) = F::open(Probe::new(&machine(&folder)), env("b", 99, &clock)).unwrap();
    let shown = tags(&fresh.view(), song);
    for n in 0..8 {
        assert!(shown.contains(&format!("t{n}")), "t{n} in {shown:?}");
    }
}

#[test]
fn a_root_within_the_apps_own_directory_leaves_that_directory_unscanned() {
    let folder = disk();
    let clock = TestClock::at(1_000);
    let layout = Layout::within(".app", "library").unwrap();
    put(&folder, "song.npno", b"song");
    put(&folder, ".app/index.ron", b"an older build's index");
    let (mut a, _) = blocking::Library::open(
        machine(&folder),
        layout.clone(),
        &schema(),
        env("a", 1, &clock),
    )
    .unwrap();
    assert_eq!(a.rescan().unwrap().scan.arrived, [path("song.npno")]);
    let (intent, _) = a.intent("Tag").create(|e| {
        e.adopt(&path("song.npno"), Expect::Holds(identity(b"song")))
            .add(TAGS, tag("Sunday"));
    });
    intent.commit().unwrap();
    a.close().unwrap();

    let written: Vec<RelPath> = folder.files(Root::Folder).into_keys().collect();
    assert!(
        written
            .iter()
            .filter(|at| at.starts_with(&path(".app")))
            .all(|at| at.starts_with(layout.root()) || *at == path(".app/index.ron")),
        "{written:?}"
    );
    let (mut b, _) =
        blocking::Library::open(machine(&folder), layout, &schema(), env("b", 2, &clock)).unwrap();
    b.rescan().unwrap();
    let view = b.view();
    assert_eq!(
        view.unbound().cloned().collect::<Vec<_>>(),
        [] as [RelPath; 0]
    );
    let song = view.at(&path("song.npno")).expect("the tagged file");
    assert_eq!(song.members(TAGS).values, [tag("Sunday")]);
}

/// A root within the app's own directory is made with its directory at the first
/// commit, and a file where the root would be is left as it is: the library does not
/// open, and nothing is written.
#[test]
fn a_root_within_a_directory_not_there_yet_is_made_at_the_first_commit() {
    let clock = TestClock::at(1_000);
    let layout = Layout::within(".app", "library").unwrap();
    let commit = |folder: &MemDisk| -> Result<(), Error> {
        let (mut a, _) = blocking::Library::open(
            machine(folder),
            layout.clone(),
            &schema(),
            env("a", 1, &clock),
        )?;
        a.rescan()?;
        let (intent, _) = a.intent("Tag").create(|e| {
            e.add(TAGS, tag("Sunday"));
        });
        intent.commit().map(drop)
    };
    let absent = disk();
    put(&absent, "song.npno", b"song");
    commit(&absent).unwrap();
    let written: Vec<RelPath> = absent.files(Root::Folder).into_keys().collect();
    assert!(
        written.iter().any(|at| at.starts_with(layout.root())),
        "{written:?}"
    );

    let blocked = disk();
    put(&blocked, ".app/library", b"not a directory");
    assert!(commit(&blocked).is_err());
    let written: Vec<RelPath> = blocked.files(Root::Folder).into_keys().collect();
    assert_eq!(written, [path(".app/library")]);
    assert_eq!(
        read(&blocked, ".app/library").as_deref(),
        Some(&b"not a directory"[..])
    );
}

/// Names as a volume that ignores case takes them.
#[cfg(not(target_arch = "wasm32"))]
struct Folding;

#[cfg(not(target_arch = "wasm32"))]
impl toshokan::Names for Folding {
    fn key(&self, path: &str) -> String {
        path.to_lowercase()
    }
}

/// A change of case alone renames a file, and is undone, and renames a folder, whether
/// or not the volume tells the two names apart; a file already holding the new name
/// exactly is left alone.
#[cfg(not(target_arch = "wasm32"))]
#[test]
fn a_change_of_case_alone_renames_a_file_and_a_folder() {
    use toshokan::blocking::{Library, Native};

    let (folder, local) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
    std::fs::create_dir(folder.path().join("live")).unwrap();
    std::fs::write(folder.path().join("live/c3.wav"), b"c3").unwrap();
    std::fs::write(folder.path().join("d4.wav"), b"d4").unwrap();
    let clock = TestClock::at(1_000);
    let env = Env {
        names: Box::new(Folding),
        ..env("a", 1, &clock)
    };
    let native = Native::new(folder.path(), local.path());
    let (mut a, _) = Library::open(native, layout(), &schema(), env).unwrap();
    a.rescan().unwrap();
    let names = |dir: &str| {
        let mut names: Vec<String> = std::fs::read_dir(folder.path().join(dir))
            .unwrap()
            .map(|entry| entry.unwrap().file_name().into_string().unwrap())
            .filter(|name| !name.starts_with('.'))
            .collect();
        names.sort();
        names
    };
    a.intent("Rename")
        .rename_file(
            &path("live/c3.wav"),
            &path("live/C3.wav"),
            Expect::Holds(identity(b"c3")),
        )
        .commit()
        .unwrap();
    assert_eq!(names("live"), ["C3.wav"]);
    assert_eq!(
        a.view().unbound().cloned().collect::<Vec<_>>(),
        [path("d4.wav"), path("live/C3.wav")],
        "the file is listed once, under its new name"
    );
    a.undo().unwrap();
    assert_eq!(names("live"), ["c3.wav"]);
    a.intent("Rename")
        .move_tree(&path("live"), &path("Live"))
        .commit()
        .unwrap();
    assert_eq!(names(""), ["Live", "d4.wav"]);
    assert_eq!(names("Live"), ["c3.wav"]);

    std::fs::write(folder.path().join("D4.wav"), b"other").unwrap();
    if names("").len() == 3 {
        a.rescan().unwrap();
        let refused = a
            .intent("Rename")
            .rename_file(
                &path("d4.wav"),
                &path("D4.wav"),
                Expect::Holds(identity(b"d4")),
            )
            .commit();
        assert!(matches!(refused, Err(Error::Refused(_))), "{refused:?}");
        assert_eq!(
            std::fs::read(folder.path().join("D4.wav")).unwrap(),
            b"other"
        );
        assert_eq!(std::fs::read(folder.path().join("d4.wav")).unwrap(), b"d4");
    }
}

/// A rename the folder of its source refuses names the source, the file to allow,
/// and once it is allowed, opening the library again finishes the rename.
#[cfg(unix)]
#[test]
fn a_rename_refused_at_its_source_names_the_source() {
    use std::os::unix::fs::PermissionsExt;
    use toshokan::blocking::{Library, Native};

    let (folder, local) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
    let locked = folder.path().join("locked");
    std::fs::create_dir(&locked).unwrap();
    std::fs::write(locked.join("a.wav"), b"a").unwrap();
    let clock = TestClock::at(1_000);
    let open = |seed| {
        let native = Native::new(folder.path(), local.path());
        Library::open(native, layout(), &schema(), env("a", seed, &clock))
            .unwrap()
            .0
    };
    let mut a = open(1);
    a.rescan().unwrap();
    let mode = |mode| std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(mode));
    mode(0o555).unwrap();
    if std::fs::write(locked.join("probe"), b"").is_ok() {
        mode(0o755).unwrap();
        eprintln!("skipped: this user writes a read-only directory");
        return;
    }
    let source = path("locked/a.wav");
    let failed = a
        .intent("Rename")
        .rename_file(&source, &path("b.wav"), Expect::Holds(identity(b"a")))
        .commit();
    let next = a.intent("Trash").trash_file(&source, Expect::File).commit();
    mode(0o755).unwrap();
    assert!(
        matches!(&failed, Err(Error::Io { path, error: IoError::Denied, .. }) if *path == source),
        "{failed:?}"
    );
    let unfinished = Why::Unfinished {
        label: "Rename".into(),
        path: source,
        error: IoError::Denied,
    };
    assert!(
        matches!(&next, Err(Error::ReadOnly(why)) if *why == unfinished),
        "{next:?}"
    );
    a.close().unwrap();

    let mut again = open(2);
    again.settle_interrupted().unwrap();
    assert_eq!(std::fs::read(folder.path().join("b.wav")).unwrap(), b"a");
    assert!(!locked.join("a.wav").exists());
}

/// Which renames a crash follows, by their source and destination.
#[cfg(not(target_arch = "wasm32"))]
type Stops = Rc<std::cell::Cell<Option<fn(&RelPath, &RelPath) -> bool>>>;

/// The machine's file system, stopped as a crash stops it: every request after a
/// rename `stops` picks panics.
#[cfg(not(target_arch = "wasm32"))]
struct Crashing {
    native: toshokan::blocking::Native,
    stops: Stops,
    stopped: bool,
}

#[cfg(not(target_arch = "wasm32"))]
impl Backend for Crashing {
    fn capabilities(&self, root: Root) -> Capabilities {
        self.native.capabilities(root)
    }

    fn perform(&mut self, io: Io) -> IoResult {
        assert!(!self.stopped, "crashed");
        let stops = match (&io, self.stops.get()) {
            (Io::Rename { from, to, .. }, Some(stops)) => stops(from, to),
            _ => false,
        };
        let done = self.native.perform(io);
        self.stopped = stops;
        done
    }
}

/// A change of case alone stopped by a crash after either of its steps, in a library
/// that hides dot names, on a volume whose names fold case: the next open settles it,
/// and the file and the folder are each listed once, at the new spelling.
#[cfg(not(target_arch = "wasm32"))]
#[test]
fn a_change_of_case_a_crash_stops_is_finished_at_the_next_open() {
    use std::panic::AssertUnwindSafe;
    use toshokan::blocking::{Library, Native};

    let into: fn(&RelPath, &RelPath) -> bool = |_, to| to.as_str().contains("/trash/");
    let out_of: fn(&RelPath, &RelPath) -> bool = |from, _| from.as_str().contains("/trash/");
    for stops in [into, out_of] {
        for (from, to, file) in [
            ("live/c3.wav", "live/C3.wav", "live/C3.wav"),
            ("live", "Live", "Live/c3.wav"),
        ] {
            let (folder, local) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
            std::fs::create_dir(folder.path().join("live")).unwrap();
            std::fs::write(folder.path().join("live/c3.wav"), b"c3").unwrap();
            let clock = TestClock::at(1_000);
            let layout = Layout::within(".app", "library").unwrap().hiding();
            let open = |stop: &Stops, seed| {
                let crashing = Crashing {
                    native: Native::new(folder.path(), local.path()),
                    stops: stop.clone(),
                    stopped: false,
                };
                let env = Env {
                    names: Box::new(Folding),
                    ..env("a", seed, &clock)
                };
                Library::open(crashing, layout.clone(), &schema(), env)
                    .unwrap()
                    .0
            };
            let names = |dir: &str| {
                let mut names: Vec<String> = std::fs::read_dir(folder.path().join(dir))
                    .unwrap()
                    .map(|entry| entry.unwrap().file_name().into_string().unwrap())
                    .filter(|name| !name.starts_with('.'))
                    .collect();
                names.sort();
                names
            };
            let stop = Rc::new(std::cell::Cell::new(None));
            let mut crashing = open(&stop, 1);
            crashing.rescan().unwrap();
            let (from, to, file) = (path(from), path(to), path(file));
            stop.set(Some(stops));
            let target = to.clone();
            let stopped = std::panic::catch_unwind(AssertUnwindSafe(move || {
                let built = crashing.intent("Rename");
                let built = match from.name() == Some("live") {
                    true => built.move_tree(&from, &target),
                    false => built.rename_file(&from, &target, Expect::Holds(identity(b"c3"))),
                };
                let _ = built.commit();
            }));
            assert!(stopped.is_err(), "the crash stopped the rename to {to}");

            let mut b = open(&Rc::new(std::cell::Cell::new(None)), 2);
            b.settle_interrupted().unwrap();
            b.rescan().unwrap();
            let dir = file.parent().unwrap();
            assert_eq!(
                b.view().unbound().cloned().collect::<Vec<_>>(),
                std::slice::from_ref(&file),
                "to {to}"
            );
            assert_eq!(names(""), [dir.as_str()], "to {to}");
            assert_eq!(names(dir.as_str()), [file.name().unwrap()], "to {to}");
            b.close().unwrap();
        }
    }
}

/// Two windows of one install write as two writers of its pool. Opened again with one
/// window, the effect the other window left unfinished is the install's own to settle.
#[cfg(not(target_arch = "wasm32"))]
#[test]
fn one_window_takes_up_what_another_window_of_the_install_left() {
    use std::panic::AssertUnwindSafe;
    use toshokan::blocking::{Library, Native};

    let (folder, local) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
    std::fs::write(folder.path().join("a.wav"), b"a").unwrap();
    std::fs::write(folder.path().join("b.wav"), b"b").unwrap();
    let clock = TestClock::at(1_000);
    let open = |stops: &Stops, seed| {
        let crashing = Crashing {
            native: Native::new(folder.path(), local.path()),
            stops: stops.clone(),
            stopped: false,
        };
        Library::open(crashing, layout(), &schema(), env("w", seed, &clock)).unwrap()
    };
    let never: Stops = Rc::new(std::cell::Cell::new(None));
    let crash: Stops = Rc::new(std::cell::Cell::new(None));
    let (mut first, _) = open(&never, 1);
    let (mut second, _) = open(&crash, 2);
    for library in [&mut first, &mut second] {
        library.rescan().unwrap();
    }
    first.close().unwrap();
    crash.set(Some(|_, to| to.as_str() == "b.wav"));
    let stopped = std::panic::catch_unwind(AssertUnwindSafe(move || {
        let _ = second
            .intent("Save")
            .save_file(
                &path("b.wav"),
                b"saved".to_vec(),
                Expect::Holds(identity(b"b")),
            )
            .commit();
    }));
    assert!(stopped.is_err(), "the crash stopped the save");

    let (mut again, opened) = open(&never, 4);
    // Whichever writer this window took, the save is this install's to finish.
    match &opened.orphaned[..] {
        [] => {
            assert_eq!(opened.settled.len(), 1, "its own writer's effect");
            again.settle_interrupted().unwrap();
        }
        [orphan] => {
            assert!(orphan.ours, "another window of this install left it");
            again.settle(orphan.clone(), Settlement::Finished).unwrap();
        }
        more => panic!("one effect to settle: {more:?}"),
    }
    assert_eq!(read_native(folder.path(), "b.wav"), b"saved");
}

#[cfg(not(target_arch = "wasm32"))]
fn read_native(folder: &std::path::Path, name: &str) -> Vec<u8> {
    std::fs::read(folder.join(name)).unwrap()
}

/// A folder of any size renames as one change: its files no entity is bound to are
/// not logged one by one, and the entities' files it moves are logged in one entry.
/// Undo and redo each move it whole.
#[cfg(not(target_arch = "wasm32"))]
#[test]
fn a_folder_of_fifty_thousand_files_renames_and_undoes_as_one_change() {
    use toshokan::blocking::{Library, Native};

    const LOOSE: usize = 50_000;
    const TAGGED: usize = 4_000;
    let (folder, local) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
    std::fs::create_dir(folder.path().join("old")).unwrap();
    for n in 0..LOOSE {
        let name = format!("old/loose {n:05}.wav");
        std::fs::write(folder.path().join(name), n.to_string()).unwrap();
    }
    let clock = TestClock::at(1_000);
    let (mut a, _) = Library::open(
        Native::new(folder.path(), local.path()),
        layout(),
        &schema(),
        env("a", 1, &clock),
    )
    .unwrap();
    a.rescan().unwrap();
    for batch in 0..TAGGED / 500 {
        let mut intent = a.intent("Import");
        for n in batch * 500..(batch + 1) * 500 {
            let at = path(&format!("old/tagged/song {n:04}.npno"));
            intent = intent
                .create(|e| {
                    e.save(&at, n.to_string().into_bytes(), Expect::Absent)
                        .add(TAGS, tag("live"));
                })
                .0;
        }
        intent.commit().unwrap();
    }
    let count = |dir: &str| {
        let mut files = 0;
        let mut waiting = vec![folder.path().join(dir)];
        while let Some(at) = waiting.pop() {
            for entry in std::fs::read_dir(at).into_iter().flatten().flatten() {
                match entry.file_type().unwrap().is_dir() {
                    true => waiting.push(entry.path()),
                    false => files += 1,
                }
            }
        }
        files
    };
    let bound_under = |a: &Library<Native>, dir: &str| {
        let view = a.view();
        let files = view
            .entities()
            .into_iter()
            .filter_map(|entity| entity.file());
        files
            .filter(|file| file.path.starts_with(&path(dir)))
            .count()
    };

    a.intent("Rename")
        .move_tree(&path("old"), &path("new"))
        .commit()
        .unwrap();
    assert_eq!((count("old"), count("new")), (0, LOOSE + TAGGED));
    assert_eq!(bound_under(&a, "new"), TAGGED);
    let writers = folder.path().join(".t/writers.2");
    let logs = std::fs::read_dir(&writers).unwrap().flatten();
    let segments = logs.flat_map(|dir| std::fs::read_dir(dir.path()).unwrap().flatten());
    let lines: String = segments
        .filter(|file| file.path().extension().is_some_and(|ext| ext == "txt"))
        .map(|file| std::fs::read_to_string(file.path()).unwrap())
        .collect();
    let renames = lines.matches(r#""label":"Rename""#).count();
    assert_eq!(renames, 1, "the file ops are one entry");
    assert_eq!(lines.matches(r#""op":"tree""#).count(), 1, "{lines}");
    assert_eq!(
        lines.matches(r#""op":"path""#).count(),
        0,
        "no file no entity is bound to is logged"
    );

    a.undo().unwrap();
    assert_eq!((count("old"), count("new")), (LOOSE + TAGGED, 0), "undone");
    assert_eq!(bound_under(&a, "old"), TAGGED);
    a.redo().unwrap();
    assert_eq!((count("old"), count("new")), (0, LOOSE + TAGGED), "redone");
    assert_eq!(bound_under(&a, "new"), TAGGED);
}

fn compaction_deletes_only_a_snapshot_cut_short_where_the_folder_cannot_rename<F: Facade>() {
    let folder = disk();
    let clock = TestClock::at(1_000);
    let here = machine(&folder);
    let (mut a, _) = F::open(Probe::new(&here), env("a", 1, &clock)).unwrap();
    create(&mut a, "song.npno", b"song");
    a.close().unwrap();
    let writers = layout().writers();
    let real = folder
        .files(Root::Folder)
        .into_keys()
        .find(|p| p.starts_with(&writers))
        .unwrap();
    let own = real.as_str().split('/').nth(2).unwrap().to_owned();
    let at = |name: &str| format!(".t/writers.2/{own}/{name}");
    put(&folder, &at("snapshot-cut.json"), br#"{"writer":"#);
    put(&folder, &at("snapshot-newer.json"), br#"{"writer":1}"#);

    let (mut a, opened) = F::open(Probe::new(&here), env("a", 2, &clock)).unwrap();
    assert_eq!(opened.unreadable.len(), 2, "{:?}", opened.unreadable);
    a.commit("Tag", |i| i.create(|e| _ = e.add(TAGS, tag("x"))).0)
        .unwrap();
    a.compact().unwrap();
    let in_place = !FOLDER.get().rename_file;
    assert_eq!(
        read(&folder, &at("snapshot-cut.json")).is_none(),
        in_place,
        "a snapshot cut short is deleted only where snapshots are written in place"
    );
    assert!(
        read(&folder, &at("snapshot-newer.json")).is_some(),
        "a whole snapshot this build refuses is kept"
    );
}

fn a_crash_in_compaction_tears_a_snapshot_only_where_the_folder_cannot_rename<F: Facade>() {
    let clock = TestClock::at(1_000);
    let one_disk = |disk: &MemDisk| Machine {
        folder: disk.clone(),
        local: disk.clone(),
    };
    // A folder that syncs, so that a crash can tear what it has not synced.
    let folder = Capabilities {
        fsync: true,
        ..FOLDER.get()
    };
    let setup = || {
        let disk = MemDisk::with_capabilities(folder, Capabilities::ALL);
        let (mut a, _) = F::open(Probe::new(&one_disk(&disk)), env("a", 1, &clock)).unwrap();
        create(&mut a, "song.npno", b"song");
        drop(a);
        disk.restart()
    };
    let renames = folder.rename_file;
    let staged = |disk: &MemDisk| -> Vec<RelPath> {
        let files = disk.files(Root::Folder).into_keys();
        let staged =
            files.filter(|path| path.parent().is_some_and(|dir| dir.name() == Some("tmp")));
        staged.collect()
    };
    let (mut torn, mut left, mut resumed) = (0, 0, 0);
    crash::sweep(
        setup,
        |disk| {
            if let Ok((mut a, _)) = F::open(Probe::new(&one_disk(disk)), env("a", 2, &clock)) {
                let _ = a.compact();
            }
        },
        |fault, disk| {
            let shown = format!("{fault:?}");
            let (mut a, opened) = F::open(Probe::new(&one_disk(&disk)), env("a", 3, &clock))
                .unwrap_or_else(|e| panic!("{shown}: {e}"));
            let snapshots = opened.unreadable.iter().filter(|path| {
                path.name()
                    .is_some_and(|name| name.starts_with("snapshot-"))
            });
            let snapshots = snapshots.count();
            if renames {
                assert_eq!(snapshots, 0, "{shown}: {:?}", opened.unreadable);
            }
            torn += snapshots;
            if !matches!(opened.start, Start::Resumed(_)) {
                return;
            }
            left += staged(&disk).len();
            let next = a.commit("Next", |i| {
                let (i, _) = i.create(|e| {
                    e.add(TAGS, tag("next"));
                });
                i
            });
            next.unwrap_or_else(|e| panic!("{shown}: {e}"));
            assert_eq!(staged(&disk), [] as [RelPath; 0], "{shown}");
            a.compact().unwrap_or_else(|e| panic!("{shown}: {e}"));
            drop(a);
            let (_, opened) = F::open(Probe::new(&one_disk(&disk)), env("a", 4, &clock))
                .unwrap_or_else(|e| panic!("{shown}: {e}"));
            assert_eq!(
                opened.unreadable,
                [],
                "{shown}: the next compaction removes it"
            );
            resumed += 1;
        },
    );
    assert!(resumed > 0, "some crash leaves the writer to resume");
    assert_eq!(torn > 0, !renames, "torn snapshots: {torn}");
    assert_eq!(left > 0, renames, "staged snapshots left: {left}");
}

/// The row at `at`.
fn row_at(view: &View, at: &str) -> Row {
    let row = view.row_at(&path(at));
    row.unwrap_or_else(|| panic!("no row at {at}")).clone()
}

/// The row of `entity`'s file.
fn row_of(view: &View, entity: EntityId) -> Row {
    let row = view.row_of(entity);
    row.unwrap_or_else(|| panic!("no row of {entity}")).clone()
}

/// Each change of the row `file`, with who and how.
fn changes_of(changes: &RowChanges, file: FileId) -> Vec<(RowWhat, Option<Who>, Option<How>)> {
    let of = changes.files.iter().filter(|change| change.file == file);
    of.map(|change| (change.what.clone(), change.by, change.how))
        .collect()
}

/// Removes the file at `at` as another program would.
fn remove_outside(folder: &MemDisk, at: &str) {
    folder
        .perform(Io::Remove {
            root: Root::Folder,
            path: path(at),
        })
        .unwrap();
}

fn own_effects_keep_each_rows_file_id_and_name_it<F: Facade>() {
    let folder = disk();
    let clock = TestClock::at(1_000);
    put(&folder, "a.wav", b"a");
    let (mut a, _) = F::open(Probe::new(&machine(&folder)), env("a", 1, &clock)).unwrap();
    a.rescan().unwrap();
    let loose = row_at(&a.view(), "a.wav").file;
    let held = Expect::Holds(identity(b"a"));
    let renamed = a
        .commit("Rename", |i| {
            i.rename_file(&path("a.wav"), &path("b.wav"), held)
        })
        .unwrap();
    assert_eq!(renamed.files, [(path("b.wav"), loose)]);
    put(&folder, "a.wav", b"another");
    a.rescan().unwrap();
    let view = a.view();
    assert_eq!(
        row_at(&view, "b.wav").file,
        loose,
        "the row moved with its file"
    );
    assert_ne!(
        row_at(&view, "a.wav").file,
        loose,
        "a file put there since is new"
    );
    let saved = a
        .commit("Save", |i| {
            i.save_file(&path("b.wav"), b"bb".to_vec(), held)
        })
        .unwrap();
    assert_eq!(
        saved.files,
        [(path("b.wav"), loose)],
        "a save over keeps it"
    );
    let before = a.view();
    let song = create(&mut a, "song.npno", b"song");
    let view = a.view();
    let made = row_of(&view, song);
    assert_eq!(made.path, path("song.npno"));
    assert_eq!(
        changes_of(&view.changes_since(&before), made.file),
        [(RowWhat::Appeared, Some(Who::This), Some(How::Made))]
    );
    let moved = a
        .commit("Rename", |i| {
            i.rename(
                song,
                &path("set/song.npno"),
                Expect::Holds(identity(b"song")),
            )
        })
        .unwrap();
    assert_eq!(moved.files, [(path("set/song.npno"), made.file)]);
    let before = a.view();
    let held = Expect::Holds(identity(b"bb"));
    let trashed = a
        .commit("Trash", |i| i.trash_file(&path("b.wav"), held))
        .unwrap();
    assert_eq!(trashed.files, []);
    let view = a.view();
    assert_eq!(view.row(loose), None);
    assert_eq!(
        changes_of(&view.changes_since(&before), loose),
        [(RowWhat::Gone, None, None)]
    );
}

/// A commit names what each file its effects wrote holds, on a volume whose one time
/// shows no write, and names none it only moved.
fn a_commit_names_what_each_file_it_wrote_holds<F: Facade>() {
    let folder = disk();
    let clock = TestClock::at(1_000);
    put(&folder, "a.wav", b"a");
    let probe = Probe::new(&machine(&folder));
    probe.seen.borrow_mut().coarse = Some(7);
    let (mut a, _) = F::open(probe, env("a", 1, &clock)).unwrap();
    a.rescan().unwrap();
    let wrote = |committed: &Committed| -> Vec<(String, Identity, u64)> {
        let wrote = committed.wrote.iter();
        let wrote = wrote.map(|fact| (fact.path.to_string(), fact.identity, fact.len));
        wrote.collect()
    };
    let renamed = a
        .commit("Rename", |i| {
            i.rename_file(
                &path("a.wav"),
                &path("b.wav"),
                Expect::Holds(identity(b"a")),
            )
        })
        .unwrap();
    assert_eq!(wrote(&renamed), [], "a move writes nothing");
    let saved = a
        .commit("Save", |i| {
            i.save_file(&path("b.wav"), b"b".to_vec(), Expect::Holds(identity(b"a")))
        })
        .unwrap();
    assert_eq!(wrote(&saved), [("b.wav".into(), identity(b"b"), 1)]);
    let undone = a.undo().unwrap();
    assert_eq!(
        wrote(&undone),
        [("b.wav".into(), identity(b"a"), 1)],
        "the undo puts back the bytes the save replaced"
    );
    let undone = a.undo().unwrap();
    assert_eq!(wrote(&undone), [], "the rename's undo only moves");
}

fn a_row_an_own_effect_moved_continues_when_logging_it_fails<F: Facade>() {
    let folder = disk();
    let clock = TestClock::at(1_000);
    put(&folder, "a.wav", b"a");
    let probe = Probe::new(&machine(&folder));
    let (mut a, _) = F::open(probe.clone(), env("a", 1, &clock)).unwrap();
    create(&mut a, "song.npno", b"song");
    a.rescan().unwrap();
    let loose = row_at(&a.view(), "a.wav").file;
    fail(&probe, segment_write);
    let held = Expect::Holds(identity(b"a"));
    let renamed = a.commit("Rename", |i| {
        i.rename_file(&path("a.wav"), &path("b.wav"), held)
    });
    heal(&probe);
    assert!(renamed.is_err(), "the log write fails: {renamed:?}");
    assert!(folder.files(Root::Folder).contains_key(&path("b.wav")));
    a.refresh().unwrap();
    let row = row_at(&a.view(), "b.wav");
    assert_eq!(row.file, loose, "a refresh lists it");
    assert!(
        matches!(row.state, RowState::Present { len: 1, .. }),
        "{row:?}"
    );
}

fn a_row_an_own_effect_moved_continues_when_the_effects_stop_part_way<F: Facade>() {
    let folder = disk();
    let clock = TestClock::at(1_000);
    put(&folder, "a.wav", b"a");
    put(&folder, "c.wav", b"c");
    let (mut a, probe) = open_probed::<F>(&folder, &clock);
    a.rescan().unwrap();
    let (first, second) = (
        row_at(&a.view(), "a.wav").file,
        row_at(&a.view(), "c.wav").file,
    );
    refuse(&probe, Some("d.wav"));
    let renamed = a.commit("Rename", |i| {
        i.rename_file(
            &path("a.wav"),
            &path("b.wav"),
            Expect::Holds(identity(b"a")),
        )
        .rename_file(
            &path("c.wav"),
            &path("d.wav"),
            Expect::Holds(identity(b"c")),
        )
    });
    refuse(&probe, None);
    assert!(renamed.is_err(), "the second rename fails: {renamed:?}");
    a.refresh().unwrap();
    let view = a.view();
    assert_eq!(
        row_at(&view, "b.wav").file,
        first,
        "the rename done moved it"
    );
    assert_eq!(view.row_at(&path("a.wav")), None);
    assert_eq!(row_at(&view, "c.wav").file, second);
    a.rescan().unwrap();
    assert_eq!(row_at(&a.view(), "b.wav").file, first, "a rescan keeps it");
    create(&mut a, "next.npno", b"next");
    let view = a.view();
    assert_eq!(row_at(&view, "b.wav").file, first, "settling keeps it");
    assert_eq!(
        row_at(&view, "d.wav").file,
        second,
        "settling moves the other"
    );
}

fn a_tree_move_carries_only_the_rows_of_files_it_moved<F: Facade>() {
    let folder = disk();
    let clock = TestClock::at(1_000);
    put(&folder, "d/loose.wav", b"loose");
    put(&folder, "d/x/held.wav", b"held");
    let (mut a, probe) = open_probed::<F>(&folder, &clock);
    let song = create(&mut a, "d/song.npno", b"song");
    a.rescan().unwrap();
    let held = row_at(&a.view(), "d/x/held.wav").file;
    remove_outside(&folder, "d/song.npno");
    fail(
        &probe,
        |io| matches!(io, Io::ListStat { dir, .. } if *dir == path("d/x")),
    );
    a.rescan().unwrap();
    heal(&probe);
    let missing = row_of(&a.view(), song);
    assert_eq!(missing.state, RowState::Missing);
    assert_eq!(a.view().row(held).unwrap().state, RowState::Unknown);
    let moved = a
        .commit("Move", |i| i.move_tree(&path("d"), &path("k")))
        .unwrap();
    let view = a.view();
    assert_eq!(
        (row_of(&view, song).file, row_of(&view, song).path.as_str()),
        (missing.file, "d/song.npno"),
        "a missing file stays where it was logged"
    );
    let row = view.row(held).unwrap().clone();
    assert!(
        read(&folder, row.path.as_str()).is_some(),
        "the unknown row is where its file is: {row:?}"
    );
    let named: Vec<FileId> = moved.files.iter().map(|(_, file)| *file).collect();
    assert!(!named.contains(&missing.file), "{moved:?}");
    assert_eq!(
        named.contains(&held),
        row.path.as_str() == "k/x/held.wav",
        "{moved:?}"
    );
    a.rescan().unwrap();
    assert_eq!(row_at(&a.view(), row.path.as_str()).file, held);
}

/// A probe on a folder that keeps names decomposed.
fn decomposing(folder: &MemDisk) -> Probe {
    let probe = Probe::new(&machine(folder));
    probe.seen.borrow_mut().decomposing = true;
    probe
}

fn own_effects_name_the_rows_of_files_the_folder_lists_respelled<F: Facade>() {
    let folder = disk();
    let clock = TestClock::at(1_000);
    put(&folder, "loose.wav", b"loose");
    let env = Env {
        names: Box::new(Composing),
        ..env("a", 1, &clock)
    };
    let (mut a, _) = F::open(decomposing(&folder), env).unwrap();
    a.rescan().unwrap();
    let loose = row_at(&a.view(), "loose.wav").file;
    let renamed = a
        .commit("Rename", |i| {
            i.rename_file(
                &path("loose.wav"),
                &path("Caf\u{e9}.wav"),
                Expect::Holds(identity(b"loose")),
            )
        })
        .unwrap();
    assert_eq!(renamed.files, [(path("Caf\u{e9}.wav"), loose)]);
    let made = a
        .commit("Import", |intent| {
            let (intent, _) = intent.create(|e| {
                e.save(
                    &path("\u{e9}t\u{e9}.npno"),
                    b"song".to_vec(),
                    Expect::Absent,
                )
                .add(TAGS, tag("new"));
            });
            intent
        })
        .unwrap();
    let song = made.created[0];
    let [(_, tagged)] = made.files[..] else {
        panic!("one file: {:?}", made.files);
    };
    for look in ["commit", "refresh", "rescan"] {
        match look {
            "refresh" => drop(a.refresh().unwrap()),
            "rescan" => drop(a.rescan().unwrap()),
            _ => {}
        }
        let view = a.view();
        let row = view.row(loose).cloned();
        let row = row.unwrap_or_else(|| panic!("{look}: no row {loose:?}"));
        assert_eq!(row.path.as_str(), "Cafe\u{301}.wav", "{look}: as listed");
        assert!(
            matches!(row.state, RowState::Present { .. }),
            "{look}: {row:?}"
        );
        let row = row_of(&view, song);
        assert_eq!(row.file, tagged, "{look}: {row:?}");
        assert!(
            matches!(row.state, RowState::Present { .. }),
            "{look}: {row:?}"
        );
        assert_eq!(
            view.rows().count(),
            2,
            "{look}: {:?}",
            view.rows().collect::<Vec<_>>()
        );
    }
}

fn an_own_effects_file_the_scan_misses_keeps_the_row_its_commit_named<F: Facade>() {
    let folder = disk();
    let clock = TestClock::at(1_000);
    let (mut a, _) = F::open(decomposing(&folder), env("a", 1, &clock)).unwrap();
    a.rescan().unwrap();
    let made = a
        .commit("Import", |intent| {
            let (intent, _) = intent.create(|e| {
                e.save(
                    &path("\u{e9}t\u{e9}.npno"),
                    b"song".to_vec(),
                    Expect::Absent,
                )
                .add(TAGS, tag("new"));
            });
            intent
        })
        .unwrap();
    let row = row_of(&a.view(), made.created[0]);
    assert_eq!(
        row.state,
        RowState::Missing,
        "names compared exactly miss it"
    );
    assert_eq!(made.files, [(row.path, row.file)]);
}

fn a_tagged_row_follows_its_file_and_an_untagged_row_ends_where_its_file_left<F: Facade>() {
    let folder = disk();
    let clock = TestClock::at(1_000);
    put(&folder, "loose.wav", b"loose");
    let (mut a, _) = F::open(Probe::new(&machine(&folder)), env("a", 1, &clock)).unwrap();
    let song = create(&mut a, "song.npno", b"song");
    a.rescan().unwrap();
    let before = a.view();
    let (tagged, loose) = (
        row_of(&before, song).file,
        row_at(&before, "loose.wav").file,
    );
    move_outside(&folder, "song.npno", "set/song.npno");
    move_outside(&folder, "loose.wav", "set/loose.wav");
    a.rescan().unwrap();
    let view = a.view();
    let row = row_of(&view, song);
    assert_eq!((row.file, row.path.as_str()), (tagged, "set/song.npno"));
    assert_eq!(
        view.row(loose),
        None,
        "no rule follows a file no entity has"
    );
    let renamed = row_at(&view, "set/loose.wav");
    let changes = view.changes_since(&before);
    let moved = RowWhat::Moved {
        from: path("song.npno"),
    };
    assert_eq!(
        changes_of(&changes, tagged),
        [(moved, Some(Who::Outside), Some(How::Moved))]
    );
    assert_eq!(changes_of(&changes, loose), [(RowWhat::Gone, None, None)]);
    assert_eq!(
        changes_of(&changes, renamed.file),
        [(RowWhat::Appeared, None, None)]
    );
}

fn the_binding_comes_before_the_same_path<F: Facade>() {
    let folder = disk();
    let clock = TestClock::at(1_000);
    let (mut a, _) = F::open(Probe::new(&machine(&folder)), env("a", 1, &clock)).unwrap();
    let song = create(&mut a, "song.npno", b"song");
    a.rescan().unwrap();
    let tagged = row_of(&a.view(), song).file;
    let (mut b, _) = F::open(Probe::new(&machine(&folder)), env("b", 2, &clock)).unwrap();
    b.rescan().unwrap();
    let held = Expect::Holds(identity(b"song"));
    b.commit("Rename", |i| i.rename(song, &path("set/song.npno"), held))
        .unwrap();
    put(&folder, "song.npno", b"a new song");
    a.refresh().unwrap();
    let view = a.view();
    let row = row_of(&view, song);
    assert_eq!((row.file, row.path.as_str()), (tagged, "set/song.npno"));
    let there = row_at(&view, "song.npno");
    assert_ne!(
        there.file, tagged,
        "the file now at its old path is another"
    );
    assert_eq!(there.entity, None);
}

fn a_row_under_a_folder_the_scan_cannot_list_is_unknown_and_never_moves_to_a_copy<F: Facade>() {
    let folder = disk();
    let clock = TestClock::at(1_000);
    let probe = Probe::new(&machine(&folder));
    put(&folder, "d/loose.wav", b"loose");
    let (mut a, _) = F::open(probe.clone(), env("a", 1, &clock)).unwrap();
    let song = create(&mut a, "d/song.npno", b"song");
    a.rescan().unwrap();
    let (tagged, loose) = (
        row_of(&a.view(), song).file,
        row_at(&a.view(), "d/loose.wav").file,
    );
    put(&folder, "copy/song.npno", b"song");
    fail(
        &probe,
        |io| matches!(io, Io::ListStat { dir, .. } if *dir == path("d")),
    );
    a.rescan().unwrap();
    let view = a.view();
    let row = row_of(&view, song);
    assert_eq!((row.file, row.path.as_str()), (tagged, "d/song.npno"));
    assert_eq!(row.state, RowState::Unknown, "never moved to the copy");
    let row = view.row(loose).unwrap();
    assert_eq!(
        (row.path.as_str(), row.state),
        ("d/loose.wav", RowState::Unknown)
    );
    assert_eq!(row_at(&view, "copy/song.npno").entity, None);

    remove_outside(&folder, "copy/song.npno");
    move_outside(&folder, "d/song.npno", "e/song.npno");
    heal(&probe);
    a.rescan().unwrap();
    let view = a.view();
    let row = row_of(&view, song);
    assert_eq!(
        (row.file, row.path.as_str()),
        (tagged, "e/song.npno"),
        "the binding, once listed"
    );
    assert!(matches!(row.state, RowState::Present { .. }));
    let row = view.row(loose).unwrap();
    assert!(
        matches!(row.state, RowState::Present { .. }),
        "the same path"
    );
}

fn an_entity_whose_file_is_nowhere_keeps_its_row_as_missing<F: Facade>() {
    let folder = disk();
    let clock = TestClock::at(1_000);
    let (mut a, _) = F::open(Probe::new(&machine(&folder)), env("a", 1, &clock)).unwrap();
    let song = create(&mut a, "song.npno", b"song");
    a.rescan().unwrap();
    let before = a.view();
    let tagged = row_of(&before, song).file;
    remove_outside(&folder, "song.npno");
    a.rescan().unwrap();
    let view = a.view();
    let row = row_of(&view, song);
    assert_eq!((row.file, row.path.as_str()), (tagged, "song.npno"));
    assert_eq!(row.state, RowState::Missing);
    assert_eq!(
        changes_of(&view.changes_since(&before), tagged),
        [(RowWhat::Missing, Some(Who::Outside), None)]
    );
    put(&folder, "song.npno", b"song");
    a.rescan().unwrap();
    let row = row_of(&a.view(), song);
    assert_eq!(row.file, tagged);
    assert!(matches!(row.state, RowState::Present { .. }));
}

fn a_row_follows_its_entity_into_the_entity_it_joined<F: Facade>() {
    let folder = disk();
    let clock = TestClock::at(1_000);
    let (mut a, mut b, made) = adopted_by_both::<F>(&folder, &clock, [None, None]);
    let (low, high) = (made[0].min(made[1]), made[0].max(made[1]));
    for writer in [&mut a, &mut b] {
        let before = writer.view();
        let row = row_at(&before, NOTES);
        writer.refresh().unwrap();
        let view = writer.view();
        let now = row_at(&view, NOTES);
        assert_eq!((now.file, now.entity), (row.file, Some(low)));
        assert_eq!(view.row_of(high), None, "a joined entity has no row");
        let changes = changes_of(&view.changes_since(&before), row.file);
        let whats: Vec<RowWhat> = changes.into_iter().map(|(what, ..)| what).collect();
        match row.entity == Some(high) {
            true => assert_eq!(whats, [RowWhat::Lost(high), RowWhat::Gained(low)]),
            false => assert_eq!(whats, []),
        }
    }
}

fn an_untagged_copy_that_becomes_an_entitys_file_merges_into_the_entitys_row<F: Facade>() {
    let folder = disk();
    let clock = TestClock::at(1_000);
    let (mut a, _) = F::open(Probe::new(&machine(&folder)), env("a", 1, &clock)).unwrap();
    let song = create(&mut a, "song.npno", b"song");
    put(&folder, "copy.npno", b"song");
    a.rescan().unwrap();
    let before = a.view();
    let (tagged, copy) = (
        row_of(&before, song).file,
        row_at(&before, "copy.npno").file,
    );
    remove_outside(&folder, "song.npno");
    a.rescan().unwrap();
    let view = a.view();
    let row = row_of(&view, song);
    assert_eq!((row.file, row.path.as_str()), (tagged, "copy.npno"));
    assert_eq!(view.row(copy), None, "a file never has two FileIds");
    let changes = view.changes_since(&before);
    let whats = |file| {
        changes_of(&changes, file)
            .into_iter()
            .map(|(what, ..)| what)
    };
    let merged = RowWhat::Merged { into: tagged };
    assert_eq!(whats(copy).collect::<Vec<_>>(), [merged]);
    let moved = RowWhat::Moved {
        from: path("song.npno"),
    };
    assert_eq!(whats(tagged).collect::<Vec<_>>(), [moved]);
}

/// The live writes of `entity`'s file register, by writer and path.
fn file_writes(view: &View, entity: EntityId) -> Vec<(WriterId, RelPath)> {
    let writes = view.folded().file_writes(entity).into_iter();
    let writes = writes.map(|write| (write.by, write.value.unwrap().path));
    writes.collect()
}

fn a_move_found_by_identity_is_pinned_only_for_a_file_fact_this_install_wrote<F: Facade>() {
    let folder = disk();
    let clock = TestClock::at(1_000);
    let here = machine(&folder);
    let (mut window, _) = F::open(Probe::new(&here), env("w", 1, &clock)).unwrap();
    let ours = create(&mut window, "ours.npno", b"ours");
    let window_writer = own_writer(&window.view()).unwrap();
    let (mut b, _) = F::open(Probe::new(&machine(&folder)), env("b", 2, &clock)).unwrap();
    let theirs = create(&mut b, "theirs.npno", b"theirs");
    let b_writer = own_writer(&b.view()).unwrap();
    let (mut a, _) = F::open(Probe::new(&here), env("a", 3, &clock)).unwrap();
    a.rescan().unwrap();
    move_outside(&folder, "ours.npno", "moved/ours.npno");
    move_outside(&folder, "theirs.npno", "moved/theirs.npno");
    a.rescan().unwrap();
    a.commit("Tag", |i| i.add(ours, TAGS, tag("kept"))).unwrap();
    let view = a.view();
    let a_writer = own_writer(&view).unwrap();
    assert_eq!(
        file_writes(&view, ours),
        [(a_writer, path("moved/ours.npno"))],
        "another window's fact is this install's to pin"
    );
    assert_eq!(
        file_writes(&view, theirs),
        [(b_writer, path("theirs.npno"))],
        "another install pins its own"
    );
    assert_ne!(window_writer, a_writer);
    assert_eq!(row_of(&view, theirs).path, path("moved/theirs.npno"));
    window.close().unwrap();
}

fn a_move_another_install_logged_before_its_file_arrived_is_not_pinned<F: Facade>() {
    let folder = disk();
    let clock = TestClock::at(1_000);
    let (mut a, _) = F::open(Probe::new(&machine(&folder)), env("a", 1, &clock)).unwrap();
    let song = create(&mut a, "song.npno", b"song");
    a.rescan().unwrap();
    let tagged = row_of(&a.view(), song).file;
    let (mut b, _) = F::open(Probe::new(&machine(&folder)), env("b", 2, &clock)).unwrap();
    b.rescan().unwrap();
    let held = Expect::Holds(identity(b"song"));
    b.commit("Rename", |i| i.rename(song, &path("set/song.npno"), held))
        .unwrap();
    let b_writer = own_writer(&b.view()).unwrap();
    // The log arrives here before the file does.
    move_outside(&folder, "set/song.npno", "song.npno");
    a.refresh().unwrap();
    a.commit("Tag", |i| i.add(song, TAGS, tag("kept"))).unwrap();
    let logged = [(b_writer, path("set/song.npno"))];
    assert_eq!(file_writes(&a.view(), song), logged, "nothing is pinned");
    assert_eq!(row_of(&a.view(), song).file, tagged);
    move_outside(&folder, "song.npno", "set/song.npno");
    a.rescan().unwrap();
    let view = a.view();
    let row = row_of(&view, song);
    assert_eq!((row.file, row.path.as_str()), (tagged, "set/song.npno"));
    let file = view.entity(song).unwrap().file().unwrap();
    assert_eq!(file.state, FileState::InSync);
}

fn a_pin_and_another_installs_move_at_another_time_are_one_fact_a_pin_replaces<F: Facade>() {
    let folder = disk();
    let clock = TestClock::at(1_000);
    let probe = Probe::new(&machine(&folder));
    probe.seen.borrow_mut().coarse = Some(7);
    let (mut a, _) = F::open(probe.clone(), env("a", 1, &clock)).unwrap();
    let song = create(&mut a, "song.npno", b"song");
    a.rescan().unwrap();
    let (mut b, _) = F::open(Probe::new(&machine(&folder)), env("b", 2, &clock)).unwrap();
    b.rescan().unwrap();
    let held = Expect::Holds(identity(b"song"));
    b.commit("Rename", |i| i.rename(song, &path("set/song.npno"), held))
        .unwrap();
    let b_writer = own_writer(&b.view()).unwrap();
    // The file arrives here before the log does.
    probe.seen.borrow_mut().unreadable = Some(layout().writer(b_writer));
    a.rescan().unwrap();
    a.commit("Tag", |i| i.add(song, TAGS, tag("kept"))).unwrap();
    heal(&probe);
    a.refresh().unwrap();
    let view = a.view();
    let a_writer = own_writer(&view).unwrap();
    let both = [
        (a_writer, path("set/song.npno")),
        (b_writer, path("set/song.npno")),
    ];
    let mut writes = file_writes(&view, song);
    writes.sort();
    let mut expected = both.to_vec();
    expected.sort();
    assert_eq!(writes, expected, "both are live, at other times");
    let [fact] = &view.folded().file(song)[..] else {
        panic!("{:?}", view.folded().file(song));
    };
    assert_eq!(fact.entries.len(), 2);
    assert!(view.conflicts().is_empty(), "{:?}", view.conflicts());
    let file = view.entity(song).unwrap().file().unwrap();
    assert_eq!(file.state, FileState::InSync);

    move_outside(&folder, "set/song.npno", "song.npno");
    a.rescan().unwrap();
    a.commit("Tag", |i| i.add(song, TAGS, tag("again")))
        .unwrap();
    let view = a.view();
    assert_eq!(
        file_writes(&view, song),
        [(a_writer, path("song.npno"))],
        "the pin replaces both"
    );
}

fn another_writers_open_record_hides_no_row<F: Facade>() {
    let folder = disk();
    let clock = TestClock::at(1_000);
    put(&folder, "a.wav", b"a");
    let (mut a, _) = F::open(Probe::new(&machine(&folder)), env("a", 1, &clock)).unwrap();
    a.rescan().unwrap();
    let loose = row_at(&a.view(), "a.wav").file;
    let probe = Probe::new(&machine(&folder));
    let (mut b, _) = F::open(probe.clone(), env("b", 2, &clock)).unwrap();
    b.rescan().unwrap();
    probe.seen.borrow_mut().refused = Some(path("c.wav"));
    let held = Expect::Holds(identity(b"a"));
    let stopped = b.commit("Rename", |i| {
        i.rename_file(&path("a.wav"), &path("c.wav"), held)
    });
    assert!(stopped.is_err(), "{stopped:?}");
    let refreshed = a.refresh().unwrap();
    assert_eq!(refreshed.orphaned.len(), 1, "b's record is open");
    let row = row_at(&a.view(), "a.wav");
    assert_eq!(row.file, loose);
    assert!(matches!(row.state, RowState::Present { .. }));
}

fn the_listing_holds_directories_and_partial_looks_update_them<F: Facade>() {
    let folder = disk();
    let clock = TestClock::at(1_000);
    put(&folder, "d/e/a.wav", b"a");
    let made = |at: &str| {
        let io = Io::MakeDir {
            root: Root::Folder,
            path: path(at),
        };
        folder.perform(io).unwrap();
    };
    made("empty");
    let (mut a, _) = F::open(Probe::new(&machine(&folder)), env("a", 1, &clock)).unwrap();
    let dirs = |a: &F| -> Vec<String> { a.view().dirs().map(|dir| dir.to_string()).collect() };
    assert_eq!(
        dirs(&a),
        Vec::<String>::new(),
        "nothing until a scan of every file"
    );
    a.rescan().unwrap();
    assert_eq!(dirs(&a), ["d", "d/e", "empty"]);
    let before = a.view();
    a.commit("New folder", |i| i.make_dir(&path("made")))
        .unwrap();
    a.commit("Remove", |i| i.remove_dir(&path("empty")))
        .unwrap();
    assert_eq!(dirs(&a), ["d", "d/e", "made"], "its own commits");
    let changes = a.view().changes_since(&before).dirs;
    let changed: Vec<(&str, DirWhat)> = changes.iter().map(|c| (c.path.as_str(), c.what)).collect();
    assert_eq!(
        changed,
        [("empty", DirWhat::Removed), ("made", DirWhat::Made)]
    );
    put(&folder, "new/x/b.wav", b"b");
    a.rescan_paths(vec![path("new/x/b.wav")]).unwrap();
    assert_eq!(
        dirs(&a),
        ["d", "d/e", "made", "new", "new/x"],
        "and what holds a path it lists"
    );
    let (mut b, _) = F::open(Probe::new(&machine(&folder)), env("b", 2, &clock)).unwrap();
    b.commit("New folder", |i| i.make_dir(&path("theirs")))
        .unwrap();
    a.refresh().unwrap();
    assert!(dirs(&a).contains(&"theirs".to_owned()), "another writer's");
}

fn changes_name_who_changed_an_entitys_row_and_no_one_for_an_untagged_row<F: Facade>() {
    let folder = disk();
    let clock = TestClock::at(1_000);
    put(&folder, "loose.wav", b"loose");
    let (mut a, _) = F::open(Probe::new(&machine(&folder)), env("a", 1, &clock)).unwrap();
    let song = create(&mut a, "song.npno", b"song");
    a.rescan().unwrap();
    let (mut b, _) = F::open(Probe::new(&machine(&folder)), env("b", 2, &clock)).unwrap();
    b.rescan().unwrap();
    let before = a.view();
    let (tagged, loose) = (
        row_of(&before, song).file,
        row_at(&before, "loose.wav").file,
    );
    let held = Expect::Holds(identity(b"song"));
    b.commit("Rename", |i| i.rename(song, &path("set/song.npno"), held))
        .unwrap();
    let held = Expect::Holds(identity(b"loose"));
    b.commit("Rename", |i| {
        i.rename_file(&path("loose.wav"), &path("set/loose.wav"), held)
    })
    .unwrap();
    a.refresh().unwrap();
    let view = a.view();
    let changes = view.changes_since(&before);
    let b_writer = own_writer(&b.view()).unwrap();
    let index = view
        .writer_table()
        .iter()
        .position(|w| *w == b_writer)
        .unwrap();
    let moved = RowWhat::Moved {
        from: path("song.npno"),
    };
    assert_eq!(
        changes_of(&changes, tagged),
        [(moved, Some(Who::Writer(index)), Some(How::Moved))]
    );
    assert_eq!(changes_of(&changes, loose), [(RowWhat::Gone, None, None)]);
    let renamed = row_at(&view, "set/loose.wav").file;
    assert_eq!(
        changes_of(&changes, renamed),
        [(RowWhat::Appeared, None, None)]
    );

    let before = a.view();
    let held = Expect::Holds(identity(b"song"));
    a.commit("Rename", |i| i.rename(song, &path("song.npno"), held))
        .unwrap();
    let back = RowWhat::Moved {
        from: path("set/song.npno"),
    };
    assert_eq!(
        changes_of(&a.view().changes_since(&before), tagged),
        [(back, Some(Who::This), Some(How::Moved))]
    );
    assert_eq!(a.view().writer_table()[index], b_writer, "the index stays");
}

fn a_row_another_writer_deleted_names_that_writer_though_this_one_made_it<F: Facade>() {
    let folder = disk();
    let clock = TestClock::at(1_000);
    let (mut a, _) = F::open(Probe::new(&machine(&folder)), env("a", 1, &clock)).unwrap();
    let song = create(&mut a, "song.npno", b"song");
    a.rescan().unwrap();
    let (mut b, _) = F::open(Probe::new(&machine(&folder)), env("b", 2, &clock)).unwrap();
    b.rescan().unwrap();
    let before = a.view();
    let tagged = row_of(&before, song).file;
    let held = Expect::Holds(identity(b"song"));
    b.commit("Delete", |i| i.trash(song, held).delete(song))
        .unwrap();
    a.refresh().unwrap();
    let view = a.view();
    let b_writer = own_writer(&b.view()).unwrap();
    let index = view.writer_table().iter().position(|w| *w == b_writer);
    assert_eq!(
        changes_of(&view.changes_since(&before), tagged),
        [(RowWhat::Gone, index.map(Who::Writer), Some(How::Trashed))]
    );
}

fn a_row_taken_for_a_file_another_writer_trashed_elsewhere_is_not_said_trashed<F: Facade>() {
    let (theirs, ours) = (disk(), disk());
    let clock = TestClock::at(1_000);
    let deliver_logs = || {
        for (path, bytes) in theirs.files(Root::Folder) {
            if path.as_str().starts_with('.') {
                let _ = ours.perform(Io::Remove {
                    root: Root::Folder,
                    path: path.clone(),
                });
                put(&ours, path.as_str(), &bytes);
            }
        }
    };
    put(&ours, "song.npno", b"song");
    let (mut b, _) = F::open(Probe::new(&machine(&theirs)), env("b", 2, &clock)).unwrap();
    let copy = create(&mut b, "copy.npno", b"song");
    // Sync brings b's log, not its file: a takes its own file of those bytes for it.
    deliver_logs();
    let (mut a, _) = F::open(Probe::new(&machine(&ours)), env("a", 1, &clock)).unwrap();
    a.rescan().unwrap();
    let before = a.view();
    let taken = row_of(&before, copy);
    assert_eq!(
        taken.path,
        path("song.npno"),
        "a takes song.npno for b's file"
    );
    let held = Expect::Holds(identity(b"song"));
    b.commit("Delete", |i| i.trash(copy, held).delete(copy))
        .unwrap();
    deliver_logs();
    a.refresh().unwrap();
    let changes = a.view().changes_since(&before);
    let changed = changes_of(&changes, taken.file);
    assert!(!changed.is_empty(), "the row changed");
    assert!(
        changed.iter().all(|(_, _, how)| *how != Some(How::Trashed)),
        "b trashed copy.npno, not song.npno: {changed:?}"
    );
}

fn a_row_whose_entity_another_writer_deleted_and_whose_file_it_moved_is_not_said_trashed<
    F: Facade,
>() {
    let folder = disk();
    let clock = TestClock::at(1_000);
    let (mut a, _) = F::open(Probe::new(&machine(&folder)), env("a", 1, &clock)).unwrap();
    let song = create(&mut a, "song.npno", b"song");
    a.rescan().unwrap();
    let (mut b, _) = F::open(Probe::new(&machine(&folder)), env("b", 2, &clock)).unwrap();
    b.rescan().unwrap();
    let before = a.view();
    let tagged = row_of(&before, song).file;
    // An undo of tagging a file deletes its entity and leaves the file.
    b.commit("Untag", |i| i.delete(song)).unwrap();
    let held = Expect::Holds(identity(b"song"));
    b.commit("Rename", |i| {
        i.rename_file(&path("song.npno"), &path("tune.npno"), held)
    })
    .unwrap();
    a.refresh().unwrap();
    let changes = a.view().changes_since(&before);
    let changed = changes_of(&changes, tagged);
    assert!(!changed.is_empty(), "the row changed");
    assert!(
        changed.iter().all(|(_, _, how)| *how != Some(How::Trashed)),
        "an entity deleted with its file kept is not trashed: {changed:?}"
    );
}

fn changes_since_an_earlier_view_reach_across_a_look_that_failed<F: Facade>() {
    let folder = disk();
    let clock = TestClock::at(1_000);
    let probe = Probe::new(&machine(&folder));
    let (mut a, _) = F::open(probe.clone(), env("a", 1, &clock)).unwrap();
    let song = create(&mut a, "song.npno", b"song");
    put(&folder, "copy.npno", b"song");
    a.rescan().unwrap();
    let shown = a.view();
    let (tagged, copy) = (row_of(&shown, song).file, row_at(&shown, "copy.npno").file);
    remove_outside(&folder, "song.npno");
    let pending = |io: &Io| matches!(io, Io::List { dir, .. } if dir.name() == Some("pending"));
    fail(&probe, pending);
    assert!(a.rescan().is_err(), "reading the records fails");
    heal(&probe);
    a.rescan().unwrap();
    put(&folder, "later.wav", b"later");
    a.rescan().unwrap();
    let view = a.view();
    let changes = view.changes_since(&shown);
    let whats = |file| {
        changes_of(&changes, file)
            .into_iter()
            .map(|(what, ..)| what)
    };
    assert_eq!(
        whats(copy).collect::<Vec<_>>(),
        [RowWhat::Merged { into: tagged }]
    );
    let moved = RowWhat::Moved {
        from: path("song.npno"),
    };
    assert_eq!(whats(tagged).collect::<Vec<_>>(), [moved]);
    let later = row_at(&view, "later.wav").file;
    assert_eq!(whats(later).collect::<Vec<_>>(), [RowWhat::Appeared]);
}

fn a_full_walk_lists_again_the_folder_of_a_file_it_missed_before_ending_its_row<F: Facade>() {
    let folder = disk();
    let clock = TestClock::at(1_000);
    let probe = Probe::new(&machine(&folder));
    put(&folder, "d/a.wav", b"a");
    put(&folder, "d/b.wav", b"b");
    let (mut a, _) = F::open(probe.clone(), env("a", 1, &clock)).unwrap();
    a.rescan().unwrap();
    let loose = row_at(&a.view(), "d/a.wav").file;
    let listed = |probe: &Probe| probe.seen.borrow().list_stats.get(&path("d")).copied();
    let before = listed(&probe);
    probe.seen.borrow_mut().hidden = [path("d/a.wav")].into();
    a.rescan().unwrap();
    assert_eq!(
        row_at(&a.view(), "d/a.wav").file,
        loose,
        "listed again, it is there"
    );
    let twice = before.map(|n| n + 2);
    assert_eq!(listed(&probe), twice, "d once more, and only d");
    remove_outside(&folder, "d/a.wav");
    a.rescan().unwrap();
    assert_eq!(a.view().row(loose), None, "gone on both looks");
}

fn a_full_walk_listing_a_folder_again_keeps_what_it_listed_first<F: Facade>() {
    let folder = disk();
    let clock = TestClock::at(1_000);
    let probe = Probe::new(&machine(&folder));
    put(&folder, "d/a.wav", b"a");
    put(&folder, "d/b.wav", b"b");
    let (mut a, _) = F::open(probe.clone(), env("a", 1, &clock)).unwrap();
    a.rescan().unwrap();
    let (first, second) = (
        row_at(&a.view(), "d/a.wav").file,
        row_at(&a.view(), "d/b.wav").file,
    );
    probe.seen.borrow_mut().hidden = [path("d/a.wav"), path("d/b.wav")].into();
    a.rescan().unwrap();
    let view = a.view();
    assert_eq!(row_at(&view, "d/a.wav").file, first, "found listed again");
    assert_eq!(
        row_at(&view, "d/b.wav").file,
        second,
        "hidden only while listed again"
    );
}

/// Each Present row's file as the folder holds it, so a reader that just scanned
/// every file shows only what its folder holds.
fn assert_present_rows_are_held(view: &View, folder: &MemDisk, case: &str) {
    let files = library_files(folder);
    for row in view.rows() {
        if let RowState::Present { len, .. } = row.state {
            let held = files.get(&row.path).map(|bytes| bytes.len() as u64);
            assert_eq!(held, Some(len), "{case}: {row:?}");
        }
    }
}

/// A reader whose folder sync delivers piece by piece shows as Present, as of each
/// scan of every file, only the files its folder holds there; once sync settles,
/// each tagged row continues wherever the binding finds its file.
#[test]
fn rows_show_what_the_folder_holds_and_tagged_rows_follow_the_binding_once_sync_settles() {
    let names = ["a", "b", "c", "d/e", "d/f", "g/h"];
    for seed in 0..16 {
        let mut chance = SeededRandom::new(seed);
        let mut pick = move |n: usize| (chance.next_u128() % n as u128) as usize;
        let mut sim = Simulator::new(1);
        let clock = TestClock::at(1_000);
        let origin = sim.machine();
        let (mut writer, _) =
            Blocking::open(Probe::new(&origin), env("w", seed * 2 + 1, &clock)).unwrap();
        let here = Machine {
            folder: sim.reader(0).clone(),
            local: MemDisk::new(),
        };
        let (mut reader, _) =
            Blocking::open(Probe::new(&here), env("r", seed * 2 + 2, &clock)).unwrap();
        for step in 0..80 {
            clock.advance(1);
            let case = format!("seed {seed}, step {step}");
            let view = writer.view();
            let rows: Vec<Row> = view.rows().cloned().collect();
            let present = rows
                .iter()
                .filter(|row| matches!(row.state, RowState::Present { .. }));
            let (tagged, loose): (Vec<&Row>, Vec<&Row>) =
                present.partition(|row| row.entity.is_some());
            let named = |kind: &str, pick: &mut dyn FnMut(usize) -> usize| {
                path(&format!("{}{step}.{kind}", names[pick(names.len())]))
            };
            let _ = match pick(9) {
                0 => {
                    let at = named("npno", &mut pick);
                    writer
                        .commit("Import", |i| {
                            let made = i.create(|e| {
                                e.save(&at, at.to_string().into_bytes(), Expect::Absent)
                                    .add(TAGS, tag("t"));
                            });
                            made.0
                        })
                        .map(drop)
                }
                1 => {
                    let at = named("wav", &mut pick);
                    put(sim.origin(), at.as_str(), at.as_str().as_bytes());
                    writer.rescan().map(drop)
                }
                2 if !tagged.is_empty() => {
                    let row = tagged[pick(tagged.len())];
                    let to = named("npno", &mut pick);
                    let entity = row.entity.unwrap();
                    writer
                        .commit("Rename", |i| i.rename(entity, &to, Expect::File))
                        .map(drop)
                }
                3 if !loose.is_empty() => {
                    let row = loose[pick(loose.len())];
                    let to = named("wav", &mut pick);
                    writer
                        .commit("Rename", |i| i.rename_file(&row.path, &to, Expect::File))
                        .map(drop)
                }
                4 => {
                    let (from, to) = (path(["d", "g"][pick(2)]), path(&format!("k{step}")));
                    writer.commit("Move", |i| i.move_tree(&from, &to)).map(drop)
                }
                5..=7 => {
                    let offered = sim.events(0);
                    if !offered.is_empty() {
                        let event = offered[pick(offered.len())].clone();
                        sim.apply(0, &event);
                    }
                    Ok(())
                }
                _ => {
                    reader.rescan().unwrap();
                    assert_present_rows_are_held(&reader.view(), sim.reader(0), &case);
                    Ok(())
                }
            };
        }
        reader.rescan().unwrap();
        let before = reader.view();
        sim.settle(0);
        reader.rescan().unwrap();
        let after = reader.view();
        let case = format!("seed {seed}, settled");
        assert_present_rows_are_held(&after, sim.reader(0), &case);
        for row in before.rows() {
            let Some(entity) = row.entity else {
                continue;
            };
            if let Some(now) = after.row_of(entity) {
                assert_eq!(now.file, row.file, "{case}: {row:?} then {now:?}");
            }
        }
    }
}
