//! The vendored Lucide glyphs and how they are drawn.
//!
//! The art in `assets/icons` is Lucide 0.469.0 under the ISC license beside it.
//! `scripts/icons.py` outlines its strokes into `drawbar-icons.ttf`, one glyph per entry
//! of the `glyphs!` list at U+E000 plus its position, and [`install`] teaches egui to
//! rasterize them white so a tint multiplies to the requested color. Sizes in use are
//! 10-15 px; a row uses one size throughout.

use std::sync::Arc;

use ab_glyph::{Font as _, FontRef, PxScale};
use eframe::egui;
use egui::load::{ImageLoadResult, ImageLoader, ImagePoll, LoadError, SizeHint};

/// The outlined art, written by `scripts/icons.py`.
const FONT: &[u8] = include_bytes!("../assets/fonts/drawbar-icons.ttf");

/// The vendored art: one line per glyph, naming the variant and its file.
///
/// The enum, the list of every variant, and the art each one loads all come from this
/// list, so a glyph cannot be vendored, named, or tested without the other two.
macro_rules! glyphs {
    ($($name:ident => $file:literal,)*) => {
        /// One vendored glyph. An enum, so a name nobody vendored is a compile error.
        #[derive(Clone, Copy, Debug, PartialEq, Eq)]
        pub enum Glyph {
            $($name,)*
        }

        impl Glyph {
            /// Every variant, so a test can prove each one still has art.
            pub const ALL: &'static [Glyph] = &[$(Glyph::$name,)*];

            /// ⚠️ egui's texture loader caches a texture per size only for a URI ending in
            /// `.svg`; any other URI keeps its first raster at every size.
            fn uri(self) -> &'static str {
                match self {
                    $(Glyph::$name => concat!("drawbar-icon://", $file),)*
                }
            }
        }
    };
}

glyphs! {
    ArrowDownToLine => "arrow-down-to-line.svg",
    ArrowRight => "arrow-right.svg",
    ArrowUpRight => "arrow-up-right.svg",
    AudioLines => "audio-lines.svg",
    AudioWaveform => "audio-waveform.svg",
    BookOpen => "book-open.svg",
    Cable => "cable.svg",
    Check => "check.svg",
    ChevronDown => "chevron-down.svg",
    ChevronRight => "chevron-right.svg",
    CircleAlert => "circle-alert.svg",
    CircleCheck => "circle-check.svg",
    CircleDashed => "circle-dashed.svg",
    CircleDot => "circle-dot.svg",
    CircleHelp => "circle-help.svg",
    CircleX => "circle-x.svg",
    Clipboard => "clipboard.svg",
    Clock => "clock.svg",
    Columns2 => "columns-2.svg",
    Copy => "copy.svg",
    Cpu => "cpu.svg",
    Disc3 => "disc-3.svg",
    Equal => "equal.svg",
    Eye => "eye.svg",
    EyeOff => "eye-off.svg",
    FileAudio => "file-audio.svg",
    FilePlus2 => "file-plus-2.svg",
    FileText => "file-text.svg",
    Folder => "folder.svg",
    FolderGit2 => "folder-git-2.svg",
    FolderOpen => "folder-open.svg",
    Gauge => "gauge.svg",
    Github => "github.svg",
    GripVertical => "grip-vertical.svg",
    Hammer => "hammer.svg",
    HardDrive => "hard-drive.svg",
    HardDriveDownload => "hard-drive-download.svg",
    Info => "info.svg",
    Keyboard => "keyboard.svg",
    LibraryBig => "library-big.svg",
    Link => "link.svg",
    Link2Off => "link-2-off.svg",
    ListMusic => "list-music.svg",
    Menu => "menu.svg",
    MessageSquareWarning => "message-square-warning.svg",
    Minus => "minus.svg",
    Moon => "moon.svg",
    PanelBottom => "panel-bottom.svg",
    PanelLeft => "panel-left.svg",
    PanelLeftClose => "panel-left-close.svg",
    PanelLeftOpen => "panel-left-open.svg",
    PanelRight => "panel-right.svg",
    PanelRightClose => "panel-right-close.svg",
    PanelRightOpen => "panel-right-open.svg",
    Pencil => "pencil.svg",
    Piano => "piano.svg",
    Plug => "plug.svg",
    Plus => "plus.svg",
    RefreshCw => "refresh-cw.svg",
    Replace => "replace.svg",
    RotateCcw => "rotate-ccw.svg",
    Save => "save.svg",
    ScanEye => "scan-eye.svg",
    ScrollText => "scroll-text.svg",
    Search => "search.svg",
    SlidersHorizontal => "sliders-horizontal.svg",
    SlidersVertical => "sliders-vertical.svg",
    Sparkles => "sparkles.svg",
    Square => "square.svg",
    Sun => "sun.svg",
    SunMoon => "sun-moon.svg",
    Tag => "tag.svg",
    Tags => "tags.svg",
    Trash2 => "trash-2.svg",
    TriangleAlert => "triangle-alert.svg",
    Upload => "upload.svg",
    Usb => "usb.svg",
    Waves => "waves.svg",
    Wrench => "wrench.svg",
    X => "x.svg",
    ZoomIn => "zoom-in.svg",
}

impl Glyph {
    /// This glyph as an image, for the widgets and the painters that take one.
    pub fn image(self) -> egui::Image<'static> {
        egui::Image::new(egui::ImageSource::Uri(self.uri().into()))
    }

    fn codepoint(self) -> char {
        char::from_u32(0xE000 + self as u32).expect("the glyph list fits the private use area")
    }
}

/// Lucide's view box, the size a glyph has before a size hint scales it.
const ART: f32 = 24.0;

