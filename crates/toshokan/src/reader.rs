//! Reading writers' logs from the folder and placing their entries by chain.
//!
//! A reader takes every file directly in `writers/<w>/`, whatever its name, and
//! reads it by its contents as segment lines or as a snapshot. It places an entry
//! once its predecessor is placed or folded by a snapshot it has seen; an entry
//! after a gap is held back. Two entries with one predecessor are a fork: both
//! branches are merged and the fork is reported once. What is placed is kept in
//! the cached view, which only grows. Reading writes nothing in the folder.

use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::rc::Rc;

use serde::{Deserialize, Serialize};

use crate::error::{Error, Result};
use crate::flow::{self, Flow};
use crate::ids::{EntryHash, Hlc, WriterId};
use crate::io::{Kind, Range, Root, Task};
use crate::layout::Layout;
use crate::line::{lines, Line, Stop};
use crate::log::{Entry, EntryKind};
use crate::path::RelPath;
use crate::report::{Fork, Gap};
use crate::schema::Raw;
use crate::snapshot::Snapshot;

/// The most of one file a reader holds. A longer segment is read up to here; a
/// longer snapshot is not read.
pub const MAX_FILE: u64 = 1 << 28;

/// What one file in a writer's directory holds, judged by its contents.
#[derive(Clone, PartialEq, Debug)]
pub enum WriterFile {
    /// Lines up to the first unreadable one. An empty file is an empty segment.
    Segment {
        lines: Vec<Line>,
        stop: Option<Stop>,
    },
    Snapshot(Box<Snapshot>),
    /// Neither: reported, and read again next time.
    Unreadable,
}

impl WriterFile {
    pub fn parse(bytes: &[u8]) -> Self {
        let mut read = lines(bytes);
        let found: Vec<Line> = read.by_ref().collect();
        let stop = read.stop().cloned();
        if !found.is_empty() || stop.is_none() {
            return Self::Segment { lines: found, stop };
        }
        match Snapshot::decode(bytes) {
            Ok(snapshot) => Self::Snapshot(Box::new(snapshot)),
            Err(_) => Self::Unreadable,
        }
    }
}

/// One writer's history as this install has placed it.
#[derive(Clone, PartialEq, Debug)]
pub struct WriterLog {
    writer: WriterId,
    /// No one extends another.
    snapshots: Vec<Snapshot>,
    entries: Vec<Entry>,
    /// Every placed or folded entry, with its predecessor.
    chain: BTreeMap<EntryHash, EntryHash>,
    /// Entries seen and never placed, with their predecessors, so that a fork with
    /// one of them is found after its file is gone.
    strays: BTreeMap<EntryHash, EntryHash>,
    forks: Vec<Fork>,
    gaps: Vec<Gap>,
}

/// What [`WriterLog::place`] added.
#[derive(Clone, PartialEq, Eq, Debug, Default)]
pub struct Placement {
    /// In placement order: each after its predecessor.
    pub placed: Vec<EntryHash>,
    pub forks: Vec<Fork>,
    /// Whether the log grew at all: a snapshot kept, a line placed or held back.
    pub changed: bool,
}

impl WriterLog {
    pub fn new(writer: WriterId) -> Self {
        Self {
            writer,
            snapshots: Vec::new(),
            entries: Vec::new(),
            chain: BTreeMap::new(),
            strays: BTreeMap::new(),
            forks: Vec::new(),
            gaps: Vec::new(),
        }
    }

    pub fn writer(&self) -> WriterId {
        self.writer
    }

    /// From the genesis entry, once placed or folded.
    pub fn label(&self) -> Option<&str> {
        let folded = self
            .snapshots
            .first()
            .map(|snapshot| snapshot.label.as_str());
        folded.or_else(|| {
            self.entries.iter().find_map(|entry| match &entry.kind {
                EntryKind::Genesis(genesis) => Some(genesis.label.as_str()),
                _ => None,
            })
        })
    }

    pub fn genesis(&self) -> Option<EntryHash> {
        self.chain
            .iter()
            .find(|(_, prev)| **prev == EntryHash::ZERO)
            .map(|(hash, _)| *hash)
    }

    /// Whether `hash` is placed, or folded by a snapshot this install has seen.
    pub fn holds(&self, hash: EntryHash) -> bool {
        self.chain.contains_key(&hash)
    }

    /// Whether `hash` is placed, folded, or was ever held back.
    fn saw(&self, hash: EntryHash) -> bool {
        self.holds(hash) || self.strays.contains_key(&hash)
    }

    /// The predecessor of a placed or folded entry.
    pub fn predecessor(&self, hash: EntryHash) -> Option<EntryHash> {
        self.chain.get(&hash).copied()
    }

    /// The placed or folded entries from the genesis entry to `head`, in chain
    /// order; `None` unless every one of them is held.
    pub fn chain_to(&self, head: EntryHash) -> Option<Vec<EntryHash>> {
        let mut chain = Vec::new();
        let mut at = head;
        while at != EntryHash::ZERO && chain.len() <= self.chain.len() {
            chain.push(at);
            at = self.predecessor(at)?;
        }
        chain.reverse();
        (at == EntryHash::ZERO).then_some(chain)
    }

