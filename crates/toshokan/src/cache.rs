//! The cached view in the local root: `view.bin`, a checkpoint of what the reader
//! remembers of each writer and knew of each file, and `view.log`, a journal of
//! what changed since, one record after another. Both are private to the
//! install, in the binary encoding entries are held in.
//!
//! A save appends what changed to the journal and syncs it. The view is written
//! whole again, and the journal removed, when the journal has grown past half
//! the checkpoint, when a record could not be appended, when a writer's tips
//! changed or the reader let go of what it remembered, and when the view holds
//! what another view of the install brought. Each checkpoint has a generation,
//! and each record names the generation it extends. Loading replays onto the
//! checkpoint the records that extend it, so a crash between writing a
//! checkpoint and removing the journal leaves the new view, not the old journal
//! over it.
//!
//! Entries are kept as an install holds them, packed and verified when they were
//! read, so loading neither parses nor hashes them again. Each record carries a
//! checksum of its bytes instead.

use std::cell::RefCell;
use std::collections::{BTreeMap, BTreeSet};
use std::rc::Rc;

use crate::derive::Memory;
use crate::error::{Error, Result};
use crate::flow::{self, Fallible, Flow};
use crate::ids::{EntryHash, WriterId};
use crate::io::{Io, IoError, Root, Task};
use crate::layout::Layout;
use crate::log::{Entry, Own};
use crate::pack::{self, bad_variant, Bad, In, Pack, Unpack, Unpacked};
use crate::path::RelPath;
use crate::reader::{CachedView, Grace, Reader, Record, Segment, Stamp, WriterFile};
use crate::snapshot::Snapshot;

/// A stored view, or why it cannot be read.
type Parsed<T> = std::result::Result<T, String>;

/// How far past half the checkpoint's length the journal grows before the view
/// is written whole again.
const JOURNAL_SLACK: u64 = 1 << 20;

/// The first byte of every record this build writes; a record starting with
/// another is not read. It changes with any change to what a record packs or
/// how, since a record whose check holds is taken as this build packed it.
const FORMAT: u8 = 6;

/// How many bytes of a record's length start it.
const LEN: usize = 8;

/// How many bytes of a record's [`Check`] follow its length.
const CHECK: usize = 16;

/// The check of a record's payload: two lanes that each take every other eight
/// bytes by a multiply and a rotation, mixed with the length at the end by
/// SplitMix64's finalizer. A view is private to the install, so the check need
/// find only records a crash tore or a disk damaged, not forgeries, and costs a
/// multiply per eight bytes where a cryptographic hash costs many.
#[derive(Clone)]
struct Check {
    lanes: [u64; 2],
    /// Bytes short of a block, from the end of what was taken.
    held: [u8; 16],
    holding: usize,
    len: u64,
}

impl Default for Check {
    fn default() -> Self {
        Self {
            // The fractional parts of the golden ratio and of the square root of 2.
            lanes: [0x9e37_79b9_7f4a_7c15, 0x6a09_e667_f3bc_c909],
            held: [0; 16],
            holding: 0,
            len: 0,
        }
    }
}

impl Check {
    fn of(bytes: &[u8]) -> [u8; CHECK] {
        let mut check = Self::default();
        check.update(bytes);
        check.finish()
    }

    fn update(&mut self, mut bytes: &[u8]) {
        self.len += bytes.len() as u64;
        if self.holding > 0 {
            let taken = (16 - self.holding).min(bytes.len());
            self.held[self.holding..][..taken].copy_from_slice(&bytes[..taken]);
            self.holding += taken;
            bytes = &bytes[taken..];
            if self.holding < 16 {
                return;
            }
            let block = self.held;
            self.block(&block);
            self.holding = 0;
        }
        let mut blocks = bytes.chunks_exact(16);
        for block in &mut blocks {
            self.block(block.try_into().expect("sixteen bytes"));
        }
        let rest = blocks.remainder();
        self.held[..rest.len()].copy_from_slice(rest);
        self.holding = rest.len();
    }

    fn block(&mut self, block: &[u8; 16]) {
        let (low, high) = block.split_at(8);
        let words = [low, high].map(|word| u64::from_le_bytes(word.try_into().expect("eight")));
        self.lanes[0] = (self.lanes[0] ^ words[0])
            .wrapping_mul(0x9e37_79b9_7f4a_7c15)
            .rotate_left(31);
        self.lanes[1] = (self.lanes[1] ^ words[1])
            .wrapping_mul(0x6a09_e667_f3bc_c909)
            .rotate_left(29);
    }

    fn finish(mut self) -> [u8; CHECK] {
        if self.holding > 0 {
            let mut block = [0; 16];
            block[..self.holding].copy_from_slice(&self.held[..self.holding]);
            self.block(&block);
        }
        let [a, b] = self.lanes;
        let low = mixed(a ^ self.len);
        let high = mixed(b ^ low.rotate_left(17));
        let mut check = [0; CHECK];
        check[..8].copy_from_slice(&low.to_le_bytes());
        check[8..].copy_from_slice(&high.to_le_bytes());
        check
    }
}

