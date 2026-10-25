//! The browser's page: what tells a library to look again, and the clock and
//! randomness it runs on.

#![cfg(all(target_arch = "wasm32", feature = "web"))]

use std::time::Duration;

use toshokan::env::{Clock, Random};
use toshokan::web::{private_dir, CryptoRandom, DateClock, Executor, Folder, Hint, Hints};
use toshokan::RelPath;
use wasm_bindgen::JsCast;
use wasm_bindgen_futures::JsFuture;
use wasm_bindgen_test::{wasm_bindgen_test, wasm_bindgen_test_configure};

wasm_bindgen_test_configure!(run_in_browser);

fn path(text: &str) -> RelPath {
    RelPath::new(text).unwrap()
}

#[wasm_bindgen_test]
async fn a_commit_announced_in_one_tab_is_heard_in_another() {
    let name = format!("{:032x}", CryptoRandom.next_u128());
    let mut heard = Hints::new(&name, None).unwrap();
    let said = Hints::new(&name, None).unwrap();
    let elsewhere = Hints::new("another library", None).unwrap();
    let entry = toshokan::EntryHash::from_u128(7);
    elsewhere.announce(toshokan::EntryHash::from_u128(8));
    said.announce(entry);
    assert_eq!(heard.next().await, Hint::Committed(entry));
}

#[wasm_bindgen_test]
async fn a_page_watching_its_folder_looks_at_focus_and_on_a_period() {
    let name = format!("{:032x}", CryptoRandom.next_u128());
    let mut watching = Hints::new(&name, Some(Duration::from_millis(50))).unwrap();
    assert_eq!(watching.next().await, Hint::Look, "the period passed");
    drop(watching);
    let mut watching = Hints::new(&name, Some(Duration::from_secs(3600))).unwrap();
    let global = js_sys::global();
    let get = |object: &wasm_bindgen::JsValue, name: &str| {
        js_sys::Reflect::get(object, &name.into()).unwrap()
    };
    let event = js_sys::Reflect::construct(
        &get(&global, "Event").into(),
        &js_sys::Array::of1(&"focus".into()),
    )
    .unwrap();
    let dispatch: js_sys::Function = get(&global, "dispatchEvent").into();
    dispatch.call1(&global, &event).unwrap();
    assert_eq!(watching.next().await, Hint::Look, "the page was focused");
}

#[wasm_bindgen_test]
fn the_browser_clock_and_randomness_are_live() {
    let now = DateClock.now_ms();
    let date = js_sys::Date::now() as u64;
    assert!(date.abs_diff(now) < 1_000, "{now} is not near {date}");
    assert_ne!(CryptoRandom.next_u128(), CryptoRandom.next_u128());
}

#[wasm_bindgen_test]
async fn an_executor_refuses_to_start_outside_a_worker() {
    let page = path("library/page");
    let executor = Executor::new(Folder::Private(page.clone()), &page).await;
    assert!(
        executor.is_err(),
        "the page cannot hold sync access handles"
    );
}

/// Writes `bytes` at `at` in the private directory `folder`, as another program would.
async fn write_outside(folder: &RelPath, at: &str, bytes: &[u8]) {
    let mut dir = private_dir(folder).await.unwrap();
    let file = path(at);
    for name in file.parent().unwrap().components() {
        let made = web_sys::FileSystemGetDirectoryOptions::new();
        made.set_create(true);
        let next = JsFuture::from(dir.get_directory_handle_with_options(name, &made));
        dir = next.await.unwrap().unchecked_into();
    }
    let created = web_sys::FileSystemGetFileOptions::new();
    created.set_create(true);
    let handle = dir.get_file_handle_with_options(file.name().unwrap(), &created);
    let handle: web_sys::FileSystemFileHandle =
        JsFuture::from(handle).await.unwrap().unchecked_into();
    let writable = JsFuture::from(handle.create_writable()).await.unwrap();
    let writable: web_sys::FileSystemWritableFileStream = writable.unchecked_into();
    JsFuture::from(writable.write_with_u8_array(bytes).unwrap())
        .await
        .unwrap();
    JsFuture::from(writable.close()).await.unwrap();
}

/// A folder watched where the browser can watch it hints the paths another program
/// wrote in it.
#[wasm_bindgen_test]
async fn a_watched_folder_hints_the_paths_another_program_wrote() {
    let folder = path(&format!("library/{:032x}/folder", CryptoRandom.next_u128()));
    let name = format!("{:032x}", CryptoRandom.next_u128());
    let mut hints = Hints::new(&name, Some(Duration::from_secs(20))).unwrap();
    let observer = js_sys::Reflect::get(&js_sys::global(), &"FileSystemObserver".into()).unwrap();
    let watched = hints.watch(&Folder::Private(folder.clone())).await.unwrap();
    assert_eq!(watched, !observer.is_undefined(), "watches where it can");
    if !watched {
        return;
    }
    write_outside(&folder, "shelf/copy.npno", b"song").await;
    loop {
        match hints.next().await {
            Hint::Changed(paths) if paths.contains(&path("shelf/copy.npno")) => break,
            Hint::Changed(_) => {}
            other => panic!("the watcher said nothing of the new file: {other:?}"),
        }
    }
}
