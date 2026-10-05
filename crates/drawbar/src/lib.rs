//! drawbar: an egui app built on [`nord_format`] and `nord-usb`.
//!
//! Everything here is target-independent except the file picker and download glue: the
//! same shell runs as a native window and as a wasm module in a browser tab.

pub mod about;
pub mod app;
pub mod audio;
pub mod browser;
pub mod bundle;
#[cfg(target_arch = "wasm32")]
mod closing;
pub mod demo;
pub mod device;
pub mod document;
pub mod drawbar_widget;
#[cfg(target_arch = "wasm32")]
mod dropped;
pub mod fields;
pub mod filter;
pub mod folders;
pub mod icon;
#[cfg(target_arch = "wasm32")]
mod idb;
pub mod inspector;
#[cfg(target_arch = "wasm32")]
mod js;
pub mod keyboard;
pub mod knob;
pub mod led;
pub mod libraries;
pub mod library;
pub mod log;
pub mod menu;
#[cfg(target_os = "macos")]
mod menubar;
pub mod midi;
pub mod named;
pub mod net;
pub mod newproject;
pub mod ondisk;
pub mod panel;
pub mod platform;
#[cfg(any(test, target_arch = "wasm32"))]
mod presence;
pub mod queue;
mod report;
pub mod rewrite;
pub mod room;
pub mod sheet;
pub mod shell;
pub mod splash;
pub mod store;
pub mod strings;
pub mod summary;
pub mod tabs;
pub mod tags;
pub mod telemetry;
#[cfg(test)]
mod testing;
pub mod work;
pub mod workspace;
pub mod zoom;

pub use app::DrawbarApp;

/// The name eframe keeps this app's state under, which names its storage directory, and
/// the window's app id. eframe names the directory after the app id when one is set.
pub const APP: &str = "drawbar";

/// Start the app on `canvas`. Called from `index.html` after the wasm module loads.
#[cfg(target_arch = "wasm32")]
#[wasm_bindgen::prelude::wasm_bindgen]
pub async fn start(canvas: web_sys::HtmlCanvasElement) -> Result<(), wasm_bindgen::JsValue> {
    let runner = eframe::WebRunner::new();
    telemetry::install();
    runner
        .start(
            canvas,
            eframe::WebOptions::default(),
            Box::new(|cc| Ok(Box::new(DrawbarApp::new(cc)))),
        )
        .await
}