fn mixed(mut z: u64) -> u64 {
    z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
    z ^ (z >> 31)
}

/// What this process knows of the view it saves to.
#[derive(Clone, Debug, Default)]
pub struct Store {
    /// The writer whose directory holds the view, by genesis entry.
    pub(crate) genesis: Option<EntryHash>,
    /// The generation of the checkpoint the journal extends.
    pub(crate) generation: u64,
    checkpoint: u64,
    journal: u64,
    /// The journal may end in a torn record, lack one that failed, or hold
    /// records of an earlier checkpoint.
    broken: bool,
    /// The view holds what the saved one does not: write it whole.
    due: bool,
    /// A save took the view's unsaved changes and has not finished.
    saving: bool,
}

/// What loading the local root found.
pub struct Loaded {
    pub view: CachedView,
    /// What this writer's reader last knew of each file, by path.
    pub files: BTreeMap<RelPath, StoredFile>,
    pub absorbed: BTreeSet<EntryHash>,
    /// The retired writers whose views were read, with the generation of each
    /// one's checkpoint.
    pub retired: BTreeMap<EntryHash, u64>,
    pub store: Store,
}

/// One writer's saved view.
pub(crate) struct Kept {
    pub(crate) view: CachedView,
    files: BTreeMap<RelPath, StoredFile>,
    absorbed: BTreeSet<EntryHash>,
    generation: u64,
    checkpoint: u64,
    journal: u64,
    broken: bool,
}

/// The views of the install's writers in `pool`, joined: `own`'s first, then
/// the other live writers', then each retired writer's that no view loaded
/// before holds. Only `own`'s records of files are kept. A view this build cannot
/// read is left out.
pub fn load(pool: Vec<EntryHash>, own: Option<EntryHash>) -> Task<'static, Result<Loaded>> {
    let mut ordered: Vec<EntryHash> = own.into_iter().filter(|own| pool.contains(own)).collect();
    ordered.extend(pool.iter().copied().filter(|genesis| Some(*genesis) != own));
    flow::fold(ordered.into_iter(), Vec::new(), |mut found, genesis| {
        flow::stat(Root::Local, &Layout::retired(genesis)).map_ok(move |retired| {
            found.push((genesis, retired.is_some()));
            found
        })
    })
    .and_then(move |found| {
        let (live, retired): (Vec<_>, Vec<_>) = found.into_iter().partition(|(_, r)| !r);
        let ordered = live.into_iter().chain(retired);
        let start = Loaded {
            view: CachedView::default(),
            files: BTreeMap::new(),
            absorbed: BTreeSet::new(),
            retired: BTreeMap::new(),
            store: Store {
                genesis: own,
                due: own.is_some(),
                ..Store::default()
            },
        };
        flow::fold(ordered, start, move |mut loaded, (genesis, retired)| {
            if retired && loaded.absorbed.contains(&genesis) {
                return flow::ok(loaded);
            }
            load_one(genesis).then(move |kept| match kept {
                Ok(kept) => {
                    loaded.join(genesis, retired, own, kept);
                    flow::ok(loaded)
                }
                Err(Error::Corrupt { .. }) => flow::ok(loaded),
                Err(error) => Flow::Done(Err(error)),
            })
        })
    })
    .task()
}

impl Loaded {
    fn join(&mut self, genesis: EntryHash, retired: bool, own: Option<EntryHash>, kept: Kept) {
        if Some(genesis) == own {
            self.store = Store {
                genesis: own,
                generation: kept.generation,
                checkpoint: kept.checkpoint,
                journal: kept.journal,
                broken: kept.broken,
                due: kept.checkpoint == 0,
                saving: false,
            };
            let others = std::mem::replace(&mut self.view, kept.view);
            let grew = self.view.join(&others);
            self.store.due |= grew;
            self.files = kept.files;
            self.absorbed.extend(kept.absorbed);
            return;
        }
        let absorbed = self.absorbed.len();
        self.absorbed.extend(kept.absorbed);
        if retired {
            self.absorbed.insert(genesis);
            self.retired.insert(genesis, kept.generation);
        }
        let grew = self.view.join(&kept.view);
        self.store.due |= grew || self.absorbed.len() != absorbed;
    }
}

/// The view kept for the writer whose genesis entry is `genesis`: its
/// checkpoint, and every record of its journal up to the first that is torn.
pub(crate) fn load_one<'a>(genesis: EntryHash) -> Fallible<'a, Kept> {
    let path = Layout::cached_view(genesis);
    flow::read_replaced(Root::Local, path.clone()).and_then(move |checkpoint| {
        flow::read_file(Root::Local, &Layout::view_journal(genesis), u64::MAX).and_then(
            move |journal| {
                let decoding = Decoding::new(checkpoint, journal);
                flow::sliced(decoding, |decoding| decoding.step(CHECK_SLICE)).then(
                    move |decoding| {
                        let kept = decoding.finish().map_err(|reason| Error::Corrupt {
                            path: path.clone(),
                            reason,
                        });
                        Flow::Done(kept)
                    },
                )
            },
        )
    })
}

