//! The browser benchmark for drawbar's library: a folder of generated files in the origin
//! private file system, opened by the page's backend and its worker, tagged, saved into,
//! refreshed and scanned again, with how long each took, how long the page's thread was
//! held meanwhile, and how large the answer that lists every file is. It runs only when
//! named:
//!
//! ```text
//! cargo test --release -p drawbar --target wasm32-unknown-unknown --test bench -- --nocapture
//! ```
//!
//! `DRAWBAR_BENCH_FILES` at build time sets how many files the folder holds (default
//! 10,000), a hundred to a folder.

#![cfg(target_arch = "wasm32")]

use drawbar::store::shown::Shown;
use drawbar::store::{Backend, Cmd, Event, Intent, Key, LibPath, Root, Slot};
use eframe::egui;
use toshokan::io::{Io, Root as Tree};
use toshokan::web::{Folder, Worker};
use toshokan::{asynch::Fs as _, Expect, RelPath};
use wasm_bindgen::prelude::wasm_bindgen;
use wasm_bindgen::JsValue;
use wasm_bindgen_test::{wasm_bindgen_test, wasm_bindgen_test_configure};

wasm_bindgen_test_configure!(run_in_browser);

#[wasm_bindgen(inline_js = r#"
let watching = null;
export function watch_start() {
  const start = performance.now();
  const w = { last: start, gap: 0, tasks: [] };
  w.timer = setInterval(() => {
    const now = performance.now();
    w.gap = Math.max(w.gap, now - w.last);
    w.last = now;
  }, 1);
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
  await new Promise((done) => setTimeout(done, 0));
  clearInterval(w.timer);
  const gap = Math.max(w.gap, end - w.last);
  if (!w.observer) return [gap, -1];
  for (const task of w.observer.takeRecords()) w.tasks.push(task.duration);
  w.observer.disconnect();
  return [gap, Math.max(0, ...w.tasks)];
}
export function browser() {
  return navigator.userAgent;
}
export async function wipe() {
  const root = await navigator.storage.getDirectory();
  for await (const [name] of root.entries()) await root.removeEntry(name, { recursive: true });
}
export function now() {
  return performance.now();
}
export function next_task() {
  return new Promise((done) => setTimeout(done, 0));
}
export function say(line) {
  console.log(line);
}
"#)]
extern "C" {
    fn watch_start();
    async fn watch_stop() -> JsValue;
    fn browser() -> String;
    async fn wipe();
    fn now() -> f64;
    async fn next_task();
    fn say(line: &str);
}

const PER_DIR: usize = 100;

fn files() -> usize {
    option_env!("DRAWBAR_BENCH_FILES").map_or(10_000, |n| n.parse().unwrap())
}

fn file_path(i: usize) -> String {
    format!("Bench/{:03}/{i:06}.ne5p", i / PER_DIR)
}

fn rel(text: &str) -> RelPath {
    RelPath::new(text).unwrap()
}

/// Write the folder's files through toshokan's storage worker, which writes in place.
async fn generate() {
    wipe().await;
    let worker = Worker::start(Folder::Private(RelPath::ROOT), &rel(".bench-local"))
        .await
        .unwrap();
    let count = files();
    for dir in 0..count.div_ceil(PER_DIR) {
        let dir = rel(&format!("Bench/{dir:03}"));
        let made = Io::MakeDir {
            root: Tree::Folder,
            path: dir,
        };
        worker.perform(made).await.unwrap();
    }
    for i in 0..count {
        let bytes = format!("CBIN program {i:06} of the bench").into_bytes();
        let create = Io::Create {
            root: Tree::Folder,
            path: rel(&file_path(i)),
            bytes,
        };
        worker.perform(create).await.unwrap();
    }
}

fn fence() -> Slot {
    Slot {
        instrument: "fence".into(),
        class: 0,
        bank: 0,
        slot: 0,
    }
}