    /// The snapshots whose folded lists the placed entries continue. More than one
    /// only when the writer's history forked and each branch compacted.
    pub fn snapshots(&self) -> &[Snapshot] {
        &self.snapshots
    }

    /// The placed entries no snapshot folds, each after its predecessor; fork
    /// branches in the order of their first entries' hashes.
    pub fn entries(&self) -> &[Entry] {
        &self.entries
    }

    /// Placed or folded entries with no placed successor: one, unless forked.
    pub fn heads(&self) -> Vec<EntryHash> {
        let prevs: BTreeSet<&EntryHash> = self.chain.values().collect();
        self.chain
            .keys()
            .filter(|hash| !prevs.contains(hash))
            .copied()
            .collect()
    }

    pub fn last_at(&self) -> Option<Hlc> {
        self.readings().max()
    }

    /// Every fork found so far. Never shrinks.
    pub fn forks(&self) -> &[Fork] {
        &self.forks
    }

    /// Entries seen in the last read but held back.
    pub fn gaps(&self) -> &[Gap] {
        &self.gaps
    }

    /// Places what `snapshots` fold and every line whose predecessor is then held,
    /// repeatedly. Lines left over are the gaps until the next placement. Only
    /// grows the log: a snapshot extended by one already kept is ignored, and one
    /// that extends kept ones replaces them.
    pub fn place(&mut self, snapshots: Vec<Snapshot>, lines: Vec<Line>) -> Placement {
        let kept = snapshots
            .into_iter()
            .fold(false, |kept, snapshot| self.keep(snapshot) | kept);
        if kept {
            self.reindex();
        }
        let mut waiting: BTreeMap<EntryHash, Line> = lines
            .into_iter()
            .filter(|line| !self.holds(line.hash()))
            .map(|line| (line.hash(), line))
            .collect();
        let mut after: BTreeMap<EntryHash, Vec<EntryHash>> = BTreeMap::new();
        for (hash, line) in &waiting {
            after.entry(line.prev()).or_default().push(*hash);
        }
        let mut ready: VecDeque<EntryHash> = after
            .keys()
            .filter(|prev| **prev == EntryHash::ZERO || self.holds(**prev))
            .copied()
            .collect();
        let mut placed = Vec::new();
        while let Some(prev) = ready.pop_front() {
            for hash in after.remove(&prev).unwrap_or_default() {
                let line = waiting
                    .remove(&hash)
                    .expect("each waiting line is placed once");
                self.chain.insert(hash, prev);
                self.strays.remove(&hash);
                self.entries.push(entry(line));
                placed.push(hash);
                ready.push_back(hash);
            }
        }
        let mut strayed = false;
        for (hash, line) in &waiting {
            strayed |= self.strays.insert(*hash, line.prev()).is_none();
        }
        self.gaps = gaps(self.writer, &waiting);
        if !placed.is_empty() {
            self.order();
        }
        let changed = kept || strayed || !placed.is_empty();
        let forks = match changed {
            true => self.find_forks(),
            false => Vec::new(),
        };
        Placement {
            placed,
            forks,
            changed,
        }
    }

    /// Places everything `other`, a log of the same writer, holds: its
    /// snapshots, entries, strays and forks.
    fn join(&mut self, other: &WriterLog) {
        for (hash, prev) in &other.strays {
            if !self.holds(*hash) {
                self.strays.insert(*hash, *prev);
            }
        }
        let reported: BTreeSet<EntryHash> = self.forks.iter().map(|fork| fork.prev).collect();
        let forks = other
            .forks
            .iter()
            .filter(|fork| !reported.contains(&fork.prev));
        self.forks.extend(forks.copied());
        let lines = other.entries.iter().map(|entry| entry.line.clone());
        self.place(other.snapshots.clone(), lines.collect());
    }

    /// Every clock reading of the placed entries and the snapshots.
    pub fn readings(&self) -> impl Iterator<Item = Hlc> + '_ {
        let entries = self.entries.iter().map(|entry| entry.at);
        entries.chain(self.snapshots.iter().map(|snapshot| snapshot.at))
    }

    fn keep(&mut self, snapshot: Snapshot) -> bool {
        if self.snapshots.iter().any(|kept| kept.extends(&snapshot)) {
            return false;
        }
        self.snapshots.retain(|kept| !snapshot.extends(kept));
        self.snapshots.push(snapshot);
        self.snapshots.sort_by_key(Snapshot::head);
        true
    }

    fn reindex(&mut self) {
        let folded: BTreeMap<EntryHash, EntryHash> =
            self.snapshots.iter().flat_map(Snapshot::pairs).collect();
        self.entries
            .retain(|entry| !folded.contains_key(&entry.hash()));
        self.chain = folded;
        self.chain.extend(
            self.entries
                .iter()
                .map(|entry| (entry.hash(), entry.prev())),
        );
        let chain = &self.chain;
        self.strays.retain(|hash, _| !chain.contains_key(hash));
    }

    fn order(&mut self) {
        let placed: BTreeSet<EntryHash> = self.entries.iter().map(Entry::hash).collect();
        let mut after: BTreeMap<EntryHash, Vec<Entry>> = BTreeMap::new();
        let mut roots = Vec::new();
        for entry in self.entries.drain(..) {
            match placed.contains(&entry.prev()) {
                true => after.entry(entry.prev()).or_default().push(entry),
                false => roots.push(entry),
            }
        }
        roots.sort_by_key(|entry| std::cmp::Reverse(entry.hash()));
        let mut stack = roots;
        while let Some(entry) = stack.pop() {
            let hash = entry.hash();
            self.entries.push(entry);
            if let Some(mut next) = after.remove(&hash) {
                next.sort_by_key(|entry| std::cmp::Reverse(entry.hash()));
                stack.extend(next);
            }
        }
    }

    fn find_forks(&mut self) -> Vec<Fork> {
        let mut after: BTreeMap<EntryHash, BTreeSet<EntryHash>> = BTreeMap::new();
        for (hash, prev) in self.chain.iter().chain(&self.strays) {
            after.entry(*prev).or_default().insert(*hash);
        }
        let reported: BTreeSet<EntryHash> = self.forks.iter().map(|fork| fork.prev).collect();
        let found: Vec<Fork> = after
            .into_iter()
            .filter(|(prev, next)| next.len() > 1 && !reported.contains(prev))
            .map(|(prev, next)| {
                let mut next = next.into_iter();
                let branches = [next.next(), next.next()].map(|hash| hash.expect("two branches"));
                Fork {
                    writer: self.writer,
                    prev,
                    branches,
                }
            })
            .collect();
        self.forks.extend(&found);
        found
    }
}