/// How many bytes of a checkpoint one slice of decoding checks.
const CHECK_SLICE: usize = 1 << 22;

/// A checkpoint and its journal decoded a slice at a time: the checkpoint's
/// check, then the checkpoint, then the journal a record at a time.
struct Decoding {
    checkpoint: Vec<u8>,
    journal: Vec<u8>,
    /// Where the checkpoint's payload ends.
    end: usize,
    /// Where the next read starts, in the checkpoint until the journal is read.
    at: usize,
    stage: Decode,
    kept: Kept,
}

enum Decode {
    Check(Box<Check>),
    Read,
    Journal,
    Done(Parsed<()>),
}

impl Decoding {
    fn new(checkpoint: Option<Vec<u8>>, journal: Option<Vec<u8>>) -> Self {
        let kept = Kept {
            view: CachedView::default(),
            files: BTreeMap::new(),
            absorbed: BTreeSet::new(),
            generation: 0,
            checkpoint: checkpoint.as_ref().map_or(0, |bytes| bytes.len() as u64),
            journal: journal.as_ref().map_or(0, |bytes| bytes.len() as u64),
            broken: false,
        };
        let stage = match &checkpoint {
            Some(_) => Decode::Check(Box::default()),
            None => Decode::Journal,
        };
        Self {
            checkpoint: checkpoint.unwrap_or_default(),
            journal: journal.unwrap_or_default(),
            end: 0,
            at: 0,
            stage,
            kept,
        }
    }

    /// Checks `bytes` more, or reads the checkpoint or one record more; true
    /// once there is nothing left.
    fn step(&mut self, bytes: usize) -> bool {
        let stage = std::mem::replace(&mut self.stage, Decode::Done(Ok(())));
        self.stage = match stage {
            Decode::Check(hasher) => self.check(hasher, bytes),
            Decode::Read => self.read(),
            Decode::Journal => self.record(),
            done @ Decode::Done(_) => done,
        };
        matches!(self.stage, Decode::Done(_))
    }

    /// Hashes about `slice` more bytes of the checkpoint's payload, then compares
    /// its check.
    fn check(&mut self, mut hasher: Box<Check>, slice: usize) -> Decode {
        let Some((len, check)) = frame_head(&self.checkpoint) else {
            return Decode::Done(Err("the checkpoint fails its check".into()));
        };
        let start = LEN + CHECK;
        if self.end == 0 {
            match start
                .checked_add(len)
                .filter(|end| *end <= self.checkpoint.len())
            {
                Some(end) => (self.end, self.at) = (end, start),
                None => return Decode::Done(Err("the checkpoint fails its check".into())),
            }
        }
        let until = self.at.saturating_add(slice).min(self.end);
        hasher.update(&self.checkpoint[self.at..until]);
        self.at = until;
        if until < self.end {
            return Decode::Check(hasher);
        }
        if hasher.finish() != check {
            return Decode::Done(Err("the checkpoint fails its check".into()));
        }
        if self.end != self.checkpoint.len() {
            return Decode::Done(Err("bytes follow the checkpoint".into()));
        }
        Decode::Read
    }

    fn read(&mut self) -> Decode {
        let payload = &self.checkpoint[LEN + CHECK..self.end];
        let stored = match StoredView::read(payload) {
            Ok(stored) => stored,
            Err(Bad(reason)) => return Decode::Done(Err(reason.to_owned())),
        };
        self.kept.absorbed = stored.absorbed.iter().copied().collect();
        self.kept.generation = stored.generation;
        self.at = 0;
        match self.kept.apply(stored) {
            Ok(()) => Decode::Journal,
            Err(reason) => Decode::Done(Err(reason)),
        }
    }

    /// Replays the journal's next record, up to the first that is torn. A record
    /// of another generation is skipped.
    fn record(&mut self) -> Decode {
        let rest = &self.journal[self.at..];
        if rest.is_empty() {
            return Decode::Done(Ok(()));
        }
        let Some((payload, after)) = unframe(rest) else {
            self.kept.broken = true;
            return Decode::Done(Ok(()));
        };
        let Ok(record) = StoredView::read(payload) else {
            self.kept.broken = true;
            return Decode::Done(Ok(()));
        };
        self.at = self.journal.len() - after.len();
        if record.generation != self.kept.generation {
            self.kept.broken = true;
            return Decode::Journal;
        }
        match self.kept.apply(record) {
            Ok(()) => Decode::Journal,
            Err(reason) => Decode::Done(Err(reason)),
        }
    }

    /// ⚠️ Before [`Decoding::step`] returns true, a view lacking what is not
    /// decoded yet.
    fn finish(self) -> Parsed<Kept> {
        match self.stage {
            Decode::Done(Err(reason)) => Err(reason),
            _ => Ok(self.kept),
        }
    }
}

impl Kept {
    /// Applies what `stored` holds to the view.
    fn apply(&mut self, stored: StoredView) -> Parsed<()> {
        apply(&mut self.view, stored.writers, &stored.let_go, stored.grace)?;
        for (path, file) in stored.files {
            match file {
                Some(file) => self.files.insert(path, file),
                None => self.files.remove(&path),
            };
        }
        Ok(())
    }
}

