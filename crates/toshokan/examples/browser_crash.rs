//! The power-loss workloads in a browser, for the sweep in `power-loss/browser`.
//!
//! A page's worker runs a workload on the origin private file system and stops at
//! the mutating request its driver chose, for the driver to kill it; a worker of a
//! later page checks what the killed one left with the power-loss checker. The
//! quota run imports files until the origin's quota refuses one, waits for the
//! driver to make room, and writes again.
//!
//! Each export runs in a dedicated worker `worker.js` starts. The library blocks on
//! a second worker, which holds an [`Executor`] for each backend, through shared
//! memory, so the page must be cross-origin isolated.

#![cfg(all(target_arch = "wasm32", feature = "web"))]

#[path = "../tests/power_loss/mod.rs"]
mod power_loss;
#[path = "../tests/common/wire.rs"]
mod wire;

use std::cell::{Cell, RefCell};
use std::collections::BTreeMap;
use std::rc::Rc;

use js_sys::{Array, Atomics, Int32Array, Object, Reflect, SharedArrayBuffer, Uint8Array};
use power_loss::{
    before, check, models, rel, verify, Acked, Faulty, Lib as _, Model, Place, Runner, Step,
    Workload, NAME,
};
use toshokan::blocking::{Backend, Library};
use toshokan::error::Why;
use toshokan::io::{Capabilities, Kind, Range};
use toshokan::report::Committed;
use toshokan::web::{Executor, Folder};
use toshokan::{Error, Expect, Io, IoError, IoResult, RelPath, Reply, Root};
use wasm_bindgen::closure::Closure;
use wasm_bindgen::prelude::wasm_bindgen;
use wasm_bindgen::{JsCast, JsValue};
use web_sys::{DedicatedWorkerGlobalScope, MessageEvent, WorkerOptions, WorkerType};

/// The most bytes a reply carries.
const CAPACITY: u32 = 8 << 20;

type Lib = Library<Faulty<Handle>>;

/// Where a run stops: before its mutating request `at`, counted from the end of its
/// setup, is sent, or once it is sent, while the browser performs it.
#[derive(Clone, Copy)]
struct Stop {
    at: u64,
    sent: bool,
}

/// The mutating requests a run has made since its setup, and where it stops.
#[derive(Default)]
struct Count {
    armed: Cell<bool>,
    made: Cell<u64>,
    stop: Cell<Option<Stop>>,
}

/// The worker holding the executors, answering through memory shared with this one.
struct Process {
    worker: web_sys::Worker,
    /// `[0]` is 1 once a reply is written, `[1]` its length.
    control: Int32Array,
    data: Uint8Array,
    next: Cell<u32>,
    count: Rc<Count>,
}

impl Process {
    async fn spawn(start: &JsValue) -> Rc<Self> {
        let shared = SharedArrayBuffer::new(8 + CAPACITY);
        let options = WorkerOptions::new();
        options.set_type(WorkerType::Module);
        let script = get(start, "script")
            .as_string()
            .expect("the worker's script");
        let worker = web_sys::Worker::new_with_options(&script, &options).unwrap();
        let (ready, resolve) = deferred();
        let hear = Closure::once_into_js(move |_: MessageEvent| {
            let _ = resolve.call0(&JsValue::NULL);
        });
        worker.set_onmessage(Some(hear.unchecked_ref()));
        let message = object(&[
            ("entry", "crashBridge".into()),
            ("module", get(start, "module")),
            ("shared", shared.clone().into()),
        ]);
        worker.post_message(&message).unwrap();
        wasm_bindgen_futures::JsFuture::from(ready).await.unwrap();
        Rc::new(Self {
            worker,
            control: Int32Array::new_with_byte_offset_and_length(&shared, 0, 2),
            data: Uint8Array::new_with_byte_offset_and_length(&shared, 8, CAPACITY),
            next: Cell::new(0),
            count: Rc::default(),
        })
    }

    fn send(&self, message: &JsValue) {
        Atomics::store(&self.control, 0, 0).unwrap();
        self.worker.post_message(message).unwrap();
    }

    fn reply(&self) -> IoResult {
        while Atomics::load(&self.control, 0).unwrap() == 0 {
            wait(&self.control);
        }
        let len = Atomics::load(&self.control, 1).unwrap() as u32;
        wire::decode_result(&self.data.subarray(0, len).to_vec())
    }

