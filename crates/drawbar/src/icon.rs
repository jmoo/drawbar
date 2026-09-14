//! The vendored Lucide glyphs, and the one way they are drawn.
//!
//! The art in `assets/icons` is Lucide 0.469.0 under the ISC licence beside it,
//! rewritten once from `currentColor` to white so a tint multiplies to exactly the
//! colour asked for. Sizes in use are 10-15 px; a row picks one and keeps it.

use eframe::egui;

/// The vendored art: one line per glyph, naming the variant and the file behind it.
///
/// The enum, the sweep over every variant and the art each one loads all come off this
/// one list, so a glyph cannot be vendored, named or swept without the other two.
macro_rules! glyphs {
    ($($name:ident => $file:literal,)*) => {
        /// One vendored glyph. An enum, so a name nobody vendored is a compile error.
        #[derive(Clone, Copy, Debug, PartialEq, Eq)]
        pub enum Glyph {
            $($name,)*
        }

        impl Glyph {
            /// Every variant, so a sweep can prove each one still has art behind it.
            pub const ALL: &'static [Glyph] = &[$(Glyph::$name,)*];

            fn source(self) -> egui::ImageSource<'static> {
                match self {
                    $(Glyph::$name => {
                        egui::include_image!(concat!("../assets/icons/", $file))
                    })*
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
    Check => "check.svg",
    ChevronDown => "chevron-down.svg",
    ChevronRight => "chevron-right.svg",
    CircleAlert => "circle-alert.svg",
    CircleCheck => "circle-check.svg",
    CircleDashed => "circle-dashed.svg",
    CircleDot => "circle-dot.svg",
    CircleHelp => "circle-help.svg",
    CircleX => "circle-x.svg",
    Clock => "clock.svg",
    Columns2 => "columns-2.svg",
    Disc3 => "disc-3.svg",
    Equal => "equal.svg",
    Eye => "eye.svg",
    EyeOff => "eye-off.svg",
    FilePlus2 => "file-plus-2.svg",
    FileText => "file-text.svg",
    Folder => "folder.svg",
    FolderGit2 => "folder-git-2.svg",
    FolderOpen => "folder-open.svg",
    Gauge => "gauge.svg",
    GripVertical => "grip-vertical.svg",
    HardDrive => "hard-drive.svg",
    Info => "info.svg",
    Keyboard => "keyboard.svg",
    LibraryBig => "library-big.svg",
    Link2Off => "link-2-off.svg",
    ListMusic => "list-music.svg",
    Minus => "minus.svg",
    Moon => "moon.svg",
    PanelBottom => "panel-bottom.svg",
    PanelLeft => "panel-left.svg",
    PanelLeftOpen => "panel-left-open.svg",
    PanelRight => "panel-right.svg",
    PanelRightOpen => "panel-right-open.svg",
    Pencil => "pencil.svg",
    Piano => "piano.svg",
    Plus => "plus.svg",
    RefreshCw => "refresh-cw.svg",
    Replace => "replace.svg",
    RotateCcw => "rotate-ccw.svg",
    Save => "save.svg",
    ScanEye => "scan-eye.svg",
    SlidersHorizontal => "sliders-horizontal.svg",
    SlidersVertical => "sliders-vertical.svg",
    Sun => "sun.svg",
    Tag => "tag.svg",
    Upload => "upload.svg",
    Waves => "waves.svg",
    Wrench => "wrench.svg",
    X => "x.svg",
}

impl Glyph {
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
            for glyph in Glyph::ALL.iter().copied() {
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
