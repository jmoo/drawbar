//! The browser benchmark: a generated library in the origin private file system,
//! opened, committed to, saved into, refreshed and queried by the async driver on
//! the page, with how long each operation held the page's thread.
//!
//! `generate` writes the library once and leaves it for later runs of the same
//! origin and profile; `measure` needs it. Neither runs unless named:
//!
//! ```text
//! cargo test --release -p toshokan --features web --target wasm32-unknown-unknown \
//!     --test bench -- --nocapture generate measure
//! ```
//!
//! `TOSHOKAN_BENCH_ENTITIES` at build time sets the size (default 100,000, with
//! twice as many entries, a file per entity and three writers). Each run starts
//! from the library as generated: the writers earlier runs' installs added are
//! removed. Beside the browser's own long tasks, where it reports them, each
//! operation reports the longest the core held the page's thread. The runner's
//! WebDriver mode starts each browser with a fresh profile, so it generates every
//! time; `scripts/bench.py` keeps a profile per browser and reports tab memory.

#![cfg(all(target_arch = "wasm32", feature = "web"))]

use std::cell::RefCell;
use std::rc::Rc;

use toshokan::asynch::{Fs, Library};
use toshokan::env::{ExactNames, Identify, PrefixIdentity, Random};
use toshokan::io::Range;
use toshokan::log::{Entry, EntryKind, FileFact, Genesis, Logged, Op};
use toshokan::web::{CryptoRandom, DateClock, Folder, Worker};
use toshokan::{
    EntityId, EntryHash, Env, Expect, Hlc, Identity, Io, Layout, MemDisk, Raw, Register, RelPath,
    Reply, Root, Schema, SegmentName, Set, WriterId,
};
use wasm_bindgen::prelude::wasm_bindgen;
use wasm_bindgen::JsValue;
use wasm_bindgen_test::{wasm_bindgen_test, wasm_bindgen_test_configure};

wasm_bindgen_test_configure!(run_in_browser);

#[wasm_bindgen(inline_js = r#"
let watching = null;
export function watch_start() {
  const start = performance.now();
  const w = { last: start, gap: 0, frame: start, frames: 0, drawn: false, tasks: [] };
  w.timer = setInterval(() => {
    const now = performance.now();
    w.gap = Math.max(w.gap, now - w.last);
    w.last = now;
  }, 1);
  const frame = (now) => {
    w.drawn = true;
    w.frames = Math.max(w.frames, now - w.frame);
    w.frame = now;
    w.request = requestAnimationFrame(frame);
  };
  w.request = requestAnimationFrame(frame);
  if (PerformanceObserver.supportedEntryTypes?.includes("longtask")) {
    w.observer = new PerformanceObserver((list) => {
      for (const task of list.getEntries()) w.tasks.push(task.duration);
    });
    w.observer.observe({ type: "longtask" });
  }
  watching = w;
}
export async function watch_stop() {
  const w = watching;
  watching = null;
  const end = performance.now();
  await new Promise((done) => {
    requestAnimationFrame(() => setTimeout(done, 0));
    setTimeout(done, 200);
  });
  clearInterval(w.timer);
  cancelAnimationFrame(w.request);
  const frames = w.drawn ? Math.max(w.frames, end - w.frame) : -1;
  const gaps = [Math.max(w.gap, end - w.last), frames];
  if (!w.observer) return [...gaps, -1, 0, 0];
  for (const task of w.observer.takeRecords()) w.tasks.push(task.duration);
  w.observer.disconnect();
  const tasks = [w.tasks.length, Math.max(0, ...w.tasks), w.tasks.reduce((a, b) => a + b, 0)];
  return [...gaps, ...tasks];
}
export function browser() {
  const longtask = PerformanceObserver.supportedEntryTypes?.includes("longtask");
  return `${navigator.userAgent} | longtask ${longtask ? "reported" : "not reported"}`;
}
async function walk(path, create) {
  let dir = await navigator.storage.getDirectory();
  for (const name of path) dir = await dir.getDirectoryHandle(name, { create });
  return dir;
}
export async function opfs_has(path) {
  try {
    await (await walk(path.slice(0, -1), false)).getFileHandle(path[path.length - 1]);
    return true;
  } catch {
    return false;
  }
}
export async function opfs_list(path) {
  try {
    const names = [];
    for await (const [name] of (await walk(path, false)).entries()) names.push(name);
    return names;
  } catch {
    return [];
  }
}
export async function opfs_remove(path) {
  try {
    await (await walk(path.slice(0, -1), false)).removeEntry(path[path.length - 1], { recursive: true });
  } catch (error) {
    if (error?.name !== "NotFoundError") throw error;
  }
}
export function say(line) {
  console.log(line);
  let shown = document.getElementById("bench");
  if (!shown) {
    shown = document.createElement("pre");
    shown.id = "bench";
    document.body.append(shown);
  }
  shown.append(line + "\n");
}
export function now() {
  return performance.now();
}
export function next_task() {
  return new Promise((done) => setTimeout(done, 0));
}
export function memory(wasm) {
  return [wasm.buffer.byteLength, performance.memory?.usedJSHeapSize ?? -1];
}
"#)]
extern "C" {
    fn watch_start();
    async fn watch_stop() -> JsValue;
    async fn opfs_has(path: Vec<String>) -> JsValue;
    async fn opfs_list(path: Vec<String>) -> JsValue;
    #[wasm_bindgen(catch)]
    async fn opfs_remove(path: Vec<String>) -> Result<JsValue, JsValue>;
    fn memory(wasm: JsValue) -> JsValue;
    fn now() -> f64;
    async fn next_task();
    fn say(line: &str);
    fn browser() -> String;
}

