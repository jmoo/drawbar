//! The browser backends behind [`Driven`]. Each process is a worker running an
//! [`Executor`] on fresh directories of the origin private file system, answered
//! through shared memory, so the suites, which block, run unchanged in a
//! dedicated worker of a headless browser.

use std::cell::RefCell;

use js_sys::Uint8Array;
use js_sys::{Array, Atomics, Function, Int32Array, Object, Promise, Reflect, SharedArrayBuffer};
use toshokan::blocking::Backend;
use toshokan::env::Random;
use toshokan::io::Capabilities;
use toshokan::web::{private_dir, wire, CryptoRandom, Executor, Folder};
use toshokan::{Io, IoError, IoResult, RelPath, Root};
use wasm_bindgen::closure::Closure;
use wasm_bindgen::prelude::wasm_bindgen;
use wasm_bindgen::{JsCast, JsValue};
use wasm_bindgen_futures::JsFuture;
use web_sys::{
    Blob, BlobPropertyBag, DedicatedWorkerGlobalScope, FileSystemDirectoryHandle, MessageEvent,
    Url, WorkerOptions, WorkerType,
};

use super::{path, Driven, Fill};

wasm_bindgen_test::wasm_bindgen_test_configure!(run_in_dedicated_worker);

/// The most bytes a reply carries.
const CAPACITY: u32 = 32 << 20;

/// The bridge worker's script: the test bundle, handed [`bridge`].
const BOOTSTRAP: &str = r#"
self.onmessage = async ({ data }) => {
  self.onmessage = null;
  const bundle = await import(data.glue);
  await bundle.default({ module_or_path: data.module });
  await bundle.toshokanBridge(data);
};
"#;

#[wasm_bindgen]
extern "C" {
    #[wasm_bindgen(thread_local_v2, js_namespace = ["import", "meta"], js_name = url)]
    static BUNDLE: String;
}

/// Which folder a suite runs on.
#[derive(Clone, Copy, Debug)]
pub enum Kind {
    Private,
    Picked { rename: bool },
}

/// Fresh roots of one kind, and a spare process on them for
/// [`Driven::other_process`]: a worker cannot start while the one making it blocks.
pub struct WebDirs {
    kind: Kind,
    folder: RelPath,
    local: RelPath,
    process: Bridge,
    spare: RefCell<Option<Bridge>>,
}

impl WebDirs {
    pub async fn new(kind: Kind) -> Self {
        let run = format!("suites/{:032x}", CryptoRandom.next_u128());
        let (folder, local) = (
            path(&format!("{run}/folder")),
            path(&format!("{run}/local")),
        );
        let process = Bridge::spawn(kind, &folder, &local).await;
        let spare = Bridge::spawn(kind, &folder, &local).await;
        Self {
            kind,
            folder,
            local,
            process,
            spare: RefCell::new(Some(spare)),
        }
    }
}

impl Driven for WebDirs {
    fn capabilities(&self, root: Root) -> Capabilities {
        Backend::capabilities(&self.process, root)
    }

    fn run_filled<O: toshokan::Operation>(
        &mut self,
        operation: O,
        contents: Vec<Fill>,
    ) -> O::Output {
        let sources = contents.into_iter().map(Fill::blocking).collect();
        toshokan::blocking::run_with(&mut self.process, sources, operation)
    }

    fn other_process(&self) -> Self {
        let spare = self.spare.borrow_mut().take();
        Self {
            kind: self.kind,
            folder: self.folder.clone(),
            local: self.local.clone(),
            process: spare.expect("a suite asks for one other process"),
            spare: RefCell::new(None),
        }
    }
}

/// One process: a worker whose replies land in memory shared with this one.
struct Bridge {
    worker: web_sys::Worker,
    /// `[0]` is 1 once a reply is written, `[1]` its length.
    control: Int32Array,
    data: Uint8Array,
    capabilities: [Capabilities; 2],
}

