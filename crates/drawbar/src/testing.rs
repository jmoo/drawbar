//! Headless frames, what they painted, and the state a browser act runs against, for
//! the UI tests of every module.

use std::sync::Arc;

use eframe::egui;

use crate::browser::{apply, Act, Browser};
use crate::device::Device;
use crate::log::Log;
use crate::queue::Queue;
use crate::shell::Shell;
use crate::tabs::Tabs;
use crate::workspace::Workspace;

/// A context styled as `DrawbarApp::new` styles one.
///
/// ⚠️ `DrawbarApp::new` installs the fonts and the named text styles the panels resolve,
/// in both the dark and the light style. A style without them panics the frame that
/// resolves one.
pub(crate) fn context() -> egui::Context {
    let ctx = egui::Context::default();
    ctx.set_fonts(crate::app::fonts());
    ctx.all_styles_mut(crate::app::metrics);
    ctx
}

/// One frame of `ui`.
///
/// Panics if egui found two widgets sharing an id. egui 0.32 reports a clash only by
/// painting a warning, so a frame that merely returned would pass over one.
///
/// ⚠️ egui paints that warning only in debug builds unless asked, and the packaged
/// suites run in release.
pub(crate) fn run(
    ctx: &egui::Context,
    input: egui::RawInput,
    ui: impl FnMut(&egui::Context),
) -> egui::FullOutput {
    ctx.options_mut(|options| options.warn_on_id_clash = true);
    let output = ctx.run(input, ui);
    let clashes: Vec<String> = words(&output)
        .into_iter()
        .filter(|word| word.starts_with("🔥 ") && word.contains(" use of "))
        .collect();
    assert!(clashes.is_empty(), "egui found an id clash: {clashes:?}");
    output
}

/// Input for one frame on a screen of `size`.
pub(crate) fn screen(size: egui::Vec2, events: Vec<egui::Event>) -> egui::RawInput {
    egui::RawInput {
        events,
        screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, size)),
        ..Default::default()
    }
}

/// One piece of text a frame painted.
#[derive(Clone)]
pub(crate) struct Word {
    pub text: String,
    /// The galley's layout box where it was painted.
    pub rect: egui::Rect,
    /// The bounds of the painted glyphs, tighter than `rect`.
    pub bounds: egui::Rect,
    /// The clip rect of the layer it was painted into.
    pub clip: egui::Rect,
    pub ink: egui::Color32,
    pub galley: Arc<egui::Galley>,
}

impl std::fmt::Debug for Word {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{:?} at {:?}", self.text, self.rect)
    }
}

/// Every leaf shape a frame painted, with the clip rect it was painted under, in paint
/// order.
pub(crate) fn shapes(output: &egui::FullOutput) -> Vec<(egui::Shape, egui::Rect)> {
    fn walk(shape: &egui::Shape, clip: egui::Rect, into: &mut Vec<(egui::Shape, egui::Rect)>) {
        match shape {
            egui::Shape::Vec(shapes) => shapes.iter().for_each(|shape| walk(shape, clip, into)),
            leaf => into.push((leaf.clone(), clip)),
        }
    }
    let mut found = Vec::new();
    for clipped in &output.shapes {
        walk(&clipped.shape, clipped.clip_rect, &mut found);
    }
    found
}

/// Every text a frame painted, in paint order.
pub(crate) fn painted(output: &egui::FullOutput) -> Vec<Word> {
    shapes(output)
        .into_iter()
        .filter_map(|(shape, clip)| match shape {
            egui::Shape::Text(text) => {
                let ink = text.override_text_color.or_else(|| {
                    text.galley
                        .job
                        .sections
                        .first()
                        .map(|section| section.format.color)
                });
                Some(Word {
                    text: text.galley.text().to_string(),
                    rect: egui::Rect::from_min_size(text.pos, text.galley.size()),
                    bounds: text.visual_bounding_rect(),
                    clip,
                    ink: ink.unwrap_or(text.fallback_color),
                    galley: text.galley,
                })
            }
            _ => None,
        })
        .collect()
}

/// Every string a frame painted, in paint order.
pub(crate) fn words(output: &egui::FullOutput) -> Vec<String> {
    painted(output).into_iter().map(|word| word.text).collect()
}