/// A verified line as an entry. A line whose JSON is not an entry is still a link
/// of the chain, so it is placed as an unknown kind.
fn entry(line: Line) -> Entry {
    Entry::decode(line.clone()).unwrap_or_else(|_| Entry {
        at: Hlc::ZERO,
        kind: EntryKind::Unknown(Raw::new(line.json()).expect("a verified line holds JSON")),
        line,
    })
}

/// Held lines grouped by the missing entry each waits for.
fn gaps(writer: WriterId, waiting: &BTreeMap<EntryHash, Line>) -> Vec<Gap> {
    let mut missing_of: BTreeMap<EntryHash, EntryHash> = BTreeMap::new();
    for &start in waiting.keys() {
        let mut path = Vec::new();
        let mut at = start;
        let missing = loop {
            if let Some(&missing) = missing_of.get(&at) {
                break missing;
            }
            match waiting.get(&at) {
                Some(line) if path.len() <= waiting.len() => {
                    path.push(at);
                    at = line.prev();
                }
                _ => break at,
            }
        };
        missing_of.extend(path.into_iter().map(|hash| (hash, missing)));
    }
    let mut held: BTreeMap<EntryHash, usize> = BTreeMap::new();
    for missing in missing_of.into_values() {
        *held.entry(missing).or_default() += 1;
    }
    held.into_iter()
        .map(|(missing, held)| Gap {
            writer,
            missing,
            held,
        })
        .collect()
}

/// What this install has placed of each writer, kept in the local root.
///
/// It only grows: a writer's facts never disappear from it because files arrive
/// in another order, are deleted early or vanish. It never holds an entry whose
/// predecessor it neither holds nor sees folded.
#[derive(Clone, PartialEq, Debug, Default)]
pub struct CachedView {
    writers: BTreeMap<WriterId, WriterLog>,
}

#[derive(Serialize, Deserialize)]
struct StoredView {
    writers: BTreeMap<WriterId, StoredLog>,
}

#[derive(Serialize, Deserialize)]
struct StoredLog {
    snapshots: Vec<Raw>,
    /// Each line without its newline.
    entries: Vec<String>,
    strays: Vec<(EntryHash, EntryHash)>,
    forks: Vec<(EntryHash, [EntryHash; 2])>,
}

impl CachedView {
    /// Refuses what this build cannot read with [`crate::Error::Corrupt`] naming
    /// `path`; the caller then reads from scratch.
    pub fn decode(path: &RelPath, bytes: &[u8]) -> Result<Self> {
        let corrupt = |reason: String| Error::Corrupt {
            path: path.clone(),
            reason,
        };
        let stored: StoredView =
            serde_json::from_slice(bytes).map_err(|error| corrupt(error.to_string()))?;
        let mut writers = BTreeMap::new();
        for (writer, stored) in stored.writers {
            let snapshots = stored
                .snapshots
                .iter()
                .map(|raw| Snapshot::decode(raw.as_str().as_bytes()))
                .collect::<std::result::Result<Vec<_>, _>>()
                .map_err(|error| corrupt(error.to_string()))?;
            let lines = stored
                .entries
                .iter()
                .map(|text| Line::parse(format!("{text}\n").as_bytes()))
                .collect::<std::result::Result<Vec<_>, _>>()
                .map_err(|error| corrupt(error.to_string()))?;
            if snapshots.iter().any(|snapshot| snapshot.writer != writer) {
                return Err(corrupt(format!(
                    "a snapshot of {writer} names another writer"
                )));
            }
            let mut log = WriterLog::new(writer);
            log.strays = stored.strays.into_iter().collect();
            log.forks = stored
                .forks
                .into_iter()
                .map(|(prev, branches)| Fork {
                    writer,
                    prev,
                    branches,
                })
                .collect();
            log.place(snapshots, lines);
            if !log.gaps.is_empty() {
                return Err(corrupt(format!(
                    "an entry of {writer} follows one it does not hold"
                )));
            }
            writers.insert(writer, log);
        }
        Ok(Self { writers })
    }

