//! The cached view in the local root: `view.json`, a checkpoint of the whole view
//! with what the reader knew of each file, and `view.log`, a journal of what the
//! view gained since, one record a line.
//!
//! A save appends what the view gained to the journal and syncs it. The view is
//! written whole again, and the journal removed, when the journal has grown past
//! half the checkpoint, when a record could not be appended, when the view keeps a
//! new snapshot, which folds entries the checkpoint holds whole, and when the view
//! holds what another view of the install brought. Loading replays the journal
//! onto the checkpoint; replaying a record twice changes nothing, so a crash
//! between writing a checkpoint and removing the journal leaves a view that loads.

use std::cell::RefCell;
use std::collections::{BTreeMap, BTreeSet};
use std::io::Write as _;
use std::rc::Rc;

use serde::{Deserialize, Serialize};
use serde_json::value::RawValue;

use crate::error::{Error, Result};
use crate::flow::{self, Fallible, Flow};
use crate::ids::{EntryHash, WriterId};
use crate::io::{Io, IoError, Root, Task};
use crate::layout::Layout;
use crate::log::Entry;
use crate::path::RelPath;
use crate::reader::{self, CachedView, Reader, Record, Segment, Stamp, WriterFile, WriterLog};
use crate::schema::Raw;
use crate::snapshot::Snapshot;

/// A stored view, or why it cannot be read.
type Parsed<T> = std::result::Result<T, String>;

/// How far past half the checkpoint's length the journal grows before the view
/// is written whole again.
const JOURNAL_SLACK: u64 = 1 << 20;

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
}

/// One writer's saved view.
pub(crate) struct Kept {
    pub(crate) view: CachedView,
    files: BTreeMap<RelPath, StoredFile>,
    absorbed: BTreeSet<EntryHash>,
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
        checkpoint: checkpoint.as_ref().map_or(0, |bytes| bytes.len() as u64),
        journal: journal.as_ref().map_or(0, |bytes| bytes.len() as u64),
        broken: false,
    };
    if let Some(bytes) = checkpoint {
        let stored: StoredView = serde_json::from_slice(&bytes).map_err(|e| e.to_string())?;
        kept.absorbed = stored.absorbed.iter().copied().collect();
        kept.apply(stored)?;
    }
    let journal = journal.unwrap_or_default();
    let mut rest = &journal[..];
    while !rest.is_empty() {
        let Some(end) = rest.iter().position(|&b| b == b'\n') else {
            kept.broken = true;
            break;
        };
        let Ok(record) = serde_json::from_slice::<StoredView>(&rest[..end]) else {
            kept.broken = true;
            break;
        };
        kept.apply(record)?;
        rest = &rest[end + 1..];
    }
    Ok(kept)
}

