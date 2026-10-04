//! Files dropped on the page, taken before eframe reads them.
//!
//! eframe reads a dropped file whole into memory before the app hears of it. While the
//! library can take a copy, the page catches the drop on its way down to the canvas and
//! keeps each `File` with where it landed, so the library copies it in a slice at a time.

use std::cell::{Cell, RefCell};

use eframe::egui;
use wasm_bindgen::closure::Closure;
use wasm_bindgen::JsCast as _;

thread_local! {
    static DROPPED: RefCell<Vec<(web_sys::File, egui::Pos2)>> = const { RefCell::new(Vec::new()) };
    /// Whether a drop is caught; otherwise eframe takes it.
    static CATCHING: Cell<bool> = const { Cell::new(false) };
    static LISTENING: Cell<bool> = const { Cell::new(false) };
}

/// Catch the drops on the page from now on while `catching` is set, and let eframe read
/// them otherwise.
pub fn catch(ctx: &egui::Context, catching: bool) {
    CATCHING.set(catching);
    if LISTENING.replace(true) {
        return;
    }
    let ctx = ctx.clone();
    let caught = Closure::<dyn FnMut(web_sys::DragEvent)>::new(move |event| caught(&ctx, event));
    let options = web_sys::AddEventListenerOptions::new();
    options.set_capture(true);
    let listened = web_sys::window().map(|window| {
        window.add_event_listener_with_callback_and_add_event_listener_options(
            "drop",
            caught.as_ref().unchecked_ref(),
            &options,
        )
    });
    match listened {
        // The page keeps the listener for as long as it lives.
        Some(Ok(())) => caught.forget(),
        _ => LISTENING.set(false),
    }
}

/// The files dropped since the last call, each with where it landed on the canvas.
pub fn take() -> Vec<(web_sys::File, egui::Pos2)> {
    DROPPED.with(|held| std::mem::take(&mut *held.borrow_mut()))
}

fn caught(ctx: &egui::Context, event: web_sys::DragEvent) {
    if !CATCHING.get() {
        return;
    }
    let files = event.data_transfer().and_then(|carried| carried.files());
    let target = event
        .target()
        .and_then(|target| target.dyn_into::<web_sys::Element>().ok());
    let (Some(files), Some(target)) = (files, target) else {
        return;
    };
    if files.length() == 0 {
        return;
    }
    event.prevent_default();
    event.stop_propagation();
    let rect = target.get_bounding_client_rect();
    let zoom = ctx.zoom_factor();
    let at = egui::pos2(
        (event.client_x() - rect.left()) as f32 / zoom,
        (event.client_y() - rect.top()) as f32 / zoom,
    );
    DROPPED.with(|held| {
        let mut held = held.borrow_mut();
        held.extend(
            (0..files.length())
                .filter_map(|n| files.get(n))
                .map(|file| (file, at)),
        );
    });
    // eframe clears the files it shows hovering when they leave, and never sees this drop.
    if let Ok(left) = web_sys::Event::new("dragleave") {
        let _ = target.dispatch_event(&left);
    }
    ctx.request_repaint();
}
