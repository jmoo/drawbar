//! drawbar's IndexedDB database in the browser: the folders picked lately, and what was
//! read of each library's files.

use std::future::Future;

use js_sys::Promise;
use wasm_bindgen::closure::Closure;
use wasm_bindgen::{JsCast as _, JsValue};
use wasm_bindgen_futures::JsFuture;
use web_sys::{IdbDatabase, IdbOpenDbRequest, IdbRequest, IdbTransaction};

use crate::js::describe;

const DATABASE: &str = "drawbar";

/// Raised each time a store is added: opening at a higher version makes what is missing.
const VERSION: u32 = 2;

/// The folders picked lately, under one key. See [`crate::libraries`].
pub const LIBRARIES: &str = "libraries";

/// One record per file read, under its library's id and its path. See
/// [`crate::store::Cache`].
pub const FILES: &str = "files";

/// drawbar's database, with every store it holds made at its first use.
pub async fn database() -> Result<IdbDatabase, String> {
    let factory = web_sys::window()
        .and_then(|window| window.indexed_db().ok().flatten())
        .ok_or("this browser has no IndexedDB")?;
    let request: IdbOpenDbRequest = factory
        .open_with_u32(DATABASE, VERSION)
        .map_err(|e| describe(&e))?;
    let upgrading = request.clone();
    let upgrade = Closure::once(move |_: JsValue| {
        let Ok(db) = upgrading.result() else {
            return;
        };
        let db: IdbDatabase = db.unchecked_into();
        for store in [LIBRARIES, FILES] {
            if !db.object_store_names().contains(store) {
                let _ = db.create_object_store(store);
            }
        }
    });
    request.set_onupgradeneeded(Some(upgrade.as_ref().unchecked_ref()));
    let opened = done(&request).await;
    request.set_onupgradeneeded(None);
    Ok(opened?.unchecked_into())
}

/// Once `transaction` has committed, or why it did not.
///
/// ⚠️ Its requests succeed before it commits, and the commit can still fail, as when the
/// origin is out of room.
pub async fn committed(transaction: &IdbTransaction) -> Result<(), String> {
    let answered = Promise::new(&mut |resolve, reject| {
        transaction.set_oncomplete(Some(&resolve));
        transaction.set_onabort(Some(&reject));
        transaction.set_onerror(Some(&reject));
    });
    let answer = JsFuture::from(answered).await;
    transaction.set_oncomplete(None);
    transaction.set_onabort(None);
    transaction.set_onerror(None);
    answer.map(|_| ()).map_err(|_| match transaction.error() {
        Some(error) => describe(&error),
        None => "IndexedDB did not commit the change".to_string(),
    })
}

/// The result of an IndexedDB request, once it has one.
///
/// ⚠️ Listens from the call, not from the first poll: a request that answers before its
/// listener is set never answers it, as the second of two requests sent together would.
pub fn done(request: &IdbRequest) -> impl Future<Output = Result<JsValue, String>> {
    let answered = Promise::new(&mut |resolve, reject| {
        request.set_onsuccess(Some(&resolve));
        request.set_onerror(Some(&reject));
    });
    let request = request.clone();
    async move {
        let answer = JsFuture::from(answered).await;
        request.set_onsuccess(None);
        request.set_onerror(None);
        if answer.is_err() {
            return Err(match request.error() {
                Ok(Some(error)) => describe(&error),
                _ => "IndexedDB refused the request".to_string(),
            });
        }
        request.result().map_err(|e| describe(&e))
    }
}
