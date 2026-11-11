//! The native entry. The wasm build starts at [`drawbar::start`] instead, called from
//! `index.html`; this target exists so `cargo run -p drawbar` opens a window.

#[cfg(not(target_arch = "wasm32"))]
use eframe::egui::ViewportBuilder;

#[cfg(not(target_arch = "wasm32"))]
fn main() -> eframe::Result {
    use drawbar::platform::{Frame, Platform};

    let mut viewport = ViewportBuilder::default()
        .with_inner_size([1280.0, 800.0])
        .with_min_inner_size([900.0, 560.0])
        .with_title("drawbar")
        .with_app_id(drawbar::APP);
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
        window_builder: Some(Box::new(at_least_min_size)),
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

/// Raises a size restored from the last session to the minimum. eframe applies the
/// restored size after `main` configures the viewport, and does not check it against
/// the minimum.
#[cfg(not(target_arch = "wasm32"))]
fn at_least_min_size(mut viewport: ViewportBuilder) -> ViewportBuilder {
    if let (Some(size), Some(min)) = (viewport.inner_size, viewport.min_inner_size) {
        viewport.inner_size = Some(size.max(min));
    }
    viewport
}

#[cfg(target_arch = "wasm32")]
fn main() {}

#[cfg(all(test, not(target_arch = "wasm32")))]
mod tests {
    use super::*;

    fn restored(size: [f32; 2]) -> ViewportBuilder {
        ViewportBuilder::default()
            .with_min_inner_size([900.0, 560.0])
            .with_inner_size(size)
    }

    #[test]
    fn restored_size_below_the_minimum_opens_at_the_minimum() {
        let size = at_least_min_size(restored([237.0, 64.0])).inner_size;
        assert_eq!(size, Some([900.0, 560.0].into()));
    }

    #[test]
    fn restored_size_is_raised_only_on_the_axis_below_the_minimum() {
        let size = at_least_min_size(restored([1400.0, 400.0])).inner_size;
        assert_eq!(size, Some([1400.0, 560.0].into()));
    }
}
