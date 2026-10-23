//! Reading writers' logs from the folder, and deriving what they show.
//!
//! A reader takes every file directly in `writers.2/<w>/`, whatever its name,
//! and reads it by its contents as segment lines or as a snapshot. From the files
//! it sees and what it remembers of each writer, [`crate::derive`] places each
//! entry, finds forks, and merges. What the reader remembers is kept in the
//! cached view. Reading writes nothing in the folder.
//!
//! Each line is parsed and decoded once: the reader keeps what it read of each
//! file, reads only the bytes a segment gained since, and shares each decoded
//! entry between the cached view and what the folder holds.

use std::cell::RefCell;
use std::collections::{BTreeMap, BTreeSet};
use std::rc::Rc;

use crate::cache::{self, Changed, LetGo, Store};
use crate::derive::{self, Derived, Memory, Seen, SeenSegment};
use crate::error::{Error, Result};
use crate::flow::{self, Flow};
use crate::ids::{EntryHash, Hlc, WriterId};
use crate::io::{Kind, Meta, Range, Root, Task};
use crate::layout::{is_swap_file, Layout};
use crate::line::{self, Stop};
use crate::log::{Entry, Own};
use crate::pack::pack_struct;
use crate::path::RelPath;
use crate::report::{Fork, Gap};
use crate::snapshot::Snapshot;

/// The most of one file a reader holds. A longer segment is read up to here; a
/// longer snapshot is not read.
pub const MAX_FILE: u64 = 1 << 28;

/// About how many bytes of lines one slice of parsing reads before it pauses.
const PARSE_SLICE: usize = 1 << 17;

/// What one file in a writer's directory holds, judged by its contents.
#[derive(Clone, PartialEq, Debug)]
pub enum WriterFile {
    /// An empty file is an empty segment.
    Segment(Segment),
    Snapshot(Rc<Snapshot>),
    /// Neither: reported, and read again next time.
    Unreadable,
}

/// The readable lines of a segment file, decoded.
#[derive(Clone, PartialEq, Debug, Default)]
pub struct Segment {
    pub entries: Vec<Rc<Entry>>,
    /// The offset just past the last line.
    pub end: u64,
    /// Why the readable lines end before the file does.
    pub stop: Option<Stop>,
    /// Whether the file ends with the seal marker after its last line.
    pub sealed: bool,
}

impl Segment {
    /// Reads `bytes`, which follow the readable lines in the file; returns the
    /// entries they add.
    pub fn extend(&mut self, bytes: &[u8]) -> &[Rc<Entry>] {
        let last = self.entries.last().map(|entry| entry.hash());
        let read = line::read_with(bytes, self.end, last, Entry::parse, Entry::hash);
        let before = self.entries.len();
        self.entries.extend(read.lines.into_iter().map(Rc::new));
        self.end = read.end;
        self.stop = read.stop;
        self.sealed = read.sealed;
        &self.entries[before..]
    }

    /// Where reading may continue once the file grows: just past the last line,
    /// unless the file is sealed.
    fn resume(&self) -> Option<Resume> {
        let last = self.entries.last().filter(|_| !self.sealed)?;
        Some(Resume {
            end: self.end,
            ending: line::ending(last.hash()),
        })
    }

    /// Whether each line follows the one before it, as the lines one process
    /// appends do.
    pub fn contiguous(&self) -> bool {
        self.entries
            .windows(2)
            .all(|pair| pair[1].prev() == pair[0].hash())
    }
}

/// A file parsed as [`WriterFile::parse`] parses it, a slice of its lines at a
/// time.
struct Parsing {
    bytes: Vec<u8>,
    segment: Segment,
}

impl Parsing {
    fn new(bytes: Vec<u8>) -> Self {
        Self {
            bytes,
            segment: Segment::default(),
        }
    }

    /// The bytes read so far.
    fn read(&self) -> usize {
        self.segment.end as usize
    }

    /// Reads the lines in about the next `slice` bytes, up to the end of a line;
    /// true once there is nothing left to read.
    fn step(&mut self, slice: usize) -> bool {
        let Self { bytes, segment } = self;
        let rest = &bytes[segment.end as usize..];
        let after = rest.get(slice..).unwrap_or_default();
        let cut = after
            .iter()
            .position(|&b| b == b'\n')
            .map_or(rest.len(), |at| slice + at + 1);
        segment.extend(&rest[..cut]);
        if cut == rest.len() {
            return true;
        }
        if segment.stop.is_some() || segment.sealed {
            // The line reading stopped at is judged again with all that follows it.
            segment.extend(&bytes[segment.end as usize..]);
            return true;
        }
        false
    }

    /// ⚠️ Before [`Parsing::step`] returns true, what the lines read so far say.
    fn finish(self) -> WriterFile {
        let Self { bytes, segment } = self;
        if !segment.entries.is_empty() || segment.stop.is_none() {
            return WriterFile::Segment(segment);
        }
        match Snapshot::decode(&bytes) {
            Ok(snapshot) => WriterFile::Snapshot(Rc::new(snapshot)),
            Err(_) => WriterFile::Unreadable,
        }
    }
}

impl WriterFile {
    pub fn parse(bytes: &[u8]) -> Self {
        let mut segment = Segment::default();
        segment.extend(bytes);
        if !segment.entries.is_empty() || segment.stop.is_none() {
            return Self::Segment(segment);
        }
        match Snapshot::decode(bytes) {
            Ok(snapshot) => Self::Snapshot(Rc::new(snapshot)),
            Err(_) => Self::Unreadable,
        }
    }
}

/// When the reader takes the grace step: once it has held something no file
/// it sees holds for [`GRACE_MS`] and at least one full rescan since.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub struct Grace {
    /// Whether it took the grace step: from then on an unsealed file shorter
    /// than a certificate no longer waits, and a reference the files contradict
    /// kills only its exact target.
    pub graced: bool,
    /// When it first held something no file it sees holds, since it last held
    /// nothing of the kind.
    pub since: Option<u64>,
    /// Whether a scan of every file finished since then.
    pub rescanned: bool,
}

pack_struct!(Grace {
    graced,
    since,
    rescanned
});

/// How long a reader holds what no file it sees holds before the grace step:
/// 24 hours.
pub const GRACE_MS: u64 = 24 * 60 * 60 * 1000;

/// What this install remembers of each writer, kept in the local root.
#[derive(Clone, PartialEq, Debug, Default)]
pub struct CachedView {
    pub(crate) writers: BTreeMap<WriterId, Memory>,
    pub(crate) grace: Grace,
}

impl CachedView {
    /// Refuses what this build cannot read with [`crate::Error::Corrupt`] naming
    /// `path`; the caller then reads from scratch.
    pub fn decode(path: &RelPath, bytes: &[u8]) -> Result<Self> {
        let mut view = Self::default();
        cache::restore(&mut view, bytes).map_err(|reason| Error::Corrupt {
            path: path.clone(),
            reason,
        })?;
        Ok(view)
    }

    pub fn encode(&self) -> Vec<u8> {
        cache::encode_view(self)
    }

    /// What is remembered of each writer.
    pub fn writers(&self) -> &BTreeMap<WriterId, Memory> {
        &self.writers
    }

    pub fn grace(&self) -> Grace {
        self.grace
    }

    /// The cached view kept for the writer whose genesis entry is `genesis`; empty
    /// when none was saved.
    pub fn load(genesis: EntryHash) -> Task<'static, Result<Self>> {
        cache::load_one(genesis).map_ok(|kept| kept.view).task()
    }

    /// Adds everything `other` remembers; whether this view grew.
    pub fn join(&mut self, other: &CachedView) -> bool {
        let mut grew = false;
        for (writer, theirs) in &other.writers {
            let mine = self.writers.entry(*writer).or_default();
            for tip in &theirs.tips {
                if !mine.tips.iter().any(|kept| kept.head == tip.head) {
                    mine.tips.push(Rc::clone(tip));
                    grew = true;
                }
            }
            for (hash, entry) in &theirs.entries {
                grew |= mine.entries.insert(*hash, Rc::clone(entry)).is_none();
            }
            for fork in &theirs.forks {
                grew |= mine.forks.insert(*fork);
            }
        }
        let grace = Grace {
            graced: self.grace.graced || other.grace.graced,
            since: match (self.grace.since, other.grace.since) {
                (Some(a), Some(b)) => Some(a.min(b)),
                (a, b) => a.or(b),
            },
            rescanned: self.grace.rescanned && other.grace.rescanned,
        };
        grew |= grace != self.grace;
        self.grace = grace;
        grew
    }

    /// Keeps this view alone as the first view of the writer whose genesis entry
    /// is `genesis`, replacing what was kept so that a crash leaves the old view
    /// or the new.
    pub fn save(&self, genesis: EntryHash) -> Task<'static, Result<()>> {
        cache::save_view(self, genesis).task()
    }
}

