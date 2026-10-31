//! This instance's writer: the only code that writes under `writers.2/<w>/` for its
//! own `w`, and it writes nowhere else in toshokan's root.
//!
//! A writer opens a new segment under a random name the first time it appends in a
//! process, appends to it, and seals it with the seal marker when the process
//! closes it cleanly or compacts, or when an append would take it past
//! [`SEGMENT_BOUND`]. Before each append it confirms the folder still
//! holds its head; after each, it records the head in the local root.

use serde::{Deserialize, Serialize};

use crate::derive::Placed;
use crate::error::{Error, Invalid, Refusal, Result};
use crate::flow::{self, Fallible, Flow};
use crate::ids::{EntryHash, Hlc, SegmentName, WriterId};
use crate::io::{Io, Kind, Lock, Meta, Range, Root, Task};
use crate::layout::Layout;
use crate::line;
use crate::log::{Entry, EntryKind, Genesis, Own};
use crate::path::RelPath;
use crate::reader::{Reader, Stamp, WriterFile, MAX_FILE};
use crate::report::{Rekey, Start};

/// The length past which an append seals the open segment and opens another. A
/// picked folder in Chromium copies a whole file to append to it, so this bounds
/// an append's copy.
pub const SEGMENT_BOUND: u64 = 1 << 20;

pub struct Writer {
    pub(crate) layout: Layout,
    id: WriterId,
    genesis: EntryHash,
    head: Own,
    open: Option<OpenSegment>,
    /// A file this process knew to hold its head.
    holder: Option<Holder>,
    /// The file the last failed write went to, which it may have reached.
    failed: Option<Failed>,
}

/// A write whose request failed: the file it went to, and the length that file
/// has if the write landed whole. One cut short leaves a torn line, no entry.
#[derive(Clone, PartialEq, Eq, Debug)]
struct Failed {
    path: RelPath,
    whole: u64,
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
    /// The open segment no longer is as this process left it, as when a sync
    /// client kept an older version under its name, and the folder holds the
    /// head elsewhere: the segment is left, and a new one opened.
    Changed,
    /// No segment is open, or the open one is gone, and the folder holds the
    /// head.
    Fresh,
}

/// `head.json` in the writer's directory of the local root, the one file there: the
/// writer's lock is taken on it.
#[derive(Serialize, Deserialize)]
struct HeadRecord {
    writer: WriterId,
    head: EntryHash,
    /// The writer is never written again.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    retired: bool,
}

impl Writer {
    /// Takes the first writer in this install's pool, the directories of the local
    /// root in order, whose lock it gets. `reader` must have read the folder just
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

    /// The id of the pool's writer `genesis`, from its head record, where that reads.
    pub fn id_of(genesis: EntryHash) -> Task<'static, Result<Option<WriterId>>> {
        read_head(genesis)
            .map_ok(|record| record.map(|record| record.writer))
            .task()
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
    /// its id, exists anywhere else.
    pub fn create(
        layout: Layout,
        id: WriterId,
        segment: SegmentName,
        label: String,
        at: Hlc,
    ) -> Task<'static, Result<Writer>> {
        let kind = EntryKind::Genesis(Genesis { writer: id, label });
        let entry = match Entry::encode(EntryHash::ZERO, 0, at, kind) {
            Ok(entry) => entry,
            Err(_) => return Task::ready(Err(Error::Refused(Refusal::Invalid(Invalid::TooLong)))),
        };
        let genesis = entry.hash();
        let bytes = entry.to_bytes();
        let len = bytes.len() as u64;
        let dir = layout.writer(id);
        let path = layout.segment(id, segment);
        let mut synced = vec![path.clone(), dir.clone(), layout.writers()];
        synced.extend(std::iter::successors(
            Some(layout.root().clone()),
            RelPath::parent,
        ));
        let writer = Writer {
            failed: None,
            holder: Some(Holder {
                path: path.clone(),
                len,
                tail: line::ending(genesis),
            }),
            layout,
            id,
            genesis,
            head: Own {
                n: 0,
                hash: genesis,
            },
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
        .and_then(move |()| flow::lock(Layout::head(genesis)))
        .and_then(move |lock| match lock {
            Lock::Acquired => record_head(id, genesis, genesis),
            Lock::Held => Flow::Done(Err(Error::Io {
                root: Root::Local,
                path: Layout::head(genesis),
                error: crate::io::IoError::Other("a new writer's lock is held".into()),
            })),
        })
        .and_then(move |()| sync_all(Root::Local, vec![Layout::local(genesis), RelPath::ROOT]))
        .map_ok(move |()| writer)
        .task()
    }

    pub fn id(&self) -> WriterId {
        self.id
    }

    pub fn genesis(&self) -> EntryHash {
        self.genesis
    }

    pub fn head(&self) -> Own {
        self.head
    }

    pub fn open_segment(&self) -> Option<OpenSegment> {
        self.open
    }

