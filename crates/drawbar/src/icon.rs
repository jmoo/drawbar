//! The vendored Lucide glyphs, and the one way they are drawn.
//!
//! The art in `assets/icons` is Lucide 0.469.0 under the ISC licence beside it,
//! rewritten once from `currentColor` to white so a tint multiplies to exactly the
//! colour asked for. Sizes in use are 10-15 px; a row picks one and keeps it.

use eframe::egui;

/// One vendored glyph. An enum, so a name nobody vendored is a compile error.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Glyph {
    ArrowDownToLine,
    ArrowRight,
    ArrowUpRight,
    AudioLines,
    AudioWaveform,
    Check,
    ChevronDown,
    ChevronRight,
    CircleAlert,
    CircleCheck,
    CircleDashed,
    CircleDot,
    CircleHelp,
    Clock,
    Columns2,
    Disc3,
    Equal,
    Eye,
    EyeOff,
    FilePlus2,
    Folder,
    FolderGit2,
    FolderOpen,
    FolderPlus,
    Gauge,
    GitCompareArrows,
    GripVertical,
    HardDrive,
    Info,
    Keyboard,
    LibraryBig,
    Link,
    Link2Off,
    ListMusic,
    Minus,
    Moon,
    PanelBottom,
    PanelLeft,
    PanelLeftClose,
    PanelLeftOpen,
    PanelRight,
    PanelRightOpen,
    Pencil,
    Piano,
    Plus,
    RefreshCw,
    Replace,
    RotateCcw,
    Save,
    ScanEye,
    Search,
    SlidersHorizontal,
    SlidersVertical,
    Sun,
    Tag,
    Tags,
    Unplug,
    Upload,
    Waves,
    Wrench,
    X,
}

impl Glyph {
    /// Every variant, so a sweep can prove each one still has art behind it.
    pub const ALL: [Glyph; 61] = [
        Glyph::ArrowDownToLine,
        Glyph::ArrowRight,
        Glyph::ArrowUpRight,
        Glyph::AudioLines,
        Glyph::AudioWaveform,
        Glyph::Check,
        Glyph::ChevronDown,
        Glyph::ChevronRight,
        Glyph::CircleAlert,
        Glyph::CircleCheck,
        Glyph::CircleDashed,
        Glyph::CircleDot,
        Glyph::CircleHelp,
        Glyph::Clock,
        Glyph::Columns2,
        Glyph::Disc3,
        Glyph::Equal,
        Glyph::Eye,
        Glyph::EyeOff,
        Glyph::FilePlus2,
        Glyph::Folder,
        Glyph::FolderGit2,
        Glyph::FolderOpen,
        Glyph::FolderPlus,
        Glyph::Gauge,
        Glyph::GitCompareArrows,
        Glyph::GripVertical,
        Glyph::HardDrive,
        Glyph::Info,
        Glyph::Keyboard,
        Glyph::LibraryBig,
        Glyph::Link,
        Glyph::Link2Off,
        Glyph::ListMusic,
        Glyph::Minus,
        Glyph::Moon,
        Glyph::PanelBottom,
        Glyph::PanelLeft,
        Glyph::PanelLeftClose,
        Glyph::PanelLeftOpen,
        Glyph::PanelRight,
        Glyph::PanelRightOpen,
        Glyph::Pencil,
        Glyph::Piano,
        Glyph::Plus,
        Glyph::RefreshCw,
        Glyph::Replace,
        Glyph::RotateCcw,
        Glyph::Save,
        Glyph::ScanEye,
        Glyph::Search,
        Glyph::SlidersHorizontal,
        Glyph::SlidersVertical,
        Glyph::Sun,
        Glyph::Tag,
        Glyph::Tags,
        Glyph::Unplug,
        Glyph::Upload,
        Glyph::Waves,
        Glyph::Wrench,
        Glyph::X,
    ];

