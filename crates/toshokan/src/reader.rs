//! Reading writers' logs from the folder and placing their entries by chain.
//!
//! A reader takes every file directly in `writers/<w>/`, whatever its name, and
//! reads it by its contents as segment lines or as a snapshot. It places an entry
//! once its predecessor is placed or folded by a snapshot it has seen; an entry
//! after a gap is held back. Two entries with one predecessor are a fork: both
//! branches are merged and the fork is reported once. What is placed is kept in
//! the cached view, which only grows. Reading writes nothing in the folder.
//!
//! Each line is parsed and decoded once: the reader keeps what it read of each
//! file, reads only the bytes a segment gained since, and shares each decoded
//! entry between the cached view and what the folder holds.

use std::cell::RefCell;
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet, VecDeque};
use std::rc::Rc;

use crate::cache::{self, Store};
use crate::error::{Error, Result};
use crate::flow::{self, Flow};
use crate::ids::{EntryHash, Hlc, WriterId};
use crate::io::{Kind, Meta, Range, Root, Task};
use crate::layout::{is_swap_file, Layout};
use crate::line::{self, Stop};
use crate::log::{Entry, EntryKind};
use crate::merge::Folded;
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

    /// Whether the file gives a reader anything to place.
    fn holds_any(&self) -> bool {
        match self {
            Self::Segment(segment) => !segment.entries.is_empty(),
            Self::Snapshot(_) => true,
            Self::Unreadable => false,
        }
    }
}

/// One writer's history as this install has placed it.
#[derive(Clone, Debug)]
pub struct WriterLog {
    writer: WriterId,
    /// No one extends another.
    snapshots: Vec<Rc<Snapshot>>,
    entries: Vec<Rc<Entry>>,
    /// Every placed or folded entry, with its predecessor. Hashed, since a log
    /// holds every entry of its writer and is looked up a few times per entry;
    /// nothing depends on its order.
    chain: HashMap<EntryHash, Link>,
    /// Entries seen and never placed, with their predecessors, so that a fork with
    /// one of them is found after its file is gone.
    strays: BTreeMap<EntryHash, EntryHash>,
    /// The lines last offered and held back.
    waiting: BTreeMap<EntryHash, Rc<Entry>>,
    /// The first successor seen of each entry, among the chain and the strays.
    next: HashMap<EntryHash, EntryHash>,
    /// Every successor of each entry with more than one.
    branches: BTreeMap<EntryHash, BTreeSet<EntryHash>>,
    forks: Vec<Fork>,
    gaps: Vec<Gap>,
    last_at: Option<Hlc>,
}

/// A placed or folded entry: its predecessor, and itself unless folded.
#[derive(Clone, Debug)]
struct Link {
    prev: EntryHash,
    entry: Option<Rc<Entry>>,
}

/// Two logs are equal when they hold the same snapshots, entries, strays and
/// forks; what they hold back from the last read does not count.
impl PartialEq for WriterLog {
    fn eq(&self, other: &Self) -> bool {
        self.writer == other.writer
            && self.snapshots == other.snapshots
            && self.entries == other.entries
            && self.strays == other.strays
            && self.forks == other.forks
    }
}

/// A placement under way, offered its entries a slice at a time.
struct Growing {
    entries: std::vec::IntoIter<Rc<Entry>>,
    /// Once every entry is offered.
    release: Option<Release>,
    /// Entries offered or released so far.
    worked: usize,
    placement: Placement,
    touched: BTreeSet<EntryHash>,
}

/// Entries held back, by the entry each waits for, and the waited-for entries
/// placed but not yet released.
struct Release {
    after: BTreeMap<EntryHash, Vec<EntryHash>>,
    ready: VecDeque<EntryHash>,
}

/// What [`WriterLog::place`] added.
#[derive(Clone, PartialEq, Debug, Default)]
pub struct Placement {
    /// In placement order: each after its predecessor.
    pub placed: Vec<Rc<Entry>>,
    pub kept: Vec<Rc<Snapshot>>,
    /// Entries held back for the first time, with their predecessors.
    pub strayed: Vec<(EntryHash, EntryHash)>,
    pub forks: Vec<Fork>,
}

impl Placement {
    /// Whether the log grew at all: a snapshot kept, a line placed or held back.
    pub fn changed(&self) -> bool {
        !self.placed.is_empty() || !self.kept.is_empty() || !self.strayed.is_empty()
    }
}

impl WriterLog {
    pub fn new(writer: WriterId) -> Self {
        Self {
            writer,
            snapshots: Vec::new(),
            entries: Vec::new(),
            chain: HashMap::new(),
            strays: BTreeMap::new(),
            waiting: BTreeMap::new(),
            next: HashMap::new(),
            branches: BTreeMap::new(),
            forks: Vec::new(),
            gaps: Vec::new(),
            last_at: None,
        }
    }

    pub fn writer(&self) -> WriterId {
        self.writer
    }

    /// From the genesis entry, once placed or folded.
    pub fn label(&self) -> Option<String> {
        if let Some(snapshot) = self.snapshots.first() {
            return Some(snapshot.label.clone());
        }
        let first = self
            .entries
            .iter()
            .filter(|entry| entry.prev() == EntryHash::ZERO);
        first.map(|entry| entry.kind()).find_map(|kind| match kind {
            EntryKind::Genesis(genesis) => Some(genesis.label),
            _ => None,
        })
    }

    /// The least genesis entry placed or folded; more than one only in a forged
    /// log.
    pub fn genesis(&self) -> Option<EntryHash> {
        self.chain
            .iter()
            .filter(|(_, link)| link.prev == EntryHash::ZERO)
            .map(|(hash, _)| *hash)
            .min()
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
        self.chain.get(&hash).map(|link| link.prev)
    }

