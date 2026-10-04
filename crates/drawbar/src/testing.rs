//! Headless frames, what they painted, and the state a browser act runs against, for
//! the UI tests of every module.

#[cfg(not(target_arch = "wasm32"))]
use std::fs;
#[cfg(not(target_arch = "wasm32"))]
use std::path::PathBuf;
#[cfg(not(target_arch = "wasm32"))]
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use eframe::egui;

use crate::browser::{apply, Act, Browser};
use crate::builds::Builds;
use crate::device::Device;
use crate::log::Log;
use crate::queue::Queue;
use crate::shell::Shell;
use crate::tabs::Tabs;
use crate::workspace::Workspace;

/// A directory of its own under the system's temp folder, removed when dropped.
#[cfg(not(target_arch = "wasm32"))]
pub(crate) struct Temp(pub PathBuf);

#[cfg(not(target_arch = "wasm32"))]
impl Temp {
    pub fn new() -> Temp {
        static NEXT: AtomicUsize = AtomicUsize::new(0);
        let dir = std::env::temp_dir().join(format!(
            "drawbar-library-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::SeqCst)
        ));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).expect("a temporary directory");
        Temp(dir)
    }

    pub fn at(&self, path: &str) -> PathBuf {
        self.0.join(path)
    }

    pub fn read(&self, path: &str) -> Vec<u8> {
        fs::read(self.at(path)).unwrap_or_else(|e| panic!("{path}: {e}"))
    }

    /// The names in one folder, sorted.
    pub fn names(&self, dir: &str) -> Vec<String> {
        let mut names: Vec<String> = fs::read_dir(self.at(dir))
            .unwrap_or_else(|e| panic!("{dir}: {e}"))
            .map(|entry| entry.unwrap().file_name().into_string().unwrap())
            .collect();
        names.sort();
        names
    }
}

#[cfg(not(target_arch = "wasm32"))]
impl Drop for Temp {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

/// eframe's string store, in a map.
#[derive(Default)]
pub(crate) struct Fake(std::collections::HashMap<String, String>);

impl eframe::Storage for Fake {
    fn get_string(&self, key: &str) -> Option<String> {
        self.0.get(key).cloned()
    }

    fn set_string(&mut self, key: &str, value: String) {
        self.0.insert(key.to_string(), value);
    }

    fn flush(&mut self) {}
}

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

/// [`sample_bytes`] with its `hdr` cut short of the name field, as the original
/// Sample Library stores it.
pub(crate) fn nameless_sample_bytes() -> Vec<u8> {
    use nord_format::formats::nsmp::section;
    let decoded = nord_format::from_stream(&mut std::io::Cursor::new(sample_bytes())).unwrap();
    let nord_format::Entity::Sample(nord_format::Sample::V2(mut body)) = decoded else {
        panic!("the default options build the narrow chain");
    };
    section::find_mut(&mut body.body.sections, section::HDR)
        .unwrap()
        .payload
        .truncate(18);
    body.to_bytes().unwrap()
}

/// A sample instrument of three zones under `layout`, whose stroke ids are out of file
/// order, each playing `frames` frames of a ramp.
#[cfg(not(target_arch = "wasm32"))]
pub(crate) fn zoned_sample(
    layout: nord_format::formats::nsmp::codec::Layout,
    frames: usize,
) -> Vec<u8> {
    use nord_format::formats::nsmp::encode::{self, Instrument, NewZone, Preset};

    let source: Vec<i16> = (0..frames).map(|k| ((k % 64) as i16 - 32) * 256).collect();
    let zone = |global_id, root_key, top_note| NewZone {
        source: &source,
        channels: 1,
        root_key,
        top_note,
        global_id,
        loops: None,
        secondary_start: encode::default_secondary_start(frames, None),
        shift: None,
        gain: 1.0,
        loop_decay: encode::DEFAULT_LOOP_DECAY,
    };
    let instrument = Instrument {
        name: "Zoned",
        map_gain: 1.0,
        predictor: encode::Predictor::Minimizing,
        layout,
        preset: Preset::default(),
    };
    let zones = [zone(3, 84, 127), zone(1, 60, 71), zone(2, 48, 59)];
    encode::multi_zone(instrument, &zones)
        .expect("the encoder lays out an instrument")
        .to_bytes()
        .expect("the instrument writes")
}

/// A piano library of three strokes, each of `blocks` blocks of audio, every key mapped to
/// the nearest. `u16::MAX` blocks is the most a stroke record can state, which makes a
/// library a vendor's size: about 200 MB in all.
#[cfg(not(target_arch = "wasm32"))]
pub(crate) fn piano(blocks: u16) -> Vec<u8> {
    use nord_format::formats::npno::synthetic::{take, Build};
    use nord_format::formats::npno::Bank;

    const ROOTS: [u8; 3] = [48, 60, 72];
    Build {
        version: 0x464,
        channels: 1,
        takes: ROOTS
            .into_iter()
            .map(|root| take(root, Bank::Attack, 0, blocks))
            .collect(),
        map: (21..=108)
            .map(|key: u8| {
                let root = ROOTS.into_iter().min_by_key(|root| root.abs_diff(key));
                (key, root.expect("three roots"))
            })
            .collect(),
    }
    .bytes()
    .expect("the builder lays out a library")
}

/// The system allocator, noting the largest single allocation a thread makes while it
/// watches.
pub(crate) struct Watching;

thread_local! {
    static LARGEST: std::cell::Cell<Option<usize>> = const { std::cell::Cell::new(None) };
}

/// Whether [`largest_allocation_anywhere`] is watching, and what it has seen.
static EVERY_THREAD: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
static LARGEST_ANYWHERE: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

fn note(size: usize) {
    let _ = LARGEST.try_with(|largest| {
        if let Some(held) = largest.get() {
            largest.set(Some(held.max(size)));
        }
    });
    if EVERY_THREAD.load(std::sync::atomic::Ordering::Relaxed) {
        LARGEST_ANYWHERE.fetch_max(size, std::sync::atomic::Ordering::Relaxed);
    }
}

// SAFETY: every call is passed to the system allocator unchanged.
unsafe impl std::alloc::GlobalAlloc for Watching {
    unsafe fn alloc(&self, layout: std::alloc::Layout) -> *mut u8 {
        note(layout.size());
        // SAFETY: as the caller promised the system allocator.
        unsafe { std::alloc::System.alloc(layout) }
    }