impl Kept {
    fn apply(&mut self, stored: StoredView) -> Parsed<()> {
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
    let stored: StoredView = serde_json::from_slice(bytes).map_err(|e| e.to_string())?;
    restore_logs(view, stored.writers)
}

fn restore_logs(view: &mut CachedView, logs: BTreeMap<WriterId, StoredLog>) -> Parsed<()> {
    for (writer, stored) in logs {
        let snapshots = stored
            .snapshots
            .iter()
            .map(|raw| Snapshot::decode(raw.get().as_bytes()).map(Rc::new))
            .collect::<std::result::Result<Vec<_>, _>>()
            .map_err(|error| error.to_string())?;
        if snapshots.iter().any(|snapshot| snapshot.writer != writer) {
            return Err(format!("a snapshot of {writer} names another writer"));
        }
        let entries = stored
            .entries
            .into_iter()
            .map(|(json, hash)| decode_line(json, hash))
            .collect::<Parsed<Vec<_>>>()?;
        view.restore(writer, snapshots, entries, stored.strays, stored.forks)?;
    }
    Ok(())
}

/// A line this install read and verified before, as it kept it.
fn decode_line(json: Box<RawValue>, hash: EntryHash) -> Parsed<Rc<Entry>> {
    let json = String::from(Box::<str>::from(json));
    let entry = Entry::kept(json, hash).map_err(|error| error.to_string())?;
    Ok(Rc::new(entry))
}

/// A checkpoint or a journal record. A checkpoint holds every log whole; a
/// record holds what each log gained, and files whose records changed.
#[derive(Deserialize)]
struct StoredView {
    writers: BTreeMap<WriterId, StoredLog>,
    #[serde(default)]
    files: BTreeMap<RelPath, Option<StoredFile>>,
    #[serde(default)]
    absorbed: Vec<EntryHash>,
}

#[derive(Deserialize)]
struct StoredLog {
    snapshots: Vec<Box<RawValue>>,
    /// Each line's JSON verbatim, and its hash.
    entries: Vec<(Box<RawValue>, EntryHash)>,
    strays: Vec<(EntryHash, EntryHash)>,
    forks: Vec<(EntryHash, [EntryHash; 2])>,
}

/// What a reader knew of one file: its stamp, and its lines or its snapshot.
#[derive(Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub struct StoredFile {
    len: u64,
    modified: u64,
    /// The file's last bytes, in hexadecimal.
    tail: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    segment: Option<StoredSegment>,
    /// The last entry the snapshot folds: the reader's kept snapshot with it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    snapshot: Option<EntryHash>,
}

/// A segment as a run of the writer's chain, `count` entries from `first` to
/// `last`. Lines the cached view holds no entry for are kept whole.
#[derive(Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
struct StoredSegment {
    count: usize,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    first: Option<EntryHash>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    last: Option<EntryHash>,
    end: u64,
    sealed: bool,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    lines: Vec<(Raw, EntryHash)>,
}

impl StoredFile {
    /// What `record` says of its file, if it can be said against `log`: a
    /// segment read to its end whose lines follow one another, or a snapshot
    /// `log` keeps.
    fn of(record: &Record, log: Option<&WriterLog>) -> Option<Self> {
        let stamp = record.stamp.as_ref()?;
        let (segment, snapshot) = match &record.file {
            WriterFile::Segment(segment) if segment.stop.is_none() && segment.contiguous() => {
                (Some(StoredSegment::of(segment, log)?), None)
            }
            WriterFile::Snapshot(snapshot) => {
                let head = snapshot.head()?;
                let kept = log?
                    .snapshots()
                    .iter()
                    .any(|kept| kept.head() == Some(head));
                (None, kept.then_some(head))
            }
            WriterFile::Segment(_) | WriterFile::Unreadable => return None,
        };
        if segment.is_none() && snapshot.is_none() {
            return None;
        }
        Some(Self {
            len: stamp.len,
            modified: stamp.modified,
            tail: hex(&stamp.tail),
            segment,
            snapshot,
        })
    }

