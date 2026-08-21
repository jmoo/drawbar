//! Pause without a runtime, on whichever backend is built.
//!
//! The one caller is the library-write op, which polls the instrument between
//! `WRITE_PREPARE_2` requests; the transport's own timeouts live in the transport.

use std::time::Duration;

/// Resolve after `d`, without blocking any other future on the same executor.
#[cfg(feature = "nusb")]
pub async fn sleep(d: Duration) {
    crate::deadline::with_timeout(std::future::pending::<()>(), d).await;
}

/// Resolve after `d`, through the page's `setTimeout` — looked up on the global object
/// so the same code runs in a window or a worker.
#[cfg(all(not(feature = "nusb"), feature = "web"))]
pub async fn sleep(d: Duration) {
    use wasm_bindgen::JsCast;
    let ms = d.as_millis().min(i32::MAX as u128) as i32;
    let promise = js_sys::Promise::new(&mut |resolve, _reject| {
        let global = js_sys::global();
        let set_timeout = js_sys::Reflect::get(&global, &"setTimeout".into())
            .expect("a JS global without setTimeout")
            .unchecked_into::<js_sys::Function>();
        set_timeout
            .call2(&global, &resolve, &ms.into())
            .expect("setTimeout refused a callback");
    });
    let _ = wasm_bindgen_futures::JsFuture::from(promise).await;
}

/// ⚠️ Parks the thread: with neither backend there is no timer source, and the only
/// callers are replay tests.
#[cfg(not(any(feature = "nusb", feature = "web")))]
pub async fn sleep(d: Duration) {
    std::thread::sleep(d);
}
