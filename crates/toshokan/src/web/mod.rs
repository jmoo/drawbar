//! Libraries in the browser, behind the `web` feature.
//!
//! A library runs in a dedicated worker, through the [`crate::asynch`] driver, on
//! an [`Executor`], which performs each request on the browser's file system.
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

use js_sys::{Function, Object, Promise, Reflect, Uint8Array};
use wasm_bindgen::{JsCast, JsValue};
use wasm_bindgen_futures::JsFuture;
use web_sys::{FileSystemDirectoryHandle, FileSystemGetDirectoryOptions, StorageManager};

use crate::env::{Clock, Random};
use crate::io::IoError;
use crate::path::RelPath;

pub use fs::{Executor, Locks};
pub use hints::{Hint, Hints};

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
    /// A folder the user picked, renaming only in a browser known to rename there.
    pub fn picked(dir: FileSystemDirectoryHandle) -> Self {
        let brave = field(&navigator(), "brave").is_some();
        Self::Picked {
            dir,
            rename: moves_picked(&brands(), brave),
        }
    }
}

/// Whether `move()` works in a picked folder, from the browser's brands and
/// whether it sets `navigator.brave`. Measured in Chrome, which moves there, and
/// Brave, which refuses; every other browser is assumed to refuse.
fn moves_picked(brands: &[String], brave: bool) -> bool {
    !brave && brands.iter().any(|brand| brand == "Google Chrome")
}

/// The names in `navigator.userAgentData.brands`, which only Chromium has.
fn brands() -> Vec<String> {
    let brands = field(&navigator(), "userAgentData").and_then(|data| field(&data, "brands"));
    let Some(brands) = brands.and_then(|brands| brands.dyn_into::<js_sys::Array>().ok()) else {
        return Vec::new();
    };
    brands
        .iter()
        .filter_map(|brand| field(&brand, "brand")?.as_string())
        .collect()
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
    named(&name_of(error), mismatch).unwrap_or_else(|| IoError::Other(describe(error)))
}

/// The failure a `DOMException` of this name is, where it is a kind of its own.
fn named(name: &str, mismatch: IoError) -> Option<IoError> {
    Some(match name {
        "NotFoundError" => IoError::NotFound,
        "TypeMismatchError" => mismatch,
        "QuotaExceededError" => IoError::NoSpace,
        "NotAllowedError" => IoError::Denied,
        _ => return None,
    })
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_browsers_errors_keep_their_kind() {
        let named = |name| named(name, IoError::IsDirectory);
        assert_eq!(named("NotFoundError"), Some(IoError::NotFound));
        assert_eq!(named("TypeMismatchError"), Some(IoError::IsDirectory));
        assert_eq!(named("QuotaExceededError"), Some(IoError::NoSpace));
        assert_eq!(named("NotAllowedError"), Some(IoError::Denied));
        assert_eq!(named("InvalidStateError"), None);
    }

    #[test]
    fn only_chrome_declares_that_a_picked_folder_renames() {
        let brands = |names: &[&str]| {
            names
                .iter()
                .map(|&name| name.to_owned())
                .collect::<Vec<_>>()
        };
        let chrome = brands(&["Not)A;Brand", "Google Chrome", "Chromium"]);
        assert!(moves_picked(&chrome, false));
        assert!(!moves_picked(&chrome, true), "Brave");
        for other in [
            brands(&["Brave", "Chromium"]),
            brands(&["Microsoft Edge", "Chromium"]),
            brands(&["Chromium"]),
            brands(&[]),
        ] {
            assert!(!moves_picked(&other, false), "{other:?}");
        }
    }
}
