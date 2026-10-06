//! This instance's writer: the only code that writes under `writers/<w>/` for its
//! own `w`, and it writes nowhere else in toshokan's root.
//!
//! A writer opens a new segment under a random name the first time it appends in a
//! process, appends to it, and seals it when the process closes it cleanly. It
//! deletes only segments this process opened and sealed. Before each append it
//! confirms the folder still holds its head; after each, it records the head in
//! the local root.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::error::{Error, Invalid, Refusal, Result};
use crate::flow::{self, Flow};
use crate::ids::{EntryHash, Hlc, SegmentName, WriterId};
use crate::io::{Io, Kind, Lock, Root, Task};
use crate::layout::Layout;
use crate::log::{Entry, EntryKind, Genesis};
use crate::path::RelPath;
use crate::reader::{CachedView, WriterFile, WriterLog, MAX_FILE};
use crate::report::{Rekey, Start};

pub struct Writer {
    pub(crate) layout: Layout,
    id: WriterId,
    genesis: EntryHash,
    head: EntryHash,
    open: Option<OpenSegment>,
    pub(crate) sealed: Vec<SegmentName>,
}

/// The segment this process appends to.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct OpenSegment {
    pub name: SegmentName,
    pub len: u64,
}

/// What claiming a writer from the pool found.
pub struct Claimed {
    /// `None` when no writer in the pool can continue: the first commit creates one.
    pub writer: Option<Writer>,
    pub start: Start,
}

enum Confirmed {
    /// The open segment is as this process left it.
    Open(OpenSegment),
    /// The open segment holds bytes after the ones this process wrote, and the
    /// folder holds the head: the segment is sealed, and a new one opened.
    Grown,
    /// No segment is open, and the folder holds the head.
    Fresh,
}

/// `head.json` in the writer's directory of the local root.
#[derive(Serialize, Deserialize)]
struct HeadRecord {
    writer: WriterId,
    head: EntryHash,
}

