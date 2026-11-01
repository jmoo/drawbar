//! Reading writers' logs from the folder, and deriving what they show.
//!
//! A reader takes every file directly in `writers.2/<w>/`, whatever its name,
//! and reads it by its contents as segment lines or as a snapshot. From the files
//! it sees, [`crate::derive`] places each entry, finds forks, and merges. Reading
//! writes nothing in the folder.
//!
//! Each line is parsed and decoded once: the reader keeps what it read of each
//! file while it lives, and reads only the bytes a segment gained since.

use std::collections::{BTreeMap, BTreeSet};
use std::rc::Rc;

use crate::derive::{self, Derived, Seen, SeenSegment};
use crate::error::Result;
use crate::flow::{self, Flow};
use crate::ids::{EntryHash, Hlc, WriterId};
use crate::io::{Kind, Meta, Range, Root, Task};
use crate::layout::{is_swap_file, Layout};
use crate::line::{self, Stop};
use crate::log::{Entry, Own};
use crate::path::RelPath;
use crate::report::{Fork, Gap};
use crate::snapshot::Snapshot;

/// The most of one file a reader holds. A longer segment is read up to here; a
/// longer snapshot is not read.
pub const MAX_FILE: u64 = 1 << 28;

/// What one file in a writer's directory holds, judged by its contents.
#[derive(Clone, PartialEq, Debug)]
pub enum WriterFile {
    /// An empty file is an empty segment.
    Segment(Segment),
    Snapshot(Rc<Snapshot>),
    /// Neither: reported, and read again next time.
    Unreadable {
        /// Whether it ends inside a JSON value, as a snapshot a crash tore does.
        cut: bool,
    },
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

impl WriterFile {
    pub fn parse(bytes: &[u8]) -> Self {
        let mut segment = Segment::default();
        segment.extend(bytes);
        if !segment.entries.is_empty() || segment.stop.is_none() {
            return Self::Segment(segment);
        }
        match Snapshot::decode(bytes) {
            Ok(snapshot) => Self::Snapshot(Rc::new(snapshot)),
            Err(not) => Self::Unreadable { cut: not.cut },
        }
    }
}

/// What one read found.
#[derive(Clone, PartialEq, Eq, Debug, Default)]
pub struct ReadReport {
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
    /// Whether the read found anything new: a file in a writer's directory
    /// came, went or changed.
    pub fn anything_new(&self) -> bool {
        self.folder_changed
    }
}

#[derive(Clone)]
pub struct Reader {
    layout: Layout,
    /// What each writer's directory in the folder held at the last read.
    folder: BTreeMap<WriterId, Folder>,
    /// What the files derive.
    derived: Rc<Derived>,
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

impl Reader {
    /// A reader that has read nothing yet.
    pub fn new(layout: Layout) -> Self {
        Self {
            layout,
            folder: BTreeMap::new(),
            derived: Rc::default(),
        }
    }

    /// Lets go of what the reader derived and found in each file, as a library
    /// closing does.
    pub(crate) fn shed(&mut self) {
        self.derived = Rc::default();
        self.folder = BTreeMap::new();
    }

    /// What the files derive, as of the last read.
    pub fn derived(&self) -> &Derived {
        &self.derived
    }

    /// Every writer the files hold.
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
                        WriterFile::Unreadable { .. } => (None, false),
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

    /// Takes in entries this instance appended as `writer` to the segment at
    /// `path`, as a read would, with the stamp a read would find on it when it is
    /// known.
    pub fn add(
        &mut self,
        writer: WriterId,
        entries: &[Entry],
        path: &RelPath,
        stamp: Option<Stamp>,
    ) -> ReadReport {
        let entries: Vec<Rc<Entry>> = entries.iter().cloned().map(Rc::new).collect();
        let folder = self.folder.entry(writer).or_default();
        folder.appended(path, &entries, stamp);
        let mut report = ReadReport {
            folder_changed: true,
            ..ReadReport::default()
        };
        let derived = Rc::make_mut(&mut self.derived);
        match derive::extend(derived, writer, &entries) {
            true => {
                let placed = entries.iter().map(|entry| entry.hash()).collect();
                report.placed.insert(writer, placed);
                report.gaps = self.gaps();
            }
            false => self.rederive(&mut report),
        }
        report
    }