const NAME: Register<String> = Register::new("name");
const ORIGIN: Register<String> = Register::new("origin");
const RATING: Register<u32> = Register::new("rating");
const TAGS: Set<String> = Set::new("tags");
const RELATED: Set<String> = Set::new("related");

const WRITERS: usize = 3;
const SEED: u64 = 0x9e37_79b9_7f4a_7c15;
const SEGMENT: usize = 500;
const PER_DIR: usize = 100;

fn entities() -> usize {
    option_env!("TOSHOKAN_BENCH_ENTITIES").map_or(100_000, |n| n.parse().unwrap())
}

fn entries() -> usize {
    2 * entities()
}

/// The library's directory in the origin private file system.
fn home() -> String {
    format!("bench/{}-{}-{WRITERS}", entities(), entries())
}

fn path(text: &str) -> RelPath {
    RelPath::new(text).unwrap()
}

fn parts(text: &str) -> Vec<String> {
    text.split('/').map(str::to_owned).collect()
}

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

fn layout() -> Layout {
    Layout::new(".tk").unwrap()
}

fn env(label: &str) -> Env {
    Env {
        clock: Box::new(DateClock),
        random: Box::new(CryptoRandom),
        identify: Rc::new(PrefixIdentity::default()),
        names: Box::new(ExactNames),
        label: label.into(),
    }
}

fn identity(bytes: &[u8]) -> Identity {
    let identify = PrefixIdentity::default();
    let len = bytes.len() as u64;
    let parts: Vec<Vec<u8>> = identify
        .ranges(len)
        .iter()
        .map(|r| bytes[r.offset as usize..][..r.len.min(len - r.offset) as usize].to_vec())
        .collect();
    identify.identify(len, &parts)
}

fn file_path(i: usize) -> RelPath {
    path(&format!("lib/d{:04}/f{i:06}.bin", i / PER_DIR))
}

fn file_bytes(i: usize) -> Vec<u8> {
    let mut bytes = vec![(i % 251) as u8; 200 + (i * 37) % 4000];
    bytes[..8].copy_from_slice(&(i as u64).to_le_bytes());
    bytes
}

