//! drawbar — an egui app over [`nord_format`] and `nord-usb`.
//!
//! Everything here is target-independent except the file-picker and download glue: the
//! same shell runs as a native window and as a wasm module in a browser tab.
//!
//! > ⚠️ 🧱 This project can brick a Nord device. See the crate README.

pub mod app;
pub mod base64;
pub mod browser;
pub mod device;
pub mod document;
pub mod drawbar_widget;
pub mod fields;
pub mod knob;
pub mod led;
pub mod log;
pub mod note;
pub mod store;
pub mod strings;
pub mod tabs;
pub mod visibility;
pub mod workspace;

pub use app::DrawbarApp;

/// Start the app on `canvas`. Called from `index.html` after the wasm module loads.
#[cfg(target_arch = "wasm32")]
#[wasm_bindgen::prelude::wasm_bindgen]
pub async fn start(canvas: web_sys::HtmlCanvasElement) -> Result<(), wasm_bindgen::JsValue> {
    eframe::WebRunner::new()
        .start(
            canvas,
            eframe::WebOptions::default(),
            Box::new(|cc| Ok(Box::new(DrawbarApp::new(cc)))),
        )
        .await
}