    /// The record this describes, with each line taken from `log` or from the
    /// lines kept whole; `None` when any is missing.
    pub(crate) fn resolve(&self, log: &WriterLog) -> Option<Record> {
        let stamp = Stamp {
            len: self.len,
            modified: self.modified,
            tail: unhex(&self.tail)?,
        };
        let file = match (&self.segment, self.snapshot) {
            (Some(segment), None) => WriterFile::Segment(segment.resolve(log)?),
            (None, Some(head)) => {
                let kept = log
                    .snapshots()
                    .iter()
                    .find(|kept| kept.head() == Some(head))?;
                WriterFile::Snapshot(Rc::clone(kept))
            }
            _ => return None,
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
            let json = Raw::new(entry.line.json()).expect("a verified line holds JSON");
            whole.push((json, hash));
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
        let mut whole = BTreeMap::new();
        for (json, hash) in &self.lines {
            let entry = Entry::kept(json.as_str().to_owned(), *hash).ok()?;
            whole.insert(*hash, Rc::new(entry));
        }
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

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

fn unhex(text: &str) -> Option<Vec<u8>> {
    if !text.len().is_multiple_of(2) {
        return None;
    }
    (0..text.len())
        .step_by(2)
        .map(|at| u8::from_str_radix(text.get(at..at + 2)?, 16).ok())
        .collect()
}

/// One log, or what it gained, as written.
struct LogOut<'a> {
    snapshots: &'a [Rc<Snapshot>],
    entries: &'a [Rc<Entry>],
    strays: Vec<(EntryHash, EntryHash)>,
    forks: Vec<(EntryHash, [EntryHash; 2])>,
}

impl<'a> LogOut<'a> {
    fn whole(log: &'a WriterLog) -> Self {
        Self {
            snapshots: log.snapshots(),
            entries: log.entries(),
            strays: log.strays().iter().map(|(h, p)| (*h, *p)).collect(),
            forks: log.forks().iter().map(|f| (f.prev, f.branches)).collect(),
        }
    }

    fn gained(placement: &'a reader::Placement) -> Self {
        Self {
            snapshots: &placement.kept,
            entries: &placement.placed,
            strays: placement.strayed.clone(),
            forks: placement
                .forks
                .iter()
                .map(|f| (f.prev, f.branches))
                .collect(),
        }
    }

    /// Lines are written verbatim, not as JSON strings, so reading them back
    /// unescapes nothing.
    fn write(&self, out: &mut Vec<u8>) {
        out.extend_from_slice(b"{\"snapshots\":[");
        for (n, snapshot) in self.snapshots.iter().enumerate() {
            if n > 0 {
                out.push(b',');
            }
            out.extend(snapshot.encode());
        }
        out.extend_from_slice(b"],\"entries\":[");
        for (n, entry) in self.entries.iter().enumerate() {
            if n > 0 {
                out.push(b',');
            }
            out.push(b'[');
            out.extend_from_slice(entry.line.json().as_bytes());
            write!(out, ",\"{}\"]", entry.hash()).expect("writing to memory");
        }
        out.extend_from_slice(b"],\"strays\":");
        serde_json::to_writer(&mut *out, &self.strays).expect("hashes are JSON");
        out.extend_from_slice(b",\"forks\":");
        serde_json::to_writer(&mut *out, &self.forks).expect("hashes are JSON");
        out.push(b'}');
    }
}

fn write_logs<'a>(out: &mut Vec<u8>, logs: impl Iterator<Item = (WriterId, LogOut<'a>)>) {
    out.extend_from_slice(b"{\"writers\":{");
    for (n, (writer, log)) in logs.enumerate() {
        if n > 0 {
            out.push(b',');
        }
        write!(out, "\"{writer}\":").expect("writing to memory");
        log.write(out);
    }
    out.push(b'}');
}

/// `view` as a checkpoint holding nothing else.
pub(crate) fn encode_view(view: &CachedView) -> Vec<u8> {
    let mut out = Vec::new();
    let logs = view.writers().iter();
    write_logs(
        &mut out,
        logs.map(|(writer, log)| (*writer, LogOut::whole(log))),
    );
    out.push(b'}');
    out
}

fn write_rest<T: Serialize>(out: &mut Vec<u8>, name: &str, value: &T) {
    write!(out, ",\"{name}\":").expect("writing to memory");
    serde_json::to_writer(&mut *out, value).expect("a stored view is JSON");
}

fn encode_checkpoint(reader: &Reader) -> Vec<u8> {
    let mut out = Vec::new();
    let logs = reader.logs().iter();
    write_logs(
        &mut out,
        logs.map(|(writer, log)| (*writer, LogOut::whole(log))),
    );
    let files: BTreeMap<&RelPath, StoredFile> = reader
        .records()
        .filter_map(|(path, record)| {
            let log = reader.writer_of(path).and_then(|w| reader.logs().get(&w));
            Some((path, StoredFile::of(record, log)?))
        })
        .collect();
    write_rest(&mut out, "files", &files);
    write_rest(&mut out, "absorbed", &reader.absorbed);
    out.push(b'}');
    out
}