impl Bridge {
    async fn spawn(kind: Kind, folder: &RelPath, local: &RelPath) -> Self {
        let shared = SharedArrayBuffer::new(8 + CAPACITY);
        let kind_of = BlobPropertyBag::new();
        kind_of.set_type("text/javascript");
        let blob =
            Blob::new_with_str_sequence_and_options(&Array::of1(&BOOTSTRAP.into()), &kind_of)
                .unwrap();
        let url = Url::create_object_url_with_blob(&blob).unwrap();
        let options = WorkerOptions::new();
        options.set_type(WorkerType::Module);
        let worker = web_sys::Worker::new_with_options(&url, &options).unwrap();
        let mut resolve = None;
        let ready = Promise::new(&mut |done, _| resolve = Some(done));
        let resolve: Function = resolve.unwrap();
        let hear = Closure::once_into_js(move |event: MessageEvent| {
            let _ = resolve.call1(&JsValue::NULL, &event.data());
        });
        worker.set_onmessage(Some(hear.unchecked_ref()));
        let (kind_name, dir, rename) = match kind {
            Kind::Private => ("private", JsValue::from_str(folder.as_str()), true),
            Kind::Picked { rename } => {
                ("picked", private_dir(folder).await.unwrap().into(), rename)
            }
        };
        let start = object(&[
            ("glue", BUNDLE.with(|url| JsValue::from_str(url))),
            ("module", wasm_bindgen::module()),
            ("kind", kind_name.into()),
            ("folder", dir),
            ("rename", rename.into()),
            ("local", local.as_str().into()),
            ("shared", shared.clone().into()),
        ]);
        worker.post_message(&start).unwrap();
        let ready = JsFuture::from(ready).await.unwrap();
        let _ = Url::revoke_object_url(&url);
        if let Some(why) = get(&ready, "failed").as_string() {
            panic!("the bridge did not start: {why}");
        }
        let bits = |root| wire::capabilities_from_bits(get(&ready, root).as_f64().unwrap() as u8);
        Self {
            worker,
            control: Int32Array::new_with_byte_offset_and_length(&shared, 0, 2),
            data: Uint8Array::new_with_byte_offset_and_length(&shared, 8, CAPACITY),
            capabilities: [bits("folder"), bits("local")],
        }
    }
}

impl Backend for Bridge {
    fn capabilities(&self, root: Root) -> Capabilities {
        self.capabilities[match root {
            Root::Folder => 0,
            Root::Local => 1,
        }]
    }

    fn perform(&mut self, io: Io) -> IoResult {
        let request = Uint8Array::from(&wire::encode_request(&io)?[..]);
        Atomics::store(&self.control, 0, 0).unwrap();
        self.worker
            .post_message(&object(&[("request", request.into())]))
            .unwrap();
        while Atomics::load(&self.control, 0).unwrap() == 0 {
            Atomics::wait(&self.control, 0, 0).unwrap();
        }
        let len = Atomics::load(&self.control, 1).unwrap() as u32;
        wire::decode_result(&self.data.subarray(0, len).to_vec())
    }
}

impl Drop for Bridge {
    fn drop(&mut self) {
        self.worker.terminate();
    }
}

/// The bridge worker's entry point.
#[wasm_bindgen(js_name = toshokanBridge)]
pub async fn bridge(start: JsValue) {
    let scope: DedicatedWorkerGlobalScope = js_sys::global().unchecked_into();
    let text = |name| path(&get(&start, name).as_string().unwrap());
    let folder = match get(&start, "kind").as_string().as_deref() {
        Some("private") => Folder::Private(text("folder")),
        _ => Folder::Picked {
            dir: get(&start, "folder").unchecked_into::<FileSystemDirectoryHandle>(),
            rename: get(&start, "rename").as_bool() == Some(true),
        },
    };
    let shared: SharedArrayBuffer = get(&start, "shared").unchecked_into();
    let executor = match Executor::new(folder, &text("local")).await {
        Ok(executor) => std::rc::Rc::new(executor),
        Err(error) => {
            let _ = scope.post_message(&object(&[("failed", error.to_string().into())]));
            return;
        }
    };
    let ready = object(&[
        (
            "folder",
            wire::capability_bits(executor.capabilities(Root::Folder)).into(),
        ),
        (
            "local",
            wire::capability_bits(executor.capabilities(Root::Local)).into(),
        ),
    ]);
    let control = Int32Array::new_with_byte_offset_and_length(&shared, 0, 2);
    let data = Uint8Array::new_with_byte_offset_and_length(&shared, 8, CAPACITY);
    let hear = Closure::<dyn FnMut(MessageEvent)>::new(move |event: MessageEvent| {
        let request: Uint8Array = get(&event.data(), "request").unchecked_into();
        let (executor, control, data) = (executor.clone(), control.clone(), data.clone());
        wasm_bindgen_futures::spawn_local(async move {
            let result = match wire::decode_request(&request.to_vec()) {
                Ok(io) => executor.perform(io).await,
                Err(wire::Malformed(what)) => Err(IoError::Other(what.into())),
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
    let _ = scope.post_message(&ready);
}

fn get(object: &JsValue, name: &str) -> JsValue {
    Reflect::get(object, &JsValue::from_str(name)).unwrap_or(JsValue::UNDEFINED)
}

fn object(fields: &[(&str, JsValue)]) -> JsValue {
    let object = Object::new();
    for (name, value) in fields {
        Reflect::set(&object, &JsValue::from_str(name), value).unwrap();
    }
    object.into()
}