/// Where `text` was first painted.
pub(crate) fn where_(said: &[Word], text: &str) -> egui::Rect {
    said.iter()
        .find(|word| word.text == text)
        .unwrap_or_else(|| panic!("{text} was never painted: {said:?}"))
        .rect
}

/// Every rectangle a frame painted, in paint order.
pub(crate) fn rects(output: &egui::FullOutput) -> Vec<egui::epaint::RectShape> {
    shapes(output)
        .into_iter()
        .filter_map(|(shape, _)| match shape {
            egui::Shape::Rect(drawn) => Some(drawn),
            _ => None,
        })
        .collect()
}

/// The fills a frame painted exactly over `rect`, innermost last.
pub(crate) fn fills(output: &egui::FullOutput, rect: egui::Rect) -> Vec<egui::Color32> {
    rects(output)
        .into_iter()
        .filter(|drawn| drawn.rect == rect)
        .map(|drawn| drawn.fill)
        .collect()
}

/// The primary button going down or coming up at `at`.
pub(crate) fn button(at: egui::Pos2, pressed: bool) -> egui::Event {
    egui::Event::PointerButton {
        pos: at,
        button: egui::PointerButton::Primary,
        pressed,
        modifiers: egui::Modifiers::NONE,
    }
}

/// A full click at one point: the pointer moves there, presses, and releases.
pub(crate) fn click(at: egui::Pos2) -> Vec<egui::Event> {
    vec![
        egui::Event::PointerMoved(at),
        button(at, true),
        button(at, false),
    ]
}

/// `key` pressed with no modifier.
pub(crate) fn key(key: egui::Key) -> egui::Event {
    egui::Event::Key {
        key,
        physical_key: None,
        pressed: true,
        repeat: false,
        modifiers: egui::Modifiers::NONE,
    }
}

/// One second of 44.1 kHz mono, long enough for the sample encoder's shortest stroke.
pub(crate) fn wav_bytes() -> Vec<u8> {
    use nord_format::formats::nsmp::codec;
    let samples: Vec<i16> = (0..codec::SOURCE_RATE as usize)
        .map(|i| ((i as f64 / 40.0).sin() * 12_000.0) as i16)
        .collect();
    nord_format::wav::mono_pcm16(&samples, codec::SOURCE_RATE).unwrap()
}

/// [`wav_bytes`] encoded as a sample named Marimba.
pub(crate) fn sample_bytes() -> Vec<u8> {
    let source = nord_format::wav::read_pcm16(&wav_bytes()).unwrap();
    let options = nord_format::formats::nsmp::encode::Options::new("Marimba");
    nord_format::formats::nsmp::encode::instrument(&source.samples, &options)
        .unwrap()
        .to_bytes()
        .unwrap()
}

/// Everything [`apply`] runs a browser act against, on one context.
pub(crate) struct Bench {
    pub ctx: egui::Context,
    pub browser: Browser,
    pub shell: Shell,
    pub workspace: Workspace,
    pub device: Device,
    pub tabs: Tabs,
    pub queue: Queue,
    pub log: Log,
}

impl Bench {
    pub fn new() -> Bench {
        let ctx = context();
        Bench {
            browser: Browser::default(),
            shell: Shell::default(),
            workspace: Workspace::new(ctx.clone()),
            device: Device::new(ctx.clone()),
            tabs: Tabs::default(),
            queue: Queue::default(),
            log: Log::default(),
            ctx,
        }
    }

    /// Run `acts` as the app runs what the browser asked for.
    pub fn act(&mut self, acts: Vec<Act>) {
        apply(
            &mut self.browser,
            &mut self.shell,
            acts,
            &mut self.workspace,
            &mut self.device,
            &mut self.tabs,
            &mut self.queue,
            &mut self.log,
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_frame_that_gives_two_widgets_one_id_fails() {
        let ctx = context();
        let clashed = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            run(&ctx, egui::RawInput::default(), |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| {
                    let id = egui::Id::new("twice");
                    ui.interact(
                        egui::Rect::from_min_size(egui::pos2(10.0, 10.0), egui::vec2(40.0, 20.0)),
                        id,
                        egui::Sense::click(),
                    );
                    ui.interact(
                        egui::Rect::from_min_size(egui::pos2(10.0, 90.0), egui::vec2(40.0, 20.0)),
                        id,
                        egui::Sense::click(),
                    );
                });
            })
        }));
        assert!(clashed.is_err(), "a clash painted only a warning");
    }
}
