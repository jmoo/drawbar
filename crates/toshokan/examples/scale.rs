//! Generates libraries at scale and measures each operation: wall time, memory
//! and allocation, and the requests it makes of its backend by kind.
//!
//! ```text
//! cargo run --release --example scale -- [--native DIR] [--entities N] [--entries N]
//!     [--writers N] [--files N] [--corpus DIR] [--stale-mtime] [--trash N]
//!     [--segment N] [--retired N] [--refresh N,N] [--runs N] [--label TEXT]
//! ```
//!
//! Without `--native` the library lives on an in-memory disk. Each request is
//! counted by kind as it reaches the backend; a browser driver pays one async
//! round trip per request, so the counts predict its cost.

use std::alloc::{GlobalAlloc, Layout as Alloc, System};
use std::cell::RefCell;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering::Relaxed};
use std::time::Instant;

use toshokan::binding::{self, Scan};
use toshokan::blocking::{run, Backend, Library, Native};
use toshokan::env::{ExactNames, OsRandom, PrefixIdentity, SystemClock};
use toshokan::io::{Capabilities, Kind, Range};
use toshokan::log::{Displaced, Entry, EntryKind, FileFact, Genesis, Logged, Op};
use toshokan::simulator::Machine;
use toshokan::{
    EntityId, EntryHash, Env, Expect, Hlc, Identify, Identity, Io, IoResult, Layout, MemDisk,
    Nonce, Policy, Raw, Register, RelPath, Reply, Root, Schema, SegmentName, Set, WriterId,
};

struct Counting;

static CURRENT: AtomicUsize = AtomicUsize::new(0);
static PEAK: AtomicUsize = AtomicUsize::new(0);
static VOLUME: AtomicU64 = AtomicU64::new(0);
static COUNT: AtomicU64 = AtomicU64::new(0);

