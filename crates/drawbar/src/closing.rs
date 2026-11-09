//! A tab closing while the library would lose an edit.
//!
//! A closing tab cannot finish drawbar's asynchronous writes, so a `beforeunload`
//! listener that cancels the event makes the browser ask whether to leave, and staying
//! lets the writes land. A browser asks only once the page has been interacted with, and
//! may skip the question when the tab is discarded or the browser quits.

use std::cell::Cell;

use wasm_bindgen::closure::Closure;
use wasm_bindgen::{JsCast as _, JsValue};

thread_local! {
    static LOSING: Cell<bool> = const { Cell::new(false) };
    static LISTENING: Cell<bool> = const { Cell::new(false) };
}

/// Ask before the tab closes from now on while `losing` is set.
pub fn ask_while(losing: bool) {
    LOSING.set(losing);
    if LISTENING.replace(true) {
        return;
    }
    let asked = Closure::<dyn FnMut(web_sys::Event)>::new(|event: web_sys::Event| {
        if LOSING.get() {
            event.prevent_default();
            // Browsers before the event's cancellation counted only this.
            let _ = js_sys::Reflect::set(&event, &"returnValue".into(), &JsValue::from_str(""));
        }
    });
    let listened = web_sys::window().map(|window| {
        window.add_event_listener_with_callback("beforeunload", asked.as_ref().unchecked_ref())
    });
    match listened {
        // The page keeps the listener for as long as it lives.
        Some(Ok(())) => asked.forget(),
        _ => LISTENING.set(false),
    }
}
