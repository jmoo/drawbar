//! drawbar's IndexedDB database in the browser: the folders picked lately.

use std::future::Future;

use js_sys::Promise;
use wasm_bindgen::closure::Closure;
use wasm_bindgen::{JsCast as _, JsValue};
use wasm_bindgen_futures::JsFuture;
use web_sys::{IdbDatabase, IdbOpenDbRequest, IdbRequest, IdbTransaction};

use crate::js::describe;

const DATABASE: &str = "drawbar";

/// The version the database is made at, with every store below.
const VERSION: u32 = 2;

/// The folders picked lately, under one key. See [`crate::libraries`].
pub const LIBRARIES: &str = "libraries";

/// drawbar's database, with every store it holds made at its first use.
///
/// ⚠️ A database at a version this build does not know was made by a newer one, still open
/// in another tab after a deploy, or opened since: it is left alone, since the folders it
/// names are where each one's own data in this browser is kept.
pub async fn database() -> Result<IdbDatabase, String> {
    let factory = web_sys::window()
        .and_then(|window| window.indexed_db().ok().flatten())
        .ok_or("this browser has no IndexedDB")?;
    open(&factory).await.map_err(Refused::into_reason)
}

/// Why the database did not open.
enum Refused {
    /// A newer build made it, at a version past this build's.
    Version,
    Other(String),
}

impl Refused {
    fn into_reason(self) -> String {
        match self {
            Refused::Version => {
                "a newer drawbar keeps its list of libraries in this browser; reload the page to \
                 use it"
                    .into()
            }
            Refused::Other(why) => why,
        }
    }
}

async fn open(factory: &web_sys::IdbFactory) -> Result<IdbDatabase, Refused> {
    let request: IdbOpenDbRequest = factory
        .open_with_u32(DATABASE, VERSION)
        .map_err(|e| Refused::Other(describe(&e)))?;
    let upgrading = request.clone();
    let upgrade = Closure::once(move |_: JsValue| {
        let Ok(db) = upgrading.result() else {
            return;
        };
        let db: IdbDatabase = db.unchecked_into();
        if !db.object_store_names().contains(LIBRARIES) {
            let _ = db.create_object_store(LIBRARIES);
        }
        // Version 1 kept what was read of each library's files here.
        if db.object_store_names().contains("files") {
            let _ = db.delete_object_store("files");
        }
    });
    request.set_onupgradeneeded(Some(upgrade.as_ref().unchecked_ref()));
    let opened = done(&request).await;
    request.set_onupgradeneeded(None);
    match opened {
        Ok(db) => Ok(db.unchecked_into()),
        Err(why) => match request.error() {
            Ok(Some(error)) if error.name() == "VersionError" => Err(Refused::Version),
            _ => Err(Refused::Other(why)),
        },
    }
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