impl Writer {
    /// Takes the first writer in this install's pool, the directories of the local
    /// root in order, whose lock it gets. `logs` must be read from the folder just
    /// before.
    ///
    /// The writer continues only if its history did not fork and the folder still
    /// holds what it last wrote; it then appends after the last entry of its
    /// history. Otherwise it leaves the pool, is never written again, and
    /// [`Claimed::start`] reports it as [`Start::Rekeyed`]. A directory without a
    /// head record, left by a crash while a writer was created, is not in the pool.
    /// Writes nothing in the folder.
    pub fn claim<'a>(
        layout: &Layout,
        logs: &'a BTreeMap<WriterId, WriterLog>,
    ) -> Task<'a, Result<Claimed>> {
        let layout = layout.clone();
        flow::run(Self::pick())
            .and_then(move |picked| match picked {
                None => flow::ok(Claimed {
                    writer: None,
                    start: Start::New,
                }),
                Some(picked) => flow::run(picked.resume(layout, logs)),
            })
            .task()
    }

    /// Locks the first writer of this install's pool that is not retired, whose
    /// lock it gets and whose head record reads; `None` when there is none. The
    /// lock is held until [`Picked::resume`] retires it or the writer closes.
    pub fn pick() -> Task<'static, Result<Option<Picked>>> {
        flow::list(Root::Local, &RelPath::ROOT)
            .and_then(move |entries| {
                let mut pool: Vec<EntryHash> = entries
                    .into_iter()
                    .filter(|entry| entry.kind == Kind::Directory)
                    .filter_map(|entry| entry.name.parse().ok())
                    .collect();
                pool.sort();
                pick_first(pool)
            })
            .task()
    }

    /// The writers of this install's pool that are retired, by genesis entry.
    pub fn retired() -> Task<'static, Result<Vec<EntryHash>>> {
        flow::list(Root::Local, &RelPath::ROOT)
            .and_then(|entries| {
                let pool = entries
                    .into_iter()
                    .filter(|entry| entry.kind == Kind::Directory)
                    .filter_map(|entry| entry.name.parse::<EntryHash>().ok());
                flow::fold(pool, Vec::new(), |mut retired, genesis| {
                    flow::stat(Root::Local, &Layout::retired(genesis)).map_ok(move |marker| {
                        retired.extend(marker.map(|_| genesis));
                        retired
                    })
                })
            })
            .task()
    }

    /// A new writer: its first segment, `segment`, holding its genesis entry, is
    /// made durable in the folder before its directory in the local root, and so
    /// its id, exists anywhere else. `view` is kept as its cached view before its
    /// head record, so a writer in the pool always has the view it started from.
    pub fn create(
        layout: Layout,
        id: WriterId,
        segment: SegmentName,
        label: String,
        at: Hlc,
        view: &CachedView,
    ) -> Task<'static, Result<Writer>> {
        let kind = EntryKind::Genesis(Genesis { writer: id, label });
        let entry = match Entry::encode(EntryHash::ZERO, at, kind) {
            Ok(entry) => entry,
            Err(_) => return Task::ready(Err(Error::Refused(Refusal::Invalid(Invalid::TooLong)))),
        };
        let genesis = entry.hash();
        let saved = view.save(genesis);
        let bytes = entry.line.to_bytes();
        let len = bytes.len() as u64;
        let dir = layout.writer(id);
        let path = layout.segment(id, segment);
        let synced = vec![
            path.clone(),
            dir.clone(),
            layout.writers(),
            layout.root().clone(),
            RelPath::ROOT,
        ];
        let writer = Writer {
            layout,
            id,
            genesis,
            head: genesis,
            open: Some(OpenSegment { name: segment, len }),
            sealed: Vec::new(),
        };
        flow::act(Io::MakeDir {
            root: Root::Folder,
            path: dir,
        })
        .and_then(move |()| {
            flow::act(Io::Create {
                root: Root::Folder,
                path,
                bytes,
            })
        })
        .and_then(move |()| sync_all(Root::Folder, synced))
        .and_then(move |()| {
            flow::act(Io::MakeDir {
                root: Root::Local,
                path: Layout::local(genesis),
            })
        })
        .and_then(move |()| flow::lock(Layout::lock(genesis)))
        .and_then(move |lock| match lock {
            Lock::Acquired => {
                flow::run(saved).and_then(move |()| record_head(id, genesis, genesis))
            }
            Lock::Held => Flow::Done(Err(Error::Io {
                root: Root::Local,
                path: Layout::lock(genesis),
                error: crate::io::IoError::Other("a new writer's lock is held".into()),
            })),
        })
        .and_then(|()| sync_all(Root::Local, vec![RelPath::ROOT]))
        .map_ok(move |()| writer)
        .task()
    }

    pub fn id(&self) -> WriterId {
        self.id
    }

    pub fn genesis(&self) -> EntryHash {
        self.genesis
    }

    pub fn head(&self) -> EntryHash {
        self.head
    }

    pub fn open_segment(&self) -> Option<OpenSegment> {
        self.open
    }

    /// Appends `kinds` as consecutive entries, durably, opening a segment named
    /// `fresh` when none is open, then records the new head in the local root.
    /// Returns the writer with the result.
    ///
    /// Refuses, appending nothing, an entry too long for a line, and fails with
    /// [`Error::Rekey`] when the folder no longer holds this writer's head. A
    /// failed append seals the segment, so a torn line is never followed.
    ///
    /// ⚠️ A failure syncing the entries or recording the head comes after the
    /// entries reached the folder: the writer has moved on to them, and its next
    /// append confirms the folder still holds them.
    pub fn append(
        self,
        kinds: Vec<(Hlc, EntryKind)>,
        fresh: SegmentName,
    ) -> Task<'static, (Writer, Result<Vec<Entry>>)> {
        let mut prev = self.head;
        let mut entries = Vec::with_capacity(kinds.len());
        for (at, kind) in kinds {
            let Ok(entry) = Entry::encode(prev, at, kind) else {
                let refused = Error::Refused(Refusal::Invalid(Invalid::TooLong));
                return Task::ready((self, Err(refused)));
            };
            prev = entry.hash();
            entries.push(entry);
        }
        if entries.is_empty() {
            return Task::ready((self, Ok(entries)));
        }
        let bytes: Vec<u8> = entries
            .iter()
            .flat_map(|entry| entry.line.to_bytes())
            .collect();
        self.confirm()
            .then(move |confirmed| {
                let mut writer = self;
                let open = match confirmed {
                    Err(error) => return Flow::Done((writer, Err(error))),
                    Ok(Confirmed::Open(open)) => Some(open),
                    Ok(Confirmed::Grown) => {
                        writer.seal();
                        None
                    }
                    Ok(Confirmed::Fresh) => None,
                };
                writer.write(open, fresh, bytes, entries)
            })
            .task()
    }

    /// Ends the open segment: this process will not append to it again, and may
    /// delete it once a snapshot folds it. No request.
    pub fn seal(&mut self) {
        if let Some(open) = self.open.take() {
            self.sealed.push(open.name);
        }
    }

    /// Segments this process opened and sealed: the only ones it may delete.
    pub fn sealed(&self) -> &[SegmentName] {
        &self.sealed
    }

    /// Seals the open segment and releases the writer's lock.
    pub fn close(&mut self) -> Task<'static, Result<()>> {
        self.seal();
        flow::act(Io::Unlock {
            name: Layout::lock(self.genesis),
        })
        .task()
    }

    /// Where to append, once the folder is confirmed to hold this writer's head.
    fn confirm<'a>(&self) -> Flow<'a, Result<Confirmed>> {
        let (layout, id, head) = (self.layout.clone(), self.id, self.head);
        let lost = move || {
            Err(Error::Rekey {
                writer: id,
                why: Rekey::Restored,
            })
        };
        let path = self.open.map(|open| layout.segment(id, open.name));
        let held = move |confirmed| {
            holds(&layout, id, head).and_then(move |held| match held {
                true => flow::ok(confirmed),
                false => Flow::Done(lost()),
            })
        };
        let (Some(open), Some(path)) = (self.open, path) else {
            return held(Confirmed::Fresh);
        };
        flow::stat(Root::Folder, &path).and_then(move |meta| match meta {
            Some(meta) if meta.len == open.len => flow::ok(Confirmed::Open(open)),
            Some(meta) if meta.len > open.len => held(Confirmed::Grown),
            _ => Flow::Done(lost()),
        })
    }

    fn write(
        mut self,
        open: Option<OpenSegment>,
        fresh: SegmentName,
        bytes: Vec<u8>,
        entries: Vec<Entry>,
    ) -> Flow<'static, (Writer, Result<Vec<Entry>>)> {
        let len = bytes.len() as u64;
        let segment = open.map_or(fresh, |open| open.name);
        let path = self.layout.segment(self.id, segment);
        let mut synced = vec![path.clone()];
        let request = match open {
            Some(_) => Io::Append {
                root: Root::Folder,
                path,
                bytes,
            },
            None => {
                synced.push(self.layout.writer(self.id));
                Io::Create {
                    root: Root::Folder,
                    path,
                    bytes,
                }
            }
        };
        flow::act(request).then(move |landed| {
            if let Err(error) = landed {
                if open.is_some() {
                    self.seal();
                }
                return Flow::Done((self, Err(error)));
            }
            let start = open.map_or(0, |open| open.len);
            self.open = Some(OpenSegment {
                name: segment,
                len: start + len,
            });
            self.head = entries.last().expect("at least one entry").hash();
            let (id, genesis, head) = (self.id, self.genesis, self.head);
            sync_all(Root::Folder, synced).then(move |result| match result {
                Err(error) => {
                    self.seal();
                    Flow::Done((self, Err(error)))
                }
                Ok(()) => record_head(id, genesis, head)
                    .then(move |recorded| Flow::Done((self, recorded.map(|()| entries)))),
            })
        })
    }
}