    unsafe fn alloc_zeroed(&self, layout: std::alloc::Layout) -> *mut u8 {
        note(layout.size());
        // SAFETY: as the caller promised the system allocator.
        unsafe { std::alloc::System.alloc_zeroed(layout) }
    }

    unsafe fn realloc(&self, ptr: *mut u8, layout: std::alloc::Layout, size: usize) -> *mut u8 {
        note(size);
        // SAFETY: as the caller promised the system allocator.
        unsafe { std::alloc::System.realloc(ptr, layout, size) }
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: std::alloc::Layout) {
        // SAFETY: as the caller promised the system allocator.
        unsafe { std::alloc::System.dealloc(ptr, layout) }
    }
}

#[global_allocator]
static WATCHING: Watching = Watching;

/// What `f` answers, and the largest single allocation it made on this thread.
#[cfg(not(target_arch = "wasm32"))]
pub(crate) fn largest_allocation<T>(f: impl FnOnce() -> T) -> (T, usize) {
    LARGEST.with(|largest| largest.set(Some(0)));
    let answer = f();
    let largest = LARGEST.with(|largest| largest.take()).unwrap_or_default();
    (answer, largest)
}

/// What `f` answers, and the largest single allocation any thread made while it ran: the
/// device's worker thread as well as this one.
///
/// ⚠️ Every thread of the test process counts, so tests running alongside count too.
#[cfg(not(target_arch = "wasm32"))]
pub(crate) fn largest_allocation_anywhere<T>(f: impl FnOnce() -> T) -> (T, usize) {
    LARGEST_ANYWHERE.store(0, Ordering::SeqCst);
    EVERY_THREAD.store(true, Ordering::SeqCst);
    let answer = f();
    EVERY_THREAD.store(false, Ordering::SeqCst);
    (answer, LARGEST_ANYWHERE.load(Ordering::SeqCst))
}

/// `bytes` written to `name` in `dir` and indexed in place, as a library opens a piano or
/// sample instrument.
#[cfg(not(target_arch = "wasm32"))]
pub(crate) fn on_disk(dir: &Temp, name: &str, bytes: &[u8]) -> Arc<crate::ondisk::OnDisk> {
    fs::write(dir.at(name), bytes).expect("the file is written");
    let file = fs::File::open(dir.at(name)).expect("the file opens");
    let indexed = crate::ondisk::OnDisk::open(file, None).expect("the file reads");
    Arc::new(indexed.expect("a piano or sample instrument is indexed"))
}

/// An asset resting in `file`, restored as a library restores one, and its id.
#[cfg(not(target_arch = "wasm32"))]
pub(crate) fn rest(workspace: &mut Workspace, name: &str, file: Arc<crate::ondisk::OnDisk>) -> u64 {
    let id = workspace.next_id();
    workspace.restore(
        vec![crate::workspace::Saved {
            id,
            name: name.to_string(),
            path: None,
            origin: crate::workspace::Origin::File(name.to_string()),
            saved: Vec::new(),
            file: Some(file),
            unread: None,
            unsaved: None,
        }],
        None,
        &mut Log::default(),
    );
    id
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
    pub builds: Builds,
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
            builds: Builds::default(),
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
            &mut self.builds,
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
