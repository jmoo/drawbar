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

use crate::cache::{self, Changed, Store};
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

    /// Keeps this view alone for the writer whose genesis entry is `genesis`,
    /// replacing what was kept so that a crash leaves the old view or the new.
    pub fn save(&self, genesis: EntryHash) -> Task<'static, Result<()>> {
        cache::write_checkpoint(genesis, self.encode(), Rc::default()).task()
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
        let mut reader = Self {
            layout,
            cached,
            folder: BTreeMap::new(),
            derived: Rc::default(),
            unsaved: Unsaved::default(),
            absorbed: BTreeSet::new(),
            store: Rc::default(),
        };
        reader.derived = Rc::new(reader.derive_now());
        reader
    }

    /// A reader starting from what [`cache::load`] found in the local root. The
    /// files the saved records describe are read again only once they change.
    pub fn open(layout: Layout, loaded: cache::Loaded) -> Self {
        let cache::Loaded {
            view,
            files,
            absorbed,
            store,
        } = loaded;
        let mut reader = Self::new(layout, view);
        reader.absorbed = absorbed;
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
        self.rederive(&mut report);
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
    /// what the files and the cached view show.
    pub(crate) fn absorbed(&mut self, mut absorbing: Absorbing) -> ReadReport {
        for writer in self.folder.keys().copied().collect::<Vec<_>>() {
            self.report_unreadable(writer, &mut absorbing.report);
        }
        self.rederive(&mut absorbing.report);
        self.drop_unsaved_without_store();
        absorbing.report
    }

    /// A view not yet saved anywhere is written whole by its first save, so what
    /// it gains is not kept apart.
    fn drop_unsaved_without_store(&mut self) {
        if self.store.borrow().genesis.is_none() {
            self.unsaved = Unsaved::default();
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
            if let Some(first) = placed.gaps.first() {
                report.gaps.push(Gap {
                    writer: *writer,
                    missing: first.prev(),
                    held: placed.gaps.len(),
                });
            }
        }
        self.derived = Rc::new(derived);
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
            let changed = Changed::between(self.cached.writers.get(&writer), kept.get(&writer));
            if !changed.is_empty() {
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