/// The entry `log` continues after, or why its writer cannot continue.
fn tip(
    log: &WriterLog,
    genesis: EntryHash,
    recorded: EntryHash,
) -> std::result::Result<EntryHash, Rekey> {
    if log.genesis() != Some(genesis) || !log.holds(recorded) {
        return Err(Rekey::Restored);
    }
    match (log.forks(), log.heads().as_slice()) {
        ([], [tip]) => Ok(*tip),
        _ => Err(Rekey::Forked),
    }
}

fn pick_first<'a>(mut pool: Vec<EntryHash>) -> Flow<'a, Result<Option<Picked>>> {
    if pool.is_empty() {
        return flow::ok(None);
    }
    let genesis = pool.remove(0);
    flow::stat(Root::Local, &Layout::retired(genesis))
        .and_then(move |retired| match retired {
            Some(_) => flow::ok(Lock::Held),
            None => flow::lock(Layout::lock(genesis)),
        })
        .and_then(move |lock| match lock {
            Lock::Held => pick_first(pool),
            Lock::Acquired => {
                flow::read_replaced(Root::Local, Layout::head(genesis)).and_then(move |bytes| {
                    let record =
                        bytes.and_then(|bytes| serde_json::from_slice::<HeadRecord>(&bytes).ok());
                    match record {
                        None => unlock(genesis).and_then(move |()| pick_first(pool)),
                        Some(record) => flow::ok(Some(Picked { genesis, record })),
                    }
                })
            }
        })
}

/// A writer of the pool whose lock this process holds, not yet confirmed to be
/// able to continue.
pub struct Picked {
    genesis: EntryHash,
    record: HeadRecord,
}

