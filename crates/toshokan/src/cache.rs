//! The cached view in the local root: `view.bin`, a checkpoint of the whole view
//! with what the reader knew of each file and the merged state of the view, and
//! `view.log`, a journal of what the view gained since, one record after another.
//! Both are private to the install, in the binary encoding entries are held in.
//!
//! A save appends what the view gained to the journal and syncs it. The view is
//! written whole again, and the journal removed, when the journal has grown past
//! half the checkpoint, when a record could not be appended, when the view keeps a
//! new snapshot, which folds entries the checkpoint holds whole, and when the view
//! holds what another view of the install brought. Loading replays the journal
//! onto the checkpoint; replaying a record twice changes nothing, so a crash
//! between writing a checkpoint and removing the journal leaves a view that loads.
//!
//! Entries are kept as an install holds them, packed and verified when they were
//! read, so loading neither parses nor hashes them again. Each record carries a
//! checksum of its bytes instead.

use std::cell::RefCell;
use std::collections::{BTreeMap, BTreeSet};
use std::rc::Rc;

use crate::error::{Error, Result};
use crate::flow::{self, Fallible, Flow};
use crate::ids::{EntryHash, WriterId};
use crate::io::{Io, IoError, Root, Task};
use crate::layout::Layout;
use crate::log::Entry;
use crate::merge::Folded;
use crate::pack::{self, bad_variant, Bad, In, Pack, Unpack, Unpacked};
use crate::path::RelPath;
use crate::reader::{self, CachedView, Reader, Record, Segment, Stamp, WriterFile, WriterLog};
use crate::snapshot::Snapshot;

/// A stored view, or why it cannot be read.
type Parsed<T> = std::result::Result<T, String>;

/// How far past half the checkpoint's length the journal grows before the view
/// is written whole again.
const JOURNAL_SLACK: u64 = 1 << 20;

/// The first byte of every record this build writes; a record starting with
/// another is not read. It changes with any change to what a record packs or
/// how, since a record whose check holds is taken as this build packed it.
const FORMAT: u8 = 1;

/// How many bytes of a record's BLAKE3 hash follow its length.
const CHECK: usize = 16;

/// What this process knows of the view it saves to.
#[derive(Clone, Debug, Default)]
pub struct Store {
    /// The writer whose directory holds the view, by genesis entry.
    pub(crate) genesis: Option<EntryHash>,
    checkpoint: u64,
    journal: u64,
    /// The journal may end in a torn record, or lack one that failed.
    broken: bool,
    /// The view holds what the saved one does not: write it whole.
    due: bool,
}

/// What loading the local root found.
pub struct Loaded {
    pub view: CachedView,
    /// What this writer's reader last knew of each file, by path.
    pub files: BTreeMap<RelPath, StoredFile>,
    pub absorbed: BTreeSet<EntryHash>,
    pub store: Store,
    /// The merged state of every log of `view`, when each view loaded kept it.
    pub state: Option<Folded>,
}

/// One writer's saved view.
pub(crate) struct Kept {
    pub(crate) view: CachedView,
    files: BTreeMap<RelPath, StoredFile>,
    absorbed: BTreeSet<EntryHash>,
    state: Option<Folded>,
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
            store: Store {
                genesis: own,
                due: own.is_some(),
                ..Store::default()
            },
            state: Some(Folded::default()),
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
        self.state = match (self.state.take(), kept.state) {
            (Some(mut state), Some(theirs)) => {
                state.join(&theirs);
                Some(state)
            }
            _ => None,
        };
        if Some(genesis) == own {
            self.store = Store {
                genesis: own,
                checkpoint: kept.checkpoint,
                journal: kept.journal,
                broken: kept.broken,
                due: kept.checkpoint == 0,
            };
            self.view = kept.view;
            self.files = kept.files;
            self.absorbed = kept.absorbed;
            return;
        }
        let absorbed = self.absorbed.len();
        self.absorbed.extend(kept.absorbed);
        if retired {
            self.absorbed.insert(genesis);
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
                let kept = parse(checkpoint, journal).map_err(|reason| Error::Corrupt {
                    path: path.clone(),
                    reason,
                });
                Flow::Done(kept)
            },
        )
    })
}