/// Places a stored view in `view`.
pub(crate) fn restore(view: &mut CachedView, bytes: &[u8]) -> Parsed<()> {
    let (payload, rest) = unframe(bytes).ok_or("the view fails its check")?;
    if !rest.is_empty() {
        return Err("bytes follow the view".into());
    }
    let stored = StoredView::read(payload).map_err(|Bad(reason)| reason.to_owned())?;
    apply(view, stored.writers, &stored.let_go, stored.grace)
}

/// Applies what each writer's memory gained or lost, what the grace step let go
/// of, and the grace step's state.
fn apply(
    view: &mut CachedView,
    writers: Vec<(WriterId, StoredMemory)>,
    let_go: &[(WriterId, LetGo)],
    grace: Option<Grace>,
) -> Parsed<()> {
    for (writer, stored) in writers {
        if let Some(tips) = &stored.tips {
            if tips.iter().any(|tip| tip.writer != writer) {
                return Err(format!("a snapshot of {writer} names another writer"));
            }
        }
        let memory = view.writers.entry(writer).or_default();
        if let Some(tips) = stored.tips {
            memory.tips = tips.into_iter().map(Rc::new).collect();
        }
        if let Some(forks) = stored.forks {
            memory.forks = forks.into_iter().collect();
        }
        for hash in stored.dropped {
            memory.entries.remove(&hash);
        }
        for entry in stored.added {
            memory.entries.insert(entry.hash(), Rc::new(entry));
        }
        if memory.is_empty() {
            view.writers.remove(&writer);
        }
    }
    for (writer, let_go) in let_go {
        if let Some(memory) = view.writers.get_mut(writer) {
            let_go.apply(memory);
            if memory.is_empty() {
                view.writers.remove(writer);
            }
        }
    }
    if let Some(grace) = grace {
        view.grace = grace;
    }
    Ok(())
}

/// `payload` as a record: its length, its check, then itself.
fn frame(payload: &[u8]) -> Vec<u8> {
    let len = payload.len() as u64;
    let mut record = Vec::with_capacity(LEN + CHECK + payload.len());
    record.extend_from_slice(&len.to_le_bytes());
    record.extend_from_slice(&Check::of(payload));
    record.extend_from_slice(payload);
    record
}

/// The payload of the record `bytes` start with, and what follows it; `None`
/// when the record is torn or fails its check.
fn unframe(bytes: &[u8]) -> Option<(&[u8], &[u8])> {
    let (len, check) = frame_head(bytes)?;
    let (payload, rest) = bytes[LEN + CHECK..].split_at_checked(len)?;
    (Check::of(payload) == check).then_some((payload, rest))
}

/// A record's length and check.
fn frame_head(bytes: &[u8]) -> Option<(usize, [u8; CHECK])> {
    let (len, rest) = bytes.split_first_chunk::<LEN>()?;
    let (check, _) = rest.split_first_chunk::<CHECK>()?;
    Some((usize::try_from(u64::from_le_bytes(*len)).ok()?, *check))
}

/// A checkpoint or a journal record. A checkpoint holds every memory whole; a
/// record holds what each memory gained or lost, and files whose records
/// changed.
#[derive(Default)]
struct StoredView {
    /// A checkpoint's own generation, or the generation a record extends.
    generation: u64,
    writers: Vec<(WriterId, StoredMemory)>,
    files: Vec<(RelPath, Option<StoredFile>)>,
    absorbed: Vec<EntryHash>,
    grace: Option<Grace>,
    let_go: Vec<(WriterId, LetGo)>,
}

impl StoredView {
    fn read(payload: &[u8]) -> Unpacked<Self> {
        let mut input = In::new(payload);
        if input.byte()? != FORMAT {
            return Err(Bad("a format this build does not read"));
        }
        let view = Self {
            generation: Unpack::unpack(&mut input)?,
            writers: Unpack::unpack(&mut input)?,
            files: Unpack::unpack(&mut input)?,
            absorbed: Unpack::unpack(&mut input)?,
            grace: Unpack::unpack(&mut input)?,
            let_go: Unpack::unpack(&mut input)?,
        };
        input.end()?;
        Ok(view)
    }
}

/// What one writer's memory holds, or what it gained or lost: its tips and fork
/// points when they changed, the entries it added and those it dropped.
#[derive(Default)]
struct StoredMemory {
    tips: Option<Vec<Snapshot>>,
    forks: Option<Vec<Own>>,
    added: Vec<Entry>,
    dropped: Vec<EntryHash>,
}

impl Unpack for StoredMemory {
    fn unpack(input: &mut In<'_>) -> Unpacked<Self> {
        let tips = Unpack::unpack(input)?;
        let forks = Unpack::unpack(input)?;
        let added = pack::list(input, Entry::unpack_from)?;
        let dropped = Unpack::unpack(input)?;
        Ok(Self {
            tips,
            forks,
            added,
            dropped,
        })
    }
}