    /// The placed entry `hash`; `None` when it is folded or not held.
    pub fn entry(&self, hash: EntryHash) -> Option<&Rc<Entry>> {
        self.chain.get(&hash)?.entry.as_ref()
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
    pub fn snapshots(&self) -> &[Rc<Snapshot>] {
        &self.snapshots
    }

    /// The placed entries no snapshot folds, each after its predecessor; fork
    /// branches in the order of their first entries' hashes.
    pub fn entries(&self) -> &[Rc<Entry>] {
        &self.entries
    }

    /// Entries seen and never placed, with their predecessors.
    pub fn strays(&self) -> &BTreeMap<EntryHash, EntryHash> {
        &self.strays
    }

    /// Placed or folded entries with no placed successor, in order: one, unless
    /// forked.
    pub fn heads(&self) -> Vec<EntryHash> {
        let prevs: BTreeSet<EntryHash> = self.chain.values().map(|link| link.prev).collect();
        let heads = self.chain.keys().filter(|hash| !prevs.contains(hash));
        let heads: BTreeSet<EntryHash> = heads.copied().collect();
        heads.into_iter().collect()
    }

    pub fn last_at(&self) -> Option<Hlc> {
        self.last_at
    }

    /// Every fork found so far. Never shrinks.
    pub fn forks(&self) -> &[Fork] {
        &self.forks
    }

    /// Entries seen in the last read but held back.
    pub fn gaps(&self) -> &[Gap] {
        &self.gaps
    }

    /// Places what `snapshots` fold and every entry whose predecessor is then
    /// held, repeatedly, together with the entries held back before. Entries left
    /// over are the gaps until the next placement. Only grows the log: a snapshot
    /// extended by one already kept is ignored, and one that extends kept ones
    /// replaces them.
    pub fn place(&mut self, snapshots: Vec<Rc<Snapshot>>, entries: Vec<Rc<Entry>>) -> Placement {
        self.grow(snapshots, entries, BTreeSet::new())
    }

    /// Whether placing `snapshots` and `entries` in a new log would make this
    /// one: they hold every entry it holds and no other, its snapshots are
    /// theirs that no other of theirs extends, and it holds nothing back, stray
    /// or forked.
    fn is_placed_from(&self, snapshots: &[Rc<Snapshot>], entries: &[Rc<Entry>]) -> bool {
        let settled = self.strays.is_empty() && self.waiting.is_empty() && self.forks.is_empty();
        if !settled || !self.branches.is_empty() {
            return false;
        }
        let latest = snapshots.iter().filter(|snapshot| {
            let extended = snapshots.iter().any(|other| {
                !Rc::ptr_eq(other, snapshot) && other.extends(snapshot) && !snapshot.extends(other)
            });
            !extended
        });
        let latest: Vec<&Rc<Snapshot>> = latest.collect();
        let same = latest.len() == self.snapshots.len()
            && self
                .snapshots
                .iter()
                .all(|kept| latest.iter().any(|snapshot| Rc::ptr_eq(snapshot, kept)));
        if !same {
            return false;
        }
        let mut held = HashSet::with_capacity(self.chain.len());
        held.extend(
            snapshots
                .iter()
                .flat_map(|snapshot| snapshot.folded.iter().copied()),
        );
        held.extend(entries.iter().map(|entry| entry.hash()));
        held.len() == self.chain.len() && held.iter().all(|hash| self.chain.contains_key(hash))
    }

    /// Whether the log holds nothing: no snapshot, entry, stray or fork, and
    /// nothing held back.
    fn is_empty(&self) -> bool {
        self.snapshots.is_empty()
            && self.chain.is_empty()
            && self.strays.is_empty()
            && self.waiting.is_empty()
            && self.forks.is_empty()
    }

    /// Forgets the entries held back, so that the next placement offers again
    /// only what is held back then.
    fn forget_waiting(&mut self) {
        self.waiting.clear();
        self.gaps.clear();
    }

    /// As [`WriterLog::place`], reporting forks also at the predecessors in
    /// `touched`.
    fn grow(
        &mut self,
        snapshots: Vec<Rc<Snapshot>>,
        entries: Vec<Rc<Entry>>,
        touched: BTreeSet<EntryHash>,
    ) -> Placement {
        let mut growing = self.growing(snapshots, entries, touched);
        self.grow_slice(&mut growing, usize::MAX);
        self.grown(growing)
    }

    /// Starts [`WriterLog::grow`]: keeps the snapshots and offers no entry yet.
    fn growing(
        &mut self,
        snapshots: Vec<Rc<Snapshot>>,
        entries: Vec<Rc<Entry>>,
        mut touched: BTreeSet<EntryHash>,
    ) -> Growing {
        let mut placement = Placement::default();
        for snapshot in snapshots {
            if self.keep(&snapshot) {
                placement.kept.push(snapshot);
            }
        }
        if !placement.kept.is_empty() {
            self.reindex(&mut touched);
        }
        if self.chain.is_empty() {
            self.chain.reserve(entries.len());
            self.next.reserve(entries.len());
        }
        Growing {
            entries: entries.into_iter(),
            release: None,
            worked: 0,
            placement,
            touched,
        }
    }

    /// Offers the next entries in order, then places those held back whose
    /// predecessors are placed, at most `slice` in all; true once nothing is
    /// left to place.
    fn grow_slice(&mut self, growing: &mut Growing, slice: usize) -> bool {
        let Growing {
            entries,
            release,
            worked,
            placement,
            touched,
        } = growing;
        let mut budget = slice;
        for entry in entries.by_ref().take(slice) {
            budget -= 1;
            *worked += 1;
            let (hash, prev) = (entry.hash(), entry.prev());
            if self.holds(hash) {
                continue;
            }
            if prev != EntryHash::ZERO && !self.holds(prev) {
                self.waiting.insert(hash, entry);
                continue;
            }
            self.waiting.remove(&hash);
            self.strays.remove(&hash);
            self.link(hash, prev, Some(Rc::clone(&entry)), touched);
            placement.placed.push(entry);
        }
        if entries.len() > 0 {
            return false;
        }
        let release = release.get_or_insert_with(|| self.held_back());
        while budget > 0 {
            let Some(prev) = release.ready.pop_front() else {
                return true;
            };
            for hash in release.after.remove(&prev).unwrap_or_default() {
                let entry = self
                    .waiting
                    .remove(&hash)
                    .expect("each waiting entry is placed once");
                self.strays.remove(&hash);
                self.link(hash, prev, Some(Rc::clone(&entry)), touched);
                placement.placed.push(entry);
                release.ready.push_back(hash);
                budget = budget.saturating_sub(1);
                *worked += 1;
            }
        }
        release.ready.is_empty()
    }

    /// The entries held back by the one each waits for, and the predecessors
    /// placed already.
    fn held_back(&self) -> Release {
        let mut after: BTreeMap<EntryHash, Vec<EntryHash>> = BTreeMap::new();
        for (hash, entry) in &self.waiting {
            after.entry(entry.prev()).or_default().push(*hash);
        }
        let ready = after
            .keys()
            .filter(|prev| **prev == EntryHash::ZERO || self.holds(**prev))
            .copied()
            .collect();
        Release { after, ready }
    }

    /// ⚠️ Before [`WriterLog::grow_slice`] returns true, entries offered are left
    /// held back.
    fn grown(&mut self, growing: Growing) -> Placement {
        let Growing {
            mut placement,
            mut touched,
            ..
        } = growing;
        let held: Vec<(EntryHash, EntryHash)> = self
            .waiting
            .iter()
            .map(|(hash, entry)| (*hash, entry.prev()))
            .collect();
        for (hash, prev) in held {
            if self.strays.insert(hash, prev).is_none() {
                self.note(hash, prev, &mut touched);
                placement.strayed.push((hash, prev));
            }
        }
        self.gaps = gaps(self.writer, &self.waiting);
        self.append(&placement.placed);
        if !placement.kept.is_empty() {
            self.last_at = self.readings().max();
        }
        placement.forks = self.forks_at(touched);
        placement
    }

    /// Places everything `other`, a log of the same writer, holds: its
    /// snapshots, entries, strays and forks.
    fn join(&mut self, other: &WriterLog) -> Placement {
        self.restore(
            other.snapshots.clone(),
            other.entries.clone(),
            other.strays.iter().map(|(hash, prev)| (*hash, *prev)),
            other.forks.iter().map(|fork| (fork.prev, fork.branches)),
        )
    }

    /// Places what a stored view of this writer holds: its strays and forks
    /// first, so that a fork it reported is not reported again.
    fn restore(
        &mut self,
        snapshots: Vec<Rc<Snapshot>>,
        entries: Vec<Rc<Entry>>,
        strays: impl IntoIterator<Item = (EntryHash, EntryHash)>,
        forks: impl IntoIterator<Item = (EntryHash, [EntryHash; 2])>,
    ) -> Placement {
        let mut touched = BTreeSet::new();
        let mut strayed = Vec::new();
        for (hash, prev) in strays {
            if !self.holds(hash) && self.strays.insert(hash, prev).is_none() {
                self.note(hash, prev, &mut touched);
                strayed.push((hash, prev));
            }
        }
        let mut reported = Vec::new();
        for (prev, branches) in forks {
            if !self.forks.iter().any(|fork| fork.prev == prev) {
                let fork = Fork {
                    writer: self.writer,
                    prev,
                    branches,
                };
                self.forks.push(fork);
                reported.push(fork);
            }
        }
        let mut placement = self.grow(snapshots, entries, touched);
        strayed.retain(|(hash, _)| self.strays.contains_key(hash));
        placement.strayed.extend(strayed);
        placement.forks.extend(reported);
        placement
    }

    /// Every clock reading of the placed entries and the snapshots.
    pub fn readings(&self) -> impl Iterator<Item = Hlc> + '_ {
        let entries = self.entries.iter().map(|entry| entry.at());
        entries.chain(self.snapshots.iter().map(|snapshot| snapshot.at))
    }