fn parse(checkpoint: Option<Vec<u8>>, journal: Option<Vec<u8>>) -> Parsed<Kept> {
    let mut kept = Kept {
        view: CachedView::default(),
        files: BTreeMap::new(),
        absorbed: BTreeSet::new(),
        state: None,
        checkpoint: checkpoint.as_ref().map_or(0, |bytes| bytes.len() as u64),
        journal: journal.as_ref().map_or(0, |bytes| bytes.len() as u64),
        broken: false,
    };
    if let Some(bytes) = checkpoint {
        let (payload, rest) = unframe(&bytes).ok_or("the checkpoint fails its check")?;
        if !rest.is_empty() {
            return Err("bytes follow the checkpoint".into());
        }
        let mut stored = StoredView::read(payload).map_err(|Bad(reason)| reason.to_owned())?;
        kept.absorbed = stored.absorbed.iter().copied().collect();
        let state = stored.state.take();
        kept.apply(stored)?;
        kept.state = state;
    }
    let journal = journal.unwrap_or_default();
    let mut rest = &journal[..];
    while !rest.is_empty() {
        let Some((payload, after)) = unframe(rest) else {
            kept.broken = true;
            break;
        };
        let Ok(record) = StoredView::read(payload) else {
            kept.broken = true;
            break;
        };
        kept.apply(record)?;
        rest = after;
    }
    Ok(kept)
}

