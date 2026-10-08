//! Libraries in the browser, behind the `web` feature.
//!
//! The library runs on the page's thread, through the [`crate::asynch`] driver,
//! on a [`Worker`]: each request crosses to a dedicated worker as one message, and
//! the worker performs it on the browser's file system and replies. The worker
//! runs this same wasm bundle, so the page ships no script of its own for it.
//!
//! A library's folder is a [`Folder`]: a directory of the origin private file
//! system, or a folder the user picked. Its local root, which only this install
//! reads, is always a directory of the origin private file system, named by its
//! path there. Each declares its [`Capabilities`](crate::io::Capabilities) for
//! the browser it runs in; SPEC.md lists them.
//!
//! [`Hints`] tells other tabs of the same library that a commit landed, so they
//! refresh without polling.

mod fs;
mod hints;
pub mod wire;
mod worker;

use js_sys::{Function, Object, Promise, Reflect, Uint8Array};
use wasm_bindgen::{JsCast, JsValue};
use wasm_bindgen_futures::JsFuture;
use web_sys::{FileSystemDirectoryHandle, FileSystemGetDirectoryOptions, StorageManager};

use crate::env::{Clock, Random};
use crate::io::IoError;
use crate::path::RelPath;

pub use fs::Executor;
pub use hints::{Hint, Hints};
pub use worker::{serve, Worker};

/// Where a library's folder is.
#[derive(Clone, Debug)]
pub enum Folder {
    /// The directory at this path of the origin private file system, written
    /// through sync access handles, which only a worker can open. WebKit cannot
    /// send a directory handle to a worker, so the worker finds it by path.
    Private(RelPath),
    /// A folder the user picked, written through writable streams. `rename` says
    /// whether the browser moves files in it.
    Picked {
        dir: FileSystemDirectoryHandle,
        rename: bool,
    },
}

impl Folder {
    /// A folder the user picked, renaming where this browser can.
    ///
    /// Brave refuses `move()` outside the origin private file system, so there a
    /// picked folder declares no rename. The check is for `navigator.brave`.
    pub fn picked(dir: FileSystemDirectoryHandle) -> Self {
        let brave = field(&navigator(), "brave").is_some();
        Self::Picked {
            dir,
            rename: !brave,
        }
    }
}

/// The directory at `path` in the origin private file system, made if missing.
/// Works on the page and in a worker.
pub async fn private_dir(path: &RelPath) -> Result<FileSystemDirectoryHandle, IoError> {
    let storage: StorageManager = field(&navigator(), "storage")
        .ok_or_else(|| IoError::Other("this browser has no storage manager".into()))?
        .unchecked_into();
    let mut dir: FileSystemDirectoryHandle = wait(storage.get_directory())
        .await
        .map_err(|error| failure(&error, IoError::NotDirectory))?
        .unchecked_into();
    let options = FileSystemGetDirectoryOptions::new();
    options.set_create(true);
    for name in path.components() {
        dir = wait(dir.get_directory_handle_with_options(name, &options))
            .await
            .map_err(|error| failure(&error, IoError::NotDirectory))?
            .unchecked_into();
    }
    Ok(dir)
}

/// The browser's wall clock, on the page or in a worker.
#[derive(Clone, Copy, Default, Debug)]
pub struct DateClock;

impl Clock for DateClock {
    fn now_ms(&mut self) -> u64 {
        let now = js_sys::Date::now();
        match now.is_finite() && now >= 0.0 {
            true => now as u64,
            false => 0,
        }
    }
}

/// Randomness from `crypto.getRandomValues`, on the page or in a worker.
#[derive(Clone, Copy, Default, Debug)]
pub struct CryptoRandom;

impl Random for CryptoRandom {
    fn next_u128(&mut self) -> u128 {
        let crypto = field(&js_sys::global(), "crypto").expect("every browser has crypto");
        let fill: Function = field(&crypto, "getRandomValues")
            .expect("crypto draws random values")
            .unchecked_into();
        let bytes = Uint8Array::new_with_length(16);
        fill.call1(&crypto, &bytes)
            .expect("16 random bytes are within the quota");
        let mut drawn = [0; 16];
        bytes.copy_to(&mut drawn);
        u128::from_le_bytes(drawn)
    }
}

fn navigator() -> JsValue {
    field(&js_sys::global(), "navigator").unwrap_or(JsValue::UNDEFINED)
}

/// A property of a JavaScript object, unless it is undefined or null.
fn field(object: &JsValue, name: &str) -> Option<JsValue> {
    let value = Reflect::get(object, &JsValue::from_str(name)).ok()?;
    (!value.is_undefined() && !value.is_null()).then_some(value)
}

/// The name of a `DOMException` or `Error`; empty for anything else.
fn name_of(error: &JsValue) -> String {
    field(error, "name")
        .and_then(|name| name.as_string())
        .unwrap_or_default()
}

/// What a thrown value says: its name and message where it has them.
fn describe(error: &JsValue) -> String {
    let text = |name| field(error, name)?.as_string();
    match (text("name"), text("message")) {
        (Some(name), Some(message)) => format!("{name}: {message}"),
        (Some(only), None) | (None, Some(only)) => only,
        (None, None) => error.as_string().unwrap_or_else(|| format!("{error:?}")),
    }
}

/// The request's failure for a thrown `error`. `mismatch` is what the request
/// means when it met the other kind of entry than it asked for.
fn failure(error: &JsValue, mismatch: IoError) -> IoError {
    match name_of(error).as_str() {
        "NotFoundError" => IoError::NotFound,
        "TypeMismatchError" => mismatch,
        "QuotaExceededError" => IoError::NoSpace,
        _ => IoError::Other(describe(error)),
    }
}

async fn wait(promise: impl Into<Promise>) -> Result<JsValue, JsValue> {
    JsFuture::from(promise.into()).await
}

/// A promise and the function that resolves it.
fn deferred() -> (Promise, Function) {
    let mut resolve = None;
    let promise = Promise::new(&mut |done, _| resolve = Some(done));
    (
        promise,
        resolve.expect("a promise runs its executor at once"),
    )
}

fn object(fields: &[(&str, JsValue)]) -> JsValue {
    let object = Object::new();
    for (name, value) in fields {
        let _ = Reflect::set(&object, &JsValue::from_str(name), value);
    }
    object.into()
}