/// One memory, or what it changed by, as written.
struct MemoryOut<'a> {
    tips: Option<&'a [Rc<Snapshot>]>,
    forks: Option<&'a BTreeSet<Own>>,
    added: Vec<&'a Entry>,
    dropped: Vec<EntryHash>,
}

impl MemoryOut<'_> {
    fn write(&self, out: &mut Vec<u8>) {
        match self.tips {
            Some(tips) => {
                out.push(1);
                tips.len().pack(out);
                for tip in tips {
                    tip.pack(out);
                }
            }
            None => out.push(0),
        }
        match self.forks {
            Some(forks) => {
                out.push(1);
                forks.iter().collect::<Vec<_>>().pack(out);
            }
            None => out.push(0),
        }
        self.added.len().pack(out);
        for entry in &self.added {
            entry.pack_into(out);
        }
        self.dropped.pack(out);
    }
}

/// What changed in one writer's memory since it was last saved.
#[derive(Clone, Default)]
pub(crate) struct Changed {
    pub(crate) tips: bool,
    pub(crate) forks: bool,
    pub(crate) added: BTreeSet<EntryHash>,
    pub(crate) dropped: BTreeSet<EntryHash>,
}

impl Changed {
    /// What changed from `before` to `after`.
    pub(crate) fn between(before: Option<&Memory>, after: Option<&Memory>) -> Self {
        let empty = Memory::default();
        let (before, after) = (before.unwrap_or(&empty), after.unwrap_or(&empty));
        Self {
            tips: before.tips != after.tips,
            forks: before.forks != after.forks,
            added: after
                .entries
                .keys()
                .filter(|hash| !before.entries.contains_key(hash))
                .copied()
                .collect(),
            dropped: before
                .entries
                .keys()
                .filter(|hash| !after.entries.contains_key(hash))
                .copied()
                .collect(),
        }
    }

    pub(crate) fn is_empty(&self) -> bool {
        !self.tips && !self.forks && self.added.is_empty() && self.dropped.is_empty()
    }

    /// Adds what changed next, so that the two read as one change.
    pub(crate) fn then(&mut self, next: Changed) {
        self.tips |= next.tips;
        self.forks |= next.forks;
        for hash in next.added {
            self.dropped.remove(&hash);
            self.added.insert(hash);
        }
        for hash in next.dropped {
            if !self.added.remove(&hash) {
                self.dropped.insert(hash);
            }
        }
    }
}

/// What the grace step let go of in one writer's memory: tips by head, fork
/// points and entries.
#[derive(Clone, Default)]
pub(crate) struct LetGo {
    tips: BTreeSet<Own>,
    forks: BTreeSet<Own>,
    entries: BTreeSet<EntryHash>,
}

impl LetGo {
    /// What `before` held that `after` does not.
    pub(crate) fn between(before: Option<&Memory>, after: Option<&Memory>) -> Self {
        let empty = Memory::default();
        let (before, after) = (before.unwrap_or(&empty), after.unwrap_or(&empty));
        let heads =
            |memory: &Memory| -> BTreeSet<Own> { memory.tips.iter().map(|tip| tip.head).collect() };
        let keys =
            |memory: &Memory| -> BTreeSet<EntryHash> { memory.entries.keys().copied().collect() };
        Self {
            tips: &heads(before) - &heads(after),
            forks: &before.forks - &after.forks,
            entries: &keys(before) - &keys(after),
        }
    }

    pub(crate) fn then(&mut self, next: LetGo) {
        self.tips.extend(next.tips);
        self.forks.extend(next.forks);
        self.entries.extend(next.entries);
    }

    fn apply(&self, memory: &mut Memory) {
        memory.tips.retain(|tip| !self.tips.contains(&tip.head));
        memory.forks.retain(|fork| !self.forks.contains(fork));
        memory
            .entries
            .retain(|hash, _| !self.entries.contains(hash));
    }
}

impl Pack for LetGo {
    fn pack(&self, out: &mut Vec<u8>) {
        self.tips.iter().collect::<Vec<_>>().pack(out);
        self.forks.iter().collect::<Vec<_>>().pack(out);
        self.entries.iter().collect::<Vec<_>>().pack(out);
    }
}

impl Unpack for LetGo {
    fn unpack(input: &mut In<'_>) -> Unpacked<Self> {
        let tips: Vec<Own> = Unpack::unpack(input)?;
        let forks: Vec<Own> = Unpack::unpack(input)?;
        let entries: Vec<EntryHash> = Unpack::unpack(input)?;
        Ok(Self {
            tips: tips.into_iter().collect(),
            forks: forks.into_iter().collect(),
            entries: entries.into_iter().collect(),
        })
    }
}

/// What a reader knew of one file: its stamp, and its lines or its snapshot.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct StoredFile {
    len: u64,
    modified: u64,
    /// The file's last bytes.
    tail: Vec<u8>,
    held: Contents,
}

/// A file's lines, or the snapshot it holds.
#[derive(Clone, PartialEq, Eq, Debug)]
enum Contents {
    Segment(StoredSegment),
    /// The head of the snapshot: one of the reader's remembered tips.
    Snapshot(Own),
}