/// What one read found.
#[derive(Clone, PartialEq, Eq, Debug, Default)]
pub struct ReadReport {
    /// Whether what the reader remembers changed.
    pub changed: bool,
    /// Whether any file in a writer's directory came, went or changed.
    pub folder_changed: bool,
    /// Entries newly merged, per writer, in chain order.
    pub placed: BTreeMap<WriterId, Vec<EntryHash>>,
    /// Fork points found by this read and not reported before.
    pub forks: Vec<Fork>,
    /// Every gap as of this read.
    pub gaps: Vec<Gap>,
    /// Files whose readable part ended early, or that are neither segment nor
    /// snapshot.
    pub unreadable: Vec<(RelPath, Option<Stop>)>,
}

impl ReadReport {
    /// Whether the read found anything new: the reader's memory changed, or a
    /// file in a writer's directory came, went or changed.
    pub fn anything_new(&self) -> bool {
        self.changed || self.folder_changed
    }
}

/// Clones share where they save the cached view.
#[derive(Clone)]
pub struct Reader {
    layout: Layout,
    cached: CachedView,
    /// What each writer's directory in the folder held at the last read.
    folder: BTreeMap<WriterId, Folder>,
    /// What the files and the cached view derive.
    derived: Rc<Derived>,
    /// What the cached view gained since it was last saved.
    pub(crate) unsaved: Unsaved,
    /// The retired writers of this install whose cached views this one holds.
    pub(crate) absorbed: BTreeSet<EntryHash>,
    /// The retired writers whose views this reader read, with the generation of
    /// each one's checkpoint.
    pub(crate) retired: BTreeMap<EntryHash, u64>,
    pub(crate) store: Rc<RefCell<Store>>,
}

/// One writer's directory as the last read found it.
#[derive(Clone, Default)]
pub(crate) struct Folder {
    pub(crate) files: BTreeMap<RelPath, Record>,
}

/// What the last read found in one file.
#[derive(Clone)]
pub(crate) struct Record {
    /// `None` when the file is read again next time.
    pub(crate) stamp: Option<Stamp>,
    pub(crate) file: WriterFile,
}

/// What the cached view and the files gained since the view was last saved.
#[derive(Clone, Default)]
pub(crate) struct Unsaved {
    pub(crate) memories: BTreeMap<WriterId, Changed>,
    /// Files whose records came, went or changed.
    pub(crate) files: BTreeSet<RelPath>,
    pub(crate) grace: bool,
    /// What the grace step let go of in each writer's memory.
    pub(crate) let_go: BTreeMap<WriterId, LetGo>,
}

impl Unsaved {
    pub(crate) fn is_empty(&self) -> bool {
        self.memories.is_empty() && self.files.is_empty() && !self.grace
    }
}

/// A file is unchanged while its length, its modification time and its last
/// bytes are: for a segment, the ending that names its last line.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Stamp {
    pub len: u64,
    pub modified: u64,
    pub tail: Vec<u8>,
}

/// What a scan knows of a file from the last read.
#[derive(Clone)]
struct Known {
    stamp: Stamp,
    resume: Option<Resume>,
    /// Whether nothing writes the file again: a sealed segment or a snapshot.
    fixed: bool,
}

/// Where a segment's readable lines end, and the bytes that end them.
#[derive(Clone)]
struct Resume {
    end: u64,
    ending: Vec<u8>,
}

enum Found {
    Unchanged,
    Read {
        file: WriterFile,
        stamp: Option<Stamp>,
    },
    /// The bytes a segment gained past its readable lines.
    Grown {
        bytes: Vec<u8>,
        stamp: Stamp,
    },
}

/// What the reads of one file found, before its bytes are parsed.
enum Fetched {
    Unchanged,
    Whole {
        bytes: Vec<u8>,
        stamp: Option<Stamp>,
    },
    Grown {
        bytes: Vec<u8>,
        stamp: Stamp,
    },
}

struct Scanned {
    path: RelPath,
    found: Found,
}

struct Listed {
    writer: WriterId,
    files: Vec<Scanned>,
}

/// What a reader read of the folder, before it places it.
pub struct Listing {
    writers: Vec<Listed>,
    everyone: bool,
}

/// A read being taken in by [`Reader::absorb_slice`].
pub(crate) struct Absorbing {
    listed: std::vec::IntoIter<Listed>,
    report: ReadReport,
}

impl Reader {
    pub fn new(layout: Layout, cached: CachedView) -> Self {
        let mut reader = Self::underived(layout, cached);
        reader.derived = Rc::new(reader.derive_now());
        reader
    }

    fn underived(layout: Layout, cached: CachedView) -> Self {
        Self {
            layout,
            cached,
            folder: BTreeMap::new(),
            derived: Rc::default(),
            unsaved: Unsaved::default(),
            absorbed: BTreeSet::new(),
            retired: BTreeMap::new(),
            store: Rc::default(),
        }
    }

    /// A reader starting from what [`cache::load`] found in the local root. The
    /// files the saved records describe are read again only once they change.
    pub fn open(layout: Layout, loaded: cache::Loaded) -> Self {
        let cache::Loaded {
            view,
            files,
            absorbed,
            retired,
            store,
        } = loaded;
        let mut reader = Self::underived(layout, view);
        reader.absorbed = absorbed;
        reader.retired = retired;
        reader.store = Rc::new(RefCell::new(store));
        for (path, record) in files {
            let Some(writer) = reader.writer_of(&path) else {
                continue;
            };
            let Some(memory) = reader.cached.writers.get(&writer) else {
                continue;
            };
            let Some(record) = record.resolve(memory) else {
                continue;
            };
            let folder = reader.folder.entry(writer).or_default();
            folder.files.insert(path, record);
        }
        reader.derived = Rc::new(reader.derive_now());
        reader
    }

    /// Lets go of what the reader remembers, as a library closing does.
    pub(crate) fn shed_logs(&mut self) {
        drop(std::mem::take(&mut self.cached));
        drop(std::mem::take(&mut self.derived));
    }

    /// Lets go of what the reader found in each file, as a library closing does.
    pub(crate) fn shed_files(&mut self) {
        drop(std::mem::take(&mut self.folder));
        drop(std::mem::take(&mut self.unsaved));
    }

    /// The writer whose directory holds `path`.
    pub(crate) fn writer_of(&self, path: &RelPath) -> Option<WriterId> {
        let dir = path.parent()?;
        let writer = dir.name()?.parse().ok()?;
        (dir == self.layout.writer(writer)).then_some(writer)
    }

    pub fn cached(&self) -> &CachedView {
        &self.cached
    }

    /// What the files and the cached view derive, as of the last read.
    pub fn derived(&self) -> &Derived {
        &self.derived
    }

    /// Every writer the reader knows of: one the files or its memory hold.
    pub fn writers_known(&self) -> BTreeSet<WriterId> {
        self.derived.writers.keys().copied().collect()
    }

    /// Each file of `writer`'s directory as the last read found it, with its
    /// stamp when it has one.
    pub fn files(&self, writer: WriterId) -> Vec<(&RelPath, Option<&Stamp>, &WriterFile)> {
        let Some(folder) = self.folder.get(&writer) else {
            return Vec::new();
        };
        folder
            .files
            .iter()
            .map(|(path, record)| (path, record.stamp.as_ref(), &record.file))
            .collect()
    }

    pub(crate) fn records(&self) -> impl Iterator<Item = (&RelPath, &Record)> {
        self.folder.values().flat_map(|folder| folder.files.iter())
    }

    pub(crate) fn record(&self, path: &RelPath) -> Option<&Record> {
        let writer = self.writer_of(path)?;
        self.folder.get(&writer)?.files.get(path)
    }