impl Kept {
    /// Places what `stored` holds, and folds what it gained into the state kept.
    fn apply(&mut self, stored: StoredView) -> Parsed<()> {
        if let Some(state) = &mut self.state {
            for (writer, log) in &stored.writers {
                for snapshot in &log.snapshots {
                    state.join(&snapshot.state);
                }
                for entry in &log.entries {
                    state.apply(*writer, entry);
                }
            }
        }
        restore_logs(&mut self.view, stored.writers)?;
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
    restore_logs(view, stored.writers)
}

fn restore_logs(view: &mut CachedView, logs: Vec<(WriterId, StoredLog)>) -> Parsed<()> {
    for (writer, stored) in logs {
        if stored
            .snapshots
            .iter()
            .any(|snapshot| snapshot.writer != writer)
        {
            return Err(format!("a snapshot of {writer} names another writer"));
        }
        let snapshots = stored.snapshots.into_iter().map(Rc::new).collect();
        let entries = stored.entries.into_iter().map(Rc::new).collect();
        view.restore(writer, snapshots, entries, stored.strays, stored.forks)?;
    }
    Ok(())
}

/// `payload` as a record: its length, its check, then itself.
fn frame(payload: &[u8]) -> Vec<u8> {
    let len = u32::try_from(payload.len()).expect("a record within 4 GiB");
    let mut record = Vec::with_capacity(4 + CHECK + payload.len());
    record.extend_from_slice(&len.to_le_bytes());
    record.extend_from_slice(&blake3::hash(payload).as_bytes()[..CHECK]);
    record.extend_from_slice(payload);
    record
}

/// The payload of the record `bytes` start with, and what follows it; `None`
/// when the record is torn or fails its check.
fn unframe(bytes: &[u8]) -> Option<(&[u8], &[u8])> {
    let (len, rest) = bytes.split_first_chunk::<4>()?;
    let (check, rest) = rest.split_first_chunk::<CHECK>()?;
    let len = usize::try_from(u32::from_le_bytes(*len)).ok()?;
    let (payload, rest) = rest.split_at_checked(len)?;
    (blake3::hash(payload).as_bytes()[..CHECK] == check[..]).then_some((payload, rest))
}

/// A checkpoint or a journal record. A checkpoint holds every log whole and the
/// merged state of them all; a record holds what each log gained, and files
/// whose records changed.
struct StoredView {
    writers: Vec<(WriterId, StoredLog)>,
    files: Vec<(RelPath, Option<StoredFile>)>,
    absorbed: Vec<EntryHash>,
    state: Option<Folded>,
}

impl StoredView {
    fn read(payload: &[u8]) -> Unpacked<Self> {
        let mut input = In::new(payload);
        if input.byte()? != FORMAT {
            return Err(Bad("a format this build does not read"));
        }
        let view = Self {
            writers: Unpack::unpack(&mut input)?,
            files: Unpack::unpack(&mut input)?,
            absorbed: Unpack::unpack(&mut input)?,
            state: Unpack::unpack(&mut input)?,
        };
        input.end()?;
        Ok(view)
    }
}

struct StoredLog {
    snapshots: Vec<Snapshot>,
    entries: Vec<Entry>,
    strays: Vec<(EntryHash, EntryHash)>,
    forks: Vec<(EntryHash, [EntryHash; 2])>,
}

impl Unpack for StoredLog {
    fn unpack(input: &mut In<'_>) -> Unpacked<Self> {
        let snapshots = Unpack::unpack(input)?;
        let entries = pack::list(input, Entry::unpack_from)?;
        let strays = Unpack::unpack(input)?;
        let forks: Vec<(EntryHash, (EntryHash, EntryHash))> = Unpack::unpack(input)?;
        Ok(Self {
            snapshots,
            entries,
            strays,
            forks: forks
                .into_iter()
                .map(|(prev, (a, b))| (prev, [a, b]))
                .collect(),
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
    held: Held,
}

#[derive(Clone, PartialEq, Eq, Debug)]
enum Held {
    Segment(StoredSegment),
    /// The last entry the snapshot folds: the reader's kept snapshot with it.
    Snapshot(EntryHash),
}

/// A segment as a run of the writer's chain, `count` entries from `first` to
/// `last`. Lines the cached view holds no entry for are kept whole.
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
            Held::Segment(segment) => {
                out.push(0);
                segment.pack(out);
            }
            Held::Snapshot(head) => {
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
            0 => Held::Segment(StoredSegment::unpack(input)?),
            1 => Held::Snapshot(EntryHash::unpack(input)?),
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
    /// What `record` says of its file, if it can be said against `log`: a
    /// segment read to its end whose lines follow one another, or a snapshot
    /// `log` keeps.
    fn of(record: &Record, log: Option<&WriterLog>) -> Option<Self> {
        let stamp = record.stamp.as_ref()?;
        let held = match &record.file {
            WriterFile::Segment(segment) if segment.stop.is_none() && segment.contiguous() => {
                Held::Segment(StoredSegment::of(segment, log)?)
            }
            WriterFile::Snapshot(snapshot) => {
                let head = snapshot.head()?;
                let kept = log?
                    .snapshots()
                    .iter()
                    .any(|kept| kept.head() == Some(head));
                if !kept {
                    return None;
                }
                Held::Snapshot(head)
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

    /// The record this describes, with each line taken from `log` or from the
    /// lines kept whole; `None` when any is missing.
    pub(crate) fn resolve(&self, log: &WriterLog) -> Option<Record> {
        let stamp = Stamp {
            len: self.len,
            modified: self.modified,
            tail: self.tail.clone(),
        };
        let file = match &self.held {
            Held::Segment(segment) => WriterFile::Segment(segment.resolve(log)?),
            Held::Snapshot(head) => {
                let kept = log
                    .snapshots()
                    .iter()
                    .find(|kept| kept.head() == Some(*head))?;
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
    /// `None` when a line is folded by a snapshot of `log`: its entry is not
    /// kept, and writing it whole would copy what the folder holds.
    fn of(segment: &Segment, log: Option<&WriterLog>) -> Option<Self> {
        let mut whole = Vec::new();
        for entry in &segment.entries {
            let hash = entry.hash();
            match log {
                Some(log) if log.entry(hash).is_some() => continue,
                Some(log) if log.holds(hash) => return None,
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

    fn resolve(&self, log: &WriterLog) -> Option<Segment> {
        let mut whole: BTreeMap<EntryHash, Rc<Entry>> = self
            .lines
            .iter()
            .map(|entry| (entry.hash(), Rc::new(entry.clone())))
            .collect();
        let mut entries = Vec::with_capacity(self.count);
        let mut at = self.last;
        while let Some(hash) = at.filter(|_| entries.len() < self.count) {
            let entry = whole.remove(&hash).or_else(|| log.entry(hash).cloned())?;
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

/// One log, or what it gained, as written.
struct LogOut<'a> {
    snapshots: &'a [Rc<Snapshot>],
    entries: &'a [Rc<Entry>],
    strays: Vec<(EntryHash, EntryHash)>,
    forks: Vec<(EntryHash, (EntryHash, EntryHash))>,
}

impl<'a> LogOut<'a> {
    fn whole(log: &'a WriterLog) -> Self {
        Self {
            snapshots: log.snapshots(),
            entries: log.entries(),
            strays: log.strays().iter().map(|(h, p)| (*h, *p)).collect(),
            forks: log.forks().iter().map(fork).collect(),
        }
    }

    fn gained(placement: &'a reader::Placement) -> Self {
        Self {
            snapshots: &placement.kept,
            entries: &placement.placed,
            strays: placement.strayed.clone(),
            forks: placement.forks.iter().map(fork).collect(),
        }
    }

    fn write(&self, out: &mut Vec<u8>) {
        self.snapshots.len().pack(out);
        for snapshot in self.snapshots {
            snapshot.pack(out);
        }
        self.entries.len().pack(out);
        for entry in self.entries {
            entry.pack_into(out);
        }
        (&self.strays, &self.forks).pack(out);
    }
}

fn fork(fork: &crate::report::Fork) -> (EntryHash, (EntryHash, EntryHash)) {
    let [a, b] = fork.branches;
    (fork.prev, (a, b))
}

/// A stored view's payload: its logs, the records of `files`, what was absorbed
/// and the merged state.
fn write_view<'a>(
    logs: impl ExactSizeIterator<Item = (WriterId, LogOut<'a>)>,
    files: Vec<(&RelPath, Option<StoredFile>)>,
    absorbed: &BTreeSet<EntryHash>,
    state: Option<&Folded>,
) -> Vec<u8> {
    let mut out = vec![FORMAT];
    logs.len().pack(&mut out);
    for (writer, log) in logs {
        writer.pack(&mut out);
        log.write(&mut out);
    }
    files.pack(&mut out);
    absorbed.iter().collect::<Vec<_>>().pack(&mut out);
    state.pack(&mut out);
    frame(&out)
}

/// `view` as a checkpoint holding nothing else.
pub(crate) fn encode_view(view: &CachedView) -> Vec<u8> {
    let logs = view.writers().iter();
    let logs = logs.map(|(writer, log)| (*writer, LogOut::whole(log)));
    write_view(logs, Vec::new(), &BTreeSet::new(), None)
}

fn encode_checkpoint(reader: &Reader, state: Option<&Folded>) -> Vec<u8> {
    let logs = reader.logs().iter();
    let logs = logs.map(|(writer, log)| (*writer, LogOut::whole(log)));
    let files = reader
        .records()
        .filter_map(|(path, record)| {
            let log = reader.writer_of(path).and_then(|w| reader.logs().get(&w));
            Some((path, Some(StoredFile::of(record, log)?)))
        })
        .collect();
    write_view(logs, files, &reader.absorbed, state)
}

fn encode_record(reader: &Reader) -> Vec<u8> {
    let logs = reader.unsaved.logs.iter();
    let logs = logs.map(|(writer, gained)| (*writer, LogOut::gained(gained)));
    let files = reader
        .unsaved
        .files
        .iter()
        .map(|path| {
            let log = reader.writer_of(path).and_then(|w| reader.logs().get(&w));
            let stored = reader.record(path).and_then(|r| StoredFile::of(r, log));
            (path, stored)
        })
        .collect();
    write_view(logs, files, &BTreeSet::new(), None)
}

/// Keeps what the view gained since its last save; `state` is the merged state
/// of every log of the view, kept when the view is written whole.
pub(crate) fn save(
    reader: &mut Reader,
    genesis: EntryHash,
    state: Option<&Folded>,
) -> Task<'static, Result<()>> {
    let folds = reader
        .unsaved
        .logs
        .values()
        .any(|gained| !gained.kept.is_empty());
    let due = {
        let store = reader.store.borrow();
        store.genesis != Some(genesis)
            || store.due
            || store.broken
            || folds
            || store.journal > store.checkpoint / 2 + JOURNAL_SLACK
    };
    if due {
        return checkpoint(reader, genesis, state);
    }
    if reader.unsaved.is_empty() {
        return Task::ready(Ok(()));
    }
    let record = encode_record(reader);
    reader.unsaved = Default::default();
    append_record(genesis, record, Rc::clone(&reader.store)).task()
}

pub(crate) fn checkpoint(
    reader: &mut Reader,
    genesis: EntryHash,
    state: Option<&Folded>,
) -> Task<'static, Result<()>> {
    let bytes = encode_checkpoint(reader, state);
    reader.unsaved = Default::default();
    write_checkpoint(genesis, bytes, Rc::clone(&reader.store)).task()
}

/// Replaces `view.bin` with `bytes`, then removes the journal it holds.
pub(crate) fn write_checkpoint<'a>(
    genesis: EntryHash,
    bytes: Vec<u8>,
    store: Rc<RefCell<Store>>,
) -> Fallible<'a, ()> {
    let len = bytes.len() as u64;
    flow::replace(Root::Local, Layout::cached_view(genesis), bytes)
        .and_then(move |()| flow::remove_if_present(Root::Local, Layout::view_journal(genesis)))
        .then(move |written| {
            let mut store = store.borrow_mut();
            *store = match written {
                Ok(()) => Store {
                    genesis: Some(genesis),
                    checkpoint: len,
                    ..Store::default()
                },
                Err(_) => Store {
                    genesis: Some(genesis),
                    broken: true,
                    ..store.clone()
                },
            };
            Flow::Done(written)
        })
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
                Ok(()) => store.journal += len,
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::blocking::{run, Backend};
    use crate::disk::MemDisk;
    use crate::ids::Hlc;
    use crate::io::{Capabilities, IoResult, Range};
    use crate::line::{self, Line};
    use crate::log::{EntryKind, Genesis, Logged};

    const W: WriterId = WriterId::from_u128(0xa);

    /// The merged state of every log `reader` holds, as a library keeps it.
    fn merged(reader: &Reader) -> Folded {
        crate::merge::merge(reader.logs().values())
    }

    fn layout() -> Layout {
        Layout::new(".lib").unwrap()
    }

    fn chain(n: usize) -> Vec<Line> {
        let kind = EntryKind::Genesis(Genesis {
            writer: W,
            label: "a".into(),
        });
        let mut lines = vec![Entry::encode(EntryHash::ZERO, Hlc::ZERO, kind)
            .unwrap()
            .line()];
        for i in 0..n {
            let kind = EntryKind::Intent(Logged {
                label: format!("e{i}"),
                ops: Vec::new(),
                displaced: Vec::new(),
                reverses: None,
            });
            let prev = lines.last().unwrap().hash();
            lines.push(Entry::encode(prev, Hlc::ZERO, kind).unwrap().line());
        }
        lines
    }

    fn bytes(lines: &[Line]) -> Vec<u8> {
        lines.iter().flat_map(Line::to_bytes).collect()
    }

    fn segment(name: &str) -> RelPath {
        layout().writer(W).join(name).unwrap()
    }

    fn act(disk: &MemDisk, io: Io) {
        disk.perform(io).unwrap();
    }

    fn put(disk: &MemDisk, root: Root, path: &RelPath, bytes: Vec<u8>) {
        let dir = path.parent().unwrap();
        act(disk, Io::MakeDir { root, path: dir });
        act(
            disk,
            Io::Create {
                root,
                path: path.clone(),
                bytes,
            },
        );
    }

    fn grow(disk: &MemDisk, path: &RelPath, bytes: Vec<u8>) {
        act(
            disk,
            Io::Append {
                root: Root::Folder,
                path: path.clone(),
                bytes,
            },
        );
    }

    fn local(disk: &MemDisk, genesis: EntryHash) {
        act(
            disk,
            Io::MakeDir {
                root: Root::Local,
                path: Layout::local(genesis),
            },
        );
    }

    fn present(disk: &MemDisk, path: RelPath) -> bool {
        disk.files(Root::Local).contains_key(&path)
    }

    fn open(disk: &MemDisk, pool: Vec<EntryHash>, own: EntryHash) -> Reader {
        let loaded = run(&mut disk.clone(), load(pool, Some(own))).unwrap();
        if let Some(state) = &loaded.state {
            let logs = loaded.view.writers().values();
            assert_eq!(
                state,
                &crate::merge::merge(logs),
                "kept as merged from scratch"
            );
        }
        Reader::open(layout(), loaded)
    }

    /// Counts the bytes it is asked to read in each root.
    struct Counting {
        disk: MemDisk,
        read: BTreeMap<Root, u64>,
        paths: Vec<RelPath>,
    }

    impl Counting {
        fn new(disk: &MemDisk) -> Self {
            Self {
                disk: disk.clone(),
                read: BTreeMap::new(),
                paths: Vec::new(),
            }
        }
    }

    impl Backend for Counting {
        fn capabilities(&self, root: Root) -> Capabilities {
            self.disk.capabilities(root)
        }

        fn perform(&mut self, io: Io) -> IoResult {
            let reads = match &io {
                Io::Read { path, range, .. } => vec![(path, range)],
                Io::ReadMany { reads, .. } => {
                    reads.iter().map(|(path, range)| (path, range)).collect()
                }
                _ => Vec::new(),
            };
            for (path, Range { len, .. }) in reads {
                *self.read.entry(io.root()).or_default() += len;
                self.paths.push(path.clone());
            }
            self.disk.perform(io)
        }
    }

    /// A reader of a folder holding `lines` in two segments, saved whole, then
    /// after the second segment gained a line, saved again in the journal.
    fn saved(disk: &MemDisk, lines: &[Line]) -> Reader {
        let genesis = lines[0].hash();
        put(disk, Root::Folder, &segment("a.jsonl"), bytes(&lines[..2]));
        put(disk, Root::Folder, &segment("b.jsonl"), bytes(&lines[2..4]));
        local(disk, genesis);
        let mut reader = Reader::new(layout(), CachedView::default());
        run(&mut disk.clone(), reader.read()).unwrap();
        run(&mut disk.clone(), {
            let state = merged(&reader);
            reader.checkpoint(genesis, Some(&state))
        })
        .unwrap();
        grow(disk, &segment("b.jsonl"), lines[4].to_bytes());
        run(&mut disk.clone(), reader.read()).unwrap();
        run(&mut disk.clone(), {
            let state = merged(&reader);
            reader.save(genesis, Some(&state))
        })
        .unwrap();
        assert!(present(disk, Layout::view_journal(genesis)));
        reader
    }

    #[test]
    fn a_cached_view_this_build_cannot_trust_is_corrupt() {
        let lines = chain(2);
        let entries = |lines: &[&Line]| -> Vec<Rc<Entry>> {
            let lines = lines.iter().map(|line| Entry::linked((*line).clone()));
            lines.map(Rc::new).collect()
        };
        let held = |held: &[Rc<Entry>]| {
            let log = LogOut {
                snapshots: &[],
                entries: held,
                strays: Vec::new(),
                forks: Vec::new(),
            };
            write_view([(W, log)].into_iter(), Vec::new(), &BTreeSet::new(), None)
        };
        let path = Layout::cached_view(lines[0].hash());
        let good = held(&entries(&[&lines[0], &lines[1]]));
        assert!(CachedView::decode(&path, &good).is_ok());
        let mut flipped = good.clone();
        *flipped.last_mut().unwrap() ^= 1;
        let mut longer = good.clone();
        longer.push(0);
        for bytes in [
            held(&entries(&[&lines[0], &lines[2]])),
            flipped,
            longer,
            good[..good.len() - 1].to_vec(),
            frame(&[FORMAT + 1]),
            Vec::new(),
        ] {
            assert!(
                matches!(
                    CachedView::decode(&path, &bytes),
                    Err(Error::Corrupt { .. })
                ),
                "{bytes:?}"
            );
        }
    }

    #[test]
    fn a_reopened_view_reads_only_the_ends_of_files_it_read_before() {
        let disk = MemDisk::new();
        let lines = chain(4);
        let before = saved(&disk, &lines);
        let mut reader = open(&disk, vec![lines[0].hash()], lines[0].hash());
        assert_eq!(reader.cached(), before.cached());
        let mut counting = Counting::new(&disk);
        let report = run(&mut counting, reader.read()).unwrap();
        assert_eq!(counting.read[&Root::Folder], 2 * line::ENDING);
        assert_eq!(report.placed, BTreeMap::new());
        assert!(!report.changed && !report.folder_changed, "{report:?}");
        assert_eq!(reader.removed(), before.removed());
    }

    #[test]
    fn a_torn_journal_record_is_left_out_and_the_view_is_next_written_whole() {
        let disk = MemDisk::new();
        let lines = chain(5);
        let genesis = lines[0].hash();
        let before = saved(&disk, &lines[..5]);
        let journal = Layout::view_journal(genesis);
        let torn = Io::Append {
            root: Root::Local,
            path: journal.clone(),
            bytes: br#"{"writers":{"#.to_vec(),
        };
        act(&disk, torn);
        let mut reader = open(&disk, vec![genesis], genesis);
        assert_eq!(reader.cached(), before.cached());
        grow(&disk, &segment("b.jsonl"), lines[5].to_bytes());
        run(&mut disk.clone(), reader.read()).unwrap();
        run(&mut disk.clone(), {
            let state = merged(&reader);
            reader.save(genesis, Some(&state))
        })
        .unwrap();
        assert!(!present(&disk, journal), "written whole, without a journal");
        let again = open(&disk, vec![genesis], genesis);
        assert_eq!(again.cached(), reader.cached());
    }

    #[test]
    fn a_journal_replayed_onto_the_view_it_led_to_changes_nothing() {
        let disk = MemDisk::new();
        let lines = chain(4);
        let genesis = lines[0].hash();
        let mut reader = saved(&disk, &lines);
        let journal = Layout::view_journal(genesis);
        let kept = disk.files(Root::Local)[&journal].clone();
        run(&mut disk.clone(), {
            let state = merged(&reader);
            reader.checkpoint(genesis, Some(&state))
        })
        .unwrap();
        assert!(!present(&disk, journal.clone()));
        put(&disk, Root::Local, &journal, kept);
        let again = open(&disk, vec![genesis], genesis);
        assert_eq!(again.cached(), reader.cached());
    }

    #[test]
    fn a_retired_writers_view_is_read_until_a_live_view_holds_it() {
        let disk = MemDisk::new();
        let lines = chain(3);
        let (retired, live) = (EntryHash::from_u128(1), EntryHash::from_u128(2));
        let path = segment("a.jsonl");
        put(&disk, Root::Folder, &path, bytes(&lines));
        local(&disk, retired);
        let mut old = Reader::new(layout(), CachedView::default());
        run(&mut disk.clone(), old.read()).unwrap();
        run(&mut disk.clone(), {
            let state = merged(&old);
            old.checkpoint(retired, Some(&state))
        })
        .unwrap();
        put(&disk, Root::Local, &Layout::retired(retired), Vec::new());
        act(
            &disk,
            Io::Remove {
                root: Root::Folder,
                path: path.clone(),
            },
        );
        put(&disk, Root::Folder, &path, bytes(&lines[..2]));
        local(&disk, live);
        let mut reader = open(&disk, vec![retired, live], live);
        run(&mut disk.clone(), reader.read()).unwrap();
        assert_eq!(
            reader.cached(),
            old.cached(),
            "what the retired view showed"
        );
        run(&mut disk.clone(), {
            let state = merged(&reader);
            reader.save(live, Some(&state))
        })
        .unwrap();

        let mut counting = Counting::new(&disk);
        let loaded = run(&mut counting, load(vec![retired, live], Some(live))).unwrap();
        assert_eq!(&loaded.view, old.cached());
        assert!(
            !counting.paths.contains(&Layout::cached_view(retired)),
            "{:?}",
            counting.paths
        );
    }

    #[test]
    fn a_view_that_joined_another_is_next_written_whole() {
        let disk = MemDisk::new();
        let lines = chain(3);
        let (own, other) = (EntryHash::from_u128(1), EntryHash::from_u128(2));
        let path = segment("a.jsonl");
        put(&disk, Root::Folder, &path, bytes(&lines[..2]));
        local(&disk, own);
        local(&disk, other);
        let mut first = Reader::new(layout(), CachedView::default());
        run(&mut disk.clone(), first.read()).unwrap();
        run(&mut disk.clone(), {
            let state = merged(&first);
            first.checkpoint(own, Some(&state))
        })
        .unwrap();
        grow(&disk, &path, lines[2].to_bytes());
        let mut second = Reader::new(layout(), CachedView::default());
        run(&mut disk.clone(), second.read()).unwrap();
        run(&mut disk.clone(), {
            let state = merged(&second);
            second.checkpoint(other, Some(&state))
        })
        .unwrap();
        grow(&disk, &path, lines[3].to_bytes());

        let mut reader = open(&disk, vec![own, other], own);
        run(&mut disk.clone(), reader.read()).unwrap();
        run(&mut disk.clone(), {
            let state = merged(&reader);
            reader.save(own, Some(&state))
        })
        .unwrap();
        assert!(!present(&disk, Layout::view_journal(own)));
        let alone = open(&disk, vec![own], own);
        assert_eq!(alone.cached(), reader.cached());
    }
}