/// A small deterministic generator, so every run generates the same library.
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
    head: EntryHash,
    segment: Vec<u8>,
    in_segment: usize,
    entries: usize,
}

struct Described {
    path: RelPath,
    len: u64,
    modified: Option<u64>,
    identity: Identity,
}

/// The log of a library whose files are `files`: entities created ten to an
/// entry, each bound to its file, then one-op tag edits by any writer.
struct Gen {
    rng: Rng,
    at: u64,
    writers: Vec<WriterGen>,
    entities: Vec<EntityId>,
    sink: MemDisk,
}

impl Gen {
    fn put(&mut self, at: &RelPath, bytes: Vec<u8>) {
        let dir = at.parent().unwrap();
        self.sink
            .perform(Io::MakeDir {
                root: Root::Folder,
                path: dir,
            })
            .unwrap();
        self.sink
            .perform(Io::Create {
                root: Root::Folder,
                path: at.clone(),
                bytes,
            })
            .unwrap();
    }

    fn tick(&mut self) -> Hlc {
        self.at += 1;
        Hlc {
            wall_ms: self.at,
            counter: 0,
        }
    }

    fn add_writer(&mut self) {
        let id = WriterId::from_u128(self.rng.id());
        let kind = EntryKind::Genesis(Genesis {
            writer: id,
            label: format!("writer {}", self.writers.len()),
        });
        let at = self.tick();
        let entry = Entry::encode(EntryHash::ZERO, at, kind).unwrap();
        self.writers.push(WriterGen {
            id,
            head: entry.hash(),
            segment: entry.to_bytes(),
            in_segment: 1,
            entries: 1,
        });
    }

    fn push(&mut self, w: usize, logged: Logged) {
        let at = self.tick();
        let writer = &mut self.writers[w];
        let entry = Entry::encode(writer.head, at, EntryKind::Intent(logged)).unwrap();
        writer.head = entry.hash();
        writer.segment.extend(entry.to_bytes());
        writer.in_segment += 1;
        writer.entries += 1;
        if writer.in_segment >= SEGMENT {
            let marker = toshokan::line::seal_marker(writer.head);
            writer.segment.extend(marker);
            self.flush(w);
        }
    }

    fn flush(&mut self, w: usize) {
        let bytes = std::mem::take(&mut self.writers[w].segment);
        if bytes.is_empty() {
            return;
        }
        let name = SegmentName::from_u128(self.rng.id());
        let at = layout().segment(self.writers[w].id, name);
        self.writers[w].in_segment = 0;
        self.put(&at, bytes);
    }

    fn create(&mut self, files: &[Described]) {
        for chunk in (0..files.len()).collect::<Vec<_>>().chunks(10) {
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
                if i % 3 != 0 {
                    ops.push(write("rating", Raw::of(&(i as u32 % 5)).unwrap()));
                }
                for t in 0..1 + self.rng.below(5) {
                    let tag = format!("t{}", (i * 7 + t * 13) % 50);
                    ops.push(Op::Add {
                        entity,
                        key: "tags".into(),
                        value: Raw::of(&tag).unwrap(),
                    });
                }
                for _ in 0..self.rng.below(3) {
                    if self.entities.is_empty() {
                        break;
                    }
                    let other = self.entities[self.rng.below(self.entities.len())];
                    ops.push(Op::Add {
                        entity,
                        key: "related".into(),
                        value: Raw::of(&other.to_string()).unwrap(),
                    });
                }
                let file = &files[i];
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
                made.push(entity);
            }
            let logged = Logged {
                label: "Import".into(),
                ops,
                displaced: Vec::new(),
                reverses: None,
            };
            self.push(w, logged);
            self.entities.extend(made);
        }
    }

    fn edit(&mut self, count: usize) {
        for _ in 0..count {
            let w = self.rng.below(self.writers.len());
            let entity = self.entities[self.rng.below(self.entities.len())];
            let tag = format!("t{}", self.rng.below(50));
            let logged = Logged {
                label: "Edit".into(),
                ops: vec![Op::Add {
                    entity,
                    key: "tags".into(),
                    value: Raw::of(&tag).unwrap(),
                }],
                displaced: Vec::new(),
                reverses: None,
            };
            self.push(w, logged);
        }
    }
}

