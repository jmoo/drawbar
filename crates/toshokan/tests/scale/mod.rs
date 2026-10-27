//! The library generator, request counts and counting allocator that the
//! `scale` example and the no-growth test share.

// The example uses all of it; the test uses part.
#![allow(dead_code)]

use std::alloc::{GlobalAlloc, Layout as Alloc, System};
use std::cell::RefCell;
use std::collections::BTreeMap;
use std::rc::Rc;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering::Relaxed};

use toshokan::blocking::Backend;
use toshokan::compaction::{snapshot, Found};
use toshokan::io::Capabilities;
use toshokan::library::COMPACT_AFTER;
use toshokan::log::{Displaced, Entry, EntryKind, FileFact, Genesis, Logged, Op, Own, Ref};
use toshokan::snapshot::Snapshot;
use toshokan::{
    EntityId, EntryHash, Hlc, Identity, Io, IoResult, Layout, Nonce, Raw, RelPath, Reply, Root,
    SegmentName, WriterId,
};

pub struct Counting;

pub static CURRENT: AtomicUsize = AtomicUsize::new(0);
pub static PEAK: AtomicUsize = AtomicUsize::new(0);
pub static VOLUME: AtomicU64 = AtomicU64::new(0);
pub static COUNT: AtomicU64 = AtomicU64::new(0);

pub fn grew(by: usize) {
    let now = CURRENT.fetch_add(by, Relaxed) + by;
    PEAK.fetch_max(now, Relaxed);
    VOLUME.fetch_add(by as u64, Relaxed);
    COUNT.fetch_add(1, Relaxed);
}

unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, layout: Alloc) -> *mut u8 {
        grew(layout.size());
        unsafe { System.alloc(layout) }
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Alloc) {
        CURRENT.fetch_sub(layout.size(), Relaxed);
        unsafe { System.dealloc(ptr, layout) }
    }

    unsafe fn realloc(&self, ptr: *mut u8, layout: Alloc, new_size: usize) -> *mut u8 {
        CURRENT.fetch_sub(layout.size(), Relaxed);
        grew(new_size);
        unsafe { System.realloc(ptr, layout, new_size) }
    }
}

#[global_allocator]
pub static ALLOCATOR: Counting = Counting;