    /// Reads every writer's directory and derives what it shows. Requests only
    /// [`crate::Io::List`], then per writer an [`crate::Io::ListStat`] and
    /// [`crate::Io::ReadMany`] of the files' ends and of the changed files, each
    /// read bounded by [`MAX_FILE`].
    pub fn read(&mut self) -> Task<'_, Result<ReadReport>> {
        flow::run(self.list())
            .map_ok(move |listing| self.absorb(listing))
            .task()
    }

    /// As [`Reader::read`], for one writer's directory.
    pub fn read_writer(&mut self, writer: WriterId) -> Task<'_, Result<ReadReport>> {
        flow::run(self.list_writer(writer))
            .map_ok(move |listing| self.absorb(listing))
            .task()
    }

    /// The reads of [`Reader::read`], for [`Reader::absorb`] to take in.
    pub fn list(&self) -> Task<'static, Result<Listing>> {
        self.scan(self.writers(), true, false)
    }

    /// As [`Reader::list`], taking each sealed segment and snapshot it read
    /// before as it was by its name alone, and asking the length and time only
    /// of the other files; a few, unless the folder is new to it.
    pub fn probe(&self) -> Task<'static, Result<Listing>> {
        self.scan(self.writers(), true, true)
    }

    /// The writers whose directories the folder holds.
    fn writers(&self) -> Flow<'static, Result<Vec<WriterId>>> {
        flow::list(Root::Folder, &self.layout.writers()).map_ok(|listed| {
            listed
                .into_iter()
                .filter(|entry| entry.kind == Kind::Directory)
                .filter_map(|entry| entry.name.parse().ok())
                .collect()
        })
    }

    /// The reads of [`Reader::read_writer`], for [`Reader::absorb`] to take in.
    pub fn list_writer(&self, writer: WriterId) -> Task<'static, Result<Listing>> {
        self.scan(flow::ok(vec![writer]), false, false)
    }

    fn scan(
        &self,
        writers: Flow<'static, Result<Vec<WriterId>>>,
        everyone: bool,
        probe: bool,
    ) -> Task<'static, Result<Listing>> {
        let layout = self.layout.clone();
        let known: Rc<BTreeMap<RelPath, Known>> = Rc::new(
            self.records()
                .filter_map(|(path, record)| {
                    let stamp = record.stamp.clone()?;
                    let (resume, fixed) = match &record.file {
                        WriterFile::Segment(segment) => (segment.resume(), segment.sealed),
                        WriterFile::Snapshot(_) => (None, true),
                        WriterFile::Unreadable => (None, false),
                    };
                    let known = Known {
                        stamp,
                        resume,
                        fixed,
                    };
                    Some((path.clone(), known))
                })
                .collect(),
        );
        writers
            .and_then(move |writers| {
                flow::fold(writers.into_iter(), Vec::new(), move |mut done, writer| {
                    let known = Rc::clone(&known);
                    scan_writer(&layout, writer, known, probe).map_ok(move |listed| {
                        done.push(listed);
                        done
                    })
                })
            })
            .map_ok(move |writers| Listing { writers, everyone })
            .task()
    }

    /// Takes in entries this instance appended as `writer`, as a read would, and
    /// records `written`, the segment the append ended and its stamp, as a read
    /// would find it.
    pub fn add(
        &mut self,
        writer: WriterId,
        entries: &[Entry],
        written: Option<(RelPath, Stamp)>,
    ) -> ReadReport {
        let entries: Vec<Rc<Entry>> = entries.iter().cloned().map(Rc::new).collect();
        let folder = self.folder.entry(writer).or_default();
        let recorded = match written {
            Some((path, stamp)) => {
                let recorded = folder.appended(&path, &entries, stamp);
                if recorded {
                    self.unsaved.files.insert(path);
                }
                recorded
            }
            None => false,
        };
        if !recorded {
            let memory = self.cached.writers.entry(writer).or_default();
            for entry in &entries {
                memory.entries.insert(entry.hash(), Rc::clone(entry));
            }
        }
        let mut report = ReadReport {
            folder_changed: recorded,
            ..ReadReport::default()
        };
        let derived = Rc::make_mut(&mut self.derived);
        match derive::extend(derived, writer, &entries, self.cached.grace.graced) {
            true => {
                let memory = &derived.memory[&writer];
                self.cached.writers.insert(writer, memory.clone());
                let added = entries.iter().map(|entry| entry.hash()).collect();
                let changed = Changed {
                    added,
                    ..Changed::default()
                };
                self.unsaved
                    .memories
                    .entry(writer)
                    .or_default()
                    .then(changed);
                report.changed = true;
                let placed = entries.iter().map(|entry| entry.hash()).collect();
                report.placed.insert(writer, placed);
                report.gaps = self.gaps();
            }
            false => self.rederive(&mut report),
        }
        report
    }

    /// Takes in what [`Reader::list`] or [`Reader::list_writer`] read.
    pub fn absorb(&mut self, listing: Listing) -> ReadReport {
        let mut absorbing = self.absorbing(listing);
        self.absorb_slice(&mut absorbing, usize::MAX);
        self.absorbed(absorbing)
    }

    /// Starts [`Reader::absorb`]: what the read found of writers' directories as a
    /// whole is taken in, and no writer's files yet.
    pub(crate) fn absorbing(&mut self, listing: Listing) -> Absorbing {
        let Listing {
            writers: listed,
            everyone,
        } = listing;
        let mut report = ReadReport::default();
        if everyone {
            let present: BTreeSet<WriterId> = listed.iter().map(|listed| listed.writer).collect();
            let gone: Vec<WriterId> = self
                .folder
                .keys()
                .filter(|writer| !present.contains(writer))
                .copied()
                .collect();
            for writer in gone {
                let folder = self.folder.remove(&writer).expect("listed above");
                self.unsaved.files.extend(folder.files.into_keys());
                report.folder_changed = true;
            }
        }
        Absorbing {
            listed: listed.into_iter(),
            report,
        }
    }

    /// Takes in what about `slice` more files gave; true once every writer's are
    /// taken in.
    pub(crate) fn absorb_slice(&mut self, absorbing: &mut Absorbing, slice: usize) -> bool {
        let mut budget = slice;
        while budget > 0 {
            let Some(listed) = absorbing.listed.next() else {
                return true;
            };
            budget = budget.saturating_sub(listed.files.len().max(1));
            self.gather(listed, &mut absorbing.report);
        }
        absorbing.listed.len() == 0
    }

    /// Ends [`Reader::absorb`] once [`Reader::absorb_slice`] returned true: derives
    /// what the files and the cached view show, when a file came, went or
    /// changed.
    pub(crate) fn absorbed(&mut self, mut absorbing: Absorbing) -> ReadReport {
        for writer in self.folder.keys().copied().collect::<Vec<_>>() {
            self.report_unreadable(writer, &mut absorbing.report);
        }
        match absorbing.report.folder_changed {
            true => self.rederive(&mut absorbing.report),
            false => absorbing.report.gaps = self.gaps(),
        }
        self.drop_unsaved_without_store();
        absorbing.report
    }

    /// A view not yet saved anywhere is written whole by its first save, so what
    /// it gains is not kept apart, except a change to the grace step, which an
    /// instance without a writer keeps in the views it read.
    fn drop_unsaved_without_store(&mut self) {
        if self.store.borrow().genesis.is_none() {
            self.unsaved = Unsaved {
                grace: self.unsaved.grace,
                let_go: std::mem::take(&mut self.unsaved.let_go),
                ..Unsaved::default()
            };
        }
    }

    /// What each writer's files show.
    fn seen(&self) -> BTreeMap<WriterId, Seen<'_>> {
        let mut seen = BTreeMap::new();
        for (writer, folder) in &self.folder {
            let mut files = Seen::default();
            for record in folder.files.values() {
                match &record.file {
                    WriterFile::Segment(segment) => files.segments.push(SeenSegment {
                        entries: &segment.entries,
                        sealed: segment.sealed,
                    }),
                    WriterFile::Snapshot(snapshot) if snapshot.writer == *writer => {
                        files.snapshots.push(Rc::clone(snapshot));
                    }
                    WriterFile::Snapshot(_) | WriterFile::Unreadable => {}
                }
            }
            seen.insert(*writer, files);
        }
        seen
    }

    fn derive_now(&self) -> Derived {
        derive::derive(&self.seen(), &self.cached.writers, self.cached.grace.graced)
    }

    /// What the files and the cached view derive now, from scratch, as tests
    /// compare with what the reader holds.
    #[doc(hidden)]
    pub fn derive_afresh(&self) -> Derived {
        self.derive_now()
    }

    /// Derives again from the files and the cached view, keeps what to remember,
    /// and reports what changed.
    fn rederive(&mut self, report: &mut ReadReport) {
        let mut derived = self.derive_now();
        let before = Rc::clone(&self.derived);
        derived.folded.share(&before.folded);
        for (writer, memory) in &derived.memory {
            let was = self.cached.writers.get(writer);
            let changed = Changed::between(was, Some(memory));
            if !changed.is_empty() {
                report.changed = true;
                self.unsaved
                    .memories
                    .entry(*writer)
                    .or_default()
                    .then(changed);
            }
        }
        let gone: Vec<WriterId> = self
            .cached
            .writers
            .keys()
            .filter(|writer| !derived.memory.contains_key(writer))
            .copied()
            .collect();
        for writer in gone {
            let changed = Changed::between(self.cached.writers.get(&writer), None);
            report.changed = true;
            self.unsaved
                .memories
                .entry(writer)
                .or_default()
                .then(changed);
        }
        self.cached.writers = derived
            .memory
            .iter()
            .filter(|(_, memory)| !memory.is_empty())
            .map(|(writer, memory)| (*writer, memory.clone()))
            .collect();
        for (writer, placed) in &derived.writers {
            let was: BTreeSet<EntryHash> = before
                .writers
                .get(writer)
                .map(|placed| placed.merged.iter().map(|entry| entry.hash()).collect())
                .unwrap_or_default();
            let new: Vec<EntryHash> = placed
                .merged
                .iter()
                .map(|entry| entry.hash())
                .filter(|hash| !was.contains(hash))
                .collect();
            if !new.is_empty() {
                report.placed.insert(*writer, new);
            }
            let known: BTreeSet<Own> = before
                .writers
                .get(writer)
                .map(|placed| placed.forks.clone())
                .unwrap_or_default();
            for fork in placed.forks.difference(&known) {
                report.forks.push(Fork {
                    writer: *writer,
                    at: fork.n,
                    prev: fork.hash,
                });
            }
        }
        self.derived = Rc::new(derived);
        report.gaps = self.gaps();
    }

    /// Every gap the reader holds back entries at.
    fn gaps(&self) -> Vec<Gap> {
        let writers = self.derived.writers.iter();
        let gaps = writers.filter_map(|(writer, placed)| {
            Some(Gap {
                writer: *writer,
                missing: placed.gaps.first()?.prev(),
                held: placed.gaps.len(),
            })
        });
        gaps.collect()
    }

    /// Whether the reader holds something no file it sees holds: a tip whose
    /// file is gone, or an entry no file holds.
    pub fn holds_unseen(&self) -> bool {
        let seen = self.seen();
        self.cached.writers.iter().any(|(writer, memory)| {
            let files = seen.get(writer);
            let lines: BTreeSet<EntryHash> = files
                .into_iter()
                .flat_map(|files| files.segments.iter())
                .flat_map(|segment| segment.entries.iter().map(|entry| entry.hash()))
                .collect();
            let heads: BTreeSet<Own> = files
                .into_iter()
                .flat_map(|files| files.snapshots.iter().map(|snapshot| snapshot.head))
                .collect();
            let tip_gone = memory.tips.iter().any(|tip| !heads.contains(&tip.head));
            let entry_gone = memory.entries.keys().any(|hash| !lines.contains(hash));
            tip_gone || entry_gone
        })
    }

    /// Notes, at wall time `now`, whether the reader holds what no file holds,
    /// and that a scan of every file finished when `rescanned`; whether the
    /// grace step is due.
    pub fn note_grace(&mut self, now: u64, rescanned: bool) -> bool {
        let unseen = self.holds_unseen();
        let grace = self.cached.grace;
        let next = match (unseen, grace.since) {
            (false, _) => Grace {
                since: None,
                rescanned: false,
                ..grace
            },
            (true, None) => Grace {
                since: Some(now),
                rescanned: false,
                ..grace
            },
            (true, Some(since)) => Grace {
                since: Some(since),
                rescanned: grace.rescanned || rescanned,
                ..grace
            },
        };
        if next != grace {
            self.cached.grace = next;
            self.unsaved.grace = true;
        }
        next.rescanned
            && next
                .since
                .is_some_and(|since| now.saturating_sub(since) >= GRACE_MS)
    }

    /// Takes the grace step: lets go of what no file holds and nothing visible
    /// descends from, forgets fork points the files no longer show, and derives
    /// again.
    pub fn take_grace(&mut self) -> ReadReport {
        let kept = derive::grace(&self.seen(), &self.cached.writers);
        let mut report = ReadReport::default();
        for writer in self
            .cached
            .writers
            .keys()
            .chain(kept.keys())
            .copied()
            .collect::<BTreeSet<_>>()
        {
            let (before, after) = (self.cached.writers.get(&writer), kept.get(&writer));
            let changed = Changed::between(before, after);
            if !changed.is_empty() {
                let let_go = LetGo::between(before, after);
                self.unsaved.let_go.entry(writer).or_default().then(let_go);
                self.unsaved
                    .memories
                    .entry(writer)
                    .or_default()
                    .then(changed);
                report.changed = true;
            }
        }
        self.cached.writers = kept;
        self.cached.grace = Grace {
            graced: true,
            since: None,
            rescanned: false,
        };
        self.unsaved.grace = true;
        self.rederive(&mut report);
        report
    }

    /// Takes in what one writer's files gave.
    fn gather(&mut self, listed: Listed, report: &mut ReadReport) {
        let Listed { writer, files } = listed;
        report.folder_changed |= !self.folder.contains_key(&writer);
        let folder = self.folder.entry(writer).or_default();
        let present: BTreeSet<&RelPath> = files.iter().map(|file| &file.path).collect();
        let gone: Vec<RelPath> = folder
            .files
            .keys()
            .filter(|path| !present.contains(path))
            .cloned()
            .collect();
        for path in gone {
            folder.files.remove(&path);
            self.unsaved.files.insert(path);
            report.folder_changed = true;
        }
        for Scanned { path, found } in files {
            match found {
                Found::Unchanged => continue,
                Found::Grown { bytes, stamp } => {
                    let record = folder
                        .files
                        .get_mut(&path)
                        .expect("only a known file grows");
                    if let WriterFile::Segment(segment) = &mut record.file {
                        segment.extend(&bytes);
                    }
                    record.stamp = Some(stamp);
                }
                Found::Read { file, stamp } => {
                    folder.files.insert(path.clone(), Record { stamp, file });
                }
            }
            self.unsaved.files.insert(path);
            report.folder_changed = true;
        }
    }

    fn report_unreadable(&self, writer: WriterId, report: &mut ReadReport) {
        let Some(folder) = self.folder.get(&writer) else {
            return;
        };
        for (path, record) in &folder.files {
            match &record.file {
                WriterFile::Segment(segment) => {
                    if let Some(stop) = &segment.stop {
                        report.unreadable.push((path.clone(), Some(stop.clone())));
                    }
                }
                WriterFile::Snapshot(snapshot) if snapshot.writer == writer => {}
                WriterFile::Snapshot(_) | WriterFile::Unreadable => {
                    report.unreadable.push((path.clone(), None));
                }
            }
        }
    }

    /// The latest clock reading among what the reader merges.
    pub fn readings(&self) -> impl Iterator<Item = Hlc> + '_ {
        self.derived.writers.values().flat_map(|placed| {
            let tips = placed.tips.iter().map(|tip| tip.at);
            let entries = placed
                .merged
                .iter()
                .chain(&placed.runs)
                .chain(&placed.gaps)
                .map(|entry| entry.at());
            tips.chain(entries)
        })
    }

    /// Keeps what the cached view gained in this writer's local directory: by
    /// appending it to the journal, or by writing the whole view again when the
    /// journal has grown too long or cannot be trusted.
    pub fn save(&mut self, genesis: EntryHash) -> Task<'static, Result<()>> {
        cache::save(self, genesis)
    }

    /// Writes the whole view, with what the reader knows of each file, as the
    /// writer whose genesis entry is `genesis`.
    pub fn checkpoint(&mut self, genesis: EntryHash) -> Task<'static, Result<()>> {
        cache::checkpoint(self, genesis)
    }
}