    fn keep(&mut self, snapshot: &Rc<Snapshot>) -> bool {
        if self.snapshots.iter().any(|kept| kept.extends(snapshot)) {
            return false;
        }
        self.snapshots.retain(|kept| !snapshot.extends(kept));
        self.snapshots.push(Rc::clone(snapshot));
        self.snapshots.sort_by_key(|snapshot| snapshot.head());
        true
    }

    /// Folds into the chain what the kept snapshots fold, and drops the entries
    /// they fold.
    fn reindex(&mut self, touched: &mut BTreeSet<EntryHash>) {
        let mut folded = BTreeSet::new();
        for snapshot in self.snapshots.clone() {
            for (hash, prev) in snapshot.pairs() {
                folded.insert(hash);
                match self.chain.get_mut(&hash) {
                    Some(link) => link.entry = None,
                    None => self.link(hash, prev, None, touched),
                }
            }
        }
        self.entries.retain(|entry| !folded.contains(&entry.hash()));
        let chain = &self.chain;
        self.strays.retain(|hash, _| !chain.contains_key(hash));
        self.waiting.retain(|hash, _| !chain.contains_key(hash));
    }

    fn link(
        &mut self,
        hash: EntryHash,
        prev: EntryHash,
        entry: Option<Rc<Entry>>,
        touched: &mut BTreeSet<EntryHash>,
    ) {
        if let Some(entry) = &entry {
            self.last_at = self.last_at.max(Some(entry.at()));
        }
        self.chain.insert(hash, Link { prev, entry });
        self.note(hash, prev, touched);
    }

    /// Records that `hash` follows `prev`; a second successor makes `prev` a
    /// fork, reported once the placement ends.
    fn note(&mut self, hash: EntryHash, prev: EntryHash, touched: &mut BTreeSet<EntryHash>) {
        let first = *self.next.entry(prev).or_insert(hash);
        if first != hash {
            let branches = self.branches.entry(prev).or_default();
            branches.insert(first);
            branches.insert(hash);
            touched.insert(prev);
        }
    }

    /// The forks at `touched` not reported before, each with its two least
    /// branches, now reported.
    fn forks_at(&mut self, touched: BTreeSet<EntryHash>) -> Vec<Fork> {
        let mut found = Vec::new();
        for prev in touched {
            if self.forks.iter().any(|fork| fork.prev == prev) {
                continue;
            }
            let Some(branches) = self.branches.get(&prev) else {
                continue;
            };
            let mut least = branches.iter().copied();
            let branches = [least.next(), least.next()].map(|hash| hash.expect("two branches"));
            found.push(Fork {
                writer: self.writer,
                prev,
                branches,
            });
        }
        self.forks.extend(&found);
        found
    }

    /// Adds `placed` to the entries, each after its predecessor. Entries that
    /// continue the last one are appended; anything else orders them all again.
    fn append(&mut self, placed: &[Rc<Entry>]) {
        let mut last = self.entries.last().map(|entry| entry.hash());
        let continues = placed.iter().all(|entry| {
            let follows = last.is_none_or(|last| entry.prev() == last);
            last = Some(entry.hash());
            follows
        });
        self.entries.extend(placed.iter().cloned());
        if !continues {
            self.order();
        }
    }