/// The library's log files, for `files`.
fn log_files(files: &[Described]) -> Vec<(RelPath, Vec<u8>)> {
    let mut gen = Gen {
        rng: Rng(SEED),
        at: js_sys::Date::now() as u64 - 100_000_000,
        writers: Vec::new(),
        entities: Vec::new(),
        sink: MemDisk::new(),
    };
    for _ in 0..WRITERS {
        gen.add_writer();
    }
    gen.create(files);
    let created: usize = gen.writers.iter().map(|w| w.entries).sum();
    gen.edit(entries() - created);
    for w in 0..gen.writers.len() {
        gen.flush(w);
    }
    gen.sink.files(Root::Folder).into_iter().collect()
}

async fn ok(worker: &Worker, io: Io) -> Reply {
    let shown = format!("{:?} {}", io.root(), io.path());
    match worker.perform(io).await {
        Ok(reply) => reply,
        Err(error) => panic!("{shown}: {error}"),
    }
}

/// Writes `files`, making each directory once.
async fn put(worker: &Worker, files: impl IntoIterator<Item = (RelPath, Vec<u8>)>) {
    let root = Root::Folder;
    let mut made = std::collections::BTreeSet::new();
    for (path, bytes) in files {
        let dir = path.parent().unwrap();
        if made.insert(dir.clone()) {
            ok(worker, Io::MakeDir { root, path: dir }).await;
        }
        ok(worker, Io::Create { root, path, bytes }).await;
    }
}

/// The writers the generator makes, the first ids it draws.
fn generated_writers() -> Vec<String> {
    let mut rng = Rng(SEED);
    let ids = (0..WRITERS).map(|_| WriterId::from_u128(rng.id()).to_string());
    ids.collect()
}

/// Removes the writers earlier runs' installs added to the library, so every
/// run measures the library as generated.
async fn only_generated_writers() {
    let writers = format!("{}/folder/{}/writers", home(), layout().root());
    let generated = generated_writers();
    let listed = js_sys::Array::from(&opfs_list(parts(&writers)).await);
    for name in listed.iter().filter_map(|name| name.as_string()) {
        if !generated.contains(&name) {
            opfs_remove(parts(&format!("{writers}/{name}")))
                .await
                .unwrap();
        }
    }
}

/// Written once the library files are.
fn files_marker() -> Vec<String> {
    parts(&format!("{}/generated/ready", home()))
}

/// Written once the log is, in its current shape: rolled-over segments sealed.
fn log_marker() -> Vec<String> {
    parts(&format!("{}/generated/sealed-log", home()))
}