    /// A backend on an executor of its own, on the private directories `folder`
    /// and `local`.
    fn open(self: &Rc<Self>, folder: &str, local: &str) -> Result<Handle, IoError> {
        let id = self.next.get();
        self.next.set(id + 1);
        let message = object(&[
            ("id", id.into()),
            ("folder", folder.into()),
            ("local", local.into()),
        ]);
        self.send(&message);
        let Reply::Bytes(bits) = self.reply()? else {
            return Err(IoError::Other("the bridge answered no capabilities".into()));
        };
        Ok(Handle {
            process: Rc::clone(self),
            id,
            capabilities: [0, 1].map(|at| wire::capabilities_from_bits(bits[at])),
        })
    }
}

impl Drop for Process {
    fn drop(&mut self) {
        self.worker.terminate();
    }
}

/// A backend: one executor of the [`Process`].
struct Handle {
    process: Rc<Process>,
    id: u32,
    capabilities: [Capabilities; 2],
}

impl Backend for Handle {
    fn capabilities(&self, root: Root) -> Capabilities {
        self.capabilities[match root {
            Root::Folder => 0,
            Root::Local => 1,
        }]
    }

    fn perform(&mut self, io: Io) -> IoResult {
        let count = &self.process.count;
        let stop = match count.armed.get() && io.mutates() {
            true => {
                let at = count.made.get();
                count.made.set(at + 1);
                count.stop.get().filter(|stop| stop.at == at)
            }
            false => None,
        };
        let request = Uint8Array::from(&wire::encode_request(&io)?[..]);
        let message = object(&[("id", self.id.into()), ("request", request.into())]);
        match stop {
            Some(Stop { at, sent: false }) => park(at),
            Some(Stop { at, sent: true }) => {
                self.process.send(&message);
                park(at)
            }
            None => {
                self.process.send(&message);
                self.process.reply()
            }
        }
    }
}

impl Drop for Handle {
    fn drop(&mut self) {
        let message = object(&[("id", self.id.into()), ("drop", true.into())]);
        let _ = self.process.worker.post_message(&message);
    }
}

/// Tells the page the run stopped at request `at`, and waits to be killed.
fn park(at: u64) -> ! {
    post(&[("parked", (at as f64).into())]);
    let never = Int32Array::new(&SharedArrayBuffer::new(4));
    loop {
        wait(&never);
    }
}

/// Waits a moment while `cell[0]` is 0.
// ⚠️ WebKit neither ends a worker that `terminate()` finds in an untimed wait nor
// lets its locks go; it does between timed ones. Measured in Playwright's WebKit.
fn wait(cell: &Int32Array) {
    let _ = Atomics::wait_with_timeout(cell, 0, 0, 10.0);
}

/// A run's folder and local root in the origin private file system. The local
/// root is lost by moving to the next of `local-0`, `local-1`, ….
struct Opfs {
    process: Rc<Process>,
    run: String,
    lost: Cell<u32>,
}

impl Opfs {
    async fn new(start: &JsValue) -> Self {
        let lost = get(start, "lost").as_f64().unwrap_or(0.0) as u32;
        Self {
            process: Process::spawn(start).await,
            run: get(start, "run").as_string().expect("the run's directory"),
            lost: Cell::new(lost),
        }
    }
}

impl Place for Opfs {
    type Backend = Handle;

    fn backend(&self) -> Handle {
        let folder = format!("{}/folder", self.run);
        let local = format!("{}/local-{}", self.run, self.lost.get());
        self.process
            .open(&folder, &local)
            .unwrap_or_else(|e| panic!("opening an executor: {e}"))
    }

    fn tree(&self) -> Result<BTreeMap<String, Vec<u8>>, String> {
        let mut backend = self.backend();
        let mut perform = |io: Io| backend.perform(io).map_err(|e| e.to_string());
        let mut tree = BTreeMap::new();
        let mut dirs = vec![RelPath::ROOT];
        while let Some(dir) = dirs.pop() {
            let listed = perform(Io::List {
                root: Root::Folder,
                dir: dir.clone(),
            })?;
            let Reply::Listed(entries) = listed else {
                return Err(format!("listing {dir} answered {listed:?}"));
            };
            for entry in entries {
                let path = dir.join(&entry.name).map_err(|e| e.to_string())?;
                if entry.kind == Kind::Directory {
                    dirs.push(path);
                    continue;
                }
                let whole = Range {
                    offset: 0,
                    len: u64::MAX,
                };
                let read = perform(Io::Read {
                    root: Root::Folder,
                    path: path.clone(),
                    range: whole,
                })?;
                let Reply::Bytes(bytes) = read else {
                    return Err(format!("reading {path} answered {read:?}"));
                };
                tree.insert(path.as_str().to_owned(), bytes);
            }
        }
        Ok(tree)
    }