    pub fn encode(&self) -> Vec<u8> {
        let writers = self
            .writers
            .iter()
            .map(|(writer, log)| {
                let snapshots = log
                    .snapshots
                    .iter()
                    .map(|snapshot| {
                        let text = String::from_utf8(snapshot.encode()).expect("JSON is UTF-8");
                        Raw::new(&text).expect("a snapshot is JSON")
                    })
                    .collect();
                let entries = log
                    .entries
                    .iter()
                    .map(|entry| format!("{}\t{}", entry.line.json(), entry.hash()))
                    .collect();
                let stored = StoredLog {
                    snapshots,
                    entries,
                    strays: log
                        .strays
                        .iter()
                        .map(|(hash, prev)| (*hash, *prev))
                        .collect(),
                    forks: log
                        .forks
                        .iter()
                        .map(|fork| (fork.prev, fork.branches))
                        .collect(),
                };
                (*writer, stored)
            })
            .collect();
        serde_json::to_vec(&StoredView { writers }).expect("a cached view is JSON")
    }

    pub fn writers(&self) -> &BTreeMap<WriterId, WriterLog> {
        &self.writers
    }

    /// The cached view kept for the writer whose genesis entry is `genesis`; empty
    /// when none was saved.
    pub fn load(genesis: EntryHash) -> Task<'static, Result<Self>> {
        let path = Layout::cached_view(genesis);
        flow::read_replaced(Root::Local, path.clone())
            .and_then(move |bytes| {
                Flow::Done(match bytes {
                    Some(bytes) => Self::decode(&path, &bytes),
                    None => Ok(Self::default()),
                })
            })
            .task()
    }

    /// The views kept for each of `genesis`, joined. A view this build cannot
    /// read is left out.
    pub fn load_all(genesis: Vec<EntryHash>) -> Task<'static, Result<Self>> {
        flow::fold(
            genesis.into_iter(),
            Self::default(),
            |mut joined, genesis| {
                flow::run(Self::load(genesis)).then(move |loaded| match loaded {
                    Ok(view) => {
                        joined.join(&view);
                        flow::ok(joined)
                    }
                    Err(Error::Corrupt { .. }) => flow::ok(joined),
                    Err(error) => Flow::Done(Err(error)),
                })
            },
        )
        .task()
    }

    /// Adds everything `other` holds.
    pub fn join(&mut self, other: &CachedView) {
        for (writer, log) in &other.writers {
            self.writers
                .entry(*writer)
                .or_insert_with(|| WriterLog::new(*writer))
                .join(log);
        }
    }

    /// Keeps this view for the writer whose genesis entry is `genesis`, replacing
    /// what was kept so that a crash leaves the old view or the new.
    pub fn save(&self, genesis: EntryHash) -> Task<'static, Result<()>> {
        flow::replace(Root::Local, Layout::cached_view(genesis), self.encode()).task()
    }
}

/// What one read placed and found.
#[derive(Clone, PartialEq, Eq, Debug, Default)]
pub struct ReadReport {
    /// Whether the cached view grew.
    pub changed: bool,
    /// Newly placed entries per writer, in placement order.
    pub placed: BTreeMap<WriterId, Vec<EntryHash>>,
    /// Forks found by this read and not reported before.
    pub forks: Vec<Fork>,
    /// Every gap as of this read.
    pub gaps: Vec<Gap>,
    /// Files whose readable part ended early, or that are neither segment nor
    /// snapshot.
    pub unreadable: Vec<(RelPath, Option<Stop>)>,
}

pub struct Reader {
    layout: Layout,
    cached: CachedView,
    /// What each writer's files in the folder held at the last read, placed
    /// without the cached view.
    folder: BTreeMap<WriterId, WriterLog>,
    /// Files read before, by path, so an unchanged file is not read again.
    seen: BTreeMap<RelPath, Seen>,
}

/// A file is unchanged while its length and modification time are.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
struct Stamp {
    len: u64,
    modified: u64,
}

struct Seen {
    stamp: Stamp,
    file: Rc<WriterFile>,
}

enum Found {
    Unchanged,
    Read(Vec<u8>),
}

/// A file in a writer's directory as a scan found it. Without a stamp it is read
/// every time.
struct Scanned {
    path: RelPath,
    stamp: Option<Stamp>,
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
    pub fn new(layout: Layout, cached: CachedView) -> Self {
        Self {
            layout,
            cached,
            folder: BTreeMap::new(),
            seen: BTreeMap::new(),
        }
    }

    pub fn cached(&self) -> &CachedView {
        &self.cached
    }

    pub fn logs(&self) -> &BTreeMap<WriterId, WriterLog> {
        &self.cached.writers
    }

    /// What `writer`'s files in the folder held at the last read, placed without
    /// the cached view; `None` when the folder held no file of it.
    pub fn folder_log(&self, writer: WriterId) -> Option<&WriterLog> {
        self.folder.get(&writer)
    }