/// Writes what an earlier run of this origin did not finish writing: the library
/// files, then the log. A new log starts the measured install afresh.
#[wasm_bindgen_test]
async fn generate() {
    let files_cached = opfs_has(files_marker()).await.is_truthy();
    if files_cached && opfs_has(log_marker()).await.is_truthy() {
        say(&format!("bench library {} is cached", home()));
        return;
    }
    let home = home();
    if !files_cached {
        opfs_remove(parts(&home)).await.unwrap();
    }
    let worker = Worker::start(
        Folder::Private(path(&format!("{home}/folder"))),
        &path(&format!("{home}/generated")),
    )
    .await
    .unwrap();
    if !files_cached {
        let start = now();
        put(
            &worker,
            (0..entities()).map(|i| (file_path(i), file_bytes(i))),
        )
        .await;
        say(&format!(
            "{} library files written in {:.0} ms",
            entities(),
            now() - start
        ));
        let marker = Io::Create {
            root: Root::Local,
            path: path("ready"),
            bytes: Vec::new(),
        };
        ok(&worker, marker).await;
    }
    let log_root = layout().root().to_string();
    opfs_remove(parts(&format!("{home}/folder/{log_root}")))
        .await
        .unwrap();
    opfs_remove(parts(&format!("{home}/own"))).await.unwrap();
    let mut files = Vec::new();
    for dir in 0..entities().div_ceil(PER_DIR) {
        let dir = path(&format!("lib/d{dir:04}"));
        let listing = Io::ListStat {
            root: Root::Folder,
            dir: dir.clone(),
        };
        let Reply::ListedStat(listed) = ok(&worker, listing).await else {
            panic!("{dir} did not list");
        };
        for (name, meta) in listed {
            let at = dir.join(&name).unwrap();
            let i: usize = name[1..7].parse().unwrap();
            files.push(Described {
                path: at,
                len: meta.len,
                modified: meta.modified,
                identity: identity(&file_bytes(i)),
            });
        }
    }
    files.sort_by(|a, b| a.path.cmp(&b.path));
    let start = now();
    let log = log_files(&files);
    drop(files);
    let bytes: usize = log.iter().map(|(_, bytes)| bytes.len()).sum();
    say(&format!(
        "{} log files of {bytes} B generated in {:.0} ms",
        log.len(),
        now() - start
    ));
    let start = now();
    put(&worker, log).await;
    say(&format!("log written in {:.0} ms", now() - start));
    let marker = Io::Create {
        root: Root::Local,
        path: path("sealed-log"),
        bytes: Vec::new(),
    };
    ok(&worker, marker).await;
}

/// One operation's samples: wall time, the longest the page went without a
/// frame and without running a timer, and the long tasks the browser reported.
#[derive(Default)]
struct Samples {
    wall: Vec<f64>,
    frames: Vec<f64>,
    timers: Vec<f64>,
    /// The longest the core held the page's thread at once.
    held: Vec<f64>,
    long_tasks: i64,
    longest_task: f64,
    in_long_tasks: f64,
}

impl Samples {
    async fn time<T>(&mut self, operation: impl std::future::Future<Output = T>) -> T {
        watch_start();
        let start = now();
        HELD.with(|held| held.set((start, 0.0)));
        let output = operation.await;
        let wall = now() - start;
        let (since, longest) = HELD.with(std::cell::Cell::get);
        self.held.push(longest.max(now() - since));
        let watched: Vec<f64> = js_sys::Array::from(&watch_stop().await)
            .iter()
            .map(|value| value.as_f64().unwrap())
            .collect();
        self.wall.push(wall);
        self.timers.push(watched[0]);
        self.frames.push(watched[1]);
        match watched[2] < 0.0 {
            true => self.long_tasks = -1,
            false => self.long_tasks += watched[2] as i64,
        }
        self.longest_task = self.longest_task.max(watched[3]);
        self.in_long_tasks += watched[4];
        output
    }

    fn report(&self, name: &str) {
        let tasks = match self.long_tasks {
            -1 => "long tasks not reported".to_owned(),
            n => format!(
                "long tasks {n} (longest {:.0}, {:.0} in all)",
                self.longest_task, self.in_long_tasks
            ),
        };
        let longest = |xs: &[f64]| xs.iter().copied().fold(0.0, f64::max);
        let frames = match self.frames.iter().any(|gap| *gap < 0.0) {
            true => "no frames drawn".to_owned(),
            false => format!("longest frame gap {:.0}", longest(&self.frames)),
        };
        say(&format!(
            "bench | {name} | {} | {frames}, timer gap {:.0}, held {:.0} | {tasks}",
            stats(&self.wall),
            longest(&self.timers),
            longest(&self.held),
        ));
    }
}