/// A segment as a run of the writer's chain, `count` entries from `first` to
/// `last`. Lines the memory holds no entry for are kept whole.
#[derive(Clone, PartialEq, Eq, Debug)]
struct StoredSegment {
    count: usize,
    first: Option<EntryHash>,
    last: Option<EntryHash>,
    end: u64,
    sealed: bool,
    lines: Vec<Entry>,
}

impl Pack for StoredFile {
    fn pack(&self, out: &mut Vec<u8>) {
        (self.len, self.modified).pack(out);
        self.tail.pack(out);
        match &self.held {
            Contents::Segment(segment) => {
                out.push(0);
                segment.pack(out);
            }
            Contents::Snapshot(head) => {
                out.push(1);
                head.pack(out);
            }
        }
    }
}

impl Unpack for StoredFile {
    fn unpack(input: &mut In<'_>) -> Unpacked<Self> {
        let (len, modified) = Unpack::unpack(input)?;
        let tail = Unpack::unpack(input)?;
        let held = match input.byte()? {
            0 => Contents::Segment(StoredSegment::unpack(input)?),
            1 => Contents::Snapshot(Own::unpack(input)?),
            _ => return bad_variant(),
        };
        Ok(Self {
            len,
            modified,
            tail,
            held,
        })
    }
}

impl Pack for StoredSegment {
    fn pack(&self, out: &mut Vec<u8>) {
        (self.count, (self.first, self.last)).pack(out);
        (self.end, self.sealed).pack(out);
        self.lines.len().pack(out);
        for line in &self.lines {
            line.pack_into(out);
        }
    }
}

impl Unpack for StoredSegment {
    fn unpack(input: &mut In<'_>) -> Unpacked<Self> {
        let count = u64::unpack(input)?;
        let count = usize::try_from(count).map_err(|_| Bad("a count past memory"))?;
        let (first, last) = Unpack::unpack(input)?;
        let (end, sealed) = Unpack::unpack(input)?;
        let lines = pack::list(input, Entry::unpack_from)?;
        Ok(Self {
            count,
            first,
            last,
            end,
            sealed,
            lines,
        })
    }
}

impl StoredFile {
    /// What `record` says of its file, if it can be said against `memory`: a
    /// segment read to its end whose lines follow one another and none of which
    /// a tip folds, or a snapshot `memory` keeps as a tip.
    fn of(record: &Record, memory: Option<&Memory>) -> Option<Self> {
        let stamp = record.stamp.as_ref()?;
        let held = match &record.file {
            WriterFile::Segment(segment) if segment.stop.is_none() && segment.contiguous() => {
                Contents::Segment(StoredSegment::of(segment, memory)?)
            }
            WriterFile::Snapshot(snapshot) => {
                let kept = memory?.tips.iter().any(|tip| tip.head == snapshot.head);
                if !kept {
                    return None;
                }
                Contents::Snapshot(snapshot.head)
            }
            WriterFile::Segment(_) | WriterFile::Unreadable => return None,
        };
        Some(Self {
            len: stamp.len,
            modified: stamp.modified,
            tail: stamp.tail.clone(),
            held,
        })
    }

    /// The record this describes, with each line taken from `memory` or from the
    /// lines kept whole; `None` when any is missing.
    pub(crate) fn resolve(&self, memory: &Memory) -> Option<Record> {
        let stamp = Stamp {
            len: self.len,
            modified: self.modified,
            tail: self.tail.clone(),
        };
        let file = match &self.held {
            Contents::Segment(segment) => WriterFile::Segment(segment.resolve(memory)?),
            Contents::Snapshot(head) => {
                let kept = memory.tips.iter().find(|tip| tip.head == *head)?;
                WriterFile::Snapshot(Rc::clone(kept))
            }
        };
        Some(Record {
            stamp: Some(stamp),
            file,
        })
    }
}

impl StoredSegment {
    /// `None` when a tip of `memory` folds a line: its entry is not kept, and
    /// writing it whole would copy what the folder holds.
    fn of(segment: &Segment, memory: Option<&Memory>) -> Option<Self> {
        let mut whole = Vec::new();
        for entry in &segment.entries {
            match memory {
                Some(memory) if memory.entries.contains_key(&entry.hash()) => continue,
                Some(memory) if memory.tips.iter().any(|tip| entry.n() <= tip.head.n) => {
                    return None
                }
                _ => {}
            }
            whole.push(Entry::clone(entry));
        }
        Some(Self {
            count: segment.entries.len(),
            first: segment.entries.first().map(|entry| entry.hash()),
            last: segment.entries.last().map(|entry| entry.hash()),
            end: segment.end,
            sealed: segment.sealed,
            lines: whole,
        })
    }

    fn resolve(&self, memory: &Memory) -> Option<Segment> {
        let mut whole: BTreeMap<EntryHash, Rc<Entry>> = self
            .lines
            .iter()
            .map(|entry| (entry.hash(), Rc::new(entry.clone())))
            .collect();
        let mut entries = Vec::with_capacity(self.count.min(pack::RESERVED));
        let mut at = self.last;
        while let Some(hash) = at.filter(|_| entries.len() < self.count) {
            let entry = whole
                .remove(&hash)
                .or_else(|| memory.entries.get(&hash).cloned())?;
            at = (Some(hash) != self.first).then(|| entry.prev());
            entries.push(entry);
        }
        if entries.len() != self.count || entries.last().map(|e| e.hash()) != self.first {
            return None;
        }
        entries.reverse();
        Some(Segment {
            entries,
            end: self.end,
            stop: None,
            sealed: self.sealed,
        })
    }
}