impl Folder {
    /// Records that the segment at `path` gained `entries`, ending it as `stamp`
    /// says: when the last read left it just where they begin, or they are all it
    /// holds. Whether it recorded them.
    fn appended(&mut self, path: &RelPath, entries: &[Rc<Entry>], stamp: Stamp) -> bool {
        let added: u64 = entries.iter().map(|e| e.to_bytes().len() as u64).sum();
        let Some(start) = stamp.len.checked_sub(added) else {
            return false;
        };
        let continues = match self.files.get(path) {
            None => start == 0,
            Some(Record {
                stamp: Some(old),
                file: WriterFile::Segment(segment),
            }) => {
                old.len == start
                    && segment.end == start
                    && !segment.sealed
                    && segment.entries.last().map(|last| last.hash())
                        == entries.first().map(|first| first.prev())
            }
            Some(_) => false,
        };
        if !continues {
            return false;
        }
        let record = self.files.entry(path.clone()).or_insert_with(|| Record {
            stamp: None,
            file: WriterFile::Segment(Segment::default()),
        });
        if let WriterFile::Segment(segment) = &mut record.file {
            segment.entries.extend(entries.iter().cloned());
            segment.end = stamp.len;
        }
        record.stamp = Some(stamp);
        true
    }
}

fn scan_writer<'a>(
    layout: &Layout,
    writer: WriterId,
    known: Rc<BTreeMap<RelPath, Known>>,
    probe: bool,
) -> Flow<'a, Result<Listed>> {
    let dir = layout.writer(writer);
    let listed = match probe {
        true => probed(dir.clone(), Rc::clone(&known)),
        false => flow::list_stat(Root::Folder, &dir),
    };
    listed
        .and_then(move |entries| {
            let files: Vec<Planned> = entries
                .into_iter()
                .filter(|(name, meta)| meta.kind == Kind::File && !is_swap_file(name))
                .filter_map(|(name, meta)| {
                    let path = dir.join(&name).ok()?;
                    let plan = Plan::of(&meta, known.get(&path));
                    Some(Planned { path, meta, plan })
                })
                .collect();
            read_planned(files)
        })
        .and_then(|fetched| parsed(fetched).then(flow::ok))
        .map_ok(move |files| Listed { writer, files })
}