    fn put(&self, path: &str, bytes: &[u8]) -> Result<(), String> {
        let path = rel(path);
        let mut backend = self.backend();
        let requests = [
            Io::MakeDir {
                root: Root::Folder,
                path: path.parent().expect("not the root"),
            },
            Io::Create {
                root: Root::Folder,
                path: path.clone(),
                bytes: bytes.to_vec(),
            },
            Io::Sync {
                root: Root::Folder,
                path,
            },
        ];
        for io in requests {
            backend.perform(io).map_err(|e| e.to_string())?;
        }
        Ok(())
    }

    fn lose_local(&self) -> Result<(), String> {
        self.lost.set(self.lost.get() + 1);
        post(&[("lost", self.lost.get().into())]);
        Ok(())
    }
}

/// The names of the power-loss workloads.
#[wasm_bindgen(js_name = crashWorkloads)]
pub fn workloads() -> Vec<String> {
    let names = power_loss::workloads().into_iter().map(|w| w.name.into());
    names.collect()
}

/// Runs the workload `start.workload` in the directory `start.run`, stopping at
/// `start.stop` if given, and tells the page `mark` once its setup is done and as
/// each step returns, `lost` as it loses its local root, then `parked`, or `done`
/// with the mutating requests its steps made.
#[wasm_bindgen(js_name = crashRun)]
pub async fn crash_run(start: JsValue) {
    report_panics();
    let place = Opfs::new(&start).await;
    let count = Rc::clone(&place.process.count);
    let stop = get(&start, "stop").as_f64().map(|at| Stop {
        at: at as u64,
        sent: get(&start, "sent").as_bool() == Some(true),
    });
    count.stop.set(stop);
    let ran = power_loss::workload(&text(&start, "workload")).and_then(|workload| {
        power_loss::run::<Opfs, Lib>(&workload, &place, |label| {
            count.armed.set(true);
            post(&[("mark", label.into())]);
            Ok(())
        })
    });
    match ran {
        Ok(()) => post(&[("done", (count.made.get() as f64).into())]),
        Err(error) => post(&[("failed", error.into())]),
    }
}

/// Checks what a killed run of `start.workload`, or of the quota run's workload
/// where it is `quota-<size>`, left in `start.run` once `start.acked` of its steps
/// returned, committing the entries `start.entries` names, and tells the page
/// `checked` or `failed`.
#[wasm_bindgen(js_name = crashCheck)]
pub async fn crash_check(start: JsValue) {
    report_panics();
    let place = Opfs::new(&start).await;
    let name = text(&start, "workload");
    let workload = match name.strip_prefix("quota-") {
        Some(size) => size.parse().map(quota).map_err(|_| format!("{name}?")),
        None => power_loss::workload(&name),
    };
    let entries: Result<Vec<Acked>, String> = Array::from(&get(&start, "entries"))
        .iter()
        .map(|entry| entry.as_string().unwrap_or_default().parse())
        .collect();
    let acked = get(&start, "acked").as_f64().unwrap_or(0.0) as usize;
    // A killed run stops at its first failure, so no writer of it retired.
    let checked = workload.and_then(|workload| check(&workload, &place, acked, &entries?, false));
    match checked {
        Ok(checked) => post(&[(
            "checked",
            object(&[
                ("model", checked.model.into()),
                ("start", format!("{:?}", checked.start).into()),
                ("settled", checked.settled.into()),
                ("orphans", checked.orphans.into()),
            ]),
        )]),
        Err(error) => post(&[("failed", error.into())]),
    }
}

/// The most files the quota run imports before it counts the quota as never met.
const QUOTA_STEPS: usize = 64;