/// How long `cmd` took to answer, how long the page's thread was held meanwhile, and
/// its answers.
async fn timed(backend: &mut Backend, shown: &mut Shown, cmd: Cmd) -> (f64, [f64; 2], Vec<Event>) {
    watch_start();
    let start = now();
    backend.send(cmd);
    backend.send(Cmd::ReadSlot(fence()));
    let mut answers = Vec::new();
    loop {
        let Some(event) = backend.try_recv() else {
            next_task().await;
            continue;
        };
        if matches!(&event, Event::SlotKept { slot, .. } if *slot == fence()) {
            break;
        }
        let viewed = match &event {
            Event::Opened(Ok(opened)) => Some(&opened.viewed),
            Event::Scanned(Ok(viewed)) => Some(viewed),
            Event::Committed {
                result: Ok(viewed), ..
            } => Some(viewed),
            _ => None,
        };
        if let Some(viewed) = viewed {
            shown.apply(viewed.shown.clone());
        }
        answers.push(event);
    }
    let took = now() - start;
    let held: Vec<f64> = numbers(watch_stop().await);
    (took, [held[0], held[1]], answers)
}

fn numbers(value: JsValue) -> Vec<f64> {
    let array: js_sys::Array = value.into();
    array.iter().map(|value| value.as_f64().unwrap()).collect()
}

fn report(what: &str, (took, held, _): &(f64, [f64; 2], Vec<Event>)) {
    let tasks = match held[1] < 0.0 {
        true => "not reported".to_string(),
        false => format!("{:.0} ms", held[1]),
    };
    say(&format!(
        "bench | {what}: {took:.0} ms; page held at most {:.0} ms; longest task {tasks}",
        held[0]
    ));
}

#[wasm_bindgen_test]
async fn measure() {
    let count = files();
    say(&format!("bench | {} | {count} files", browser()));
    let started = now();
    generate().await;
    say(&format!(
        "bench | generated in {:.0} s",
        (now() - started) / 1000.0
    ));

    let mut shown = Shown::default();
    let mut backend = Backend::start(&egui::Context::default(), Root::Private);
    let opened = timed(&mut backend, &mut shown, Cmd::Open).await;
    report("open, through the first view", &opened);
    report(
        "rescan after open",
        &timed(&mut backend, &mut shown, Cmd::Rescan).await,
    );
    let Some(Event::Opened(Ok(open))) = opened.2.first() else {
        panic!("{:?}", opened.2)
    };
    let text = ron::to_string(&open.viewed.shown).unwrap();
    say(&format!(
        "bench | the answer listing every file: {} bytes of RON, {} files shown",
        text.len(),
        shown.files.len()
    ));
    assert_eq!(shown.files.len(), count);

    for (n, name) in ["Gig", "Set"].into_iter().enumerate() {
        let file = Key::File(LibPath::parse(&file_path(n)).unwrap());
        let tag = Cmd::Intent(Intent::Tag {
            files: vec![file],
            name: name.into(),
            make: true,
        });
        let tagged = timed(&mut backend, &mut shown, tag).await;
        let text = match tagged
            .2
            .iter()
            .find(|event| matches!(event, Event::Committed { .. }))
        {
            Some(Event::Committed {
                result: Ok(viewed), ..
            }) => ron::to_string(&viewed.shown).unwrap().len(),
            other => panic!("{other:?}"),
        };
        report(&format!("tag {} ({text} bytes of RON)", n + 1), &tagged);
    }
    let save = Cmd::Intent(Intent::Save {
        id: 1,
        path: LibPath::parse("Bench/new.ne5p").unwrap(),
        bytes: b"a new program".to_vec(),
        expect: Expect::Absent,
        came: None,
    });
    report("save", &timed(&mut backend, &mut shown, save).await);
    report(
        "refresh",
        &timed(&mut backend, &mut shown, Cmd::Refresh).await,
    );
    // A scan reads the identity of each file whose length a fact names: every file
    // here, once a tag names one. The next takes them from what this one read.
    let rescan = Cmd::Rescan;
    report(
        "rescan after the tags",
        &timed(&mut backend, &mut shown, rescan).await,
    );
    report(
        "rescan, nothing new",
        &timed(&mut backend, &mut shown, Cmd::Rescan).await,
    );
    backend.finish();
}