    fn order(&mut self) {
        let placed: BTreeSet<EntryHash> = self.entries.iter().map(|entry| entry.hash()).collect();
        let mut after: BTreeMap<EntryHash, Vec<Rc<Entry>>> = BTreeMap::new();
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
}

/// Held entries grouped by the missing entry each waits for.
fn gaps(writer: WriterId, waiting: &BTreeMap<EntryHash, Rc<Entry>>) -> Vec<Gap> {
    let mut missing_of: BTreeMap<EntryHash, EntryHash> = BTreeMap::new();
    for &start in waiting.keys() {
        let mut path = Vec::new();
        let mut at = start;
        let missing = loop {
            if let Some(&missing) = missing_of.get(&at) {
                break missing;
            }
            match waiting.get(&at) {
                Some(entry) if path.len() <= waiting.len() => {
                    path.push(at);
                    at = entry.prev();
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

    pub fn writers(&self) -> &BTreeMap<WriterId, WriterLog> {
        &self.writers
    }

    pub(crate) fn log_mut(&mut self, writer: WriterId) -> &mut WriterLog {
        self.writers
            .entry(writer)
            .or_insert_with(|| WriterLog::new(writer))
    }

    /// Places a stored part of `writer`'s log; refuses one that holds an entry
    /// whose predecessor the view does not hold.
    pub(crate) fn restore(
        &mut self,
        writer: WriterId,
        snapshots: Vec<Rc<Snapshot>>,
        entries: Vec<Rc<Entry>>,
        strays: Vec<(EntryHash, EntryHash)>,
        forks: Vec<(EntryHash, [EntryHash; 2])>,
    ) -> std::result::Result<(), String> {
        let log = self.log_mut(writer);
        log.restore(snapshots, entries, strays, forks);
        match log.waiting.is_empty() {
            true => Ok(()),
            false => {
                log.forget_waiting();
                Err(format!("an entry of {writer} follows one it does not hold"))
            }
        }
    }

    /// The cached view kept for the writer whose genesis entry is `genesis`; empty
    /// when none was saved.
    pub fn load(genesis: EntryHash) -> Task<'static, Result<Self>> {
        cache::load_one(genesis).map_ok(|kept| kept.view).task()
    }

    /// Adds everything `other` holds; whether this view grew.
    pub fn join(&mut self, other: &CachedView) -> bool {
        let mut grew = false;
        for (writer, log) in &other.writers {
            let placement = self.log_mut(*writer).join(log);
            grew |= placement.changed() || !placement.forks.is_empty();
        }
        grew
    }

    /// Keeps this view alone for the writer whose genesis entry is `genesis`,
    /// replacing what was kept so that a crash leaves the old view or the new.
    pub fn save(&self, genesis: EntryHash) -> Task<'static, Result<()>> {
        cache::write_checkpoint(genesis, self.encode(), Rc::default()).task()
    }
}

/// What one read placed and found.
#[derive(Clone, PartialEq, Eq, Debug, Default)]
pub struct ReadReport {
    /// Whether the cached view grew.
    pub changed: bool,
    /// Whether any file in a writer's directory came, went or changed.
    pub folder_changed: bool,
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

impl ReadReport {
    /// Whether the read found anything new: the cached view grew, or a file in a
    /// writer's directory came, went or changed.
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
    /// Each writer's placed and folded entries no file in the folder held at the
    /// last read; writers with none are left out.
    removed: BTreeMap<WriterId, BTreeSet<EntryHash>>,
    /// What the cached view gained since it was last saved.
    pub(crate) unsaved: Unsaved,
    /// The retired writers of this install whose cached views this one holds.
    pub(crate) absorbed: BTreeSet<EntryHash>,
    pub(crate) store: Rc<RefCell<Store>>,
}

/// One writer's directory as the last read found it.
#[derive(Clone)]
pub(crate) struct Folder {
    pub(crate) files: BTreeMap<RelPath, Record>,
    /// What the files hold, placed without the cached view.
    log: WriterLog,
    /// Whether `log`, what the cached view holds back of this writer and the
    /// entries the folder lost must be found again from every file at the next
    /// read: until the first read, and after records loaded from the local root.
    stale: bool,
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
    pub(crate) logs: BTreeMap<WriterId, Placement>,
    /// Files whose records came, went or changed.
    pub(crate) files: BTreeSet<RelPath>,
}

impl Unsaved {
    fn add(&mut self, writer: WriterId, placement: &Placement) {
        if !placement.changed() && placement.forks.is_empty() {
            return;
        }
        let kept = self.logs.entry(writer).or_default();
        kept.placed.extend(placement.placed.iter().cloned());
        kept.kept.extend(placement.kept.iter().cloned());
        kept.strayed.extend(placement.strayed.iter().copied());
        kept.forks.extend(placement.forks.iter().copied());
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.logs.is_empty() && self.files.is_empty()
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

/// A read being placed by [`Reader::absorb_slice`], a slice of entries at a time.
pub(crate) struct Absorbing {
    listed: std::vec::IntoIter<Listed>,
    placing: Option<Placing>,
    report: ReadReport,
}

/// One writer's entries being placed: first in what the folder holds of it,
/// then in the cached view.
struct Placing {
    writer: WriterId,
    /// Whether what the folder holds of the writer is placed again from every
    /// file.
    whole: bool,
    stage: Stage,
    growing: Growing,
}

enum Stage {
    /// What the folder holds; the cached view is offered these next.
    Folder {
        snapshots: Vec<Rc<Snapshot>>,
        entries: Vec<Rc<Entry>>,
    },
    Cached,
}

impl Reader {
    pub fn new(layout: Layout, cached: CachedView) -> Self {
        Self {
            layout,
            cached,
            folder: BTreeMap::new(),
            removed: BTreeMap::new(),
            unsaved: Unsaved::default(),
            absorbed: BTreeSet::new(),
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
            store,
            state: _,
        } = loaded;
        let mut reader = Self::new(layout, view);
        reader.absorbed = absorbed;
        reader.store = Rc::new(RefCell::new(store));
        for (path, record) in files {
            let Some(writer) = reader.writer_of(&path) else {
                continue;
            };
            let Some(log) = reader.cached.writers.get(&writer) else {
                continue;
            };
            let Some(record) = record.resolve(log) else {
                continue;
            };
            let folder = reader
                .folder
                .entry(writer)
                .or_insert_with(|| Folder::new(writer));
            folder.files.insert(path, record);
        }
        reader
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

    pub fn logs(&self) -> &BTreeMap<WriterId, WriterLog> {
        &self.cached.writers
    }

    /// What `writer`'s files in the folder held at the last read, placed without
    /// the cached view; `None` when the folder held no file of it.
    pub fn folder_log(&self, writer: WriterId) -> Option<&WriterLog> {
        self.folder.get(&writer).map(|folder| &folder.log)
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

    /// Each writer's placed and folded entries that no file in the folder held at
    /// the last read: a restore took them, or sync has not yet brought the files
    /// that hold them now. Writers with none are left out.
    pub fn removed(&self) -> BTreeMap<WriterId, BTreeSet<EntryHash>> {
        self.removed.clone()
    }

    /// Reads every writer's directory and places what it can. Requests only
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
                    scan_writer(&layout, writer, Rc::clone(&known)).map_ok(move |listed| {
                        done.push(listed);
                        done
                    })
                })
            })
            .map_ok(move |writers| Listing { writers, everyone })
            .task()
    }

    /// Places entries this instance appended as `writer`, as a read would, and
    /// records `written`, the segment the append ended and its stamp, as a read
    /// would find it.
    pub fn add(
        &mut self,
        writer: WriterId,
        entries: &[Entry],
        written: Option<(RelPath, Stamp)>,
    ) -> Placement {
        let entries: Vec<Rc<Entry>> = entries.iter().cloned().map(Rc::new).collect();
        let folder = self
            .folder
            .entry(writer)
            .or_insert_with(|| Folder::new(writer));
        folder.log.place(Vec::new(), entries.clone());
        if let Some((path, stamp)) = written {
            if folder.appended(&path, &entries, stamp) {
                self.unsaved.files.insert(path);
            }
        }
        let placement = self.cached.log_mut(writer).place(Vec::new(), entries);
        self.unsaved.add(writer, &placement);
        self.drop_unsaved_without_store();
        placement
    }

    /// A view not yet saved anywhere is written whole by its first save, so what
    /// it gains is not kept apart.
    fn drop_unsaved_without_store(&mut self) {
        if self.store.borrow().genesis.is_none() {
            self.unsaved = Unsaved::default();
        }
    }

    /// Places what [`Reader::list`] or [`Reader::list_writer`] read.
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
            let absent: Vec<WriterId> = self
                .cached
                .writers
                .keys()
                .filter(|writer| !present.contains(writer))
                .copied()
                .collect();
            for writer in absent {
                self.cached.log_mut(writer).forget_waiting();
                self.recount(writer, true);
            }
        }
        Absorbing {
            listed: listed.into_iter(),
            placing: None,
            report,
        }
    }

    /// Places about `slice` more entries; true once every writer's are placed.
    pub(crate) fn absorb_slice(&mut self, absorbing: &mut Absorbing, slice: usize) -> bool {
        let mut budget = slice;
        while budget > 0 {
            let mut placing = match absorbing.placing.take() {
                Some(placing) => placing,
                None => match absorbing.listed.next() {
                    Some(listed) => self.gather(listed, &mut absorbing.report),
                    None => return true,
                },
            };
            let log = match placing.stage {
                Stage::Folder { .. } => {
                    &mut self.folder.get_mut(&placing.writer).expect("gathered").log
                }
                Stage::Cached => self.cached.log_mut(placing.writer),
            };
            let worked = placing.growing.worked;
            let all = log.grow_slice(&mut placing.growing, budget);
            budget = budget.saturating_sub(placing.growing.worked - worked);
            if !all {
                absorbing.placing = Some(placing);
                return false;
            }
            absorbing.placing = self.placed(placing, &mut absorbing.report);
        }
        absorbing.placing.is_none() && absorbing.listed.len() == 0
    }

    /// Ends [`Reader::absorb`] once [`Reader::absorb_slice`] returned true.
    pub(crate) fn absorbed(&mut self, absorbing: Absorbing) -> ReadReport {
        self.drop_unsaved_without_store();
        absorbing.report
    }

    /// Takes in what one writer's files gave, and starts placing it in what the
    /// folder holds of the writer.
    fn gather(&mut self, listed: Listed, report: &mut ReadReport) -> Placing {
        let Listed { writer, files } = listed;
        let folder = self
            .folder
            .entry(writer)
            .or_insert_with(|| Folder::new(writer));
        let mut whole = std::mem::take(&mut folder.stale);
        let present: BTreeSet<&RelPath> = files.iter().map(|file| &file.path).collect();
        let gone: Vec<RelPath> = folder
            .files
            .keys()
            .filter(|path| !present.contains(path))
            .cloned()
            .collect();
        for path in gone {
            let record = folder.files.remove(&path).expect("listed above");
            whole |= record.file.holds_any();
            self.unsaved.files.insert(path);
            report.folder_changed = true;
        }
        let (mut snapshots, mut entries) = (Vec::new(), Vec::new());
        for Scanned { path, found } in files {
            match found {
                Found::Unchanged => continue,
                Found::Grown { bytes, stamp } => {
                    let record = folder
                        .files
                        .get_mut(&path)
                        .expect("only a known file grows");
                    if let WriterFile::Segment(segment) = &mut record.file {
                        entries.extend(segment.extend(&bytes).iter().cloned());
                    }
                    record.stamp = Some(stamp);
                }
                Found::Read { file, stamp } => {
                    match &file {
                        WriterFile::Segment(segment) => {
                            entries.extend(segment.entries.iter().cloned());
                        }
                        WriterFile::Snapshot(snapshot) if snapshot.writer == writer => {
                            snapshots.push(Rc::clone(snapshot));
                        }
                        WriterFile::Snapshot(_) | WriterFile::Unreadable => {}
                    }
                    let old = folder.files.insert(path.clone(), Record { stamp, file });
                    whole |= old.is_some_and(|old| old.file.holds_any());
                }
            }
            self.unsaved.files.insert(path);
            report.folder_changed = true;
        }
        if whole {
            (snapshots, entries) = folder.contents(writer);
            let cached = self.cached.writers.get(&writer);
            folder.log = match cached.filter(|log| log.is_placed_from(&snapshots, &entries)) {
                Some(log) => log.clone(),
                None => WriterLog::new(writer),
            };
        }
        let growing = folder
            .log
            .growing(snapshots.clone(), entries.clone(), BTreeSet::new());
        Placing {
            writer,
            whole,
            stage: Stage::Folder { snapshots, entries },
            growing,
        }
    }

    /// Ends a placement [`Reader::gather`] started: in what the folder holds, then
    /// in the cached view. The placement in the cached view to go on with, if any.
    fn placed(&mut self, placing: Placing, report: &mut ReadReport) -> Option<Placing> {
        let Placing {
            writer,
            whole,
            stage,
            growing,
        } = placing;
        match stage {
            Stage::Folder { snapshots, entries } => {
                let folder = self.folder.get_mut(&writer).expect("gathered");
                let in_folder = folder.log.grown(growing);
                self.report_unreadable(writer, report);
                let folder = &self.folder[&writer].log;
                let log = self.cached.log_mut(writer);
                if whole {
                    log.forget_waiting();
                }
                // A view holding nothing of the writer places what the folder's log
                // placed from nothing, and ends as that log does.
                if whole && log.is_empty() {
                    *log = folder.clone();
                    self.took(writer, &in_folder, report);
                } else if whole || !snapshots.is_empty() || !entries.is_empty() {
                    let growing = log.growing(snapshots, entries, BTreeSet::new());
                    let stage = Stage::Cached;
                    return Some(Placing {
                        writer,
                        whole,
                        stage,
                        growing,
                    });
                }
            }
            Stage::Cached => {
                let placement = self.cached.log_mut(writer).grown(growing);
                self.took(writer, &placement, report);
            }
        }
        report
            .gaps
            .extend(self.cached.log_mut(writer).gaps.iter().copied());
        self.recount(writer, whole);
        None
    }

    /// Reports what `placement` placed in the cached view of `writer`, and keeps
    /// it to save.
    fn took(&mut self, writer: WriterId, placement: &Placement, report: &mut ReadReport) {
        report.changed |= placement.changed();
        if !placement.placed.is_empty() {
            let placed = placement.placed.iter().map(|entry| entry.hash()).collect();
            report.placed.insert(writer, placed);
        }
        report.forks.extend(placement.forks.iter().copied());
        self.unsaved.add(writer, placement);
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

    /// Brings [`Reader::removed`] up to date for `writer`: from scratch after its
    /// files lost anything, else by dropping what its files now hold.
    fn recount(&mut self, writer: WriterId, whole: bool) {
        let folder = self.folder.get(&writer).map(|folder| &folder.log);
        let saw = |hash: &EntryHash| folder.is_some_and(|folder| folder.saw(*hash));
        let gone: BTreeSet<EntryHash> = match (whole, self.removed.remove(&writer)) {
            (false, Some(mut gone)) => {
                gone.retain(|hash| !saw(hash));
                gone
            }
            (false, None) => BTreeSet::new(),
            (true, _) => self
                .cached
                .writers
                .get(&writer)
                .map(|log| {
                    log.chain
                        .keys()
                        .filter(|hash| !saw(hash))
                        .copied()
                        .collect()
                })
                .unwrap_or_default(),
        };
        if !gone.is_empty() {
            self.removed.insert(writer, gone);
        }
    }

    /// Keeps what the cached view gained in this writer's local directory: by
    /// appending it to the journal, or by writing the whole view again when the
    /// journal has grown too long or cannot be trusted. `state`, when given, is
    /// the merged state of every log of the view, kept with it when it is written
    /// whole.
    pub fn save(
        &mut self,
        genesis: EntryHash,
        state: Option<&Folded>,
    ) -> Task<'static, Result<()>> {
        cache::save(self, genesis, state)
    }

    /// Writes the whole view, with what the reader knows of each file and
    /// `state`, as the writer whose genesis entry is `genesis`.
    pub fn checkpoint(
        &mut self,
        genesis: EntryHash,
        state: Option<&Folded>,
    ) -> Task<'static, Result<()>> {
        cache::checkpoint(self, genesis, state)
    }
}