/// The quota run's workload: its library opened, then `QUOTA_STEPS` imports of
/// `size` bytes each, every file's bytes its own.
fn quota(size: usize) -> Workload {
    let steps = (0..QUOTA_STEPS).map(|i| {
        let bytes: Vec<u8> = (0..size)
            .map(|at| (at * 7 + i * 13 + at / 251) as u8)
            .collect();
        Step::Import {
            name: Box::leak(format!("file-{i}").into_boxed_str()),
            path: Box::leak(format!("files/{i}.bin").into_boxed_str()),
            bytes: Box::leak(bytes.into_boxed_slice()),
        }
    });
    Workload {
        name: "quota",
        setup: vec![Step::Open],
        steps: steps.collect(),
    }
}

/// Imports files of `start.size` bytes into `start.run` until one is refused, and
/// tells the page `mark` as each returns, then `full`, with how that import and
/// the same import again failed. Once the page notifies `start.room`, the same
/// library imports it once more and tells the page `after`, then waits to be killed.
#[wasm_bindgen(js_name = crashQuota)]
pub async fn crash_quota(start: JsValue) {
    report_panics();
    let place = Opfs::new(&start).await;
    let size = get(&start, "size").as_f64().unwrap_or(0.0) as usize;
    let room = Int32Array::new(&get(&start, "room").unchecked_into::<SharedArrayBuffer>());
    let workload = quota(size);
    let models = models(&workload);
    let mut runner = Runner::<Opfs, Lib>::new(&place);
    let set_up = before(&place).and_then(|()| runner.perform(Step::Open, &models[0]));
    if let Err(error) = set_up {
        return post(&[("failed", error.into())]);
    }
    post(&[("mark", "setup".into())]);
    let mut imports = workload.steps.iter().enumerate();
    let (n, step, refused) = loop {
        let Some((n, &step)) = imports.next() else {
            return post(&[("failed", "every import fit in the quota".into())]);
        };
        let acked = match import(&mut runner, step) {
            Ok(committed) => acknowledge(&runner, &models[n + 2], n, committed),
            Err(error) => break (n, step, outcome(&Err::<(), _>(error))),
        };
        if let Err(error) = acked {
            return post(&[("failed", error.into())]);
        }
    };
    let again = outcome(&import(&mut runner, step));
    post(&[(
        "full",
        object(&[("refused", refused.into()), ("again", again.into())]),
    )]);
    while Atomics::load(&room, 0).unwrap() == 0 {
        wait(&room);
    }
    let after = import(&mut runner, step);
    let shown = outcome(&after);
    let acked = after.map(|committed| acknowledge(&runner, &models[n + 2], n, committed));
    if let Ok(Err(error)) = acked {
        return post(&[("failed", error.into())]);
    }
    post(&[("after", shown.into())]);
    park(u64::MAX);
}

/// Commits the quota run's import `step`.
fn import(runner: &mut Runner<'_, Opfs, Lib>, step: Step) -> toshokan::Result<Committed> {
    let Step::Import { name, path, bytes } = step else {
        unreachable!("the quota run only imports");
    };
    runner.clock.advance(1000);
    runner
        .library
        .as_mut()
        .expect("open")
        .commit("Import", |intent| {
            let (intent, _) = intent.create(|e| {
                e.save(&rel(path), bytes.to_vec(), Expect::Absent)
                    .set(NAME, name.into());
            });
            intent
        })
}

/// Checks the folder holds `model` once the quota run's import `n` committed, and
/// marks the step.
fn acknowledge(
    runner: &Runner<'_, Opfs, Lib>,
    model: &Model,
    n: usize,
    committed: Committed,
) -> Result<(), String> {
    verify(runner, model, false)?;
    let acked = Acked::find(runner.place, committed.intent)?;
    post(&[("mark", format!("step-{}:{acked}", n + 1).into())]);
    Ok(())
}

/// A write's outcome as the sweep tallies it: `ok`, `no space at` the path that had
/// no room, `read-only: no space at` it where the library stopped writing, or what
/// else it failed with.
fn outcome<T>(result: &toshokan::Result<T>) -> String {
    match result {
        Ok(_) => "ok".into(),
        Err(Error::Io {
            path,
            error: IoError::NoSpace,
            ..
        }) => format!("no space at {path}"),
        Err(Error::ReadOnly(Why::Unfinished {
            path,
            error: IoError::NoSpace,
            ..
        })) => format!("read-only: no space at {path}"),
        Err(error) => error.to_string(),
    }
}

