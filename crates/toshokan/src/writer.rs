//! This instance's writer: the only code that writes under `writers/<w>/` for its
//! own `w`, and it writes nowhere else in toshokan's root.
//!
//! A writer opens a new segment under a random name the first time it appends in a
//! process, appends to it, and seals it with the seal marker when the process
//! closes it cleanly or compacts. Before each append it confirms the folder still
//! holds its head; after each, once the cached view holds the new entries, it
//! records the head in the local root.

use serde::{Deserialize, Serialize};

use crate::error::{Error, Invalid, Refusal, Result};
use crate::flow::{self, Fallible, Flow};
use crate::ids::{EntryHash, Hlc, SegmentName, WriterId};
use crate::io::{Io, Kind, Lock, Range, Root, Task};
use crate::layout::Layout;
use crate::line;
use crate::log::{Entry, EntryKind, Genesis};
use crate::path::RelPath;
use crate::reader::{Reader, WriterFile, WriterLog, MAX_FILE};
use crate::report::{Rekey, Start};

pub struct Writer {
    pub(crate) layout: Layout,
    id: WriterId,
    genesis: EntryHash,
    head: EntryHash,
    open: Option<OpenSegment>,
    /// A file this process knew to hold its head.
    holder: Option<Holder>,
}

/// A file known to hold a writer's head while its length and last bytes are
/// as they were.
#[derive(Clone, PartialEq, Eq, Debug)]
struct Holder {
    path: RelPath,
    len: u64,
    tail: Vec<u8>,
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
    /// The open segment no longer ends as this process left it, and the folder
    /// holds the head: the segment is left, and a new one opened.
    Changed,
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
    pub fn claim<'a>(layout: &Layout, reader: &'a Reader) -> Task<'a, Result<Claimed>> {
        let layout = layout.clone();
        flow::run(Self::pick())
            .and_then(move |picked| match picked {
                None => flow::ok(Claimed {
                    writer: None,
                    start: Start::New,
                }),
                Some(picked) => flow::run(picked.resume(layout, reader)),
            })
            .task()
    }

    /// Locks the first writer of this install's pool that is not retired, whose
    /// lock it gets and whose head record reads; `None` when there is none. The
    /// lock is held until [`Picked::resume`] retires it or the writer closes.
    pub fn pick() -> Task<'static, Result<Option<Picked>>> {
        flow::run(Self::pool()).and_then(pick_first).task()
    }

    /// Every writer this install has a directory for in its local root, live or
    /// retired, by genesis entry in order.
    pub fn pool() -> Task<'static, Result<Vec<EntryHash>>> {
        flow::list(Root::Local, &RelPath::ROOT)
            .map_ok(|entries| {
                let mut pool: Vec<EntryHash> = entries
                    .into_iter()
                    .filter(|entry| entry.kind == Kind::Directory)
                    .filter_map(|entry| entry.name.parse().ok())
                    .collect();
                pool.sort();
                pool
            })
            .task()
    }

    /// A new writer: its first segment, `segment`, holding its genesis entry, is
    /// made durable in the folder before its directory in the local root, and so
    /// its id, exists anywhere else. The view `view` writes for the genesis entry
    /// is kept as its cached view before its head record, so a writer in the pool
    /// always has the view it started from.
    pub fn create(
        layout: Layout,
        id: WriterId,
        segment: SegmentName,
        label: String,
        at: Hlc,
        view: impl FnOnce(EntryHash) -> Task<'static, Result<()>>,
    ) -> Task<'static, Result<Writer>> {
        let kind = EntryKind::Genesis(Genesis { writer: id, label });
        let entry = match Entry::encode(EntryHash::ZERO, at, kind) {
            Ok(entry) => entry,
            Err(_) => return Task::ready(Err(Error::Refused(Refusal::Invalid(Invalid::TooLong)))),
        };
        let genesis = entry.hash();
        let saved = view(genesis);
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
            holder: Some(Holder {
                path: path.clone(),
                len,
                tail: line::ending(genesis),
            }),
            layout,
            id,
            genesis,
            head: genesis,
            open: Some(OpenSegment { name: segment, len }),
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
    /// `fresh` when none is open. Returns the writer with the result. The caller
    /// keeps the entries in the cached view, then calls [`Writer::record`].
    ///
    /// Refuses, appending nothing, an entry too long for a line, and fails with
    /// [`Error::Rekey`] when the folder no longer holds this writer's head. A
    /// failed append leaves the segment, so a torn line is never followed.
    ///
    /// ⚠️ A failure syncing the entries comes after they reached the folder: the
    /// writer has moved on to them, and its next append confirms the folder still
    /// holds them.
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
                    Ok(Confirmed::Changed) => {
                        writer.leave();
                        None
                    }
                    Ok(Confirmed::Fresh) => None,
                };
                writer.write(open, fresh, bytes, entries)
            })
            .task()
    }

    /// Stops appending to the open segment and leaves it without a seal marker:
    /// the next append opens another. No request.
    pub fn leave(&mut self) {
        self.open = None;
    }

    /// Records this writer's head in the local root: `head.json`, replaced.
    pub fn record(&self) -> Task<'static, Result<()>> {
        record_head(self.id, self.genesis, self.head).task()
    }

    /// Seals the open segment and releases the writer's lock.
    pub fn close(self) -> Task<'static, Result<()>> {
        let genesis = self.genesis;
        self.seal()
            .then(move |(_, sealed)| {
                flow::act(Io::Unlock {
                    name: Layout::lock(genesis),
                })
                .then(move |unlocked| Flow::Done(sealed.and(unlocked)))
            })
            .task()
    }

    /// Appends the seal marker to the open segment and syncs it, once the segment
    /// is confirmed as this process left it; otherwise leaves it without one.
    /// Either way nothing more is appended to it. Returns the writer with the
    /// result.
    pub(crate) fn seal<'a>(mut self) -> Flow<'a, (Writer, Result<()>)> {
        let Some(open) = self.open.take() else {
            return Flow::Done((self, Ok(())));
        };
        let path = self.layout.segment(self.id, open.name);
        let head = self.head;
        let marked = flow::stat(Root::Folder, &path)
            .and_then({
                let path = path.clone();
                move |meta| match meta {
                    Some(meta) if meta.len == open.len => ends_with(path, open.len, head),
                    _ => flow::ok(false),
                }
            })
            .and_then({
                let path = path.clone();
                move |left| match left {
                    false => flow::ok(false),
                    true => flow::act(Io::Append {
                        root: Root::Folder,
                        path: path.clone(),
                        bytes: line::seal_marker(head),
                    })
                    .and_then(move |()| flow::sync(Root::Folder, &path))
                    .map_ok(|()| true),
                }
            });
        marked.then(move |marked| {
            if let Ok(true) = marked {
                self.held_by(path, open.len + line::SEAL_MARKER, line::ending(head));
            }
            Flow::Done((self, marked.map(|_| ())))
        })
    }

    /// Remembers `path`, `len` bytes long and ending with `tail`, as holding
    /// this writer's head.
    pub(crate) fn held_by(&mut self, path: RelPath, len: u64, tail: Vec<u8>) {
        self.holder = Some(Holder { path, len, tail });
    }

    /// Where to append, once the folder is confirmed to hold this writer's head.
    fn confirm<'a>(&self) -> Flow<'a, Result<Confirmed>> {
        let (id, head) = (self.id, self.head);
        let lost = move || {
            Err(Error::Rekey {
                writer: id,
                why: Rekey::Restored,
            })
        };
        let check = self.head_held();
        let held = move |confirmed| {
            check.and_then(move |held| match held {
                true => flow::ok(confirmed),
                false => Flow::Done(lost()),
            })
        };
        let Some(open) = self.open else {
            return held(Confirmed::Fresh);
        };
        let path = self.layout.segment(id, open.name);
        flow::stat(Root::Folder, &path).and_then(move |meta| match meta {
            Some(meta) if meta.len == open.len => {
                ends_with(path, open.len, head).and_then(move |ends| match ends {
                    true => flow::ok(Confirmed::Open(open)),
                    false => held(Confirmed::Changed),
                })
            }
            Some(meta) if meta.len > open.len => held(Confirmed::Changed),
            _ => Flow::Done(lost()),
        })
    }

    /// Whether a file of this writer's directory holds its head: the file it last
    /// knew to hold it, unchanged, or any file, judged by its last bytes first.
    pub(crate) fn head_held<'a>(&self) -> Fallible<'a, bool> {
        let (layout, id, head) = (self.layout.clone(), self.id, self.head);
        let any = move || holds(&layout, id, head);
        let Some(holder) = self.holder.clone() else {
            return any();
        };
        let tail = Range {
            offset: holder.len.saturating_sub(line::ENDING),
            len: holder.len.min(line::ENDING),
        };
        flow::stat(Root::Folder, &holder.path)
            .and_then(move |meta| match meta {
                Some(meta) if meta.len == holder.len => {
                    flow::read_present(Root::Folder, &holder.path, tail)
                        .map_ok(move |bytes| bytes == Some(holder.tail))
                }
                _ => flow::ok(false),
            })
            .and_then(move |unchanged| match unchanged {
                true => flow::ok(true),
                false => any(),
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
                path: path.clone(),
                bytes,
            },
            None => {
                synced.push(self.layout.writer(self.id));
                Io::Create {
                    root: Root::Folder,
                    path: path.clone(),
                    bytes,
                }
            }
        };
        flow::act(request).then(move |landed| {
            if let Err(error) = landed {
                self.leave();
                return Flow::Done((self, Err(error)));
            }
            let start = open.map_or(0, |open| open.len);
            self.open = Some(OpenSegment {
                name: segment,
                len: start + len,
            });
            self.head = entries.last().expect("at least one entry").hash();
            self.held_by(path, start + len, line::ending(self.head));
            sync_all(Root::Folder, synced).then(move |result| match result {
                Err(error) => {
                    self.leave();
                    Flow::Done((self, Err(error)))
                }
                Ok(()) => Flow::Done((self, Ok(entries))),
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
    /// holds what it last wrote, or retires it. `reader` must have read the
    /// folder just before.
    pub fn resume(self, layout: Layout, reader: &Reader) -> Task<'static, Result<Claimed>> {
        let Picked { genesis, record } = self;
        let id = record.writer;
        let tip = match reader.logs().get(&id) {
            Some(log) => tip(log, genesis, record.head),
            None => Err(Rekey::Restored),
        };
        let tip = match tip {
            Ok(tip) => tip,
            Err(why) => return retire(genesis, id, why).task(),
        };
        let writer = Writer {
            holder: holder(reader, id, tip),
            layout,
            id,
            genesis,
            head: tip,
            open: None,
        };
        writer
            .head_held()
            .and_then(move |held| match held {
                false => retire(genesis, id, Rekey::Restored),
                true => flow::ok(Claimed {
                    writer: Some(writer),
                    start: Start::Resumed(id),
                }),
            })
            .task()
    }
}

/// A file the reader's last read found holding `head` of `writer`'s chain.
fn holder(reader: &Reader, writer: WriterId, head: EntryHash) -> Option<Holder> {
    reader
        .files(writer)
        .into_iter()
        .find_map(|(path, stamp, file)| {
            let stamp = stamp?;
            let held = match file {
                WriterFile::Segment(segment) => {
                    segment.entries.iter().any(|entry| entry.hash() == head)
                }
                WriterFile::Snapshot(snapshot) => snapshot.writer == writer && snapshot.folds(head),
                WriterFile::Unreadable => false,
            };
            held.then(|| Holder {
                path: path.clone(),
                len: stamp.len,
                tail: stamp.tail.clone(),
            })
        })
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

/// Whether the file at `path`, `len` bytes long, ends with the line whose hash is
/// `hash`.
fn ends_with<'a>(path: RelPath, len: u64, hash: EntryHash) -> Flow<'a, Result<bool>> {
    let Some(offset) = len.checked_sub(line::ENDING) else {
        return flow::ok(false);
    };
    let range = Range {
        offset,
        len: line::ENDING,
    };
    flow::read_present(Root::Folder, &path, range)
        .map_ok(move |bytes| bytes.is_some_and(|bytes| bytes == line::ending(hash)))
}

/// Whether a file in `writer`'s directory of the folder holds `hash` now, in a
/// segment or folded in a snapshot: first any file whose last bytes end the line
/// `hash` names, then any file read whole.
pub(crate) fn holds<'a>(
    layout: &Layout,
    writer: WriterId,
    hash: EntryHash,
) -> Flow<'a, Result<bool>> {
    let dir = layout.writer(writer);
    let ending = line::ending(hash);
    flow::list(Root::Folder, &dir).and_then(move |entries| {
        let paths: Vec<RelPath> = entries
            .into_iter()
            .filter(|entry| entry.kind == Kind::File)
            .filter_map(|entry| dir.join(&entry.name).ok())
            .collect();
        let whole = paths.clone();
        flow::fold(paths.into_iter(), false, move |found, path| match found {
            true => flow::ok(true),
            false => flow::stat(Root::Folder, &path.clone()).and_then({
                let ending = ending.clone();
                move |meta| {
                    let Some(len) = meta.map(|meta| meta.len).filter(|len| *len >= line::ENDING)
                    else {
                        return flow::ok(false);
                    };
                    let tail = Range {
                        offset: len - line::ENDING,
                        len: line::ENDING,
                    };
                    flow::read_present(Root::Folder, &path, tail)
                        .map_ok(move |bytes| bytes == Some(ending))
                }
            }),
        })
        .and_then(move |found| {
            flow::fold(whole.into_iter(), found, move |found, path| match found {
                true => flow::ok(true),
                false => flow::read_file(Root::Folder, &path, MAX_FILE).map_ok(move |bytes| {
                    bytes.is_some_and(|bytes| match WriterFile::parse(&bytes) {
                        WriterFile::Segment(segment) => {
                            segment.entries.iter().any(|entry| entry.hash() == hash)
                        }
                        WriterFile::Snapshot(snapshot) => snapshot.folds(hash),
                        WriterFile::Unreadable => false,
                    })
                }),
            })
        })
    })
}