fn grew(by: usize) {
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
static ALLOCATOR: Counting = Counting;

/// Requests by kind and root: how many, and the bytes they carried.
#[derive(Clone, Default)]
struct IoStats(BTreeMap<(&'static str, Root), (u64, u64)>);

impl IoStats {
    fn total(&self) -> u64 {
        self.0.values().map(|(n, _)| n).sum()
    }

    fn bytes(&self, kinds: &[&str]) -> u64 {
        let named = self.0.iter().filter(|((kind, _), _)| kinds.contains(kind));
        named.map(|(_, (_, bytes))| bytes).sum()
    }

    fn local_bytes(&self) -> u64 {
        let local = self.0.iter().filter(|((_, root), _)| *root == Root::Local);
        local.map(|(_, (_, bytes))| bytes).sum()
    }

    /// `Kind count` per kind, roots summed, with bytes where a kind moves any.
    fn shown(&self) -> String {
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

fn size(bytes: u64) -> String {
    match bytes {
        b if b >= 1 << 30 => format!("{:.2} GB", b as f64 / (1u64 << 30) as f64),
        b if b >= 1 << 20 => format!("{:.1} MB", b as f64 / (1u64 << 20) as f64),
        b if b >= 1 << 10 => format!("{:.1} kB", b as f64 / 1024.0),
        b => format!("{b} B"),
    }
}

/// A backend whose every request is counted in `stats`.
struct Counted {
    inner: Box<dyn Backend>,
    stats: Rc<RefCell<IoStats>>,
}

fn kind_of(io: &Io) -> (&'static str, u64) {
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

/// Where the library lives: an in-memory folder with one in-memory local root per
/// install, or two directories of the machine's file system.
enum World {
    Mem {
        folder: MemDisk,
        locals: BTreeMap<String, MemDisk>,
    },
    Native {
        folder: PathBuf,
        locals: PathBuf,
    },
}

impl World {
    fn raw(&mut self, local: &str) -> Box<dyn Backend> {
        match self {
            Self::Mem { folder, locals } => {
                let local = locals.entry(local.to_owned()).or_default().clone();
                Box::new(Machine {
                    folder: folder.clone(),
                    local,
                })
            }
            Self::Native { folder, locals } => {
                let local = locals.join(local);
                std::fs::create_dir_all(&local).expect("a local root");
                Box::new(Native::new(folder.clone(), local))
            }
        }
    }

    fn counted(&mut self, local: &str, stats: &Rc<RefCell<IoStats>>) -> Counted {
        Counted {
            inner: self.raw(local),
            stats: Rc::clone(stats),
        }
    }
}

const NAME: Register<String> = Register::new("name");
const ORIGIN: Register<String> = Register::new("origin");
const RATING: Register<u32> = Register::new("rating");
const TAGS: Set<String> = Set::new("tags");
const RELATED: Set<String> = Set::new("related");

fn schema() -> Schema {
    Schema::of(&[
        NAME.key(),
        ORIGIN.key(),
        RATING.key(),
        TAGS.key(),
        RELATED.key(),
    ])
    .unwrap()
}

fn env() -> Env {
    Env {
        clock: Box::new(SystemClock),
        random: Box::new(OsRandom::new()),
        identify: Rc::new(PrefixIdentity::default()),
        names: Box::new(ExactNames),
        label: "scale".into(),
    }
}

fn layout() -> Layout {
    Layout::new(".tk").unwrap()
}

/// A small deterministic generator, so a run can be repeated.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }

    fn below(&mut self, n: usize) -> usize {
        (self.next() % n as u64) as usize
    }

    fn id(&mut self) -> u128 {
        (u128::from(self.next()) << 64) | u128::from(self.next())
    }
}

struct WriterGen {
    id: WriterId,
    genesis: EntryHash,
    head: EntryHash,
    segment: Vec<u8>,
    in_segment: usize,
    bytes: u64,
    segments: usize,
    entries: usize,
}

struct EntityGen {
    id: EntityId,
    name: EntryHash,
    rating: Option<EntryHash>,
    tags: Vec<(String, EntryHash)>,
}

/// A library file as the facts record it.
#[derive(Clone)]
struct FileGen {
    path: RelPath,
    len: u64,
    modified: Option<u64>,
    identity: Identity,
}

struct Gen {
    layout: Layout,
    rng: Rng,
    at: u64,
    segment_entries: usize,
    writers: Vec<WriterGen>,
    entities: Vec<EntityGen>,
    sink: Box<dyn Backend>,
}

impl Gen {
    fn put(&mut self, path: &RelPath, bytes: Vec<u8>) {
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

    fn add_writer(&mut self) -> usize {
        let id = WriterId::from_u128(self.rng.id());
        let kind = EntryKind::Genesis(Genesis {
            writer: id,
            label: format!("writer {}", self.writers.len()),
        });
        self.at += 1;
        let at = Hlc {
            wall_ms: self.at,
            counter: 0,
        };
        let entry = Entry::encode(EntryHash::ZERO, at, kind).unwrap();
        self.writers.push(WriterGen {
            id,
            genesis: entry.hash(),
            head: entry.hash(),
            segment: entry.line.to_bytes(),
            in_segment: 1,
            bytes: 0,
            segments: 0,
            entries: 1,
        });
        self.writers.len() - 1
    }

    fn push(&mut self, w: usize, logged: Logged) -> EntryHash {
        self.at += 1;
        let at = Hlc {
            wall_ms: self.at,
            counter: 0,
        };
        let writer = &mut self.writers[w];
        let entry = Entry::encode(writer.head, at, EntryKind::Intent(logged)).unwrap();
        writer.head = entry.hash();
        writer.segment.extend(entry.line.to_bytes());
        writer.in_segment += 1;
        writer.entries += 1;
        if writer.in_segment >= self.segment_entries {
            self.flush(w);
        }
        entry.hash()
    }

    fn flush(&mut self, w: usize) {
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
        self.put(&path, bytes);
    }

    fn flush_all(&mut self) {
        for w in 0..self.writers.len() {
            self.flush(w);
        }
    }

    /// Creates `count` entities, ten to an entry, the first of them with `files`.
    fn create(&mut self, count: usize, files: &[FileGen]) {
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
                });
                let write = |key: &str, value: Raw| Op::Write {
                    entity,
                    key: key.into(),
                    value: Some(value),
                    replaces: Vec::new(),
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
                    let tag = format!("t{}", (i * 7 + t * 13) % 50);
                    ops.push(Op::Add {
                        entity,
                        key: "tags".into(),
                        value: Raw::of(&tag).unwrap(),
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
                    reverses: None,
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
    /// a tag added or a tag removed.
    fn edit(&mut self, count: usize, only: Option<usize>) {
        for _ in 0..count {
            let w = only.unwrap_or_else(|| self.rng.below(self.writers.len()));
            let e = self.rng.below(self.entities.len());
            let entity = self.entities[e].id;
            let choice = self.rng.below(10);
            let (op, update): (Op, fn(&mut EntityGen, EntryHash, &str)) = match choice {
                0..5 => (
                    Op::Write {
                        entity,
                        key: "rating".into(),
                        value: Some(Raw::of(&(self.rng.below(5) as u32)).unwrap()),
                        replaces: self.entities[e].rating.into_iter().collect(),
                    },
                    |e, hash, _| e.rating = Some(hash),
                ),
                5 => (
                    Op::Write {
                        entity,
                        key: "name".into(),
                        value: Some(Raw::of(&format!("Renamed {}", self.rng.next())).unwrap()),
                        replaces: vec![self.entities[e].name],
                    },
                    |e, hash, _| e.name = hash,
                ),
                6..9 => {
                    let tag = format!("t{}", self.rng.below(50));
                    (
                        Op::Add {
                            entity,
                            key: "tags".into(),
                            value: Raw::of(&tag).unwrap(),
                        },
                        |e, hash, tag| e.tags.push((tag.to_owned(), hash)),
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
            let hash = self.push(
                w,
                Logged {
                    label: "Edit".into(),
                    ops: vec![op],
                    displaced: Vec::new(),
                    reverses: None,
                },
            );
            update(&mut self.entities[e], hash, &tag);
        }
    }

    /// `count` items in writer `w`'s trash, logged as displaced a hundred to an
    /// entry, each with a file of 64 bytes in the trash.
    fn trash(&mut self, w: usize, count: usize) {
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
                    reverses: None,
                },
            );
        }
    }
}

fn identity_of(backend: &mut dyn Backend, path: &RelPath, len: u64) -> Identity {
    let identify = PrefixIdentity::default();
    let parts: Vec<Vec<u8>> = identify
        .ranges(len)
        .into_iter()
        .map(|range| {
            let read = backend.perform(Io::Read {
                root: Root::Folder,
                path: path.clone(),
                range,
            });
            match read.unwrap() {
                Reply::Bytes(bytes) => bytes,
                reply => panic!("{reply:?}"),
            }
        })
        .collect();
    identify.identify(len, &parts)
}

fn stat(backend: &mut dyn Backend, path: &RelPath) -> (u64, Option<u64>) {
    match backend.perform(Io::Stat {
        root: Root::Folder,
        path: path.clone(),
    }) {
        Ok(Reply::Stat(Some(meta))) => (meta.len, meta.modified),
        other => panic!("{path:?}: {other:?}"),
    }
}

/// `count` small files of varied lengths under `lib/`, a hundred to a directory.
fn make_files(backend: &mut dyn Backend, count: usize, stale: bool) -> Vec<FileGen> {
    let mut files = Vec::new();
    for i in 0..count {
        let path = RelPath::new(&format!("lib/d{:04}/f{i:06}.bin", i / 100)).unwrap();
        let len = 200 + (i * 37) % 4000;
        let mut bytes = vec![(i % 251) as u8; len];
        bytes[..8].copy_from_slice(&(i as u64).to_le_bytes());
        let dir = path.parent().unwrap();
        backend
            .perform(Io::MakeDir {
                root: Root::Folder,
                path: dir,
            })
            .unwrap();
        backend
            .perform(Io::Create {
                root: Root::Folder,
                path: path.clone(),
                bytes,
            })
            .unwrap();
        files.push(path);
    }
    describe(backend, files, stale)
}

/// Every file under the folder, as `std::fs` walks it, outside toshokan's root.
fn corpus_files(backend: &mut dyn Backend, top: &Path, stale: bool) -> Vec<FileGen> {
    fn walk(dir: &Path, top: &Path, out: &mut Vec<RelPath>) {
        let mut entries: Vec<_> = std::fs::read_dir(dir).unwrap().flatten().collect();
        entries.sort_by_key(|entry| entry.file_name());
        for entry in entries {
            let kind = entry.file_type().unwrap();
            let path = entry.path();
            let rel = path.strip_prefix(top).unwrap().to_str().unwrap().to_owned();
            if rel == ".tk" {
                continue;
            }
            if kind.is_dir() {
                walk(&path, top, out);
            } else if kind.is_file() {
                out.push(RelPath::new(&rel).unwrap());
            }
        }
    }
    let mut paths = Vec::new();
    walk(top, top, &mut paths);
    describe(backend, paths, stale)
}

fn describe(backend: &mut dyn Backend, paths: Vec<RelPath>, stale: bool) -> Vec<FileGen> {
    paths
        .into_iter()
        .map(|path| {
            let (len, modified) = stat(backend, &path);
            let identity = identity_of(backend, &path, len);
            FileGen {
                path,
                len,
                modified: if stale { Some(1) } else { modified },
                identity,
            }
        })
        .collect()
}

struct Sample {
    ms: f64,
    peak: usize,
    retained: isize,
    volume: u64,
    allocs: u64,
    io: IoStats,
}

struct Bench {
    stats: Rc<RefCell<IoStats>>,
    samples: BTreeMap<String, Vec<Sample>>,
    order: Vec<String>,
    label: String,
}

impl Bench {
    fn measure<T>(&mut self, op: &str, f: impl FnOnce() -> T) -> T {
        *self.stats.borrow_mut() = IoStats::default();
        let base = CURRENT.load(Relaxed);
        PEAK.store(base, Relaxed);
        VOLUME.store(0, Relaxed);
        COUNT.store(0, Relaxed);
        let start = Instant::now();
        let out = f();
        let ms = start.elapsed().as_secs_f64() * 1e3;
        let sample = Sample {
            ms,
            peak: PEAK.load(Relaxed) - base,
            retained: CURRENT.load(Relaxed) as isize - base as isize,
            volume: VOLUME.load(Relaxed),
            allocs: COUNT.load(Relaxed),
            io: self.stats.borrow().clone(),
        };
        eprintln!(
            "  {op}: {ms:.1} ms, {} requests [{}]",
            sample.io.total(),
            sample.io.shown()
        );
        if !self.samples.contains_key(op) {
            self.order.push(op.to_owned());
        }
        self.samples.entry(op.to_owned()).or_default().push(sample);
        out
    }

    fn report(&self) {
        println!("\n### {}\n", self.label);
        println!(
            "| op | n | median ms | min–max ms | peak MB | retained MB | alloc MB | allocs | requests | by kind (first run) | read | written | local |"
        );
        println!("|---|---|---|---|---|---|---|---|---|---|---|---|---|");
        for op in &self.order {
            let samples = &self.samples[op];
            let mut ms: Vec<f64> = samples.iter().map(|s| s.ms).collect();
            ms.sort_by(f64::total_cmp);
            let median = ms[ms.len() / 2];
            let first = &samples[0];
            let mb = |b: f64| b / (1u64 << 20) as f64;
            println!(
                "| {op} | {} | {median:.1} | {:.1}–{:.1} | {:.1} | {:.1} | {:.1} | {} | {} | {} | {} | {} | {} |",
                samples.len(),
                ms[0],
                ms[ms.len() - 1],
                mb(first.peak as f64),
                mb(first.retained as f64),
                mb(first.volume as f64),
                first.allocs,
                first.io.total(),
                first.io.shown(),
                size(first.io.bytes(&["Read", "ReadMany"])),
                size(first.io.bytes(&["Create", "Append", "Write"])),
                size(first.io.local_bytes()),
            );
        }
    }
}

struct Args {
    native: Option<PathBuf>,
    entities: usize,
    entries: usize,
    writers: usize,
    files: usize,
    corpus: Option<PathBuf>,
    stale: bool,
    trash: usize,
    segment: usize,
    retired: usize,
    refresh: Vec<usize>,
    runs: usize,
    label: String,
    skip: Vec<String>,
}

fn args() -> Args {
    let mut args = Args {
        native: None,
        entities: 1000,
        entries: 2000,
        writers: 3,
        files: 0,
        corpus: None,
        stale: false,
        trash: 0,
        segment: 500,
        retired: 0,
        refresh: vec![1, 1000],
        runs: 3,
        label: String::new(),
        skip: Vec::new(),
    };
    let mut it = std::env::args().skip(1);
    while let Some(flag) = it.next() {
        let mut value = || it.next().expect("a value");
        match flag.as_str() {
            "--native" => args.native = Some(value().into()),
            "--entities" => args.entities = value().parse().unwrap(),
            "--entries" => args.entries = value().parse().unwrap(),
            "--writers" => args.writers = value().parse().unwrap(),
            "--files" => args.files = value().parse().unwrap(),
            "--corpus" => args.corpus = Some(value().into()),
            "--stale-mtime" => args.stale = true,
            "--trash" => args.trash = value().parse().unwrap(),
            "--segment" => args.segment = value().parse().unwrap(),
            "--retired" => args.retired = value().parse().unwrap(),
            "--refresh" => args.refresh = value().split(',').map(|n| n.parse().unwrap()).collect(),
            "--runs" => args.runs = value().parse().unwrap(),
            "--label" => args.label = value(),
            "--skip" => args.skip = value().split(',').map(str::to_owned).collect(),
            other => panic!("unknown flag {other}"),
        }
    }
    args
}

fn dir_size(backend: &mut dyn Backend, root: Root, dir: &RelPath) -> (u64, usize) {
    let listed = match backend.perform(Io::List {
        root,
        dir: dir.clone(),
    }) {
        Ok(Reply::Listed(entries)) => entries,
        _ => return (0, 0),
    };
    let mut total = (0, 0);
    for entry in listed {
        let path = dir.join(&entry.name).unwrap();
        match entry.kind {
            Kind::Directory => {
                let (bytes, files) = dir_size(backend, root, &path);
                total = (total.0 + bytes, total.1 + files);
            }
            Kind::File => {
                if let Ok(Reply::Stat(Some(meta))) = backend.perform(Io::Stat { root, path }) {
                    total = (total.0 + meta.len, total.1 + 1);
                }
            }
        }
    }
    total
}

fn main() {
    let args = args();
    let world = match &args.native {
        None => World::Mem {
            folder: MemDisk::new(),
            locals: BTreeMap::new(),
        },
        Some(dir) => {
            let folder = dir.join("folder");
            std::fs::create_dir_all(&folder).unwrap();
            World::Native {
                folder,
                locals: dir.join("locals"),
            }
        }
    };
    let mut world = world;
    if let (Some(corpus), Some(dir)) = (&args.corpus, &args.native) {
        let folder = dir.join("folder");
        std::fs::remove_dir(&folder).unwrap();
        let cloned = std::process::Command::new("cp")
            .args(["-cR"])
            .arg(corpus)
            .arg(&folder)
            .status()
            .unwrap();
        assert!(cloned.success());
    }
    let skip = |op: &str| args.skip.iter().any(|s| s == op);
    let started = Instant::now();
    let mut sink = world.raw("setup");
    let files = match (&args.corpus, &args.native) {
        (Some(_), Some(dir)) => corpus_files(&mut *sink, &dir.join("folder"), args.stale),
        _ => make_files(&mut *sink, args.files, args.stale),
    };
    let mut gen = Gen {
        layout: layout(),
        rng: Rng(0x9e3779b97f4a7c15),
        at: SystemTimeMs::now() - 10_000_000,
        segment_entries: args.segment,
        writers: Vec::new(),
        entities: Vec::new(),
        sink,
    };
    for _ in 0..args.writers {
        gen.add_writer();
    }
    let entities = args.entities.max(files.len());
    gen.create(entities, &files);
    let created: usize = gen.writers.iter().map(|w| w.entries).sum();
    gen.edit(
        args.entries.saturating_sub(created + args.trash / 100),
        None,
    );
    gen.trash(0, args.trash);
    gen.flush_all();
    let entries: usize = gen.writers.iter().map(|w| w.entries).sum();
    let log_bytes: u64 = gen.writers.iter().map(|w| w.bytes).sum();
    let segments: usize = gen.writers.iter().map(|w| w.segments).sum();
    eprintln!(
        "generated {entities} entities, {entries} entries in {segments} segments ({}), {} files, in {:.1} s",
        size(log_bytes),
        files.len(),
        started.elapsed().as_secs_f64()
    );
    let own = (
        gen.writers[0].id,
        gen.writers[0].genesis,
        gen.writers[0].head,
    );
    let own_local = {
        let mut local = world.raw("own");
        let (id, genesis, head) = own;
        local
            .perform(Io::MakeDir {
                root: Root::Local,
                path: Layout::local(genesis),
            })
            .unwrap();
        let record = format!(r#"{{"writer":"{id}","head":"{head}"}}"#);
        local
            .perform(Io::Create {
                root: Root::Local,
                path: Layout::head(genesis),
                bytes: record.into_bytes(),
            })
            .unwrap();
        "own"
    };

    let stats = Rc::new(RefCell::new(IoStats::default()));
    let mut bench = Bench {
        stats: Rc::clone(&stats),
        samples: BTreeMap::new(),
        order: Vec::new(),
        label: format!(
            "{}{} — {entities} entities, {entries} entries, {} writers, {segments} segments ({}), {} files, {} trash items",
            if args.native.is_some() { "native" } else { "memory" },
            if args.label.is_empty() { String::new() } else { format!(" {}", args.label) },
            args.writers,
            size(log_bytes),
            files.len(),
            args.trash,
        ),
    };
    let schema = schema();

    for run in 0..args.runs {
        let backend = world.counted(&format!("cold-{run}"), &stats);
        let opened = bench.measure("open cold (new install)", || {
            Library::open(backend, layout(), &schema, env()).unwrap()
        });
        drop(opened);
    }

    let backend = world.counted(own_local, &stats);
    let (lib, _) = bench.measure("open own, no cached view", || {
        Library::open(backend, layout(), &schema, env()).unwrap()
    });
    bench.measure("close (writes cached view)", || lib.close().unwrap());

    let open_warm = |bench: &mut Bench, world: &mut World, op: &str| {
        let backend = world.counted(own_local, &stats);
        bench.measure(op, || {
            Library::open(backend, layout(), &schema, env()).unwrap()
        })
    };
    for _ in 0..args.runs {
        let (lib, _) = open_warm(&mut bench, &mut world, "open warm");
        drop(lib.close().unwrap());
    }
    let (mut lib, opened) = open_warm(&mut bench, &mut world, "open warm");
    eprintln!(
        "  start {:?}, scan: {} arrived, {} moved, {} changed, {} departed",
        opened.start,
        opened.scan.arrived.len(),
        opened.scan.moved.len(),
        opened.scan.changed.len(),
        opened.scan.departed.len()
    );

    let view = lib.view();
    let sample: Vec<EntityId> = (0..1000)
        .map(|i| gen.entities[(i * 7919) % gen.entities.len()].id)
        .collect();
    for _ in 0..args.runs {
        bench.measure("view: clone", || lib.view());
        bench.measure("view: entities()", || view.entities().len());
        bench.measure("view: 1000 × get(name)+members(tags)", || {
            sample
                .iter()
                .map(|id| {
                    let entity = view.entity(*id).unwrap();
                    (entity.get(NAME), entity.members(TAGS).values.len())
                })
                .count()
        });
        bench.measure("view: find(tags, t7)", || {
            view.find(TAGS, &"t7".to_owned()).len()
        });
        bench.measure("view: conflicts()", || view.conflicts().len());
        bench.measure("view: 1000 × file()", || {
            sample
                .iter()
                .filter_map(|id| view.entity(*id).unwrap().file())
                .count()
        });
    }
    let facts = view.folded().files();
    let identify: Rc<dyn Identify> = Rc::new(PrefixIdentity::default());
    let mut scanner = world.counted("scan", &stats);
    let scanned = bench.measure("scan: walk, stat, identify (no previous scan)", || {
        run(
            &mut scanner,
            binding::scan(&layout(), &identify, &facts, &Scan::default()),
        )
        .unwrap()
    });
    for _ in 0..args.runs {
        bench.measure("scan: walk and stat (previous scan given)", || {
            run(
                &mut scanner,
                binding::scan(&layout(), &identify, &facts, &scanned),
            )
            .unwrap()
        });
        bench.measure("bind", || binding::bind(&facts, &scanned, &ExactNames));
        bench.measure("folded.files()", || view.folded().files().len());
    }
    let moved = Scan {
        files: scanned
            .files
            .iter()
            .map(|(path, file)| {
                let to = RelPath::new(&format!("moved/{}", path.as_str())).unwrap();
                (to, *file)
            })
            .collect(),
    };
    bench.measure("bind after every file moved outside", || {
        binding::bind(&facts, &moved, &ExactNames)
            .report
            .moved
            .len()
    });
    drop(view);

    for _ in 0..args.runs {
        bench.measure("refresh, nothing new", || lib.refresh().unwrap());
    }
    let foreign = match gen.writers.len() {
        1 => gen.add_writer(),
        _ => 1,
    };
    for &n in &args.refresh {
        for _ in 0..args.runs {
            gen.edit(n, Some(foreign));
            gen.flush(foreign);
            let op = format!("refresh, {n} new foreign entries");
            bench.measure(&op, || lib.refresh().unwrap());
        }
    }

    let target = gen.entities[3].id;
    for run in 0..args.runs {
        let committed = bench.measure("commit: one register", || {
            lib.intent("Rename")
                .set(target, NAME, format!("Renamed {run}"))
                .commit()
        });
        match committed {
            Ok(committed) => eprintln!("    {} changes logged", committed.changes.len()),
            Err(error) => eprintln!("    refused: {error}"),
        }
    }
    for run in 0..args.runs {
        let path = RelPath::new(&format!("new/one-{run}.bin")).unwrap();
        bench.measure("commit: one new file saved", || {
            let (intent, _) = lib.intent("Import").create(|e| {
                e.save(&path, vec![run as u8; 4096], Expect::Absent)
                    .set(NAME, "One".into());
            });
            outcome(intent.commit())
        });
    }
    if !skip("hundred") {
        for run in 0..args.runs {
            bench.measure("commit: 100 new files in one intent", || {
                let mut intent = lib.intent("Import 100");
                for i in 0..100 {
                    let path = RelPath::new(&format!("batch-{run}/f{i:03}.bin")).unwrap();
                    intent = intent
                        .create(|e| {
                            e.save(&path, vec![i as u8; 4096], Expect::Absent)
                                .set(NAME, format!("Batch {i}"));
                        })
                        .0;
                }
                outcome(intent.commit())
            });
        }
        for _ in 0..args.runs {
            bench.measure("undo (100-file intent)", || outcome(lib.undo()));
        }
    }
    for _ in 0..args.runs {
        bench.measure("undo (one new file)", || outcome(lib.undo()));
    }
    if args.trash > 0 {
        for _ in 0..args.runs {
            bench.measure("trash() list", || lib.trash().unwrap().len());
        }
        for run in 0..args.runs {
            let total = (args.trash * 64) as u64;
            let keep = total * (args.runs - run - 1) as u64 / args.runs as u64;
            let policy = Policy {
                max_age_ms: u64::MAX,
                max_bytes: keep,
            };
            let emptied = bench.measure("empty_trash (1/runs of items each)", || {
                lib.empty_trash(policy).unwrap()
            });
            eprintln!("    removed {}", emptied.removed.len());
        }
    }
    if !skip("compact") {
        for _ in 0..args.runs {
            bench.measure("compact", || outcome(lib.compact()));
        }
        let _ = bench.measure("commit: one register after compact", || {
            lib.intent("Rename")
                .set(target, NAME, "After".into())
                .commit()
                .map_err(|error| eprintln!("    failed: {error}"))
        });
    }
    bench.measure("close", || lib.close().unwrap());
    if !skip("compact") {
        for _ in 0..args.runs {
            let (lib, _) = open_warm(&mut bench, &mut world, "open warm after compact");
            drop(lib.close().unwrap());
        }
        for run in 0..args.runs {
            let backend = world.counted(&format!("cold-after-{run}"), &stats);
            let opened = bench.measure("open cold after compact", || {
                Library::open(backend, layout(), &schema, env()).unwrap()
            });
            drop(opened);
        }
    }

    if args.retired > 0 {
        let mut local = world.raw(own_local);
        let views: Vec<(String, Vec<u8>)> = local_views(&mut *local);
        let (_, bytes) = views.first().cloned().expect("a cached view");
        for _ in 0..args.retired {
            let genesis = EntryHash::from_u128(gen.rng.id());
            let dir = Layout::local(genesis);
            local
                .perform(Io::MakeDir {
                    root: Root::Local,
                    path: dir,
                })
                .unwrap();
            for (path, bytes) in [
                (Layout::cached_view(genesis), bytes.clone()),
                (Layout::retired(genesis), Vec::new()),
            ] {
                local
                    .perform(Io::Create {
                        root: Root::Local,
                        path,
                        bytes,
                    })
                    .unwrap();
            }
        }
        for _ in 0..args.runs {
            let op = format!("open warm, {} retired writers in the pool", args.retired);
            let (lib, _) = open_warm(&mut bench, &mut world, &op);
            drop(lib.close().unwrap());
        }
    }

    bench.report();

    let mut raw = world.raw(own_local);
    println!("\nOn disk:\n");
    for w in gen.writers.iter().take(3) {
        let (bytes, files) = dir_size(&mut *raw, Root::Folder, &layout().writer(w.id));
        println!(
            "- writer {} dir: {} in {files} files; segments {} for {} entries ({:.0} B/entry)",
            &w.id.to_string()[..8],
            size(bytes),
            size(w.bytes),
            w.entries,
            w.bytes as f64 / w.entries as f64
        );
    }
    for (path, bytes) in local_views(&mut *raw) {
        println!(
            "- cached view {path}: {} ({:.0} B/entry, {:.0} B/entity)",
            size(bytes.len() as u64),
            bytes.len() as f64 / entries as f64,
            bytes.len() as f64 / entities as f64
        );
    }
    let listing = raw.perform(Io::List {
        root: Root::Folder,
        dir: layout().writer(own.0),
    });
    if let Ok(Reply::Listed(entries)) = listing {
        for entry in entries.iter().filter(|e| e.name.starts_with("snapshot-")) {
            let path = layout().writer(own.0).join(&entry.name).unwrap();
            let (len, _) = stat(&mut *raw, &path);
            println!(
                "- {}: {} ({} own entries, {:.0} B/entry)",
                entry.name,
                size(len),
                gen.writers[0].entries,
                len as f64 / gen.writers[0].entries as f64
            );
        }
    }
}

/// Reports a failure without stopping the run, so later operations still run.
fn outcome<T>(result: toshokan::Result<T>) -> Option<T> {
    result
        .map_err(|error| eprintln!("    failed: {error}"))
        .ok()
}

/// Every cached view in the local root, by path.
fn local_views(local: &mut dyn Backend) -> Vec<(String, Vec<u8>)> {
    let Ok(Reply::Listed(dirs)) = local.perform(Io::List {
        root: Root::Local,
        dir: RelPath::ROOT,
    }) else {
        return Vec::new();
    };
    let mut views = Vec::new();
    for dir in dirs.into_iter().filter(|d| d.kind == Kind::Directory) {
        let path = RelPath::new(&format!("{}/view.json", dir.name)).unwrap();
        let Ok(Reply::Stat(Some(meta))) = local.perform(Io::Stat {
            root: Root::Local,
            path: path.clone(),
        }) else {
            continue;
        };
        let range = Range {
            offset: 0,
            len: meta.len,
        };
        if let Ok(Reply::Bytes(bytes)) = local.perform(Io::Read {
            root: Root::Local,
            path: path.clone(),
            range,
        }) {
            views.push((path.as_str().to_owned(), bytes));
        }
    }
    views
}

struct SystemTimeMs;

impl SystemTimeMs {
    fn now() -> u64 {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_millis() as u64
    }
}