    /// Each writer's placed and folded entries that no file in the folder held at
    /// the last read: a restore took them, or sync has not yet brought the files
    /// that hold them now. Writers with none are left out.
    pub fn removed(&self) -> BTreeMap<WriterId, BTreeSet<EntryHash>> {
        let removed = self.cached.writers.iter().map(|(writer, log)| {
            let folder = self.folder.get(writer);
            let gone = log.chain.keys().copied();
            let gone = gone.filter(|hash| !folder.is_some_and(|folder| folder.saw(*hash)));
            (*writer, gone.collect::<BTreeSet<EntryHash>>())
        });
        removed.filter(|(_, gone)| !gone.is_empty()).collect()
    }

    /// Reads every writer's directory and places what it can. Requests only
    /// [`crate::Io::List`], [`crate::Io::Stat`] and [`crate::Io::Read`] in the
    /// folder, each read bounded by [`MAX_FILE`].
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

    /// The reads of [`Reader::read`], for [`Reader::absorb`] to place.
    pub fn list(&self) -> Task<'static, Result<Listing>> {
        let writers = flow::list(Root::Folder, &self.layout.writers()).map_ok(|listed| {
            listed
                .into_iter()
                .filter(|entry| entry.kind == Kind::Directory)
                .filter_map(|entry| entry.name.parse().ok())
                .collect()
        });
        self.scan(writers, true)
    }

    /// The reads of [`Reader::read_writer`], for [`Reader::absorb`] to place.
    pub fn list_writer(&self, writer: WriterId) -> Task<'static, Result<Listing>> {
        self.scan(flow::ok(vec![writer]), false)
    }

    fn scan(
        &self,
        writers: Flow<'static, Result<Vec<WriterId>>>,
        everyone: bool,
    ) -> Task<'static, Result<Listing>> {
        let layout = self.layout.clone();
        let stamps: Rc<BTreeMap<RelPath, Stamp>> = Rc::new(
            self.seen
                .iter()
                .map(|(path, seen)| (path.clone(), seen.stamp))
                .collect(),
        );
        writers
            .and_then(move |writers| {
                flow::fold(writers.into_iter(), Vec::new(), move |mut done, writer| {
                    scan_writer(&layout, writer, Rc::clone(&stamps)).map_ok(move |listed| {
                        done.push(listed);
                        done
                    })
                })
            })
            .map_ok(move |writers| Listing { writers, everyone })
            .task()
    }

    /// Places entries this instance appended as `writer`, as a read would.
    pub fn add(&mut self, writer: WriterId, entries: &[Entry]) -> Placement {
        let lines: Vec<Line> = entries.iter().map(|entry| entry.line.clone()).collect();
        self.folder
            .entry(writer)
            .or_insert_with(|| WriterLog::new(writer))
            .place(Vec::new(), lines.clone());
        self.cached
            .writers
            .entry(writer)
            .or_insert_with(|| WriterLog::new(writer))
            .place(Vec::new(), lines)
    }

    /// Places what [`Reader::list`] or [`Reader::list_writer`] read.
    pub fn absorb(&mut self, listing: Listing) -> ReadReport {
        let Listing {
            writers: listed,
            everyone,
        } = listing;
        if everyone {
            let dirs: BTreeSet<RelPath> = listed
                .iter()
                .map(|listed| self.layout.writer(listed.writer))
                .collect();
            self.seen
                .retain(|path, _| path.parent().is_some_and(|dir| dirs.contains(&dir)));
            self.folder
                .retain(|writer, _| listed.iter().any(|listed| listed.writer == *writer));
            for (writer, log) in &mut self.cached.writers {
                if !listed.iter().any(|listed| listed.writer == *writer) {
                    log.gaps.clear();
                }
            }
        }
        let mut report = ReadReport::default();
        for listed in listed {
            self.absorb_writer(listed, &mut report);
        }
        report
    }

    fn absorb_writer(&mut self, listed: Listed, report: &mut ReadReport) {
        let writer = listed.writer;
        let dir = self.layout.writer(writer);
        let present: BTreeSet<RelPath> =
            listed.files.iter().map(|file| file.path.clone()).collect();
        let before = self.seen.len();
        self.seen
            .retain(|path, _| path.parent().as_ref() != Some(&dir) || present.contains(path));
        let mut changed = before != self.seen.len() || !self.cached.writers.contains_key(&writer);
        let (mut snapshots, mut lines) = (Vec::new(), Vec::new());
        for Scanned { path, stamp, found } in listed.files {
            let file = match found {
                Found::Unchanged => Rc::clone(&self.seen[&path].file),
                Found::Read(bytes) => {
                    changed = true;
                    let file = Rc::new(WriterFile::parse(&bytes));
                    match stamp {
                        Some(stamp) => {
                            let seen = Seen {
                                stamp,
                                file: Rc::clone(&file),
                            };
                            self.seen.insert(path.clone(), seen);
                        }
                        None => {
                            self.seen.remove(&path);
                        }
                    }
                    file
                }
            };
            match &*file {
                WriterFile::Segment { lines: found, stop } => {
                    lines.extend(found.iter().cloned());
                    if let Some(stop) = stop {
                        report.unreadable.push((path, Some(stop.clone())));
                    }
                }
                WriterFile::Snapshot(snapshot) if snapshot.writer == writer => {
                    snapshots.push(Snapshot::clone(snapshot));
                }
                WriterFile::Snapshot(_) | WriterFile::Unreadable => {
                    report.unreadable.push((path, None));
                }
            }
        }
        if changed || !self.folder.contains_key(&writer) {
            let mut folder = WriterLog::new(writer);
            folder.place(snapshots.clone(), lines.clone());
            self.folder.insert(writer, folder);
        }
        let log = self
            .cached
            .writers
            .entry(writer)
            .or_insert_with(|| WriterLog::new(writer));
        if changed {
            let placement = log.place(snapshots, lines);
            report.changed |= placement.changed;
            if !placement.placed.is_empty() {
                report.placed.insert(writer, placement.placed);
            }
            report.forks.extend(placement.forks);
        }
        report.gaps.extend(log.gaps.iter().copied());
    }
}