/// Rasterizes `drawbar-icon://` URIs from `drawbar-icons.ttf`.
///
/// It keeps nothing: egui's texture loader asks only for a size it does not hold, and of
/// a glyph's sizes it keeps the ones drawn in the last pass, or the last one drawn.
struct Loader;

/// Teach `ctx` to draw [`Glyph`]s.
pub fn install(ctx: &egui::Context) {
    if !ctx.is_loader_installed(Loader::ID) {
        ctx.add_image_loader(Arc::new(Loader));
    }
}

impl Loader {
    const ID: &'static str = egui::generate_loader_id!(Loader);
}

/// The side in device pixels: egui's hints already include the pixels per point.
fn side(hint: SizeHint) -> u32 {
    let side = match hint {
        SizeHint::Scale(scale) => ART * scale.into_inner(),
        SizeHint::Width(side) | SizeHint::Height(side) => side as f32,
        SizeHint::Size { width, height, .. } => width.min(height) as f32,
    };
    side.round().max(1.0) as u32
}

fn rasterize(glyph: Glyph, side: u32) -> egui::ColorImage {
    let font = FontRef::try_from_slice(FONT).expect("scripts/icons.py writes a valid font");
    let mut image = egui::ColorImage::filled([side as usize; 2], egui::Color32::TRANSPARENT);
    let scaled = font.glyph_id(glyph.codepoint()).with_scale_and_position(
        PxScale::from(side as f32),
        ab_glyph::point(0.0, side as f32),
    );
    if let Some(outline) = font.outline_glyph(scaled) {
        let bounds = outline.px_bounds();
        outline.draw(|x, y, coverage| {
            let (x, y) = (
                x as i64 + bounds.min.x as i64,
                y as i64 + bounds.min.y as i64,
            );
            if (0..side as i64).contains(&x) && (0..side as i64).contains(&y) {
                let alpha = (coverage.clamp(0.0, 1.0) * 255.0).round() as u8;
                image.pixels[y as usize * side as usize + x as usize] =
                    egui::Color32::from_white_alpha(alpha);
            }
        });
    }
    image
}

impl ImageLoader for Loader {
    fn id(&self) -> &str {
        Self::ID
    }

    fn load(&self, _: &egui::Context, uri: &str, hint: SizeHint) -> ImageLoadResult {
        let Some(glyph) = Glyph::ALL.iter().copied().find(|glyph| glyph.uri() == uri) else {
            return Err(LoadError::NotSupported);
        };
        Ok(ImagePoll::Ready {
            image: Arc::new(rasterize(glyph, side(hint))),
        })
    }

    fn forget(&self, _: &str) {}

    fn forget_all(&self) {}

    fn byte_size(&self) -> usize {
        0
    }
}

/// Draw `glyph` in a `size` by `size` box, painted in `tint`.
pub fn icon(ui: &mut egui::Ui, glyph: Glyph, size: f32, tint: egui::Color32) -> egui::Response {
    ui.add(sized(glyph, size, tint))
}

/// Draw `glyph` into `rect`, painted in `tint`, claiming no space of its own.
///
/// For strips that compute their own geometry; inside a layout, use [`icon`].
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
    use crate::testing;
    use egui::load::{ImagePoll, SizeHint};

    /// A 10 px glyph on a 2x display, the smallest raster the shell requests.
    const RASTER: u32 = 20;

    fn headless() -> (egui::Context, egui::RawInput) {
        let ctx = testing::context();
        install(&ctx);
        (ctx, testing::screen(egui::vec2(900.0, 540.0), Vec::new()))
    }

    #[test]
    fn every_glyph_rasterizes_to_something_that_can_be_seen() {
        let (ctx, input) = headless();
        testing::run(&ctx, input, |ctx| {
            for glyph in Glyph::ALL.iter().copied() {
                let poll = ctx
                    .try_load_image(
                        glyph.uri(),
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
                    "{glyph:?} rasterized to nothing"
                );
                // Premultiplied white is (a, a, a, a); no tint can lighten anything darker.
                assert!(
                    image.pixels.iter().all(|pixel| pixel.r() == pixel.a()
                        && pixel.g() == pixel.a()
                        && pixel.b() == pixel.a()),
                    "{glyph:?} has pixels a tint cannot color"
                );
            }
        });
    }

    #[test]
    fn the_icon_font_holds_exactly_one_outline_per_glyph() {
        let font = FontRef::try_from_slice(FONT).expect("drawbar-icons.ttf parses");
        assert_eq!(
            font.glyph_count(),
            Glyph::ALL.len() + 1,
            "drawbar-icons.ttf holds {} outlines and .notdef for {} glyphs; rerun scripts/icons.py",
            font.glyph_count() - 1,
            Glyph::ALL.len()
        );
        for glyph in Glyph::ALL.iter().copied() {
            assert_ne!(
                font.glyph_id(glyph.codepoint()).0,
                0,
                "{glyph:?} has no outline"
            );
        }
    }

    #[test]
    fn a_raster_has_the_device_pixels_the_hint_asks_for() {
        for (hint, side) in [
            (SizeHint::Scale(1.5.into()), 36),
            (SizeHint::Width(23), 23),
            (SizeHint::Height(15), 15),
            (
                SizeHint::Size {
                    width: 30,
                    height: 30,
                    maintain_aspect_ratio: true,
                },
                30,
            ),
        ] {
            let image = rasterize(Glyph::Check, super::side(hint));
            assert_eq!(image.size, [side; 2], "{hint:?}");
        }
    }

    /// A row of glyphs lines up whatever their art.
    #[test]
    fn the_widget_takes_exactly_the_box_it_was_asked_for() {
        let (ctx, input) = headless();
        testing::run(&ctx, input, |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                for glyph in Glyph::ALL.iter().copied() {
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