/// The most files of a writer's directory, other than the sealed segments and
/// snapshots read before, that a probe asks the length and time of one at a time
/// rather than all at once.
const PROBED: usize = 16;

/// The entries of the writer's directory `dir`, as a listing with their lengths
/// and times gives them, where `known` gives those of sealed segments and
/// snapshots by name.
fn probed<'a>(
    dir: RelPath,
    known: Rc<BTreeMap<RelPath, Known>>,
) -> Flow<'a, Result<Vec<(String, Meta)>>> {
    flow::list(Root::Folder, &dir).and_then(move |entries| {
        let kept = |name: &str| {
            let path = dir.join(name).ok()?;
            let known = known.get(&path).filter(|known| known.fixed)?;
            Some(Meta {
                kind: Kind::File,
                len: known.stamp.len,
                modified: Some(known.stamp.modified),
            })
        };
        let mut listed = Vec::new();
        let mut asked = Vec::new();
        for entry in entries {
            match (entry.kind, kept(&entry.name)) {
                (Kind::Directory, _) => listed.push((entry.name, DIRECTORY)),
                (Kind::File, Some(meta)) => listed.push((entry.name, meta)),
                (Kind::File, None) => asked.push(entry.name),
            }
        }
        if asked.len() > PROBED {
            return flow::list_stat(Root::Folder, &dir);
        }
        flow::fold(asked.into_iter(), listed, move |mut listed, name| {
            let Ok(path) = dir.join(&name) else {
                return flow::ok(listed);
            };
            flow::stat(Root::Folder, &path).map_ok(move |meta| {
                listed.extend(meta.map(|meta| (name, meta)));
                listed
            })
        })
        .map_ok(|mut listed| {
            listed.sort_by(|a, b| a.0.cmp(&b.0));
            listed
        })
    })
}

/// What a listing with lengths and times gives of a directory.
const DIRECTORY: Meta = Meta {
    kind: Kind::Directory,
    len: 0,
    modified: None,
};

/// Files fetched whole and not parsed yet, parsed a slice at a time.
struct Parse {
    fetched: std::vec::IntoIter<(RelPath, Fetched)>,
    parsing: Option<(RelPath, Option<Stamp>, Parsing)>,
    done: Vec<Scanned>,
}

impl Parse {
    /// Parses about [`PARSE_SLICE`] bytes; true once every file is parsed.
    fn slice(&mut self) -> bool {
        let mut budget = PARSE_SLICE;
        while budget > 0 {
            let (path, stamp, mut parsing) = match self.parsing.take() {
                Some(parsing) => parsing,
                None => match self.fetched.next() {
                    None => return true,
                    Some((path, Fetched::Whole { bytes, stamp })) => {
                        (path, stamp, Parsing::new(bytes))
                    }
                    Some((path, Fetched::Unchanged)) => {
                        self.done.push(Scanned {
                            path,
                            found: Found::Unchanged,
                        });
                        continue;
                    }
                    Some((path, Fetched::Grown { bytes, stamp })) => {
                        let found = Found::Grown { bytes, stamp };
                        self.done.push(Scanned { path, found });
                        continue;
                    }
                },
            };
            let before = parsing.read();
            let finished = parsing.step(budget);
            budget = budget.saturating_sub(parsing.read() - before);
            match finished {
                true => self.done.push(Scanned {
                    path,
                    found: Found::Read {
                        file: parsing.finish(),
                        stamp,
                    },
                }),
                false => self.parsing = Some((path, stamp, parsing)),
            }
        }
        self.parsing.is_none() && self.fetched.len() == 0
    }
}

fn parsed<'a>(fetched: Vec<(RelPath, Fetched)>) -> Flow<'a, Vec<Scanned>> {
    let parse = Parse {
        fetched: fetched.into_iter(),
        parsing: None,
        done: Vec::new(),
    };
    flow::sliced(parse, Parse::slice).then(|parse| Flow::Done(parse.done))
}

/// A file a writer's directory listed, and what to read of it first.
struct Planned {
    path: RelPath,
    meta: Meta,
    plan: Plan,
}

enum Plan {
    /// Nothing: a sealed segment or a snapshot with the length and modification
    /// time of the last read.
    Kept,
    /// Its tail, to compare with that of the last read, whose length and
    /// modification time it still has.
    Tail(Vec<u8>),
    /// What a segment gained past the readable lines of the last read, from the
    /// ending of the last of them on.
    Grown {
        resume: Resume,
        modified: u64,
    },
    Whole,
}

impl Plan {
    fn of(meta: &Meta, known: Option<&Known>) -> Self {
        let Some(modified) = meta.modified else {
            return Plan::Whole;
        };
        let same = |stamp: &Stamp| stamp.len == meta.len && stamp.modified == modified;
        match known {
            Some(Known { stamp, fixed, .. }) if same(stamp) => match fixed {
                true => Plan::Kept,
                false => Plan::Tail(stamp.tail.clone()),
            },
            Some(Known {
                stamp,
                resume: Some(resume),
                ..
            }) if meta.len > stamp.len && meta.len <= MAX_FILE => Plan::Grown {
                resume: resume.clone(),
                modified,
            },
            _ => Plan::Whole,
        }
    }

    fn reads(&self, len: u64, modified: Option<u64>) -> Vec<Range> {
        match self {
            Plan::Kept => Vec::new(),
            Plan::Tail(_) => vec![tail_range(len)],
            Plan::Grown { resume, .. } => {
                let offset = resume.end - line::ENDING;
                vec![Range {
                    offset,
                    len: len - offset,
                }]
            }
            Plan::Whole => whole_ranges(len, modified),
        }
    }
}

/// The last [`line::ENDING`] bytes of a file `len` bytes long.
fn tail_range(len: u64) -> Range {
    Range {
        offset: len.saturating_sub(line::ENDING),
        len: len.min(line::ENDING),
    }
}

/// A file from its start, up to [`MAX_FILE`] bytes, and its tail apart when that
/// is past what is read and the file has a modification time to stamp.
fn whole_ranges(len: u64, modified: Option<u64>) -> Vec<Range> {
    let whole = Range {
        offset: 0,
        len: len.min(MAX_FILE),
    };
    match modified.is_some() && len > MAX_FILE {
        true => vec![whole, tail_range(len)],
        false => vec![whole],
    }
}

/// Reads `files` as planned, in one [`crate::Io::ReadMany`], then whole in a
/// second those whose tail or grown part showed the last read no longer holds.
/// A file gone before its read is left out.
fn read_planned<'a>(files: Vec<Planned>) -> Flow<'a, Result<Vec<(RelPath, Fetched)>>> {
    let reads = files.iter().flat_map(|file| {
        let ranges = file.plan.reads(file.meta.len, file.meta.modified);
        ranges.into_iter().map(|range| (file.path.clone(), range))
    });
    flow::read_many(Root::Folder, reads.collect()).and_then(move |read| {
        let mut read = read.into_iter();
        let mut found = Vec::new();
        let mut again = Vec::new();
        for Planned { path, meta, plan } in files {
            let parts: Vec<Option<Vec<u8>>> = read
                .by_ref()
                .take(plan.reads(meta.len, meta.modified).len())
                .collect();
            match planned(plan, &meta, parts) {
                Some(Some(fetched)) => found.push((path, fetched)),
                Some(None) => again.push((path, meta)),
                None => {}
            }
        }
        let reads = again.iter().flat_map(|(path, meta)| {
            let ranges = whole_ranges(meta.len, meta.modified);
            ranges.into_iter().map(|range| (path.clone(), range))
        });
        flow::read_many(Root::Folder, reads.collect()).map_ok(move |read| {
            let mut read = read.into_iter();
            for (path, meta) in again {
                let ranges = whole_ranges(meta.len, meta.modified).len();
                let parts: Vec<Option<Vec<u8>>> = read.by_ref().take(ranges).collect();
                if let Some(whole) = whole(&meta, parts) {
                    found.push((path, whole));
                }
            }
            found
        })
    })
}

/// What `parts`, read as `plan` asked, found: `None` when the file is gone, and
/// `Some(None)` when it must be read whole.
fn planned(plan: Plan, meta: &Meta, parts: Vec<Option<Vec<u8>>>) -> Option<Option<Fetched>> {
    match plan {
        Plan::Kept => Some(Some(Fetched::Unchanged)),
        Plan::Whole => whole(meta, parts).map(Some),
        Plan::Tail(known) => {
            let tail = parts.into_iter().next()??;
            Some((tail == known).then_some(Fetched::Unchanged))
        }
        Plan::Grown { resume, modified } => {
            let bytes = parts.into_iter().next()??;
            let ending = line::ENDING as usize;
            let expected = meta.len - (resume.end - line::ENDING);
            if bytes.len() as u64 != expected || bytes[..ending] != resume.ending[..] {
                return Some(None);
            }
            let stamp = Stamp {
                len: meta.len,
                modified,
                tail: bytes[bytes.len() - ending..].to_vec(),
            };
            Some(Some(Fetched::Grown {
                bytes: bytes[ending..].to_vec(),
                stamp,
            }))
        }
    }
}