/// A stored view's payload: its generation, its memories, the records of
/// `files`, what was absorbed, the grace step's state and what it let go of.
fn write_view<'a>(
    generation: u64,
    memories: impl ExactSizeIterator<Item = (WriterId, MemoryOut<'a>)>,
    files: Vec<(&RelPath, Option<StoredFile>)>,
    absorbed: &BTreeSet<EntryHash>,
    grace: Option<&Grace>,
    let_go: &BTreeMap<WriterId, LetGo>,
) -> Vec<u8> {
    let mut out = vec![FORMAT];
    generation.pack(&mut out);
    memories.len().pack(&mut out);
    for (writer, memory) in memories {
        writer.pack(&mut out);
        memory.write(&mut out);
    }
    files.pack(&mut out);
    absorbed.iter().collect::<Vec<_>>().pack(&mut out);
    grace.pack(&mut out);
    let_go.pack(&mut out);
    frame(&out)
}

fn whole(memory: &Memory) -> MemoryOut<'_> {
    MemoryOut {
        tips: Some(&memory.tips),
        forks: Some(&memory.forks),
        added: memory.entries.values().map(|entry| &**entry).collect(),
        dropped: Vec::new(),
    }
}

/// `view` as a checkpoint of the first generation holding nothing else.
pub(crate) fn encode_view(view: &CachedView) -> Vec<u8> {
    let memories = view
        .writers
        .iter()
        .map(|(writer, memory)| (*writer, whole(memory)));
    let (absorbed, let_go) = (BTreeSet::new(), BTreeMap::new());
    write_view(
        0,
        memories,
        Vec::new(),
        &absorbed,
        Some(&view.grace),
        &let_go,
    )
}

/// A checkpoint's bytes, and its generation.
pub(crate) struct Checkpoint {
    generation: u64,
    bytes: Vec<u8>,
}

/// Keeps `view` alone as the first view of the writer whose genesis entry is
/// `genesis`.
pub(crate) fn save_view<'a>(view: &CachedView, genesis: EntryHash) -> Fallible<'a, ()> {
    let checkpoint = Checkpoint {
        generation: 0,
        bytes: encode_view(view),
    };
    write_checkpoint(genesis, checkpoint, Rc::default())
}

fn encode_checkpoint(reader: &Reader) -> Checkpoint {
    let generation = reader.store.borrow().generation.wrapping_add(1);
    let view = reader.cached();
    let memories = view
        .writers
        .iter()
        .map(|(writer, memory)| (*writer, whole(memory)));
    let files: Vec<(&RelPath, Option<StoredFile>)> = reader
        .records()
        .filter_map(|(path, record)| {
            let memory = reader.writer_of(path).and_then(|w| view.writers.get(&w));
            Some((path, Some(StoredFile::of(record, memory)?)))
        })
        .collect();
    let bytes = write_view(
        generation,
        memories,
        files,
        &reader.absorbed,
        Some(&view.grace),
        &BTreeMap::new(),
    );
    Checkpoint { generation, bytes }
}

/// A checkpoint of a reader's view, encoded as [`Encoding::step`] is asked to.
pub(crate) struct Encoding {
    out: Checkpoint,
}

impl Encoding {
    pub(crate) fn new(reader: &Reader) -> Self {
        Self {
            out: encode_checkpoint(reader),
        }
    }

    /// True once the checkpoint is encoded.
    pub(crate) fn step(&mut self) -> bool {
        true
    }

    pub(crate) fn finish(self) -> Checkpoint {
        self.out
    }
}

fn encode_record(reader: &Reader) -> Vec<u8> {
    let view = reader.cached();
    let memories = reader.unsaved.memories.iter().map(|(writer, changed)| {
        let memory = view.writers.get(writer);
        let out = MemoryOut {
            tips: changed
                .tips
                .then(|| memory.map_or(&[][..], |memory| &memory.tips[..])),
            forks: memory.filter(|_| changed.forks).map(|memory| &memory.forks),
            added: changed
                .added
                .iter()
                .filter_map(|hash| memory?.entries.get(hash).map(|entry| &**entry))
                .collect(),
            dropped: changed.dropped.iter().copied().collect(),
        };
        (*writer, out)
    });
    let memories: Vec<_> = memories.collect();
    let files = reader
        .unsaved
        .files
        .iter()
        .map(|path| {
            let memory = reader.writer_of(path).and_then(|w| view.writers.get(&w));
            let stored = reader.record(path).and_then(|r| StoredFile::of(r, memory));
            (path, stored)
        })
        .collect();
    let grace = reader.unsaved.grace.then_some(&view.grace);
    let generation = reader.store.borrow().generation;
    write_view(
        generation,
        memories.into_iter(),
        files,
        &BTreeSet::new(),
        grace,
        &BTreeMap::new(),
    )
}