fn stats(xs: &[f64]) -> String {
    let mut xs = xs.to_vec();
    xs.sort_by(f64::total_cmp);
    let q = |p: f64| xs[((xs.len() as f64 * p) as usize).min(xs.len() - 1)];
    format!(
        "n={} median={:.1} p90={:.1} max={:.1}",
        xs.len(),
        q(0.5),
        q(0.9),
        xs[xs.len() - 1]
    )
}

fn report_memory(when: &str) {
    let used = js_sys::Array::from(&memory(wasm_bindgen::memory()));
    let mb = |value: JsValue| value.as_f64().unwrap() / (1 << 20) as f64;
    say(&format!(
        "bench | memory {when} | wasm {:.0} MB | JS heap {:.0} MB",
        mb(used.get(0)),
        mb(used.get(1)),
    ));
}

thread_local! {
    /// When the core last got the page's thread back, and the longest it has
    /// held it at once since a sample started.
    static HELD: std::cell::Cell<(f64, f64)> = const { std::cell::Cell::new((0.0, 0.0)) };
}

/// The worker, timing how long the core holds the page's thread: from a reply
/// or a pause that gave the thread back, or from the library's start, to the
/// next request or such pause. It gives the thread back when the worker does:
/// once the core has held it for [`TURN_MS`].
struct Watched(Worker);

/// As the worker's pause decides.
const TURN_MS: f64 = 6.0;

impl Watched {
    fn holding(&self) {
        HELD.with(|held| {
            let (since, longest) = held.get();
            held.set((since, longest.max(now() - since)));
        });
    }

    fn resumed(&self) {
        HELD.with(|held| held.set((now(), held.get().1)));
    }
}

impl Fs for Watched {
    fn capabilities(&self, root: Root) -> toshokan::io::Capabilities {
        self.0.capabilities(root)
    }

    async fn perform(&self, io: Io) -> toshokan::IoResult {
        self.holding();
        let result = self.0.perform(io).await;
        self.resumed();
        result
    }

    async fn pause(&self) {
        let gives_back = HELD.with(|held| now() - held.get().0 >= TURN_MS);
        if gives_back {
            self.holding();
        }
        self.0.pause().await;
        if gives_back {
            self.resumed();
        }
    }

    fn own(&self, root: &RelPath) {
        self.0.own(root);
    }
}

type Lib = Library<Watched>;

async fn open(install: &str) -> Lib {
    let home = home();
    let worker = Worker::start(
        Folder::Private(path(&format!("{home}/folder"))),
        &path(&format!("{home}/{install}")),
    )
    .await
    .unwrap();
    let watched = Watched(worker);
    watched.resumed();
    let (library, _) = Library::open(watched, layout(), &schema(), env(install))
        .await
        .unwrap();
    library
}

/// The bytes at `at` now, read through the library's storage.
async fn current<F: Fs>(library: &Library<F>, at: &RelPath) -> Vec<u8> {
    let stat = Io::Stat {
        root: Root::Folder,
        path: at.clone(),
    };
    let Ok(Reply::Stat(Some(meta))) = library.fs().perform(stat).await else {
        panic!("{at} is gone");
    };
    let read = Io::Read {
        root: Root::Folder,
        path: at.clone(),
        range: Range {
            offset: 0,
            len: meta.len,
        },
    };
    let Ok(Reply::Bytes(bytes)) = library.fs().perform(read).await else {
        panic!("{at} did not read");
    };
    bytes
}

/// The worker, with each request it performed and how long it took.
struct Traced(Worker, RefCell<Vec<(String, f64)>>);

impl Fs for Traced {
    fn capabilities(&self, root: Root) -> toshokan::io::Capabilities {
        self.0.capabilities(root)
    }

    async fn perform(&self, io: Io) -> toshokan::IoResult {
        let shown = format!("{io:?}");
        let kind = shown
            .split([' ', '{'])
            .next()
            .unwrap_or_default()
            .to_owned();
        let what = format!("{kind} {:?} {}", io.root(), io.path());
        let start = now();
        let result = self.0.perform(io).await;
        self.1.borrow_mut().push((what, now() - start));
        result
    }