impl Picked {
    pub fn genesis(&self) -> EntryHash {
        self.genesis
    }

    pub fn writer(&self) -> WriterId {
        self.record.writer
    }

    /// Continues the writer if its history did not fork and the folder still
    /// holds what it last wrote, or retires it. `logs` must be read from the
    /// folder just before.
    pub fn resume(
        self,
        layout: Layout,
        logs: &BTreeMap<WriterId, WriterLog>,
    ) -> Task<'static, Result<Claimed>> {
        let Picked { genesis, record } = self;
        let id = record.writer;
        let tip = match logs.get(&id) {
            Some(log) => tip(log, genesis, record.head),
            None => Err(Rekey::Restored),
        };
        let tip = match tip {
            Ok(tip) => tip,
            Err(why) => return retire(genesis, id, why).task(),
        };
        holds(&layout, id, tip)
            .and_then(move |held| match held {
                false => retire(genesis, id, Rekey::Restored),
                true => flow::ok(Claimed {
                    writer: Some(Writer {
                        layout,
                        id,
                        genesis,
                        head: tip,
                        open: None,
                        sealed: Vec::new(),
                    }),
                    start: Start::Resumed(id),
                }),
            })
            .task()
    }
}

/// Retires the writer whose genesis entry is `genesis`, which this process
/// writes as: it leaves the pool and is never written again.
pub fn retire_writer(genesis: EntryHash) -> Task<'static, Result<()>> {
    mark_retired(genesis)
        .and_then(move |()| unlock(genesis))
        .task()
}

fn mark_retired<'a>(genesis: EntryHash) -> Flow<'a, Result<()>> {
    let marker = Io::Create {
        root: Root::Local,
        path: Layout::retired(genesis),
        bytes: Vec::new(),
    };
    flow::attempt(marker.clone())
        .then(move |result| match result {
            Ok(_) | Err(crate::io::IoError::AlreadyExists) => flow::ok(()),
            Err(error) => Flow::Done(Err(marker.failed(error))),
        })
        .and_then(move |()| flow::sync(Root::Local, &Layout::local(genesis)))
}

fn retire<'a>(genesis: EntryHash, old: WriterId, why: Rekey) -> Flow<'a, Result<Claimed>> {
    mark_retired(genesis)
        .and_then(move |()| unlock(genesis))
        .map_ok(move |()| Claimed {
            writer: None,
            start: Start::Rekeyed { old, why },
        })
}

fn unlock<'a>(genesis: EntryHash) -> Flow<'a, Result<()>> {
    flow::act(Io::Unlock {
        name: Layout::lock(genesis),
    })
}

/// The writer and head a `head.json` names; `None` when it does not read.
pub(crate) fn head_record(bytes: &[u8]) -> Option<(WriterId, EntryHash)> {
    let record: HeadRecord = serde_json::from_slice(bytes).ok()?;
    Some((record.writer, record.head))
}

fn record_head<'a>(writer: WriterId, genesis: EntryHash, head: EntryHash) -> Flow<'a, Result<()>> {
    let bytes = serde_json::to_vec(&HeadRecord { writer, head }).expect("a head record is JSON");
    flow::replace(Root::Local, Layout::head(genesis), bytes)
}

fn sync_all<'a>(root: Root, paths: Vec<RelPath>) -> Flow<'a, Result<()>> {
    flow::each(paths.into_iter(), move |path| flow::sync(root, &path))
}

/// Whether a file in `writer`'s directory of the folder holds `hash` now, in a
/// segment or folded in a snapshot.
pub(crate) fn holds<'a>(
    layout: &Layout,
    writer: WriterId,
    hash: EntryHash,
) -> Flow<'a, Result<bool>> {
    let dir = layout.writer(writer);
    flow::list(Root::Folder, &dir).and_then(move |entries| {
        let paths: Vec<RelPath> = entries
            .into_iter()
            .filter(|entry| entry.kind == Kind::File)
            .filter_map(|entry| dir.join(&entry.name).ok())
            .collect();
        flow::fold(paths.into_iter(), false, move |found, path| match found {
            true => flow::ok(true),
            false => flow::read_file(Root::Folder, &path, MAX_FILE).map_ok(move |bytes| {
                bytes.is_some_and(|bytes| match WriterFile::parse(&bytes) {
                    WriterFile::Segment { lines, .. } => {
                        lines.iter().any(|line| line.hash() == hash)
                    }
                    WriterFile::Snapshot(snapshot) => snapshot.folds(hash),
                    WriterFile::Unreadable => false,
                })
            }),
        })
    })
}