/// Requests by kind and root: how many, and the bytes they carried.
#[derive(Clone, Default)]
pub struct IoStats(pub BTreeMap<(&'static str, Root), (u64, u64)>);

impl IoStats {
    pub fn total(&self) -> u64 {
        self.0.values().map(|(n, _)| n).sum()
    }

    pub fn bytes(&self, kinds: &[&str]) -> u64 {
        let named = self.0.iter().filter(|((kind, _), _)| kinds.contains(kind));
        named.map(|(_, (_, bytes))| bytes).sum()
    }

    pub fn local_bytes(&self) -> u64 {
        let local = self.0.iter().filter(|((_, root), _)| *root == Root::Local);
        local.map(|(_, (_, bytes))| bytes).sum()
    }

    /// `Kind count` per kind, roots summed, with bytes where a kind moves any.
    pub fn shown(&self) -> String {
        let mut by_kind: BTreeMap<&str, (u64, u64)> = BTreeMap::new();
        for ((kind, _), (n, bytes)) in &self.0 {
            let sum = by_kind.entry(kind).or_default();
            sum.0 += n;
            sum.1 += bytes;
        }
        let order = [
            "List",
            "Stat",
            "ListStat",
            "Read",
            "ReadMany",
            "Create",
            "Append",
            "Write",
            "Rename",
            "Remove",
            "RemoveDir",
            "MakeDir",
            "Sync",
            "Lock",
            "Unlock",
        ];
        order
            .iter()
            .filter_map(|kind| {
                let (n, bytes) = by_kind.get(kind)?;
                Some(match (*kind, bytes) {
                    (_, 0) | ("List" | "ListStat", _) => format!("{kind} {n}"),
                    _ => format!("{kind} {n} ({})", size(*bytes)),
                })
            })
            .collect::<Vec<_>>()
            .join(", ")
    }
}

pub fn size(bytes: u64) -> String {
    match bytes {
        b if b >= 1 << 30 => format!("{:.2} GB", b as f64 / (1u64 << 30) as f64),
        b if b >= 1 << 20 => format!("{:.1} MB", b as f64 / (1u64 << 20) as f64),
        b if b >= 1 << 10 => format!("{:.1} kB", b as f64 / 1024.0),
        b => format!("{b} B"),
    }
}

/// A backend whose every request is counted in `stats`.
pub struct Counted {
    pub inner: Box<dyn Backend>,
    pub stats: Rc<RefCell<IoStats>>,
}

pub fn kind_of(io: &Io) -> (&'static str, u64) {
    match io {
        Io::List { .. } => ("List", 0),
        Io::Stat { .. } => ("Stat", 0),
        Io::ListStat { .. } => ("ListStat", 0),
        Io::Read { .. } => ("Read", 0),
        Io::ReadMany { .. } => ("ReadMany", 0),
        Io::Create { bytes, .. } => ("Create", bytes.len() as u64),
        Io::Append { bytes, .. } => ("Append", bytes.len() as u64),
        Io::Write { bytes, .. } => ("Write", bytes.len() as u64),
        Io::Fill { .. } => ("Fill", 0),
        Io::Rename { .. } => ("Rename", 0),
        Io::Remove { .. } => ("Remove", 0),
        Io::RemoveDir { .. } => ("RemoveDir", 0),
        Io::MakeDir { .. } => ("MakeDir", 0),
        Io::Sync { .. } => ("Sync", 0),
        Io::Lock { .. } => ("Lock", 0),
        Io::Unlock { .. } => ("Unlock", 0),
    }
}

impl Backend for Counted {
    fn capabilities(&self, root: Root) -> Capabilities {
        self.inner.capabilities(root)
    }

    fn perform(&mut self, io: Io) -> IoResult {
        let (kind, sent) = kind_of(&io);
        let root = io.root();
        let result = self.inner.perform(io);
        let got = match &result {
            Ok(Reply::Bytes(bytes)) => bytes.len() as u64,
            Ok(Reply::ReadMany(read)) => {
                read.iter().flatten().map(|bytes| bytes.len() as u64).sum()
            }
            Ok(Reply::Listed(entries)) => entries.len() as u64,
            Ok(Reply::ListedStat(entries)) => entries.len() as u64,
            _ => 0,
        };
        let mut stats = self.stats.borrow_mut();
        let slot = stats.0.entry((kind, root)).or_default();
        slot.0 += 1;
        slot.1 += sent + got;
        result
    }
}

/// A small deterministic generator, so a run can be repeated.
pub struct Rng(pub u64);

impl Rng {
    pub fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }

    pub fn below(&mut self, n: usize) -> usize {
        (self.next() % n as u64) as usize
    }

    pub fn id(&mut self) -> u128 {
        (u128::from(self.next()) << 64) | u128::from(self.next())
    }
}

/// A writer, which compacts as a writer does: every [`COMPACT_AFTER`] own
/// entries it seals its segment, writes a snapshot, and deletes the segments and
/// the snapshot the new one certifies.
pub struct WriterGen {
    pub id: WriterId,
    pub label: String,
    pub genesis: EntryHash,
    pub head: Own,
    pub segment: Vec<u8>,
    /// The open segment's lines.
    pub lines: Vec<Rc<Entry>>,
    /// The segments written since the last snapshot, with their lines and
    /// whether they end with a seal marker.
    pub written: Vec<(RelPath, Vec<Rc<Entry>>, bool)>,
    pub since: Vec<Rc<Entry>>,
    pub snapshot: Option<(RelPath, Snapshot)>,
    pub in_segment: usize,
    pub bytes: u64,
    pub segments: usize,
    pub entries: usize,
}

pub struct EntityGen {
    pub id: EntityId,
    pub name: Ref,
    pub rating: Option<Ref>,
    pub tags: Vec<(String, Ref)>,
}

/// A library file as the facts record it.
#[derive(Clone)]
pub struct FileGen {
    pub path: RelPath,
    pub len: u64,
    pub modified: Option<u64>,
    pub identity: Identity,
}

pub struct Gen {
    pub layout: Layout,
    pub rng: Rng,
    pub at: u64,
    pub segment_entries: usize,
    /// How many tag values entities draw from.
    pub tags: usize,
    /// Whether writers compact as a writer does, or leave every entry in
    /// segments, as one that never closed.
    pub compacts: bool,
    pub writers: Vec<WriterGen>,
    pub entities: Vec<EntityGen>,
    pub sink: Box<dyn Backend>,
    /// While generating, the files the folder will hold, kept here so that what
    /// compaction deletes is never written; `None` once they are written.
    pub staged: Option<BTreeMap<RelPath, Vec<u8>>>,
    /// The pending records generated entries closed.
    pub records: usize,
}

impl Gen {
    pub fn put(&mut self, path: &RelPath, bytes: Vec<u8>) {
        if let Some(staged) = &mut self.staged {
            staged.insert(path.clone(), bytes);
            return;
        }
        let dir = path.parent().unwrap();
        let made = self.sink.perform(Io::MakeDir {
            root: Root::Folder,
            path: dir,
        });
        made.unwrap();
        let created = self.sink.perform(Io::Create {
            root: Root::Folder,
            path: path.clone(),
            bytes,
        });
        created.unwrap();
    }

    pub fn remove(&mut self, path: &RelPath) {
        if let Some(staged) = &mut self.staged {
            staged.remove(path);
            return;
        }
        let removed = self.sink.perform(Io::Remove {
            root: Root::Folder,
            path: path.clone(),
        });
        removed.unwrap();
    }

    pub fn add_writer(&mut self) -> usize {
        let id = WriterId::from_u128(self.rng.id());
        let label = format!("writer {}", self.writers.len());
        let kind = EntryKind::Genesis(Genesis {
            writer: id,
            label: label.clone(),
        });
        self.at += 1;
        let at = Hlc {
            wall_ms: self.at,
            counter: 0,
        };
        let entry = Rc::new(Entry::encode(EntryHash::ZERO, 0, at, kind).unwrap());
        self.writers.push(WriterGen {
            id,
            label,
            genesis: entry.hash(),
            head: entry.own(),
            segment: entry.to_bytes(),
            lines: vec![Rc::clone(&entry)],
            written: Vec::new(),
            since: vec![entry],
            snapshot: None,
            in_segment: 1,
            bytes: 0,
            segments: 0,
            entries: 1,
        });
        self.writers.len() - 1
    }

    pub fn push(&mut self, w: usize, logged: Logged) -> Ref {
        self.at += 1;
        let at = Hlc {
            wall_ms: self.at,
            counter: 0,
        };
        let writer = &mut self.writers[w];
        let (prev, n) = (writer.head.hash, writer.head.n + 1);
        let entry = Rc::new(Entry::encode(prev, n, at, EntryKind::Intent(logged)).unwrap());
        writer.head = entry.own();
        writer.segment.extend(entry.to_bytes());
        writer.lines.push(Rc::clone(&entry));
        writer.since.push(Rc::clone(&entry));
        writer.in_segment += 1;
        writer.entries += 1;
        let named = entry.own().of(writer.id);
        if self.compacts && writer.since.len() >= COMPACT_AFTER {
            self.compact(w);
        } else if writer.in_segment >= self.segment_entries {
            self.seal(w);
        }
        named
    }

    /// Ends the open segment with its seal marker and writes it.
    pub fn seal(&mut self, w: usize) {
        let writer = &mut self.writers[w];
        if writer.lines.is_empty() {
            return;
        }
        let marker = toshokan::line::seal_marker(writer.head.hash);
        writer.segment.extend(marker);
        self.write(w, true);
    }

    /// Writes the open segment as it is, as a writer still running leaves it.
    pub fn flush(&mut self, w: usize) {
        self.write(w, false);
    }

    pub fn write(&mut self, w: usize, sealed: bool) {
        let bytes = std::mem::take(&mut self.writers[w].segment);
        if bytes.is_empty() {
            return;
        }
        let name = SegmentName::from_u128(self.rng.id());
        let path = self.layout.segment(self.writers[w].id, name);
        let writer = &mut self.writers[w];
        writer.bytes += bytes.len() as u64;
        writer.segments += 1;
        writer.in_segment = 0;
        let lines = std::mem::take(&mut writer.lines);
        writer.written.push((path.clone(), lines, sealed));
        self.put(&path, bytes);
    }

    /// Seals the open segment, folds the entries since the last snapshot into a
    /// new one, and deletes the segments and the snapshot it certifies.
    pub fn compact(&mut self, w: usize) {
        self.seal(w);
        let writer = &self.writers[w];
        let previous = writer.snapshot.as_ref().map(|(_, snapshot)| snapshot);
        let lines = writer
            .written
            .iter()
            .map(|(_, lines, _)| Found::Lines(lines));
        let found = lines.chain(previous.map(Found::Snapshot));
        let label = writer.label.clone();
        let folded = snapshot(
            writer.id,
            writer.genesis,
            label,
            previous,
            &writer.since,
            found,
        );
        let folded = folded.expect("entries to fold");
        let path = self
            .layout
            .snapshot(writer.id, Nonce::from_u128(self.rng.id()));
        if self.staged.is_none() {
            self.put(&path, folded.encode());
        }
        let writer = &mut self.writers[w];
        let written = writer.written.drain(..);
        let gone: Vec<RelPath> = written
            .filter(|(_, _, sealed)| *sealed)
            .map(|(path, ..)| path)
            .collect();
        let replaced = writer
            .snapshot
            .replace((path, folded))
            .map(|(path, _)| path);
        writer.since.clear();
        for path in gone.iter().chain(&replaced) {
            self.remove(path);
        }
    }

    /// Writes each writer's open segment as it is, and every file generating
    /// left, its last snapshot among them.
    pub fn flush_all(&mut self) {
        for w in 0..self.writers.len() {
            self.flush(w);
        }
        let mut staged = self.staged.take().unwrap_or_default();
        for writer in &self.writers {
            if let Some((path, snapshot)) = &writer.snapshot {
                staged.insert(path.clone(), snapshot.encode());
            }
        }
        for (path, bytes) in staged {
            self.put(&path, bytes);
        }
    }

    /// Creates `count` entities, ten to an entry, the first of them with `files`.
    pub fn create(&mut self, count: usize, files: &[FileGen]) {
        let start = self.entities.len();
        for chunk in (start..start + count).collect::<Vec<_>>().chunks(10) {
            let w = (chunk[0] / 10) % self.writers.len();
            let mut ops = Vec::new();
            let mut made = Vec::new();
            for &i in chunk {
                let entity = EntityId::from_u128(self.rng.id());
                ops.push(Op::Create {
                    entity,
                    replaces: Vec::new(),
                    take: None,
                });
                let write = |key: &str, value: Raw| Op::Write {
                    entity,
                    key: key.into(),
                    value: Some(value),
                    replaces: Vec::new(),
                    take: None,
                };
                ops.push(write("name", Raw::of(&format!("Program {i}")).unwrap()));
                ops.push(write(
                    "origin",
                    Raw::of(&format!("Bank {}", i % 97)).unwrap(),
                ));
                let rated = i % 3 != 0;
                if rated {
                    ops.push(write("rating", Raw::of(&(i as u32 % 5)).unwrap()));
                }
                let mut tags = Vec::new();
                for t in 0..1 + self.rng.below(5) {
                    let tag = format!("t{}", (i * 7 + t * 13) % self.tags);
                    ops.push(Op::Add {
                        entity,
                        key: "tags".into(),
                        value: Raw::of(&tag).unwrap(),
                        replaces: Vec::new(),
                        take: None,
                    });
                    tags.push(tag);
                }
                for _ in 0..self.rng.below(3) {
                    if self.entities.is_empty() {
                        break;
                    }
                    let other = self.entities[self.rng.below(self.entities.len())].id;
                    ops.push(Op::Add {
                        entity,
                        key: "related".into(),
                        value: Raw::of(&other.to_string()).unwrap(),
                        replaces: Vec::new(),
                        take: None,
                    });
                }
                if let Some(file) = files.get(i - start) {
                    ops.push(Op::File {
                        entity,
                        file: Some(FileFact {
                            path: file.path.clone(),
                            identity: file.identity,
                            len: file.len,
                            modified: file.modified,
                        }),
                        replaces: Vec::new(),
                        take: None,
                    });
                }
                made.push((entity, rated, tags));
            }
            let hash = self.push(
                w,
                Logged {
                    label: "Import".into(),
                    ops,
                    displaced: Vec::new(),
                    forks: Vec::new(),
                    record: None,
                },
            );
            for (id, rated, tags) in made {
                self.entities.push(EntityGen {
                    id,
                    name: hash,
                    rating: rated.then_some(hash),
                    tags: tags.into_iter().map(|tag| (tag, hash)).collect(),
                });
            }
        }
    }

    /// `count` one-op entries, by `only` or by any writer, each a rating, a name,
    /// a tag added or a tag removed, one in twenty closing a pending record.
    pub fn edit(&mut self, count: usize, only: Option<usize>) {
        for _ in 0..count {
            let w = only.unwrap_or_else(|| self.rng.below(self.writers.len()));
            let e = self.rng.below(self.entities.len());
            let entity = self.entities[e].id;
            let choice = self.rng.below(10);
            let (op, update): (Op, fn(&mut EntityGen, Ref, &str)) = match choice {
                0..5 => (
                    Op::Write {
                        entity,
                        key: "rating".into(),
                        value: Some(Raw::of(&(self.rng.below(5) as u32)).unwrap()),
                        replaces: self.entities[e].rating.into_iter().collect(),
                        take: None,
                    },
                    |e, hash, _| e.rating = Some(hash),
                ),
                5 => (
                    Op::Write {
                        entity,
                        key: "name".into(),
                        value: Some(Raw::of(&format!("Renamed {}", self.rng.next())).unwrap()),
                        replaces: vec![self.entities[e].name],
                        take: None,
                    },
                    |e, hash, _| e.name = hash,
                ),
                6..9 => {
                    let tag = format!("t{}", self.rng.below(self.tags));
                    let tags = self.entities[e].tags.iter();
                    let replaces = tags.filter(|(t, _)| *t == tag).map(|(_, at)| *at).collect();
                    (
                        Op::Add {
                            entity,
                            key: "tags".into(),
                            value: Raw::of(&tag).unwrap(),
                            replaces,
                            take: None,
                        },
                        |e, at, tag| {
                            e.tags.retain(|(t, _)| t != tag);
                            e.tags.push((tag.to_owned(), at));
                        },
                    )
                }
                _ => match self.entities[e].tags.first().cloned() {
                    Some((tag, _)) => {
                        let tags = self.entities[e].tags.iter();
                        let tags = tags.filter(|(t, _)| *t == tag).map(|(_, h)| *h).collect();
                        (
                            Op::Remove {
                                entity,
                                key: "tags".into(),
                                value: Raw::of(&tag).unwrap(),
                                tags,
                                take: None,
                            },
                            |e, _, tag| e.tags.retain(|(t, _)| t != tag),
                        )
                    }
                    None => continue,
                },
            };
            let tag = match &op {
                Op::Add { value, .. } | Op::Remove { value, .. } => {
                    value.decode::<String>().unwrap()
                }
                _ => String::new(),
            };
            // One in twenty closes a pending record, as a commit that moves a file does.
            let record = (self.rng.below(20) == 0).then(|| Nonce::from_u128(self.rng.id()));
            self.records += usize::from(record.is_some());
            let hash = self.push(
                w,
                Logged {
                    label: "Edit".into(),
                    ops: vec![op],
                    displaced: Vec::new(),
                    forks: Vec::new(),
                    record,
                },
            );
            update(&mut self.entities[e], hash, &tag);
        }
    }

    /// `count` items in writer `w`'s trash, logged as displaced a hundred to an
    /// entry, each with a file of 64 bytes in the trash.
    pub fn trash(&mut self, w: usize, count: usize) {
        let id = self.writers[w].id;
        for chunk in (0..count).collect::<Vec<_>>().chunks(100) {
            let mut displaced = Vec::new();
            for &i in chunk {
                let item = Nonce::from_u128(self.rng.id());
                let bytes = vec![i as u8; 64];
                self.put(&self.layout.trash(id, item), bytes);
                displaced.push(Displaced {
                    item,
                    from: RelPath::new(&format!("gone/{i}.bin")).unwrap(),
                    identity: Identity::from_u128(i as u128),
                    len: 64,
                });
            }
            self.push(
                w,
                Logged {
                    label: "Save".into(),
                    ops: Vec::new(),
                    displaced,
                    forks: Vec::new(),
                    record: None,
                },
            );
        }
    }
}