/// The bridge worker's entry point: an executor for each backend that asks.
#[wasm_bindgen(js_name = crashBridge)]
pub fn crash_bridge(start: JsValue) {
    report_panics();
    let scope: DedicatedWorkerGlobalScope = js_sys::global().unchecked_into();
    let shared: SharedArrayBuffer = get(&start, "shared").unchecked_into();
    let control = Int32Array::new_with_byte_offset_and_length(&shared, 0, 2);
    let data = Uint8Array::new_with_byte_offset_and_length(&shared, 8, CAPACITY);
    let executors: Rc<RefCell<BTreeMap<u32, Rc<Executor>>>> = Rc::default();
    let hear = Closure::<dyn FnMut(MessageEvent)>::new(move |event: MessageEvent| {
        let message = event.data();
        let id = get(&message, "id").as_f64().unwrap_or(-1.0) as u32;
        if get(&message, "drop").is_truthy() {
            executors.borrow_mut().remove(&id);
            return;
        }
        let (executors, control, data) = (executors.clone(), control.clone(), data.clone());
        wasm_bindgen_futures::spawn_local(async move {
            let result = match get(&message, "request").dyn_into::<Uint8Array>() {
                Ok(request) => perform(&executors, id, &request.to_vec()).await,
                Err(_) => open(&executors, id, &message).await,
            };
            let mut reply = wire::encode_result(&result);
            if reply.len() > CAPACITY as usize {
                reply = wire::encode_result(&Err(IoError::Other("the reply is too large".into())));
            }
            data.subarray(0, reply.len() as u32).copy_from(&reply);
            Atomics::store(&control, 1, reply.len() as i32).unwrap();
            Atomics::store(&control, 0, 1).unwrap();
            Atomics::notify(&control, 0).unwrap();
        });
    });
    scope.set_onmessage(Some(hear.as_ref().unchecked_ref()));
    hear.forget();
    let _ = scope.post_message(&JsValue::TRUE);
}

type Executors = RefCell<BTreeMap<u32, Rc<Executor>>>;

async fn perform(executors: &Executors, id: u32, request: &[u8]) -> IoResult {
    let executor = executors.borrow().get(&id).cloned();
    let executor = executor.ok_or_else(|| IoError::Other(format!("no executor {id}")))?;
    match wire::decode_request(request) {
        Ok(io) => executor.perform(io).await,
        Err(wire::Malformed(what)) => Err(IoError::Other(what.into())),
    }
}

/// Opens an executor on the folder and local root `message` names, and answers
/// their capabilities as two bytes.
async fn open(executors: &Executors, id: u32, message: &JsValue) -> IoResult {
    let dir = |name| RelPath::new(&text(message, name)).map_err(|e| IoError::Other(e.to_string()));
    let folder = Folder::Private(dir("folder")?);
    let executor = Executor::new(folder, &dir("local")?).await?;
    let bits =
        [Root::Folder, Root::Local].map(|root| wire::capability_bits(executor.capabilities(root)));
    executors.borrow_mut().insert(id, Rc::new(executor));
    Ok(Reply::Bytes(bits.to_vec()))
}

fn report_panics() {
    std::panic::set_hook(Box::new(|info| {
        post(&[("failed", info.to_string().into())])
    }));
}

fn post(fields: &[(&str, JsValue)]) {
    let scope: DedicatedWorkerGlobalScope = js_sys::global().unchecked_into();
    let _ = scope.post_message(&object(fields));
}

fn get(object: &JsValue, name: &str) -> JsValue {
    Reflect::get(object, &JsValue::from_str(name)).unwrap_or(JsValue::UNDEFINED)
}

fn text(object: &JsValue, name: &str) -> String {
    get(object, name).as_string().unwrap_or_default()
}

fn object(fields: &[(&str, JsValue)]) -> JsValue {
    let object = Object::new();
    for (name, value) in fields {
        Reflect::set(&object, &JsValue::from_str(name), value).unwrap();
    }
    object.into()
}

/// A promise and the function that resolves it.
fn deferred() -> (js_sys::Promise, js_sys::Function) {
    let mut resolve = None;
    let promise = js_sys::Promise::new(&mut |done, _| resolve = Some(done));
    (
        promise,
        resolve.expect("a promise runs its executor at once"),
    )
}