/// Keeps what the view gained since its last save.
pub(crate) fn save(reader: &mut Reader, genesis: EntryHash) -> Task<'static, Result<()>> {
    match due(reader, genesis) {
        true => checkpoint(reader, genesis),
        false => journal(reader, genesis),
    }
}

/// Whether the view is next written whole rather than appended to the journal.
pub(crate) fn due(reader: &Reader, genesis: EntryHash) -> bool {
    let rewritten = reader
        .unsaved
        .memories
        .values()
        .any(|changed| changed.tips || !changed.dropped.is_empty());
    let store = reader.store.borrow();
    store.genesis != Some(genesis)
        || store.due
        || store.saving
        || store.broken
        || rewritten
        || store.journal > store.checkpoint / 2 + JOURNAL_SLACK
}

/// Appends what the view gained since its last save to the journal.
pub(crate) fn journal(reader: &mut Reader, genesis: EntryHash) -> Task<'static, Result<()>> {
    if reader.unsaved.is_empty() {
        return Task::ready(Ok(()));
    }
    let record = encode_record(reader);
    reader.unsaved = Default::default();
    reader.store.borrow_mut().saving = true;
    append_record(genesis, record, Rc::clone(&reader.store)).task()
}

pub(crate) fn checkpoint(reader: &mut Reader, genesis: EntryHash) -> Task<'static, Result<()>> {
    let checkpoint = encode_checkpoint(reader);
    keep_checkpoint(reader, genesis, checkpoint)
}

/// Writes `checkpoint`, which [`Encoding`] encoded of the view as it is, as the
/// view of the writer whose genesis entry is `genesis`.
pub(crate) fn keep_checkpoint(
    reader: &mut Reader,
    genesis: EntryHash,
    checkpoint: Checkpoint,
) -> Task<'static, Result<()>> {
    reader.unsaved = Default::default();
    reader.store.borrow_mut().saving = true;
    write_checkpoint(genesis, checkpoint, Rc::clone(&reader.store)).task()
}

/// Replaces `view.bin` with `checkpoint`, then removes the journal it holds.
pub(crate) fn write_checkpoint<'a>(
    genesis: EntryHash,
    checkpoint: Checkpoint,
    store: Rc<RefCell<Store>>,
) -> Fallible<'a, ()> {
    let Checkpoint { generation, bytes } = checkpoint;
    let len = bytes.len() as u64;
    flow::replace(Root::Local, Layout::cached_view(genesis), bytes)
        .and_then(move |()| flow::remove_if_present(Root::Local, Layout::view_journal(genesis)))
        .then(move |written| {
            let mut store = store.borrow_mut();
            *store = match written {
                Ok(()) => Store {
                    genesis: Some(genesis),
                    generation,
                    checkpoint: len,
                    ..Store::default()
                },
                Err(_) => Store {
                    genesis: Some(genesis),
                    generation,
                    broken: true,
                    ..store.clone()
                },
            };
            Flow::Done(written)
        })
}

/// Appends the grace step's state and what it let go of, alone, to the journal
/// of the writer whose genesis entry is `genesis`, onto its checkpoint of
/// `generation`.
pub(crate) fn keep_grace(
    genesis: EntryHash,
    generation: u64,
    grace: Grace,
    let_go: &BTreeMap<WriterId, LetGo>,
) -> Task<'static, Result<()>> {
    let record = write_view(
        generation,
        std::iter::empty(),
        Vec::new(),
        &BTreeSet::new(),
        Some(&grace),
        let_go,
    );
    append_record(genesis, record, Rc::default()).task()
}

/// Appends one record to the journal, creating it when there is none, and syncs
/// it.
fn append_record<'a>(
    genesis: EntryHash,
    record: Vec<u8>,
    store: Rc<RefCell<Store>>,
) -> Fallible<'a, ()> {
    let path = Layout::view_journal(genesis);
    let len = record.len() as u64;
    let fresh = store.borrow().journal == 0;
    let create = Io::Create {
        root: Root::Local,
        path: path.clone(),
        bytes: record.clone(),
    };
    let appended = match fresh {
        true => flow::attempt(create.clone()).then({
            let path = path.clone();
            move |result| match result {
                Ok(_) => flow::sync_parent(Root::Local, &path),
                Err(IoError::AlreadyExists) => append(path, record),
                Err(error) => Flow::Done(Err(create.failed(error))),
            }
        }),
        false => append(path.clone(), record),
    };
    appended
        .and_then(move |()| flow::sync(Root::Local, &path))
        .then(move |result| {
            let mut store = store.borrow_mut();
            match result {
                Ok(()) => {
                    store.journal += len;
                    store.saving = false;
                }
                Err(_) => store.broken = true,
            }
            Flow::Done(result)
        })
}

fn append<'a>(path: RelPath, bytes: Vec<u8>) -> Fallible<'a, ()> {
    flow::act(Io::Append {
        root: Root::Local,
        path,
        bytes,
    })
}