fn encode_record(reader: &Reader) -> Vec<u8> {
    let mut out = Vec::new();
    let logs = reader.unsaved.logs.iter();
    write_logs(
        &mut out,
        logs.map(|(writer, gained)| (*writer, LogOut::gained(gained))),
    );
    let files: BTreeMap<&RelPath, Option<StoredFile>> = reader
        .unsaved
        .files
        .iter()
        .map(|path| {
            let log = reader.writer_of(path).and_then(|w| reader.logs().get(&w));
            let stored = reader.record(path).and_then(|r| StoredFile::of(r, log));
            (path, stored)
        })
        .collect();
    write_rest(&mut out, "files", &files);
    out.extend_from_slice(b"}\n");
    out
}

pub(crate) fn save(reader: &mut Reader, genesis: EntryHash) -> Task<'static, Result<()>> {
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
        return checkpoint(reader, genesis);
    }
    if reader.unsaved.is_empty() {
        return Task::ready(Ok(()));
    }
    let record = encode_record(reader);
    reader.unsaved = Default::default();
    append_record(genesis, record, Rc::clone(&reader.store)).task()
}

pub(crate) fn checkpoint(reader: &mut Reader, genesis: EntryHash) -> Task<'static, Result<()>> {
    let bytes = encode_checkpoint(reader);
    reader.unsaved = Default::default();
    write_checkpoint(genesis, bytes, Rc::clone(&reader.store)).task()
}

/// Replaces `view.json` with `bytes`, then removes the journal it holds.
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

    fn layout() -> Layout {
        Layout::new(".lib").unwrap()
    }

    fn chain(n: usize) -> Vec<Line> {
        let kind = EntryKind::Genesis(Genesis {
            writer: W,
            label: "a".into(),
        });
        let mut lines = vec![
            Entry::encode(EntryHash::ZERO, Hlc::ZERO, kind)
                .unwrap()
                .line,
        ];
        for i in 0..n {
            let kind = EntryKind::Intent(Logged {
                label: format!("e{i}"),
                ops: Vec::new(),
                displaced: Vec::new(),
                reverses: None,
            });
            let prev = lines.last().unwrap().hash();
            lines.push(Entry::encode(prev, Hlc::ZERO, kind).unwrap().line);
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
        run(&mut disk.clone(), reader.checkpoint(genesis)).unwrap();
        grow(disk, &segment("b.jsonl"), lines[4].to_bytes());
        run(&mut disk.clone(), reader.read()).unwrap();
        run(&mut disk.clone(), reader.save(genesis)).unwrap();
        assert!(present(disk, Layout::view_journal(genesis)));
        reader
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
        run(&mut disk.clone(), reader.save(genesis)).unwrap();
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
        run(&mut disk.clone(), reader.checkpoint(genesis)).unwrap();
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
        run(&mut disk.clone(), old.checkpoint(retired)).unwrap();
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
        run(&mut disk.clone(), reader.save(live)).unwrap();

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
        run(&mut disk.clone(), first.checkpoint(own)).unwrap();
        grow(&disk, &path, lines[2].to_bytes());
        let mut second = Reader::new(layout(), CachedView::default());
        run(&mut disk.clone(), second.read()).unwrap();
        run(&mut disk.clone(), second.checkpoint(other)).unwrap();
        grow(&disk, &path, lines[3].to_bytes());

        let mut reader = open(&disk, vec![own, other], own);
        run(&mut disk.clone(), reader.read()).unwrap();
        run(&mut disk.clone(), reader.save(own)).unwrap();
        assert!(!present(&disk, Layout::view_journal(own)));
        let alone = open(&disk, vec![own], own);
        assert_eq!(alone.cached(), reader.cached());
    }
}