fn scan_writer<'a>(
    layout: &Layout,
    writer: WriterId,
    stamps: Rc<BTreeMap<RelPath, Stamp>>,
) -> Flow<'a, Result<Listed>> {
    let dir = layout.writer(writer);
    flow::list(Root::Folder, &dir)
        .and_then(move |entries| {
            let paths: Vec<RelPath> = entries
                .into_iter()
                .filter(|entry| entry.kind == Kind::File)
                .filter_map(|entry| dir.join(&entry.name).ok())
                .collect();
            flow::fold(paths.into_iter(), Vec::new(), move |mut files, path| {
                let known = stamps.get(&path).copied();
                scan_file(path, known).map_ok(move |found| {
                    files.extend(found);
                    files
                })
            })
        })
        .map_ok(move |files| Listed { writer, files })
}

fn scan_file<'a>(path: RelPath, known: Option<Stamp>) -> Flow<'a, Result<Option<Scanned>>> {
    flow::stat(Root::Folder, &path.clone()).and_then(move |meta| match meta {
        Some(meta) if meta.kind == Kind::File => {
            let stamp = meta.modified.map(|modified| Stamp {
                len: meta.len,
                modified,
            });
            if stamp.is_some() && stamp == known {
                let found = Found::Unchanged;
                return flow::ok(Some(Scanned { path, stamp, found }));
            }
            let range = Range {
                offset: 0,
                len: meta.len.min(MAX_FILE),
            };
            flow::read_present(Root::Folder, &path.clone(), range).map_ok(move |bytes| {
                bytes.map(|bytes| Scanned {
                    path,
                    stamp,
                    found: Found::Read(bytes),
                })
            })
        }
        _ => flow::ok(None),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::disk::MemDisk;
    use crate::io::Io;
    use crate::log::{Genesis, Logged};
    use crate::merge::Folded;

    const W: WriterId = WriterId::from_u128(0xa);

    fn at(n: u64) -> Hlc {
        Hlc {
            wall_ms: n,
            counter: 0,
        }
    }

    fn genesis() -> Line {
        let kind = EntryKind::Genesis(Genesis {
            writer: W,
            label: "a".into(),
        });
        Entry::encode(EntryHash::ZERO, at(0), kind).unwrap().line
    }

    fn after(prev: &Line, label: &str) -> Line {
        let kind = EntryKind::Intent(Logged {
            label: label.into(),
            ops: Vec::new(),
            displaced: Vec::new(),
            reverses: None,
        });
        Entry::encode(prev.hash(), at(prev.json().len() as u64), kind)
            .unwrap()
            .line
    }

    /// The genesis entry and `n` entries after it.
    fn chain(n: usize) -> Vec<Line> {
        let mut lines = vec![genesis()];
        for i in 0..n {
            let next = after(lines.last().unwrap(), &format!("e{i}"));
            lines.push(next);
        }
        lines
    }

    fn snapshot(lines: &[Line]) -> Snapshot {
        Snapshot {
            writer: W,
            label: "a".into(),
            at: at(9),
            folded: lines.iter().map(Line::hash).collect(),
            state: Folded::default(),
            unknown: BTreeMap::new(),
        }
    }

    fn hashes(lines: &[Line]) -> Vec<EntryHash> {
        lines.iter().map(Line::hash).collect()
    }

    fn entry_hashes(log: &WriterLog) -> Vec<EntryHash> {
        log.entries().iter().map(Entry::hash).collect()
    }

    #[test]
    fn entries_arriving_in_any_order_are_placed_in_chain_order() {
        let lines = chain(3);
        let mut log = WriterLog::new(W);
        let first = log.place(Vec::new(), vec![lines[3].clone(), lines[2].clone()]);
        assert_eq!(first.placed, []);
        assert_eq!(
            log.gaps(),
            [Gap {
                writer: W,
                missing: lines[1].hash(),
                held: 2
            }]
        );
        let second = log.place(Vec::new(), lines.iter().rev().cloned().collect());
        assert_eq!(second.placed, hashes(&lines));
        assert_eq!(entry_hashes(&log), hashes(&lines));
        assert_eq!(log.gaps(), []);
        assert_eq!(log.heads(), [lines[3].hash()]);
        assert_eq!(log.genesis(), Some(lines[0].hash()));
        assert_eq!(log.label(), Some("a"));
        assert_eq!(log.chain_to(lines[2].hash()), Some(hashes(&lines[..3])));
    }

    #[test]
    fn two_entries_after_one_are_a_fork_reported_once() {
        let lines = chain(1);
        let (a, b) = (after(&lines[1], "a"), after(&lines[1], "b"));
        let mut log = WriterLog::new(W);
        let first = log.place(
            Vec::new(),
            vec![lines[0].clone(), lines[1].clone(), a.clone()],
        );
        assert_eq!(first.forks, []);
        let second = log.place(Vec::new(), vec![b.clone()]);
        let mut branches = [a.hash(), b.hash()];
        branches.sort();
        let fork = Fork {
            writer: W,
            prev: lines[1].hash(),
            branches,
        };
        assert_eq!(second.forks, [fork]);
        assert_eq!(log.forks(), [fork]);
        assert_eq!(log.heads().len(), 2);
        let third = log.place(Vec::new(), vec![after(&lines[1], "c"), after(&a, "d")]);
        assert_eq!(third.forks, []);
        assert_eq!(log.forks(), [fork]);
    }

    #[test]
    fn a_fork_between_held_entries_is_found_after_their_file_is_gone() {
        let lines = chain(1);
        let (a, b) = (after(&lines[1], "a"), after(&lines[1], "b"));
        let mut log = WriterLog::new(W);
        log.place(Vec::new(), vec![a.clone()]);
        let found = log.place(Vec::new(), vec![b.clone()]);
        assert_eq!(found.forks.len(), 1, "{found:?}");
        assert_eq!(log.gaps()[0].missing, lines[1].hash());
    }

    // A snapshot that recorded only its last hash would leave `b` held forever,
    // and the fork at its predecessor unreported.
    #[test]
    fn an_entry_after_any_folded_one_is_placed_and_its_fork_reported() {
        let lines = chain(3);
        let b = after(&lines[1], "clone");
        let mut log = WriterLog::new(W);
        let placement = log.place(vec![snapshot(&lines)], vec![b.clone()]);
        assert_eq!(placement.placed, [b.hash()]);
        assert_eq!(placement.forks.len(), 1);
        assert_eq!(placement.forks[0].prev, lines[1].hash());
        assert!(lines.iter().all(|line| log.holds(line.hash())));
        assert_eq!(log.label(), Some("a"));
    }

    #[test]
    fn a_snapshot_replaces_the_entries_it_folds_and_only_grows() {
        let lines = chain(4);
        let mut log = WriterLog::new(W);
        log.place(vec![snapshot(&lines[..2])], lines.clone());
        assert_eq!(entry_hashes(&log), hashes(&lines[2..]));
        let grown = log.place(vec![snapshot(&lines[..4])], Vec::new());
        assert!(grown.changed, "a snapshot alone grows the log");
        assert_eq!(grown.placed, []);
        assert_eq!(log.snapshots(), [snapshot(&lines[..4])]);
        assert_eq!(entry_hashes(&log), hashes(&lines[4..]));
        let older = log.place(vec![snapshot(&lines[..3])], Vec::new());
        assert_eq!(older, Placement::default());
        assert_eq!(log.snapshots(), [snapshot(&lines[..4])]);
        assert!(lines.iter().all(|line| log.holds(line.hash())));
        assert_eq!(log.heads(), [lines[4].hash()]);
        assert_eq!(log.chain_to(lines[4].hash()), Some(hashes(&lines)));
    }

    #[test]
    fn a_line_that_is_not_an_entry_still_links_the_chain() {
        let lines = chain(0);
        let odd = Line::seal(format!(r#"{{"prev":"{}"}}"#, lines[0].hash())).unwrap();
        let next = after(&odd, "next");
        let mut log = WriterLog::new(W);
        let placement = log.place(
            Vec::new(),
            vec![lines[0].clone(), odd.clone(), next.clone()],
        );
        assert_eq!(placement.placed, [lines[0].hash(), odd.hash(), next.hash()]);
        assert!(matches!(log.entries()[1].kind, EntryKind::Unknown(_)));
    }

    #[test]
    fn a_cached_view_reads_back_as_written() {
        let lines = chain(3);
        let stray = after(&after(&lines[0], "lost"), "held");
        let fork = after(&lines[2], "other");
        let mut log = WriterLog::new(W);
        log.place(
            vec![snapshot(&lines[..2])],
            vec![lines[2].clone(), lines[3].clone(), stray.clone(), fork],
        );
        let view = CachedView {
            writers: BTreeMap::from([(W, log)]),
        };
        let path = Layout::cached_view(lines[0].hash());
        let read = CachedView::decode(&path, &view.encode()).unwrap();
        assert_eq!(read.encode(), view.encode());
        let log = &read.writers()[&W];
        assert_eq!(log.forks(), view.writers()[&W].forks());
        assert!(!log.holds(stray.hash()));
        assert_eq!(log.gaps(), []);
    }

    #[test]
    fn a_cached_view_this_build_cannot_trust_is_corrupt() {
        let lines = chain(2);
        let held = |entries: &[&Line]| {
            let entries: Vec<String> = entries
                .iter()
                .map(|line| format!("{}\t{}", line.json(), line.hash()))
                .collect();
            serde_json::json!({ "writers": { W.to_string(): {
                "snapshots": [], "entries": entries, "strays": [], "forks": [] } } })
            .to_string()
        };
        let path = Layout::cached_view(lines[0].hash());
        assert!(CachedView::decode(&path, held(&[&lines[0], &lines[1]]).as_bytes()).is_ok());
        let flipped = held(&[&lines[0]]).replace("\\\"a\\\"", "\\\"b\\\"");
        for bytes in [
            held(&[&lines[0], &lines[2]]),
            flipped,
            "{}".into(),
            "[".into(),
        ] {
            assert!(
                matches!(
                    CachedView::decode(&path, bytes.as_bytes()),
                    Err(Error::Corrupt { .. })
                ),
                "{bytes}"
            );
        }
    }

    #[test]
    fn a_file_is_judged_by_its_contents() {
        let lines = chain(1);
        let segment: Vec<u8> = lines.iter().flat_map(Line::to_bytes).collect();
        let mut torn = segment.clone();
        torn.extend_from_slice(b"{\"prev\"");
        let snapshot = snapshot(&lines);
        let cases = [
            (
                Vec::new(),
                WriterFile::Segment {
                    lines: Vec::new(),
                    stop: None,
                },
            ),
            (
                segment.clone(),
                WriterFile::Segment {
                    lines: lines.clone(),
                    stop: None,
                },
            ),
            (snapshot.encode(), WriterFile::Snapshot(Box::new(snapshot))),
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
        let WriterFile::Segment { lines: read, stop } = WriterFile::parse(&torn) else {
            panic!("a torn segment is a segment")
        };
        assert_eq!(read, lines);
        assert_eq!(stop.map(|stop| stop.offset), Some(segment.len() as u64));
    }

    #[test]
    fn damaged_files_are_read_without_panicking() {
        use crate::env::{Random, SeededRandom};
        let lines = chain(3);
        let mut log = WriterLog::new(W);
        log.place(vec![snapshot(&lines[..2])], lines[2..].to_vec());
        let view = CachedView {
            writers: BTreeMap::from([(W, log)]),
        };
        let seeds = [
            lines.iter().flat_map(Line::to_bytes).collect::<Vec<u8>>(),
            snapshot(&lines).encode(),
            view.encode(),
        ];
        let path = Layout::cached_view(lines[0].hash());
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
            let mut log = WriterLog::new(W);
            match WriterFile::parse(&bytes) {
                WriterFile::Segment { lines, .. } => log.place(Vec::new(), lines),
                WriterFile::Snapshot(snapshot) => log.place(vec![*snapshot], Vec::new()),
                WriterFile::Unreadable => Placement::default(),
            };
            for head in log.heads() {
                assert!(log.chain_to(head).is_some());
            }
            if let Ok(view) = CachedView::decode(&path, &bytes) {
                assert!(CachedView::decode(&path, &view.encode()).is_ok());
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

    /// A backend that counts reads of file contents.
    struct Counting(MemDisk, usize);

    impl crate::blocking::Backend for Counting {
        fn capabilities(&self, root: Root) -> crate::io::Capabilities {
            self.0.capabilities(root)
        }

        fn perform(&mut self, io: Io) -> crate::io::IoResult {
            self.1 += usize::from(matches!(io, Io::Read { .. }));
            self.0.perform(io)
        }
    }

    #[test]
    fn a_reader_reads_every_file_by_contents_and_writes_nothing() {
        let layout = Layout::new(".lib").unwrap();
        let disk = MemDisk::new();
        let lines = chain(3);
        let dir = layout.writer(W);
        let bytes = |lines: &[Line]| lines.iter().flat_map(Line::to_bytes).collect::<Vec<u8>>();
        put(
            &disk,
            &dir.join("x (conflicted copy).jsonl").unwrap(),
            &bytes(&lines[2..]),
        );
        put(
            &disk,
            &dir.join("notes.txt").unwrap(),
            &snapshot(&lines[..2]).encode(),
        );
        put(&disk, &dir.join("junk").unwrap(), b"junk");
        let mut backend = Counting(disk.clone(), 0);
        let mut reader = Reader::new(layout.clone(), CachedView::default());
        let report = crate::blocking::run(&mut backend, reader.read()).unwrap();
        assert_eq!(report.placed[&W], hashes(&lines[2..]));
        assert_eq!(report.unreadable, [(dir.join("junk").unwrap(), None)]);
        assert_eq!(disk.mutations(), 3 * 2, "only the setup wrote");
        assert_eq!(backend.1, 3);

        let again = crate::blocking::run(&mut backend, reader.read()).unwrap();
        assert_eq!(backend.1, 3, "unchanged files are not read again");
        assert_eq!(again.placed, BTreeMap::new());
        assert_eq!(again.unreadable, report.unreadable);
    }

    #[test]
    fn a_cached_view_saved_in_the_local_root_loads_back() {
        let mut disk = MemDisk::new();
        let lines = chain(2);
        let mut log = WriterLog::new(W);
        log.place(Vec::new(), lines.clone());
        let view = CachedView {
            writers: BTreeMap::from([(W, log)]),
        };
        let genesis = lines[0].hash();
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
        assert_eq!(loaded.encode(), view.encode());
        let mut restarted = disk.restart();
        let loaded = crate::blocking::run(&mut restarted, CachedView::load(genesis)).unwrap();
        assert_eq!(loaded.encode(), view.encode());
    }
}