    async fn pause(&self) {
        self.0.pause().await;
    }

    fn own(&self, root: &RelPath) {
        self.0.own(root);
    }
}

/// Says each request `library` made since the last call, with its time.
fn said(name: &str, library: &Library<Traced>, total: f64) {
    let requests = std::mem::take(&mut *library.fs().1.borrow_mut());
    let spent: f64 = requests.iter().map(|(_, ms)| ms).sum();
    say(&format!(
        "trace | {name} | {total:.1} ms, {} requests, {spent:.1} ms in them",
        requests.len()
    ));
    for (what, ms) in requests {
        say(&format!("trace |   {ms:6.2} {what}"));
    }
}

/// The requests of a commit, a save and a refresh on the measured install, each
/// with its time.
#[wasm_bindgen_test]
async fn trace() {
    let home = home();
    let worker = Worker::start(
        Folder::Private(path(&format!("{home}/folder"))),
        &path(&format!("{home}/own")),
    )
    .await
    .unwrap();
    let traced = Traced(worker, RefCell::default());
    let (mut a, _) = Library::open(traced, layout(), &schema(), env("own"))
        .await
        .unwrap();
    say(&format!("trace | {}", browser()));
    a.rescan().await.unwrap();
    let ids: Vec<EntityId> = a.view().entities().iter().map(|e| e.id()).collect();
    for i in 0..3 {
        let entity = ids[(i * 7919) % ids.len()];
        a.fs().1.borrow_mut().clear();
        let start = now();
        a.intent("Tag")
            .add(entity, TAGS, format!("trace {i}"))
            .commit()
            .await
            .unwrap();
        said("one-field commit", &a, now() - start);
    }
    for i in 0..2 {
        let entity = ids[(1000 + i * 7919) % ids.len()];
        let file = a.view().entity(entity).unwrap().file().unwrap();
        let old = current(&a, &file.path).await;
        let mut new = old.clone();
        new.extend_from_slice(format!("trace save {i}").as_bytes());
        a.fs().1.borrow_mut().clear();
        let start = now();
        a.intent("Save")
            .save(entity, &file.path, new, Expect::Holds(identity(&old)))
            .commit()
            .await
            .unwrap();
        said("one-file save", &a, now() - start);
    }
    for _ in 0..2 {
        a.fs().1.borrow_mut().clear();
        let start = now();
        a.refresh().await.unwrap();
        said("refresh, nothing new", &a, now() - start);
    }
    a.close().await.unwrap();
    let worker = Worker::start(
        Folder::Private(path(&format!("{home}/folder"))),
        &path(&format!("{home}/own")),
    )
    .await
    .unwrap();
    let start = now();
    let (b, _) = Library::open(
        Traced(worker, RefCell::default()),
        layout(),
        &schema(),
        env("own"),
    )
    .await
    .unwrap();
    said("open, existing install", &b, now() - start);
    b.close().await.unwrap();
    say("trace | done");
}

thread_local! {
    /// The library left open after measuring, so tab memory can be read from
    /// outside the browser.
    static KEPT: RefCell<Option<Lib>> = const { RefCell::new(None) };
}