    fn source(self) -> egui::ImageSource<'static> {
        match self {
            Glyph::ArrowDownToLine => {
                egui::include_image!("../assets/icons/arrow-down-to-line.svg")
            }
            Glyph::ArrowRight => egui::include_image!("../assets/icons/arrow-right.svg"),
            Glyph::ArrowUpRight => egui::include_image!("../assets/icons/arrow-up-right.svg"),
            Glyph::AudioLines => egui::include_image!("../assets/icons/audio-lines.svg"),
            Glyph::AudioWaveform => egui::include_image!("../assets/icons/audio-waveform.svg"),
            Glyph::Check => egui::include_image!("../assets/icons/check.svg"),
            Glyph::ChevronDown => egui::include_image!("../assets/icons/chevron-down.svg"),
            Glyph::ChevronRight => egui::include_image!("../assets/icons/chevron-right.svg"),
            Glyph::CircleAlert => egui::include_image!("../assets/icons/circle-alert.svg"),
            Glyph::CircleCheck => egui::include_image!("../assets/icons/circle-check.svg"),
            Glyph::CircleDashed => egui::include_image!("../assets/icons/circle-dashed.svg"),
            Glyph::CircleDot => egui::include_image!("../assets/icons/circle-dot.svg"),
            Glyph::CircleHelp => egui::include_image!("../assets/icons/circle-help.svg"),
            Glyph::Clock => egui::include_image!("../assets/icons/clock.svg"),
            Glyph::Columns2 => egui::include_image!("../assets/icons/columns-2.svg"),
            Glyph::Disc3 => egui::include_image!("../assets/icons/disc-3.svg"),
            Glyph::Equal => egui::include_image!("../assets/icons/equal.svg"),
            Glyph::Eye => egui::include_image!("../assets/icons/eye.svg"),
            Glyph::EyeOff => egui::include_image!("../assets/icons/eye-off.svg"),
            Glyph::FilePlus2 => egui::include_image!("../assets/icons/file-plus-2.svg"),
            Glyph::Folder => egui::include_image!("../assets/icons/folder.svg"),
            Glyph::FolderGit2 => egui::include_image!("../assets/icons/folder-git-2.svg"),
            Glyph::FolderOpen => egui::include_image!("../assets/icons/folder-open.svg"),
            Glyph::FolderPlus => egui::include_image!("../assets/icons/folder-plus.svg"),
            Glyph::Gauge => egui::include_image!("../assets/icons/gauge.svg"),
            Glyph::GitCompareArrows => {
                egui::include_image!("../assets/icons/git-compare-arrows.svg")
            }
            Glyph::GripVertical => egui::include_image!("../assets/icons/grip-vertical.svg"),
            Glyph::HardDrive => egui::include_image!("../assets/icons/hard-drive.svg"),
            Glyph::Info => egui::include_image!("../assets/icons/info.svg"),
            Glyph::Keyboard => egui::include_image!("../assets/icons/keyboard.svg"),
            Glyph::LibraryBig => egui::include_image!("../assets/icons/library-big.svg"),
            Glyph::Link => egui::include_image!("../assets/icons/link.svg"),
            Glyph::Link2Off => egui::include_image!("../assets/icons/link-2-off.svg"),
            Glyph::ListMusic => egui::include_image!("../assets/icons/list-music.svg"),
            Glyph::Minus => egui::include_image!("../assets/icons/minus.svg"),
            Glyph::Moon => egui::include_image!("../assets/icons/moon.svg"),
            Glyph::PanelBottom => egui::include_image!("../assets/icons/panel-bottom.svg"),
            Glyph::PanelLeft => egui::include_image!("../assets/icons/panel-left.svg"),
            Glyph::PanelLeftClose => egui::include_image!("../assets/icons/panel-left-close.svg"),
            Glyph::PanelLeftOpen => egui::include_image!("../assets/icons/panel-left-open.svg"),
            Glyph::PanelRight => egui::include_image!("../assets/icons/panel-right.svg"),
            Glyph::PanelRightOpen => egui::include_image!("../assets/icons/panel-right-open.svg"),
            Glyph::Pencil => egui::include_image!("../assets/icons/pencil.svg"),
            Glyph::Piano => egui::include_image!("../assets/icons/piano.svg"),
            Glyph::Plus => egui::include_image!("../assets/icons/plus.svg"),
            Glyph::RefreshCw => egui::include_image!("../assets/icons/refresh-cw.svg"),
            Glyph::Replace => egui::include_image!("../assets/icons/replace.svg"),
            Glyph::RotateCcw => egui::include_image!("../assets/icons/rotate-ccw.svg"),
            Glyph::Save => egui::include_image!("../assets/icons/save.svg"),
            Glyph::ScanEye => egui::include_image!("../assets/icons/scan-eye.svg"),
            Glyph::Search => egui::include_image!("../assets/icons/search.svg"),
            Glyph::SlidersHorizontal => {
                egui::include_image!("../assets/icons/sliders-horizontal.svg")
            }
            Glyph::SlidersVertical => egui::include_image!("../assets/icons/sliders-vertical.svg"),
            Glyph::Sun => egui::include_image!("../assets/icons/sun.svg"),
            Glyph::Tag => egui::include_image!("../assets/icons/tag.svg"),
            Glyph::Tags => egui::include_image!("../assets/icons/tags.svg"),
            Glyph::Unplug => egui::include_image!("../assets/icons/unplug.svg"),
            Glyph::Upload => egui::include_image!("../assets/icons/upload.svg"),
            Glyph::Waves => egui::include_image!("../assets/icons/waves.svg"),
            Glyph::Wrench => egui::include_image!("../assets/icons/wrench.svg"),
            Glyph::X => egui::include_image!("../assets/icons/x.svg"),
        }
    }

    /// This glyph as an image, for the widgets and the painters that take one.
    pub fn image(self) -> egui::Image<'static> {
        egui::Image::new(self.source())
    }
}