    /// Takes in what [`Reader::list`] or [`Reader::list_writer`] read, and derives
    /// what the files show when a file came, went or changed.
    pub fn absorb(&mut self, listing: Listing) -> ReadReport {
        let Listing {
            writers: listed,
            everyone,
        } = listing;
        let mut report = ReadReport::default();
        if everyone {
            let present: BTreeSet<WriterId> = listed.iter().map(|listed| listed.writer).collect();
            let before = self.folder.len();
            self.folder.retain(|writer, _| present.contains(writer));
            report.folder_changed = self.folder.len() != before;
        }
        for listed in listed {
            self.gather(listed, &mut report);
        }
        for writer in self.folder.keys().copied().collect::<Vec<_>>() {
            self.report_unreadable(writer, &mut report);
        }
        match report.folder_changed {
            true => self.rederive(&mut report),
            false => report.gaps = self.gaps(),
        }
        report
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
                    WriterFile::Snapshot(_) | WriterFile::Unreadable { .. } => {}
                }
            }
            seen.insert(*writer, files);
        }
        seen
    }

    fn derive_now(&self) -> Derived {
        derive::derive(&self.seen())
    }

    /// What the files derive now, from scratch, as tests compare with what the
    /// reader holds.
    #[doc(hidden)]
    pub fn derive_afresh(&self) -> Derived {
        self.derive_now()
    }

    /// Derives again from the files, and reports what changed.
    fn rederive(&mut self, report: &mut ReadReport) {
        let mut derived = self.derive_now();
        let before = Rc::clone(&self.derived);
        derived.folded.share(&before.folded);
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
                WriterFile::Snapshot(_) | WriterFile::Unreadable { .. } => {
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
}

impl Folder {
    /// Records that the segment at `path` gained `entries`. Where the last read
    /// left it just where they begin, it ends as `stamp` says; otherwise, or
    /// without a stamp, the file is read whole again next time.
    fn appended(&mut self, path: &RelPath, entries: &[Rc<Entry>], stamp: Option<Stamp>) {
        let added: u64 = entries.iter().map(|e| e.to_bytes().len() as u64).sum();
        let start = stamp
            .as_ref()
            .and_then(|stamp| stamp.len.checked_sub(added));
        let continues = match (self.files.get(path), start) {
            (None, Some(start)) => start == 0,
            (
                Some(Record {
                    stamp: Some(old),
                    file: WriterFile::Segment(segment),
                }),
                Some(start),
            ) => {
                old.len == start
                    && segment.end == start
                    && !segment.sealed
                    && segment.entries.last().map(|last| last.hash())
                        == entries.first().map(|first| first.prev())
            }
            _ => false,
        };
        let record = self.files.entry(path.clone()).or_insert_with(|| Record {
            stamp: None,
            file: WriterFile::Segment(Segment::default()),
        });
        let stamp = stamp.filter(|_| continues);
        match &mut record.file {
            WriterFile::Segment(segment) => {
                segment.entries.extend(entries.iter().cloned());
                if let Some(stamp) = &stamp {
                    segment.end = stamp.len;
                }
            }
            file => {
                *file = WriterFile::Segment(Segment {
                    entries: entries.to_vec(),
                    ..Segment::default()
                });
            }
        }
        record.stamp = stamp;
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
        .map_ok(parsed)
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

/// What each file fetched found, those fetched whole parsed.
fn parsed(fetched: Vec<(RelPath, Fetched)>) -> Vec<Scanned> {
    let found = |fetched| match fetched {
        Fetched::Whole { bytes, stamp } => Found::Read {
            file: WriterFile::parse(&bytes),
            stamp,
        },
        Fetched::Unchanged => Found::Unchanged,
        Fetched::Grown { bytes, stamp } => Found::Grown { bytes, stamp },
    };
    let scanned = fetched.into_iter().map(|(path, fetched)| Scanned {
        path,
        found: found(fetched),
    });
    scanned.collect()
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
    use crate::compaction::{snapshot as fold, Found};
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

    fn genesis_of(writer: WriterId) -> Entry {
        let kind = EntryKind::Genesis(Genesis {
            writer,
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
            record: None,
        });
        let n = prev.n() + 1;
        Entry::encode(prev.hash(), n, at(n + label.len() as u64), kind).unwrap()
    }

    /// The genesis entry and `n` entries after it.
    fn chain(n: usize) -> Vec<Entry> {
        chain_of(W, n)
    }

    fn chain_of(writer: WriterId, n: usize) -> Vec<Entry> {
        let mut entries = vec![genesis_of(writer)];
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

    #[test]
    fn a_compaction_names_the_head_of_a_snapshot_it_folds_whose_segment_is_gone() {
        let entries: Vec<Rc<Entry>> = chain(3).into_iter().map(Rc::new).collect();
        let genesis = entries[0].hash();
        let sealed = Found::Lines(&entries[..3]);
        let old = fold(W, genesis, "a".into(), None, &entries[..3], [sealed]).unwrap();
        let found = [Found::Lines(&entries), Found::Snapshot(&old)];
        let new = fold(W, genesis, "a".into(), None, &entries, found).unwrap();
        assert!(
            new.certified.named().any(|own| own == old.head),
            "{:?} names {:?}",
            new.certified,
            old.head
        );
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
        let whole = snapshot.encode();
        let cut = whole[..whole.len() / 2].to_vec();
        let mut zeroed = cut.clone();
        zeroed.resize(whole.len(), 0);
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
            (b"hello".to_vec(), WriterFile::Unreadable { cut: false }),
            (
                br#"{"writer":1}"#.to_vec(),
                WriterFile::Unreadable { cut: false },
            ),
            (cut, WriterFile::Unreadable { cut: true }),
            (zeroed, WriterFile::Unreadable { cut: true }),
            (vec![0; 40], WriterFile::Unreadable { cut: true }),
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
        let seeds = [
            bytes_of(&entries.iter().collect::<Vec<_>>()),
            snapshot(&entries).encode(),
        ];
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
                WriterFile::Unreadable { .. } => {}
            }
            let derived = derive::derive(&BTreeMap::from([(W, seen)]));
            for placed in derived.writers.values() {
                for entry in placed.merged.iter().skip(1) {
                    assert!(placed.holds(entry.prev()), "merged after its predecessor");
                }
            }
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
        let mut reader = Reader::new(layout.clone());
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

    #[test]
    fn a_grown_segment_is_read_from_the_end_of_its_last_line() {
        let layout = Layout::new(".lib").unwrap();
        let disk = MemDisk::new();
        let entries = chain(3);
        let path = layout.writer(W).join("s.jsonl").unwrap();
        put(&disk, &path, &bytes_of(&[&entries[0], &entries[1]]));
        let mut backend = Counting(disk.clone(), 0);
        let mut reader = Reader::new(layout);
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
        let mut reader = Reader::new(layout);
        crate::blocking::run(&mut disk.clone(), reader.read()).unwrap();
        let remove = Io::Remove {
            root: Root::Folder,
            path: path.clone(),
        };
        disk.perform(remove).unwrap();
        let c = after(&b, "c");
        put(&disk, &path, &bytes_of(&[&entries[0], &entries[1], &b, &c]));

        crate::blocking::run(&mut disk.clone(), reader.read()).unwrap();
        let merged = &reader.derived().writers[&W].merged;
        let merged: Vec<EntryHash> = merged.iter().map(|entry| entry.hash()).collect();
        let expected = [entries[0].hash(), entries[1].hash(), b.hash(), c.hash()];
        assert_eq!(merged, expected, "the file read whole");
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
        let mut reader = Reader::new(layout);
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
        let mut reader = Reader::new(layout);
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
        let mut reader = Reader::new(layout);
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

        crate::blocking::run(&mut disk, reader.read()).unwrap();
        let merged = &reader.derived().writers[&W].merged;
        let merged: Vec<EntryHash> = merged.iter().map(|entry| entry.hash()).collect();
        assert_eq!(
            merged,
            hashes(&[entries[0].clone(), entries[1].clone(), other])
        );
    }
}
