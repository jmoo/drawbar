//! The native entry. The wasm build starts at [`drawbar::start`] instead, called from
//! `index.html`; this target exists so `cargo run -p drawbar` opens a window.

#[cfg(not(target_arch = "wasm32"))]
fn main() -> eframe::Result {
    let options = eframe::NativeOptions {
        viewport: eframe::egui::ViewportBuilder::default()
            .with_inner_size([1280.0, 800.0])
            .with_min_inner_size([900.0, 560.0])
            .with_title("drawbar"),
        ..Default::default()
    };
    eframe::run_native(
        "drawbar",
        options,
        Box::new(|cc| Ok(Box::new(drawbar::DrawbarApp::new(cc)))),
    )
}

#[cfg(target_arch = "wasm32")]
fn main() {}