impl Folder {
    fn new(writer: WriterId) -> Self {
        Self {
            files: BTreeMap::new(),
            log: WriterLog::new(writer),
            stale: true,
        }
    }

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

    /// Every snapshot of `writer` and every entry the files hold.
    fn contents(&self, writer: WriterId) -> (Vec<Rc<Snapshot>>, Vec<Rc<Entry>>) {
        let (mut snapshots, mut entries) = (Vec::new(), Vec::new());
        for record in self.files.values() {
            match &record.file {
                WriterFile::Segment(segment) => entries.extend(segment.entries.iter().cloned()),
                WriterFile::Snapshot(snapshot) if snapshot.writer == writer => {
                    snapshots.push(Rc::clone(snapshot));
                }
                WriterFile::Snapshot(_) | WriterFile::Unreadable => {}
            }
        }
        (snapshots, entries)
    }
}

fn scan_writer<'a>(
    layout: &Layout,
    writer: WriterId,
    known: Rc<BTreeMap<RelPath, Known>>,
) -> Flow<'a, Result<Listed>> {
    let dir = layout.writer(writer);
    flow::list_stat(Root::Folder, &dir)
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
    use crate::disk::MemDisk;
    use crate::io::Io;
    use crate::line::Line;
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
        Entry::encode(EntryHash::ZERO, at(0), kind).unwrap().line()
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
            .line()
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

    fn snapshot(lines: &[Line]) -> Rc<Snapshot> {
        Rc::new(Snapshot {
            writer: W,
            label: "a".into(),
            at: at(9),
            folded: lines.iter().map(Line::hash).collect(),
            state: Folded::default(),
            unknown: BTreeMap::new(),
        })
    }

    fn entries(lines: &[Line]) -> Vec<Rc<Entry>> {
        lines
            .iter()
            .cloned()
            .map(|line| Rc::new(Entry::linked(line)))
            .collect()
    }

    fn placed(placement: &Placement) -> Vec<EntryHash> {
        placement.placed.iter().map(|entry| entry.hash()).collect()
    }

    fn hashes(lines: &[Line]) -> Vec<EntryHash> {
        lines.iter().map(Line::hash).collect()
    }

    fn entry_hashes(log: &WriterLog) -> Vec<EntryHash> {
        log.entries().iter().map(|entry| entry.hash()).collect()
    }

    #[test]
    fn entries_arriving_in_any_order_are_placed_in_chain_order() {
        let lines = chain(3);
        let mut log = WriterLog::new(W);
        let first = log.place(Vec::new(), entries(&[lines[3].clone(), lines[2].clone()]));
        assert_eq!(placed(&first), []);
        assert_eq!(
            log.gaps(),
            [Gap {
                writer: W,
                missing: lines[1].hash(),
                held: 2
            }]
        );
        let reversed: Vec<Line> = lines.iter().rev().cloned().collect();
        let second = log.place(Vec::new(), entries(&reversed));
        assert_eq!(placed(&second), hashes(&lines));
        assert_eq!(entry_hashes(&log), hashes(&lines));
        assert_eq!(log.gaps(), []);
        assert_eq!(log.heads(), [lines[3].hash()]);
        assert_eq!(log.genesis(), Some(lines[0].hash()));
        assert_eq!(log.label().as_deref(), Some("a"));
        assert_eq!(log.chain_to(lines[2].hash()), Some(hashes(&lines[..3])));
    }

    #[test]
    fn two_entries_after_one_are_a_fork_reported_once() {
        let lines = chain(1);
        let (a, b) = (after(&lines[1], "a"), after(&lines[1], "b"));
        let mut log = WriterLog::new(W);
        let first = log.place(
            Vec::new(),
            entries(&[lines[0].clone(), lines[1].clone(), a.clone()]),
        );
        assert_eq!(first.forks, []);
        let second = log.place(Vec::new(), entries(std::slice::from_ref(&b)));
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
        let third = log.place(
            Vec::new(),
            entries(&[after(&lines[1], "c"), after(&a, "d")]),
        );
        assert_eq!(third.forks, []);
        assert_eq!(log.forks(), [fork]);
    }

    #[test]
    fn a_fork_between_held_entries_is_found_after_their_file_is_gone() {
        let lines = chain(1);
        let (a, b) = (after(&lines[1], "a"), after(&lines[1], "b"));
        let mut log = WriterLog::new(W);
        log.place(Vec::new(), entries(std::slice::from_ref(&a)));
        let found = log.place(Vec::new(), entries(std::slice::from_ref(&b)));
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
        let placement = log.place(vec![snapshot(&lines)], entries(std::slice::from_ref(&b)));
        assert_eq!(placed(&placement), [b.hash()]);
        assert_eq!(placement.forks.len(), 1);
        assert_eq!(placement.forks[0].prev, lines[1].hash());
        assert!(lines.iter().all(|line| log.holds(line.hash())));
        assert_eq!(log.label().as_deref(), Some("a"));
    }

    #[test]
    fn a_snapshot_replaces_the_entries_it_folds_and_only_grows() {
        let lines = chain(4);
        let mut log = WriterLog::new(W);
        log.place(vec![snapshot(&lines[..2])], entries(&lines));
        assert_eq!(entry_hashes(&log), hashes(&lines[2..]));
        let grown = log.place(vec![snapshot(&lines[..4])], Vec::new());
        assert!(grown.changed(), "a snapshot alone grows the log");
        assert_eq!(placed(&grown), []);
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
    fn a_log_is_placed_from_exactly_what_it_holds() {
        let lines = chain(4);
        let snapshots = vec![snapshot(&lines[..2])];
        let held = entries(&lines[2..]);
        let mut log = WriterLog::new(W);
        log.place(snapshots.clone(), held.clone());
        assert!(log.is_placed_from(&snapshots, &held));
        let older = snapshot(&lines[..1]);
        let with_older = vec![Rc::clone(&older), Rc::clone(&snapshots[0])];
        assert!(
            log.is_placed_from(&with_older, &held),
            "an older snapshot is extended"
        );
        let mut fresh = WriterLog::new(W);
        fresh.place(with_older, held.clone());
        assert_eq!(fresh, log);
        assert_eq!(fresh.chain.len(), log.chain.len());
        assert_eq!(fresh.heads(), log.heads());

        let fork = entries(&[after(&lines[2], "other")]);
        let stray = entries(&[after(&after(&lines[0], "lost"), "held")]);
        let copy = vec![snapshot(&lines[..2])];
        let lacking = &held[..1];
        let more: Vec<Rc<Entry>> = held.iter().chain(&fork).cloned().collect();
        for (case, snapshots, held) in [
            ("an entry the files lack", &snapshots, lacking),
            ("an entry the log lacks", &snapshots, &more[..]),
            ("a snapshot read again", &copy, &held[..]),
        ] {
            assert!(!log.is_placed_from(snapshots, held), "{case}");
        }
        for (case, extra) in [("a fork", fork), ("a stray", stray)] {
            let mut grown = log.clone();
            grown.place(Vec::new(), extra.clone());
            let all: Vec<Rc<Entry>> = held.iter().chain(&extra).cloned().collect();
            assert!(!grown.is_placed_from(&snapshots, &all), "{case}");
        }
    }

    #[test]
    fn a_line_that_is_not_an_entry_still_links_the_chain() {
        let lines = chain(0);
        let odd = Line::seal(format!(r#"{{"prev":"{}"}}"#, lines[0].hash())).unwrap();
        let next = after(&odd, "next");
        let mut log = WriterLog::new(W);
        let placement = log.place(
            Vec::new(),
            entries(&[lines[0].clone(), odd.clone(), next.clone()]),
        );
        assert_eq!(
            placed(&placement),
            [lines[0].hash(), odd.hash(), next.hash()]
        );
        assert!(matches!(log.entries()[1].kind(), EntryKind::Unknown(_)));
    }

    #[test]
    fn a_cached_view_reads_back_as_written() {
        let lines = chain(3);
        let stray = after(&after(&lines[0], "lost"), "held");
        let fork = after(&lines[2], "other");
        let mut log = WriterLog::new(W);
        log.place(
            vec![snapshot(&lines[..2])],
            entries(&[lines[2].clone(), lines[3].clone(), stray.clone(), fork]),
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
    fn a_file_is_judged_by_its_contents() {
        let lines = chain(1);
        let segment: Vec<u8> = lines.iter().flat_map(Line::to_bytes).collect();
        let mut torn = segment.clone();
        torn.extend_from_slice(b"{\"prev\"");
        let snapshot = snapshot(&lines);
        let cases = [
            (Vec::new(), WriterFile::Segment(Segment::default())),
            (
                segment.clone(),
                WriterFile::Segment(Segment {
                    entries: entries(&lines),
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
        assert_eq!(read.entries, entries(&lines));
        assert_eq!(
            read.stop.map(|stop| stop.offset),
            Some(segment.len() as u64)
        );
    }

    #[test]
    fn damaged_files_are_read_without_panicking() {
        use crate::env::{Random, SeededRandom};
        let lines = chain(3);
        let mut log = WriterLog::new(W);
        log.place(vec![snapshot(&lines[..2])], entries(&lines[2..]));
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
                WriterFile::Segment(segment) => log.place(Vec::new(), segment.entries),
                WriterFile::Snapshot(snapshot) => log.place(vec![snapshot], Vec::new()),
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

    #[test]
    fn a_file_parsed_in_slices_is_judged_as_parsed_whole() {
        use crate::env::{Random, SeededRandom};
        let lines = chain(6);
        let segment: Vec<u8> = lines.iter().flat_map(Line::to_bytes).collect();
        let mut sealed = segment.clone();
        sealed.extend(line::seal_marker(lines[6].hash()));
        let seeds = [segment, sealed, snapshot(&lines).encode()];
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
                    3 => bytes.extend(line::seal_marker(lines[pick(7)].hash())),
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

    fn bytes_of(lines: &[&Line]) -> Vec<u8> {
        lines.iter().flat_map(|line| line.to_bytes()).collect()
    }

    /// What a reader holds that a read changes, comparable.
    fn state(reader: &Reader) -> impl PartialEq + std::fmt::Debug {
        let logs = |log: &WriterLog| {
            let waiting: Vec<EntryHash> = log.waiting.keys().copied().collect();
            (log.clone(), waiting, log.gaps.clone(), log.chain.len())
        };
        let cached: Vec<_> = reader.cached.writers.values().map(logs).collect();
        let folder: Vec<_> = reader.folder.values().map(|f| logs(&f.log)).collect();
        let files: Vec<_> = reader
            .records()
            .map(|(path, record)| (path.clone(), record.stamp.clone(), record.file.clone()))
            .collect();
        let unsaved = (reader.unsaved.logs.clone(), reader.unsaved.files.clone());
        (cached, folder, files, reader.removed(), unsaved)
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
                let genesis = EntryKind::Genesis(Genesis {
                    writer,
                    label: "w".into(),
                });
                let mut lines = vec![Entry::encode(EntryHash::ZERO, at(0), genesis).unwrap().line()];
                for n in 1..pick(30) {
                    let prev = &lines[pick(lines.len()).max(n.saturating_sub(2)).min(n - 1)];
                    lines.push(after(prev, &format!("{w}-{n}")));
                }
                let mut order: Vec<&Line> = lines.iter().collect();
                for i in (1..order.len()).rev() {
                    order.swap(i, pick(i + 1));
                }
                let dir = layout.writer(writer);
                for (i, part) in order.chunks(1 + pick(6)).enumerate() {
                    files.push((dir.join(&format!("s{i}.txt")).unwrap(), bytes_of(part)));
                }
                if pick(3) == 0 {
                    let mut snapshot = (*snapshot(&lines[..1 + pick(lines.len())])).clone();
                    snapshot.writer = writer;
                    files.push((dir.join("snap.json").unwrap(), snapshot.encode()));
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

    fn grow(disk: &MemDisk, path: &RelPath, bytes: &[u8]) {
        let append = Io::Append {
            root: Root::Folder,
            path: path.clone(),
            bytes: bytes.to_vec(),
        };
        disk.perform(append).unwrap();
    }

    #[test]
    fn a_grown_segment_is_read_from_the_end_of_its_last_line() {
        let layout = Layout::new(".lib").unwrap();
        let disk = MemDisk::new();
        let lines = chain(3);
        let path = layout.writer(W).join("s.jsonl").unwrap();
        put(&disk, &path, &bytes_of(&[&lines[0], &lines[1]]));
        let mut backend = Counting(disk.clone(), 0);
        let mut reader = Reader::new(layout, CachedView::default());
        crate::blocking::run(&mut backend, reader.read()).unwrap();
        let before = backend.1;
        let gained = bytes_of(&[&lines[2], &lines[3]]);
        grow(&disk, &path, &gained);

        let report = crate::blocking::run(&mut backend, reader.read()).unwrap();
        assert_eq!(report.placed[&W], hashes(&lines[2..]));
        assert_eq!(backend.1 - before, line::ENDING + gained.len() as u64);
        assert_eq!(report.forks, []);
    }

    #[test]
    fn a_segment_whose_last_line_changed_as_it_grew_is_read_whole() {
        let layout = Layout::new(".lib").unwrap();
        let disk = MemDisk::new();
        let lines = chain(1);
        let (a, b) = (after(&lines[1], "a"), after(&lines[1], "bb"));
        let path = layout.writer(W).join("s.jsonl").unwrap();
        put(&disk, &path, &bytes_of(&[&lines[0], &lines[1], &a]));
        let mut reader = Reader::new(layout, CachedView::default());
        crate::blocking::run(&mut disk.clone(), reader.read()).unwrap();
        let remove = Io::Remove {
            root: Root::Folder,
            path: path.clone(),
        };
        disk.perform(remove).unwrap();
        let c = after(&b, "c");
        put(&disk, &path, &bytes_of(&[&lines[0], &lines[1], &b, &c]));

        let report = crate::blocking::run(&mut disk.clone(), reader.read()).unwrap();
        assert_eq!(report.placed[&W], [b.hash(), c.hash()]);
        assert_eq!(report.forks.len(), 1, "{report:?}");
        assert_eq!(reader.removed().get(&W).map(BTreeSet::len), Some(1));
    }

    #[test]
    fn a_torn_line_completed_later_is_read_from_where_the_lines_ended() {
        let layout = Layout::new(".lib").unwrap();
        let disk = MemDisk::new();
        let lines = chain(2);
        let path = layout.writer(W).join("s.jsonl").unwrap();
        let whole = bytes_of(&[&lines[0], &lines[1], &lines[2]]);
        let torn = lines[0].to_bytes().len() + lines[1].to_bytes().len() + 9;
        put(&disk, &path, &whole[..torn]);
        let mut reader = Reader::new(layout, CachedView::default());
        let first = crate::blocking::run(&mut disk.clone(), reader.read()).unwrap();
        assert_eq!(first.unreadable.len(), 1);
        assert_eq!(first.placed[&W], hashes(&lines[..2]));
        grow(&disk, &path, &whole[torn..]);

        let mut backend = Counting(disk.clone(), 0);
        let report = crate::blocking::run(&mut backend, reader.read()).unwrap();
        assert_eq!(report.placed[&W], [lines[2].hash()]);
        assert_eq!(report.unreadable, []);
        let read_on = line::ENDING + lines[2].to_bytes().len() as u64;
        assert_eq!(backend.1, read_on);
    }

    #[test]
    fn a_sealed_segment_is_read_whole_once_anything_follows_its_marker() {
        let layout = Layout::new(".lib").unwrap();
        let disk = MemDisk::new();
        let lines = chain(2);
        let path = layout.writer(W).join("s.jsonl").unwrap();
        let mut sealed = bytes_of(&[&lines[0], &lines[1]]);
        let end = sealed.len() as u64;
        sealed.extend(line::seal_marker(lines[1].hash()));
        put(&disk, &path, &sealed);
        let mut reader = Reader::new(layout, CachedView::default());
        let first = crate::blocking::run(&mut disk.clone(), reader.read()).unwrap();
        assert_eq!(first.unreadable, []);
        let files = reader.files(W);
        let [(_, _, WriterFile::Segment(segment))] = files.as_slice() else {
            panic!("{files:?}")
        };
        assert!(segment.sealed);
        grow(&disk, &path, &lines[2].to_bytes());

        let mut backend = Counting(disk.clone(), 0);
        let report = crate::blocking::run(&mut backend, reader.read()).unwrap();
        assert_eq!(report.placed, BTreeMap::new());
        let stops: Vec<u64> = report
            .unreadable
            .iter()
            .filter_map(|(_, stop)| stop.as_ref().map(|stop| stop.offset))
            .collect();
        assert_eq!(stops, [end]);
        let len = sealed.len() as u64 + lines[2].to_bytes().len() as u64;
        assert_eq!(backend.1, len, "read whole");
    }

    #[test]
    fn a_segment_replaced_at_its_length_and_time_is_read_again() {
        let layout = Layout::new(".lib").unwrap();
        let mut disk = MemDisk::new();
        let lines = chain(2);
        let other = after(&lines[1], "x1");
        assert_eq!(other.to_bytes().len(), lines[2].to_bytes().len());
        let path = layout.writer(W).join("s.jsonl").unwrap();
        let bytes = |lines: &[&Line]| -> Vec<u8> {
            lines.iter().flat_map(|line| line.to_bytes()).collect()
        };
        put(&disk, &path, &bytes(&[&lines[0], &lines[1], &lines[2]]));
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
        put(&disk, &path, &bytes(&[&lines[0], &lines[1], &other]));
        disk.set_modified(Root::Folder, &path, before).unwrap();
        assert_eq!(modified(&mut disk), before);

        let report = crate::blocking::run(&mut disk, reader.read()).unwrap();
        assert_eq!(report.placed[&W], [other.hash()]);
        assert_eq!(report.forks.len(), 1, "{report:?}");
    }

    #[test]
    fn a_cached_view_saved_in_the_local_root_loads_back() {
        let mut disk = MemDisk::new();
        let lines = chain(2);
        let mut log = WriterLog::new(W);
        log.place(Vec::new(), entries(&lines));
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
