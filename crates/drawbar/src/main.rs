//! The native entry. The wasm build starts at [`drawbar::start`] instead, called from
//! `index.html`; this target exists so `cargo run -p drawbar` opens a window.

#[cfg(not(target_arch = "wasm32"))]
fn main() -> eframe::Result {
    use drawbar::platform::{Frame, Platform};

    let mut viewport = eframe::egui::ViewportBuilder::default()
        .with_inner_size([1280.0, 800.0])
        .with_min_inner_size([900.0, 560.0])
        .with_title("drawbar");
    // The top bar is the title bar. On the Mac the system still draws the traffic lights
    // over it; elsewhere the app draws its own window buttons.
    viewport = match Platform::current() {
        Platform::Mac => viewport
            .with_fullsize_content_view(true)
            .with_titlebar_shown(false)
            .with_title_shown(false),
        platform => viewport.with_decorations(!Frame::of(platform).undecorated()),
    };
    #[cfg(unix)]
    drawbar::ondisk::raise_open_files();
    let options = eframe::NativeOptions {
        viewport,
        ..Default::default()
    };
    eframe::run_native(
        drawbar::APP,
        options,
        Box::new(|cc| {
            let app = drawbar::DrawbarApp::new(cc);
            #[cfg(target_os = "macos")]
            let app = app.in_mac_window(cc);
            Ok(Box::new(app))
        }),
    )
}

#[cfg(target_arch = "wasm32")]
fn main() {}