/// A file read whole as [`whole_ranges`] asks, stamped when the backend gave a
/// modification time and its tail was read; `None` when the file is gone.
fn whole(meta: &Meta, parts: Vec<Option<Vec<u8>>>) -> Option<Fetched> {
    let mut parts = parts.into_iter();
    let bytes = parts.next()??;
    let len = meta.len;
    let tail = match bytes.len() as u64 == len {
        true => Some(bytes[tail_range(len).offset as usize..].to_vec()),
        false => parts.next().flatten(),
    };
    let stamp = meta.modified.zip(tail).map(|(modified, tail)| Stamp {
        len,
        modified,
        tail,
    });
    Some(Fetched::Whole { bytes, stamp })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::compaction::snapshot as fold;
    use crate::disk::MemDisk;
    use crate::io::Io;
    use crate::log::{EntryKind, Genesis, Logged};

    const W: WriterId = WriterId::from_u128(0xa);

    fn at(n: u64) -> Hlc {
        Hlc {
            wall_ms: n,
            counter: 0,
        }
    }

    fn genesis() -> Entry {
        let kind = EntryKind::Genesis(Genesis {
            writer: W,
            label: "a".into(),
        });
        Entry::encode(EntryHash::ZERO, 0, at(0), kind).unwrap()
    }

    fn after(prev: &Entry, label: &str) -> Entry {
        let kind = EntryKind::Intent(Logged {
            label: label.into(),
            ops: Vec::new(),
            displaced: Vec::new(),
            forks: Vec::new(),
        });
        let n = prev.n() + 1;
        Entry::encode(prev.hash(), n, at(n + label.len() as u64), kind).unwrap()
    }

    /// The genesis entry and `n` entries after it.
    fn chain(n: usize) -> Vec<Entry> {
        let mut entries = vec![genesis()];
        for i in 0..n {
            let next = after(entries.last().unwrap(), &format!("e{i}"));
            entries.push(next);
        }
        entries
    }

    /// The snapshot of `writer` that folds `entries`, a chain from its genesis
    /// entry, naming each by hash.
    fn snapshot_of(writer: WriterId, entries: &[Entry]) -> Rc<Snapshot> {
        let entries: Vec<Rc<Entry>> = entries.iter().cloned().map(Rc::new).collect();
        let genesis = entries[0].hash();
        let folded = fold(writer, genesis, "a".into(), None, &entries, []);
        Rc::new(folded.unwrap())
    }

    fn snapshot(entries: &[Entry]) -> Rc<Snapshot> {
        snapshot_of(W, entries)
    }

    fn linked(entries: &[Entry]) -> Vec<Rc<Entry>> {
        let lines = entries.iter().map(|entry| Entry::linked(entry.line()));
        lines.map(Rc::new).collect()
    }

    fn hashes(entries: &[Entry]) -> Vec<EntryHash> {
        entries.iter().map(Entry::hash).collect()
    }

    fn bytes_of(entries: &[&Entry]) -> Vec<u8> {
        entries.iter().flat_map(|entry| entry.to_bytes()).collect()
    }

    #[test]
    fn a_file_is_judged_by_its_contents() {
        let entries = chain(1);
        let segment = bytes_of(&[&entries[0], &entries[1]]);
        let mut torn = segment.clone();
        torn.extend_from_slice(b"{\"prev\"");
        let snapshot = snapshot(&entries);
        let cases = [
            (Vec::new(), WriterFile::Segment(Segment::default())),
            (
                segment.clone(),
                WriterFile::Segment(Segment {
                    entries: linked(&entries),
                    end: segment.len() as u64,
                    stop: None,
                    sealed: false,
                }),
            ),
            (snapshot.encode(), WriterFile::Snapshot(snapshot)),
            (b"hello".to_vec(), WriterFile::Unreadable),
            (vec![0; 40], WriterFile::Unreadable),
        ];
        for (bytes, expected) in cases {
            assert_eq!(
                WriterFile::parse(&bytes),
                expected,
                "{:?}",
                String::from_utf8_lossy(&bytes)
            );
        }
        let WriterFile::Segment(read) = WriterFile::parse(&torn) else {
            panic!("a torn segment is a segment")
        };
        assert_eq!(read.entries, linked(&entries));
        assert_eq!(
            read.stop.map(|stop| stop.offset),
            Some(segment.len() as u64)
        );
    }

    /// What a reader that read `disk` derives and remembers.
    fn read(disk: &MemDisk) -> Reader {
        let layout = Layout::new(".lib").unwrap();
        let mut reader = Reader::new(layout, CachedView::default());
        crate::blocking::run(&mut disk.clone(), reader.read()).unwrap();
        reader
    }

    #[test]
    fn damaged_files_are_read_without_panicking() {
        use crate::env::{Random, SeededRandom};
        let entries = chain(3);
        let disk = MemDisk::new();
        let layout = Layout::new(".lib").unwrap();
        let dir = layout.writer(W);
        put(
            &disk,
            &dir.join("s.txt").unwrap(),
            &bytes_of(&[&entries[2], &entries[3]]),
        );
        put(
            &disk,
            &dir.join("snap.json").unwrap(),
            &snapshot(&entries[..2]).encode(),
        );
        let view = read(&disk).cached().clone();
        let seeds = [
            bytes_of(&entries.iter().collect::<Vec<_>>()),
            snapshot(&entries).encode(),
            view.encode(),
        ];
        let path = Layout::cached_view(entries[0].hash());
        let mut random = SeededRandom::new(7);
        let mut pick = |n: usize| (random.next_u128() % n as u128) as usize;
        for _ in 0..3000 {
            let mut bytes = seeds[pick(seeds.len())].clone();
            for _ in 0..=pick(4) {
                let at = pick(bytes.len().max(1));
                match pick(4) {
                    0 => bytes.truncate(at),
                    1 if at < bytes.len() => bytes[at] ^= 1 << pick(8),
                    2 => bytes.insert(at.min(bytes.len()), b"\t\n\0{}\"0"[pick(7)]),
                    _ => bytes.extend_from_within(at.min(bytes.len())..),
                }
            }
            let mut seen = Seen::default();
            let segments;
            match WriterFile::parse(&bytes) {
                WriterFile::Segment(segment) => {
                    segments = segment;
                    seen.segments.push(SeenSegment {
                        entries: &segments.entries,
                        sealed: segments.sealed,
                    });
                }
                WriterFile::Snapshot(snapshot) => seen.snapshots.push(snapshot),
                WriterFile::Unreadable => {}
            }
            let derived = derive::derive(&BTreeMap::from([(W, seen)]), &BTreeMap::new(), false);
            for placed in derived.writers.values() {
                for entry in placed.merged.iter().skip(1) {
                    assert!(placed.holds(entry.prev()), "merged after its predecessor");
                }
            }
            if let Ok(view) = CachedView::decode(&path, &bytes) {
                assert!(CachedView::decode(&path, &view.encode()).is_ok());
            }
        }
    }

    #[test]
    fn a_file_parsed_in_slices_is_judged_as_parsed_whole() {
        use crate::env::{Random, SeededRandom};
        let entries = chain(6);
        let mut segment = bytes_of(&entries.iter().collect::<Vec<_>>());
        let whole = segment.clone();
        segment.extend(line::seal_marker(entries[6].hash()));
        let seeds = [whole, segment, snapshot(&entries).encode()];
        let mut random = SeededRandom::new(11);
        let mut pick = |n: usize| (random.next_u128() % n as u128) as usize;
        for _ in 0..3000 {
            let mut bytes = seeds[pick(seeds.len())].clone();
            for _ in 0..pick(3) {
                let at = pick(bytes.len().max(1));
                match pick(5) {
                    0 => bytes.truncate(at),
                    1 if at < bytes.len() => bytes[at] ^= 1 << pick(8),
                    2 => bytes.insert(at.min(bytes.len()), b"\t\n\0{}\"0"[pick(7)]),
                    3 => bytes.extend(line::seal_marker(entries[pick(7)].hash())),
                    _ => bytes.extend_from_within(at.min(bytes.len())..),
                }
            }
            let mut parsing = Parsing::new(bytes.clone());
            let slice = 1 + pick(400);
            let mut steps = 0;
            while !parsing.step(slice) {
                steps += 1;
                assert!(steps <= bytes.len(), "parsing stalls");
            }
            assert_eq!(
                parsing.finish(),
                WriterFile::parse(&bytes),
                "{slice}-byte slices of {:?}",
                String::from_utf8_lossy(&bytes)
            );
        }
    }

    fn put(disk: &MemDisk, path: &RelPath, bytes: &[u8]) {
        let dir = path.parent().unwrap();
        disk.perform(Io::MakeDir {
            root: Root::Folder,
            path: dir,
        })
        .unwrap();
        disk.perform(Io::Create {
            root: Root::Folder,
            path: path.clone(),
            bytes: bytes.to_vec(),
        })
        .unwrap();
    }

    fn grow(disk: &MemDisk, path: &RelPath, bytes: &[u8]) {
        let append = Io::Append {
            root: Root::Folder,
            path: path.clone(),
            bytes: bytes.to_vec(),
        };
        disk.perform(append).unwrap();
    }

    /// A backend that counts the bytes of file contents it is asked to read.
    struct Counting(MemDisk, u64);

    impl crate::blocking::Backend for Counting {
        fn capabilities(&self, root: Root) -> crate::io::Capabilities {
            self.0.capabilities(root)
        }

        fn perform(&mut self, io: Io) -> crate::io::IoResult {
            match &io {
                Io::Read { range, .. } => self.1 += range.len,
                Io::ReadMany { reads, .. } => {
                    self.1 += reads.iter().map(|(_, r)| r.len).sum::<u64>()
                }
                _ => {}
            }
            self.0.perform(io)
        }
    }

    #[test]
    fn a_reader_reads_every_file_by_contents_and_writes_nothing() {
        let layout = Layout::new(".lib").unwrap();
        let disk = MemDisk::new();
        let entries = chain(3);
        let dir = layout.writer(W);
        put(
            &disk,
            &dir.join("x (conflicted copy).jsonl").unwrap(),
            &bytes_of(&[&entries[2], &entries[3]]),
        );
        put(
            &disk,
            &dir.join("notes.txt").unwrap(),
            &snapshot(&entries[..2]).encode(),
        );
        put(&disk, &dir.join("junk").unwrap(), b"junk");
        let mut backend = Counting(disk.clone(), 0);
        let mut reader = Reader::new(layout.clone(), CachedView::default());
        let report = crate::blocking::run(&mut backend, reader.read()).unwrap();
        assert_eq!(report.placed[&W], hashes(&entries[2..]));
        assert_eq!(report.unreadable, [(dir.join("junk").unwrap(), None)]);
        assert_eq!(disk.mutations(), 3 * 2, "only the setup wrote");
        let first = backend.1;

        let again = crate::blocking::run(&mut backend, reader.read()).unwrap();
        let ends = line::ENDING + 4;
        assert_eq!(
            backend.1 - first,
            ends,
            "an unchanged segment is read at its end, the junk whole, the snapshot not at all"
        );
        assert_eq!(again.placed, BTreeMap::new());
        assert_eq!(again.unreadable, report.unreadable);
    }

    /// What a reader holds that a read changes, comparable.
    fn state(reader: &Reader) -> impl PartialEq + std::fmt::Debug {
        let files: Vec<_> = reader
            .records()
            .map(|(path, record)| (path.clone(), record.stamp.clone(), record.file.clone()))
            .collect();
        let unsaved: BTreeSet<WriterId> = reader.unsaved.memories.keys().copied().collect();
        let files_unsaved = reader.unsaved.files.clone();
        (reader.cached().clone(), files, unsaved, files_unsaved)
    }

    #[test]
    fn a_read_placed_in_slices_places_what_it_places_at_once() {
        use crate::env::{Random, SeededRandom};
        let layout = Layout::new(".lib").unwrap();
        let mut random = SeededRandom::new(3);
        let mut pick = |n: usize| (random.next_u128() % n.max(1) as u128) as usize;
        for _ in 0..60 {
            let disk = MemDisk::new();
            let mut files: Vec<(RelPath, Vec<u8>)> = Vec::new();
            for w in 1..=1 + pick(3) as u128 {
                let writer = WriterId::from_u128(w);
                let kind = EntryKind::Genesis(Genesis {
                    writer,
                    label: "w".into(),
                });
                let mut entries = vec![Entry::encode(EntryHash::ZERO, 0, at(0), kind).unwrap()];
                for n in 1..pick(30) {
                    let prev = &entries[pick(entries.len()).max(n.saturating_sub(2)).min(n - 1)];
                    entries.push(after(prev, &format!("{w}-{n}")));
                }
                let mut order: Vec<&Entry> = entries.iter().collect();
                for i in (1..order.len()).rev() {
                    order.swap(i, pick(i + 1));
                }
                let dir = layout.writer(writer);
                for (i, part) in order.chunks(1 + pick(6)).enumerate() {
                    files.push((dir.join(&format!("s{i}.txt")).unwrap(), bytes_of(part)));
                }
                if pick(3) == 0 {
                    let mut chain = vec![entries[0].clone()];
                    while let Some(next) = entries
                        .iter()
                        .find(|e| e.prev() == chain.last().unwrap().hash())
                    {
                        chain.push(next.clone());
                    }
                    let folded = snapshot_of(writer, &chain[..1 + pick(chain.len())]);
                    files.push((dir.join("snap.json").unwrap(), folded.encode()));
                }
            }
            let mut whole = Reader::new(layout.clone(), CachedView::default());
            let mut sliced = whole.clone();
            let mut grown: Vec<(RelPath, Vec<u8>)> = Vec::new();
            for round in 0..3 {
                for (path, bytes) in std::mem::take(&mut grown) {
                    grow(&disk, &path, &bytes);
                }
                for (path, bytes) in std::mem::take(&mut files) {
                    let ends = bytes.iter().enumerate().filter(|(_, b)| **b == b'\n');
                    let ends: Vec<usize> = ends.map(|(at, _)| at + 1).collect();
                    match pick(4) {
                        0 if round < 2 => files.push((path, bytes)),
                        1 if round < 2 && ends.len() > 1 => {
                            let cut = ends[pick(ends.len() - 1)];
                            put(&disk, &path, &bytes[..cut]);
                            grown.push((path, bytes[cut..].to_vec()));
                        }
                        _ => put(&disk, &path, &bytes),
                    }
                }
                if round == 2 && pick(2) == 0 {
                    let gone = disk.files(Root::Folder).into_keys().next().unwrap();
                    disk.perform(Io::Remove {
                        root: Root::Folder,
                        path: gone,
                    })
                    .unwrap();
                }
                let listing = crate::blocking::run(&mut disk.clone(), whole.list()).unwrap();
                let expected = whole.absorb(listing);
                let listing = crate::blocking::run(&mut disk.clone(), sliced.list()).unwrap();
                let slice = 1 + pick(5);
                let mut absorbing = sliced.absorbing(listing);
                while !sliced.absorb_slice(&mut absorbing, slice) {}
                let report = sliced.absorbed(absorbing);
                assert_eq!(report, expected, "round {round}, {slice}-entry slices");
                assert_eq!(
                    state(&sliced),
                    state(&whole),
                    "round {round}, {slice}-entry slices"
                );
            }
        }
    }

    #[test]
    fn a_grown_segment_is_read_from_the_end_of_its_last_line() {
        let layout = Layout::new(".lib").unwrap();
        let disk = MemDisk::new();
        let entries = chain(3);
        let path = layout.writer(W).join("s.jsonl").unwrap();
        put(&disk, &path, &bytes_of(&[&entries[0], &entries[1]]));
        let mut backend = Counting(disk.clone(), 0);
        let mut reader = Reader::new(layout, CachedView::default());
        crate::blocking::run(&mut backend, reader.read()).unwrap();
        let before = backend.1;
        let gained = bytes_of(&[&entries[2], &entries[3]]);
        grow(&disk, &path, &gained);

        let report = crate::blocking::run(&mut backend, reader.read()).unwrap();
        assert_eq!(report.placed[&W], hashes(&entries[2..]));
        assert_eq!(backend.1 - before, line::ENDING + gained.len() as u64);
        assert_eq!(report.forks, []);
    }

    #[test]
    fn a_segment_whose_last_line_changed_as_it_grew_is_read_whole() {
        let layout = Layout::new(".lib").unwrap();
        let disk = MemDisk::new();
        let entries = chain(1);
        let (a, b) = (after(&entries[1], "a"), after(&entries[1], "bb"));
        let path = layout.writer(W).join("s.jsonl").unwrap();
        put(&disk, &path, &bytes_of(&[&entries[0], &entries[1], &a]));
        let mut reader = Reader::new(layout, CachedView::default());
        crate::blocking::run(&mut disk.clone(), reader.read()).unwrap();
        let remove = Io::Remove {
            root: Root::Folder,
            path: path.clone(),
        };
        disk.perform(remove).unwrap();
        let c = after(&b, "c");
        put(&disk, &path, &bytes_of(&[&entries[0], &entries[1], &b, &c]));

        let report = crate::blocking::run(&mut disk.clone(), reader.read()).unwrap();
        let fork = Fork {
            writer: W,
            at: 1,
            prev: entries[1].hash(),
        };
        assert_eq!(report.forks, [fork], "{report:?}");
        let runs = &reader.derived().writers[&W].runs;
        let runs: BTreeSet<EntryHash> = runs.iter().map(|entry| entry.hash()).collect();
        let branches = BTreeSet::from([a.hash(), b.hash(), c.hash()]);
        assert_eq!(runs, branches, "the remembered branch stays a run");
    }

    #[test]
    fn a_torn_line_completed_later_is_read_from_where_the_lines_ended() {
        let layout = Layout::new(".lib").unwrap();
        let disk = MemDisk::new();
        let entries = chain(2);
        let path = layout.writer(W).join("s.jsonl").unwrap();
        let whole = bytes_of(&[&entries[0], &entries[1], &entries[2]]);
        let torn = entries[0].to_bytes().len() + entries[1].to_bytes().len() + 9;
        put(&disk, &path, &whole[..torn]);
        let mut reader = Reader::new(layout, CachedView::default());
        let first = crate::blocking::run(&mut disk.clone(), reader.read()).unwrap();
        assert_eq!(first.unreadable.len(), 1);
        assert_eq!(first.placed[&W], hashes(&entries[..2]));
        grow(&disk, &path, &whole[torn..]);

        let mut backend = Counting(disk.clone(), 0);
        let report = crate::blocking::run(&mut backend, reader.read()).unwrap();
        assert_eq!(report.placed[&W], [entries[2].hash()]);
        assert_eq!(report.unreadable, []);
        let read_on = line::ENDING + entries[2].to_bytes().len() as u64;
        assert_eq!(backend.1, read_on);
    }

    #[test]
    fn a_sealed_segment_is_read_whole_once_anything_follows_its_marker() {
        let layout = Layout::new(".lib").unwrap();
        let disk = MemDisk::new();
        let entries = chain(2);
        let path = layout.writer(W).join("s.jsonl").unwrap();
        let mut sealed = bytes_of(&[&entries[0], &entries[1]]);
        let end = sealed.len() as u64;
        sealed.extend(line::seal_marker(entries[1].hash()));
        put(&disk, &path, &sealed);
        let mut reader = Reader::new(layout, CachedView::default());
        let first = crate::blocking::run(&mut disk.clone(), reader.read()).unwrap();
        assert_eq!(first.unreadable, []);
        let files = reader.files(W);
        let [(_, _, WriterFile::Segment(segment))] = files.as_slice() else {
            panic!("{}", files.len())
        };
        assert!(segment.sealed);
        grow(&disk, &path, &entries[2].to_bytes());

        let mut backend = Counting(disk.clone(), 0);
        let report = crate::blocking::run(&mut backend, reader.read()).unwrap();
        assert_eq!(report.placed, BTreeMap::new());
        let stops: Vec<u64> = report
            .unreadable
            .iter()
            .filter_map(|(_, stop)| stop.as_ref().map(|stop| stop.offset))
            .collect();
        assert_eq!(stops, [end]);
        let len = sealed.len() as u64 + entries[2].to_bytes().len() as u64;
        assert_eq!(backend.1, len, "read whole");
    }

    #[test]
    fn a_segment_replaced_at_its_length_and_time_is_read_again() {
        let layout = Layout::new(".lib").unwrap();
        let mut disk = MemDisk::new();
        let entries = chain(2);
        let other = after(&entries[1], "x1");
        assert_eq!(other.to_bytes().len(), entries[2].to_bytes().len());
        let path = layout.writer(W).join("s.jsonl").unwrap();
        put(
            &disk,
            &path,
            &bytes_of(&[&entries[0], &entries[1], &entries[2]]),
        );
        let mut reader = Reader::new(layout, CachedView::default());
        crate::blocking::run(&mut disk, reader.read()).unwrap();
        let modified = |disk: &mut MemDisk| {
            let stat = Io::Stat {
                root: Root::Folder,
                path: path.clone(),
            };
            match disk.perform(stat).unwrap() {
                crate::io::Reply::Stat(Some(meta)) => meta.modified.unwrap(),
                reply => panic!("{reply:?}"),
            }
        };
        let before = modified(&mut disk);
        let remove = Io::Remove {
            root: Root::Folder,
            path: path.clone(),
        };
        disk.perform(remove).unwrap();
        put(&disk, &path, &bytes_of(&[&entries[0], &entries[1], &other]));
        disk.set_modified(Root::Folder, &path, before).unwrap();
        assert_eq!(modified(&mut disk), before);

        let report = crate::blocking::run(&mut disk, reader.read()).unwrap();
        assert_eq!(report.forks.len(), 1, "{report:?}");
        let runs = &reader.derived().writers[&W].runs;
        assert!(runs.iter().any(|entry| entry.hash() == other.hash()));
    }

    #[test]
    fn a_cached_view_saved_in_the_local_root_loads_back() {
        let mut disk = MemDisk::new();
        let entries = chain(2);
        let layout = Layout::new(".lib").unwrap();
        let path = layout.writer(W).join("s.txt").unwrap();
        let all: Vec<&Entry> = entries.iter().collect();
        put(&disk, &path, &bytes_of(&all));
        let view = read(&disk).cached().clone();
        assert_eq!(view.writers()[&W].entries.len(), 3);
        let genesis = entries[0].hash();
        let empty = crate::blocking::run(&mut disk, CachedView::load(genesis)).unwrap();
        assert_eq!(empty, CachedView::default());
        for io in [
            Io::MakeDir {
                root: Root::Local,
                path: Layout::local(genesis),
            },
            Io::Sync {
                root: Root::Local,
                path: RelPath::ROOT,
            },
        ] {
            disk.perform(io).unwrap();
        }
        crate::blocking::run(&mut disk, view.save(genesis)).unwrap();
        crate::blocking::run(&mut disk, view.save(genesis)).unwrap();
        let loaded = crate::blocking::run(&mut disk, CachedView::load(genesis)).unwrap();
        assert_eq!(loaded, view);
        let mut restarted = disk.restart();
        let loaded = crate::blocking::run(&mut restarted, CachedView::load(genesis)).unwrap();
        assert_eq!(loaded, view);
    }

    #[test]
    fn a_journal_a_crash_left_beside_a_newer_checkpoint_is_not_replayed_onto_it() {
        let mut disk = MemDisk::new();
        let entries = chain(2);
        let layout = Layout::new(".lib").unwrap();
        let path = layout.writer(W).join("s.txt").unwrap();
        put(
            &disk,
            &path,
            &bytes_of(&[&entries[0], &entries[1], &entries[2]]),
        );
        let genesis = entries[0].hash();
        disk.perform(Io::MakeDir {
            root: Root::Local,
            path: Layout::local(genesis),
        })
        .unwrap();
        let mut reader = read(&disk);
        crate::blocking::run(&mut disk, reader.checkpoint(genesis)).unwrap();
        disk.perform(Io::Remove {
            root: Root::Folder,
            path,
        })
        .unwrap();
        crate::blocking::run(&mut disk.clone(), reader.read()).unwrap();
        reader.note_grace(1_000, false);
        crate::blocking::run(&mut disk, reader.save(genesis)).unwrap();
        let journal = Layout::view_journal(genesis);
        let stale = disk.files(Root::Local)[&journal].clone();
        assert!(reader.note_grace(1_000 + GRACE_MS, true));
        reader.take_grace();
        crate::blocking::run(&mut disk, reader.save(genesis)).unwrap();
        assert!(!disk.files(Root::Local).contains_key(&journal));
        disk.perform(Io::Create {
            root: Root::Local,
            path: journal,
            bytes: stale,
        })
        .unwrap();

        let loaded = crate::blocking::run(&mut disk, CachedView::load(genesis)).unwrap();
        assert_eq!(&loaded, reader.cached());
    }

    #[test]
    fn the_grace_step_is_due_a_day_and_a_full_rescan_after_something_went_unseen() {
        let disk = MemDisk::new();
        let entries = chain(2);
        let layout = Layout::new(".lib").unwrap();
        let path = layout.writer(W).join("s.txt").unwrap();
        put(
            &disk,
            &path,
            &bytes_of(&[&entries[0], &entries[1], &entries[2]]),
        );
        let mut reader = read(&disk);
        disk.perform(Io::Remove {
            root: Root::Folder,
            path,
        })
        .unwrap();
        crate::blocking::run(&mut disk.clone(), reader.read()).unwrap();
        let merged = |reader: &Reader| reader.derived().writers[&W].merged.len();
        assert_eq!(merged(&reader), 3, "what no file holds stays shown");
        assert!(!reader.note_grace(1_000, false));
        assert!(
            !reader.note_grace(1_000 + GRACE_MS, false),
            "no full rescan yet"
        );
        assert!(reader.note_grace(1_000 + GRACE_MS, true));
        let report = reader.take_grace();
        assert!(report.changed);
        assert_eq!(merged(&reader), 0, "nothing visible descends from it");
        assert!(reader.cached().grace.graced);
        assert!(
            !reader.note_grace(2_000 + GRACE_MS, true),
            "nothing is unseen now"
        );
    }
}