/// Draw `glyph` in a `size` by `size` box, painted in `tint`.
pub fn icon(ui: &mut egui::Ui, glyph: Glyph, size: f32, tint: egui::Color32) -> egui::Response {
    ui.add(sized(glyph, size, tint))
}

/// Draw `glyph` into `rect`, painted in `tint`, claiming no space of its own.
///
/// For the strips that compute their own geometry; [`icon`] is the one to reach for
/// inside a layout.
pub fn painted(ui: &egui::Ui, glyph: Glyph, rect: egui::Rect, tint: egui::Color32) {
    glyph.image().tint(tint).paint_at(ui, rect);
}

/// A glyph fixed to a square box, for a button that carries one.
pub fn sized(glyph: Glyph, size: f32, tint: egui::Color32) -> egui::Image<'static> {
    glyph
        .image()
        .fit_to_exact_size(egui::vec2(size, size))
        .tint(tint)
}

#[cfg(test)]
mod tests {
    use super::*;
    use egui::load::{ImagePoll, SizeHint};

    /// A 10 px glyph on a 2x panel — the smallest raster the shell asks for.
    const RASTER: u32 = 20;

    fn headless() -> (egui::Context, egui::RawInput) {
        let ctx = egui::Context::default();
        egui_extras::install_image_loaders(&ctx);
        let input = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(900.0, 540.0),
            )),
            ..Default::default()
        };
        (ctx, input)
    }

    #[test]
    fn every_glyph_rasterises_to_something_that_can_be_seen() {
        let (ctx, input) = headless();
        let _ = ctx.run(input, |ctx| {
            for glyph in Glyph::ALL {
                let source = glyph.source();
                let egui::ImageSource::Bytes { uri, bytes } = source else {
                    panic!("{glyph:?} is not vendored bytes");
                };
                ctx.include_bytes(uri.clone(), bytes);
                let poll = ctx
                    .try_load_image(
                        &uri,
                        SizeHint::Size {
                            width: RASTER,
                            height: RASTER,
                            maintain_aspect_ratio: true,
                        },
                    )
                    .unwrap_or_else(|e| panic!("{glyph:?} did not load: {e}"));
                let ImagePoll::Ready { image } = poll else {
                    panic!("{glyph:?} is still pending; nothing would be painted");
                };
                assert!(
                    image.pixels.iter().any(|pixel| pixel.a() > 0),
                    "{glyph:?} rasterised to nothing"
                );
                // Premultiplied white is (a, a, a, a). Lucide ships `currentColor`,
                // which resvg rasterises black — and no tint can lift black.
                assert!(
                    image.pixels.iter().all(|pixel| pixel.r() == pixel.a()
                        && pixel.g() == pixel.a()
                        && pixel.b() == pixel.a()),
                    "{glyph:?} has ink a tint cannot colour"
                );
            }
        });
    }

    /// A glyph takes the box it was given, so a row of them lines up whatever art is in
    /// them.
    #[test]
    fn the_widget_takes_exactly_the_box_it_was_asked_for() {
        let (ctx, input) = headless();
        let _ = ctx.run(input, |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                for glyph in Glyph::ALL {
                    for size in [10.0_f32, 15.0] {
                        let drawn = icon(ui, glyph, size, egui::Color32::WHITE);
                        assert_eq!(
                            drawn.rect.size(),
                            egui::vec2(size, size),
                            "{glyph:?} at {size}"
                        );
                    }
                }
            });
        });
    }
}