    /// The open segment, `None` when none is open, with the stamp a read would
    /// find on it now; no stamp, for the next read to read it again, when the
    /// stat fails or the segment is no longer as long as this process left it.
    pub(crate) fn stamp<'a>(&self) -> Flow<'a, Option<(RelPath, Option<Stamp>)>> {
        let Some(open) = self.open else {
            return Flow::Done(None);
        };
        let path = self.layout.segment(self.id, open.name);
        let tail = line::ending(self.head.hash);
        flow::stat(Root::Folder, &path).then(move |meta| {
            let meta = meta.ok().flatten().filter(|meta| meta.len == open.len);
            let stamp = meta.and_then(|meta| meta.modified).map(|modified| Stamp {
                len: open.len,
                modified,
                tail,
            });
            Flow::Done(Some((path, stamp)))
        })
    }

    /// Appends `kinds` as consecutive entries, durably, opening a segment named
    /// `fresh` when none is open. Returns the writer with the result. The caller
    /// takes the entries into its reader, then calls [`Writer::record`].
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
            let encoded = prev
                .n
                .checked_add(1)
                .and_then(|n| Entry::encode(prev.hash, n, at, kind).ok());
            let Some(entry) = encoded else {
                let refused = Error::Refused(Refusal::Invalid(Invalid::TooLong));
                return Task::ready((self, Err(refused)));
            };
            prev = entry.own();
            entries.push(entry);
        }
        if entries.is_empty() {
            return Task::ready((self, Ok(entries)));
        }
        let bytes: Vec<u8> = entries.iter().flat_map(|entry| entry.to_bytes()).collect();
        let len = bytes.len() as u64;
        self.confirm()
            .then(move |confirmed| {
                let mut writer = self;
                let open = match confirmed {
                    Err(error) => return Flow::Done((writer, Err(error))),
                    Ok(Confirmed::Open(open)) if open.len + len > SEGMENT_BOUND => {
                        return writer
                            .seal()
                            .then(move |(writer, _)| writer.write(None, fresh, bytes, entries));
                    }
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

    /// Records this writer's head in the local root: `head.json`.
    pub fn record(&self) -> Task<'static, Result<()>> {
        record_head(self.id, self.genesis, self.head.hash).task()
    }

    /// Seals the open segment and releases the writer's lock.
    pub fn close(self) -> Task<'static, Result<()>> {
        let genesis = self.genesis;
        self.seal()
            .then(move |(_, sealed)| {
                unlock(genesis).then(move |unlocked| Flow::Done(sealed.and(unlocked)))
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
        let head = self.head.hash;
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

    /// Where to append, once the folder is confirmed to hold this writer's head
    /// and not to hold what its last failed write would have added.
    fn confirm<'a>(&self) -> Flow<'a, Result<Confirmed>> {
        let Some(failed) = self.failed.clone() else {
            return self.confirm_held();
        };
        let (id, held) = (self.id, self.confirm_held());
        flow::stat(Root::Folder, &failed.path).and_then(move |meta| {
            match meta.is_some_and(|meta| meta.len >= failed.whole) {
                true => Flow::Done(Err(Error::Rekey {
                    writer: id,
                    why: Rekey::Landed,
                })),
                false => held,
            }
        })
    }

    /// Where to append, once the folder is confirmed to hold this writer's head.
    fn confirm_held<'a>(&self) -> Flow<'a, Result<Confirmed>> {
        let (id, head) = (self.id, self.head.hash);
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
            Some(_) => held(Confirmed::Changed),
            None => held(Confirmed::Fresh),
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
        let start = open.map_or(0, |open| open.len);
        flow::act(request).then(move |landed| {
            if let Err(error) = landed {
                self.leave();
                let whole = start + len;
                self.failed = Some(Failed { path, whole });
                return Flow::Done((self, Err(error)));
            }
            self.failed = None;
            self.open = Some(OpenSegment {
                name: segment,
                len: start + len,
            });
            self.head = entries.last().expect("at least one entry").own();
            self.held_by(path, start + len, line::ending(self.head.hash));
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

/// The entry the writer whose history the reader places as `placed` continues
/// after, or why it cannot continue.
fn tip(
    placed: &Placed,
    genesis: EntryHash,
    recorded: EntryHash,
) -> std::result::Result<Own, Rekey> {
    let tips_ours = placed.tips.iter().all(|tip| tip.genesis == genesis);
    let first_ours = placed
        .merged
        .first()
        .is_none_or(|first| first.n() != 0 || first.hash() == genesis);
    if !tips_ours || !first_ours || !placed.holds(recorded) {
        return Err(Rekey::Restored);
    }
    if placed.fork.is_some() {
        return Err(Rekey::Forked);
    }
    match placed.head() {
        Some(head) if placed.reaches(recorded) => Ok(head),
        _ => Err(Rekey::Restored),
    }
}

fn pick_first<'a>(mut pool: Vec<EntryHash>) -> Flow<'a, Result<Option<Picked>>> {
    if pool.is_empty() {
        return flow::ok(None);
    }
    let genesis = pool.remove(0);
    flow::lock(Layout::head(genesis)).and_then(move |lock| match lock {
        Lock::Held => pick_first(pool),
        Lock::Acquired => read_head(genesis).and_then(move |record| match record {
            Some(record) if !record.retired => flow::ok(Some(Picked { genesis, record })),
            _ => unlock(genesis).and_then(move |()| pick_first(pool)),
        }),
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
        let tip = match reader.derived().writers.get(&id) {
            Some(placed) => tip(placed, genesis, record.head),
            None => Err(Rekey::Restored),
        };
        let tip = match tip {
            Ok(tip) => tip,
            Err(why) => return retire(genesis, id, why).task(),
        };
        let writer = Writer {
            failed: None,
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
fn holder(reader: &Reader, writer: WriterId, head: Own) -> Option<Holder> {
    reader
        .files(writer)
        .into_iter()
        .find_map(|(path, stamp, file)| {
            let stamp = stamp?;
            let held = match file {
                WriterFile::Segment(segment) => {
                    segment.entries.iter().any(|entry| entry.own() == head)
                }
                WriterFile::Snapshot(snapshot) => snapshot.writer == writer && snapshot.names(head),
                WriterFile::Unreadable { .. } => false,
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

/// Records in `head.json` that the writer is never written again. One whose record
/// does not read is never continued anyway, and is left as it is.
fn mark_retired<'a>(genesis: EntryHash) -> Flow<'a, Result<()>> {
    read_head(genesis).and_then(move |record| match record {
        Some(record) => write_head(
            genesis,
            &HeadRecord {
                retired: true,
                ..record
            },
        ),
        None => flow::ok(()),
    })
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
        name: Layout::head(genesis),
    })
}

/// The writer and head a `head.json` names; `None` when it does not read.
pub(crate) fn head_record(bytes: &[u8]) -> Option<(WriterId, EntryHash)> {
    let record: HeadRecord = serde_json::from_slice(bytes).ok()?;
    Some((record.writer, record.head))
}

/// The record `head.json` of the writer `genesis` holds; `None` where none reads.
fn read_head<'a>(genesis: EntryHash) -> Fallible<'a, Option<HeadRecord>> {
    flow::read_file(Root::Local, &Layout::head(genesis), u64::MAX)
        .map_ok(|bytes| bytes.and_then(|bytes| serde_json::from_slice(&bytes).ok()))
}

/// Records `head` as the last entry `writer` wrote.
fn record_head<'a>(writer: WriterId, genesis: EntryHash, head: EntryHash) -> Flow<'a, Result<()>> {
    let record = HeadRecord {
        writer,
        head,
        retired: false,
    };
    write_head(genesis, &record)
}

/// Writes `record` over `head.json` in place, or as a new file where none is.
///
/// ⚠️ Never replaced: the writer's lock is held on the file, and a new file at its
/// path would be one another process could lock. Every record of one writer has the
/// same length but the retired one, which is longer and written last, so no write
/// leaves the end of an older record behind.
fn write_head<'a>(genesis: EntryHash, record: &HeadRecord) -> Flow<'a, Result<()>> {
    let mut bytes = serde_json::to_vec(record).expect("a head record is JSON");
    let path = Layout::head(genesis);
    flow::stat(Root::Local, &path).and_then(move |meta| {
        let write = match meta {
            Some(Meta {
                kind: Kind::File,
                len,
                ..
            }) => {
                // A longer record, as a newer build may have written, is padded over
                // with spaces, which JSON reads past.
                let held = usize::try_from(len).unwrap_or(usize::MAX);
                if held > bytes.len() {
                    bytes.resize(held, b' ');
                }
                Io::Write {
                    root: Root::Local,
                    path: path.clone(),
                    offset: 0,
                    bytes,
                }
            }
            _ => Io::Create {
                root: Root::Local,
                path: path.clone(),
                bytes,
            },
        };
        flow::act(write).and_then(move |()| flow::sync(Root::Local, &path))
    })
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

/// Whether a file in `writer`'s directory of the folder holds `head` now, in a
/// segment or named by a snapshot: first any file whose last bytes end the line
/// `head` names, then any file read whole.
pub(crate) fn holds<'a>(layout: &Layout, writer: WriterId, head: Own) -> Flow<'a, Result<bool>> {
    let dir = layout.writer(writer);
    let ending = line::ending(head.hash);
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
                            segment.entries.iter().any(|entry| entry.own() == head)
                        }
                        WriterFile::Snapshot(snapshot) => snapshot.names(head),
                        WriterFile::Unreadable { .. } => false,
                    })
                }),
            })
        })
    })
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use super::*;

    /// `head.json` is written over in place, so a shorter record would leave the end of
    /// a longer one behind it.
    #[test]
    fn every_live_head_record_has_one_length_and_the_retired_one_is_longer() {
        let len = |n: u128, retired| {
            let record = HeadRecord {
                writer: WriterId::from_u128(n),
                head: EntryHash::from_u128(n),
                retired,
            };
            serde_json::to_vec(&record).unwrap().len()
        };
        let live: BTreeSet<usize> = [0, 1, 0x1234, u128::MAX]
            .into_iter()
            .map(|n| len(n, false))
            .collect();
        assert_eq!(live.len(), 1, "{live:?}");
        assert!(len(0, true) > len(0, false));
    }
}