#[wasm_bindgen_test]
async fn measure() {
    assert!(
        opfs_has(log_marker()).await.is_truthy(),
        "no bench library at {}: run the generate test first",
        home()
    );
    say(&format!("bench | {}", browser()));
    report_memory("before");
    opfs_remove(parts(&format!("{}/own", home())))
        .await
        .unwrap();
    only_generated_writers().await;
    let run = format!("{:08x}", CryptoRandom.next_u128() as u32);

    let mut cold = Samples::default();
    for i in 0..3 {
        let install = format!("fresh-{run}-{i}");
        let library = cold.time(open(&install)).await;
        assert_eq!(library.view().entities().len(), entities());
        library.close().await.unwrap();
        next_task().await;
        opfs_remove(parts(&format!("{}/{install}", home())))
            .await
            .unwrap();
    }
    cold.report("open, new install");

    let mut a = open("own").await;
    report_memory("after open");
    let mut first_scan = Samples::default();
    first_scan.time(a.rescan()).await.unwrap();
    first_scan.report("rescan after open");
    report_memory("after the first rescan");
    let ids: Vec<EntityId> = a.view().entities().iter().map(|e| e.id()).collect();
    let offset = (CryptoRandom.next_u128() % ids.len() as u128) as usize;
    let pick = |i: usize| ids[(offset + i * 7919) % ids.len()];

    let mut first = Samples::default();
    let mut commits = Samples::default();
    for i in 0..31 {
        let commit = a
            .intent("Tag")
            .add(pick(i), TAGS, format!("bench {run} {i}"))
            .commit();
        let samples = if i == 0 { &mut first } else { &mut commits };
        samples.time(commit).await.unwrap();
    }
    first.report("one-field commit, first of the session");
    commits.report("one-field commit");

    let mut saves = Samples::default();
    for i in 0..10 {
        let entity = pick(1000 + i);
        let file = a.view().entity(entity).unwrap().file().unwrap();
        let old = current(&a, &file.path).await;
        let mut new = old.clone();
        new.extend_from_slice(format!("bench {run} save {i}").as_bytes());
        let save = a
            .intent("Save")
            .save(entity, &file.path, new, Expect::Holds(identity(&old)))
            .commit();
        saves.time(save).await.unwrap();
    }
    saves.report("one-file save");

    let mut refreshes = Samples::default();
    for _ in 0..10 {
        refreshes.time(a.refresh()).await.unwrap();
    }
    refreshes.report("refresh, nothing new");
    let mut rescans = Samples::default();
    for _ in 0..3 {
        rescans.time(a.rescan()).await.unwrap();
    }
    rescans.report("rescan, nothing new");

    let view = a.view();
    let mut find = Samples::default();
    for t in 0..5 {
        let found = find
            .time(async { view.find(TAGS, &format!("t{}", t * 11)) })
            .await;
        assert!(!found.is_empty());
    }
    find.report("find(tags)");
    let mut conflicts = Samples::default();
    for _ in 0..3 {
        conflicts.time(async { view.conflicts() }).await;
    }
    conflicts.report("conflicts()");
    let lookups: [(&str, &dyn Fn() -> usize); 6] = [
        ("with(rating)", &|| view.with(RATING).len()),
        ("range(rating, 3..)", &|| view.range(RATING, 3..).len()),
        ("values(tags)", &|| view.values(TAGS).values.len()),
        ("values(name)", &|| view.values(NAME).values.len()),
        ("under(lib/d0042)", &|| view.under(&path("lib/d0042")).len()),
        ("1000 x at(path)", &|| {
            let at = |i| view.at(&view.entity(pick(i))?.file()?.path);
            (0..1000).filter_map(at).count()
        }),
    ];
    for (name, lookup) in lookups {
        let mut samples = Samples::default();
        for _ in 0..3 {
            assert!(samples.time(async { lookup() }).await > 0, "{name}");
        }
        samples.report(name);
    }
    let mut files = Samples::default();
    files
        .time(async {
            for i in 0..1000 {
                assert!(view.entity(pick(i)).unwrap().file().is_some());
            }
        })
        .await;
    files.report("1000 x file()");
    drop(view);
    a.close().await.unwrap();

    next_task().await;
    let mut warm = Samples::default();
    let mut closes = Samples::default();
    for _ in 0..3 {
        let library = warm.time(open("own")).await;
        next_task().await;
        closes.time(library.close()).await.unwrap();
        next_task().await;
    }
    warm.report("open, existing install");
    closes.report("close, dropping the library");

    let library = open("own").await;
    report_memory("with the library open");
    KEPT.with(|kept| *kept.borrow_mut() = Some(library));
    say("bench | done");
}
