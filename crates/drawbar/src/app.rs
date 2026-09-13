//! The app: the theme both faces share, and the routing between the regions.
//!
//! The regions themselves — the title bar, the toolbar, the three docks, the status
//! bar — are [`crate::shell`]; [`DrawbarApp::update`] is the order they claim space in
//! and nothing else.

use eframe::egui;

use crate::browser::{self, Browser};
use crate::device::Device;
use crate::document::Document;
use crate::keyboard::Keyboard;
use crate::library::Library;
use crate::log::Log;
use crate::queue::Queue;
use crate::shell::Shell;
use crate::tabs::{Spot, Tabs};
use crate::workspace::{Origin, Workspace};

/// A theme-specific success color with enough contrast for small text.
pub fn good(visuals: &egui::Visuals) -> egui::Color32 {
    match visuals.dark_mode {
        true => egui::Color32::from_rgb(0x60, 0xc0, 0x70),
        false => egui::Color32::from_rgb(0x0f, 0x62, 0x2a),
    }
}

pub fn warn(visuals: &egui::Visuals) -> egui::Color32 {
    match visuals.dark_mode {
        true => egui::Color32::from_rgb(0xe0, 0xa0, 0x30),
        false => egui::Color32::from_rgb(0x8a, 0x4b, 0x00),
    }
}

pub fn bad(visuals: &egui::Visuals) -> egui::Color32 {
    match visuals.dark_mode {
        true => egui::Color32::from_rgb(0xe0, 0x50, 0x40),
        false => egui::Color32::from_rgb(0xa3, 0x14, 0x0c),
    }
}

/// The red the instrument itself is: a lit lamp, a knob's travelled arc, a selected row.
pub fn accent(visuals: &egui::Visuals) -> egui::Color32 {
    match visuals.dark_mode {
        true => egui::Color32::from_rgb(0xd6, 0x46, 0x3a),
        false => egui::Color32::from_rgb(0xb0, 0x1e, 0x12),
    }
}

/// The ink a MICRO-caps header wears: a step quieter than the body ink beneath it.
pub fn caption(visuals: &egui::Visuals) -> egui::Color32 {
    match visuals.dark_mode {
        true => egui::Color32::from_gray(0xa0),
        false => egui::Color32::from_gray(0x28),
    }
}

/// The unlit half of a control, kept visible against either panel.
pub fn unlit(visuals: &egui::Visuals) -> egui::Color32 {
    match visuals.dark_mode {
        true => egui::Color32::from_gray(0x5a),
        false => egui::Color32::from_gray(0x82),
    }
}

/// The ivory of a white key or an unison drawbar stop.
///
/// Both faces share it: a key is the same colour under any light, and the stops are the
/// instrument's own plastic rather than part of the app's dress.
pub fn stop_white(_visuals: &egui::Visuals) -> egui::Color32 {
    egui::Color32::from_rgb(0xd8, 0xd6, 0xd0)
}

/// The ebony of a black key or a mutation drawbar stop.
pub fn stop_black(_visuals: &egui::Visuals) -> egui::Color32 {
    egui::Color32::from_rgb(0x2a, 0x2a, 0x2e)
}

/// The persisted theme choice. `System` follows the host preference.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ThemeChoice {
    #[default]
    System,
    Light,
    Dark,
}

impl ThemeChoice {
    /// Where the choice is kept between sessions.
    pub(crate) const KEY: &'static str = "drawbar.theme";

    fn read(text: &str) -> ThemeChoice {
        match text {
            "light" => ThemeChoice::Light,
            "dark" => ThemeChoice::Dark,
            _ => ThemeChoice::System,
        }
    }

    pub(crate) fn stored(self) -> &'static str {
        match self {
            ThemeChoice::System => "system",
            ThemeChoice::Light => "light",
            ThemeChoice::Dark => "dark",
        }
    }

    pub(crate) fn next(self) -> ThemeChoice {
        match self {
            ThemeChoice::System => ThemeChoice::Light,
            ThemeChoice::Light => ThemeChoice::Dark,
            ThemeChoice::Dark => ThemeChoice::System,
        }
    }

    /// The three words beside the sun or the moon.
    pub(crate) fn label(self) -> &'static str {
        match self {
            ThemeChoice::System => "Theme: auto",
            ThemeChoice::Light => "Theme: light",
            ThemeChoice::Dark => "Theme: dark",
        }
    }

    pub(crate) fn hint(self) -> &'static str {
        match self {
            ThemeChoice::System => "following the system — click for light",
            ThemeChoice::Light => "held light — click for dark",
            ThemeChoice::Dark => "held dark — click to follow the system again",
        }
    }

    pub(crate) fn preference(self) -> egui::ThemePreference {
        match self {
            ThemeChoice::System => egui::ThemePreference::System,
            ThemeChoice::Light => egui::ThemePreference::Light,
            ThemeChoice::Dark => egui::ThemePreference::Dark,
        }
    }
}

/// A small filled dot: something changed here, or something is attached here.
///
/// `size` is the box it claims; the dot inside keeps a pixel of air either side, so a
/// row of them reads as marks rather than as a rule.
///
/// ⚠️ Painted rather than typed. The bundled fonts have no glyph for `●`, and a missing
/// one renders as an empty box — which reads as a checkbox nobody can tick.
pub fn dot(ui: &mut egui::Ui, color: egui::Color32, size: f32) -> egui::Response {
    let (rect, response) = ui.allocate_exact_size(egui::Vec2::splat(size), egui::Sense::hover());
    ui.painter()
        .circle_filled(rect.center(), size / 2.0 - 1.0, color);
    response
}

pub struct DrawbarApp {
    pub(crate) workspace: Workspace,
    pub(crate) device: Device,
    pub(crate) browser: Browser,
    pub(crate) library: Library,
    pub(crate) keyboard: Keyboard,
    pub(crate) queue: Queue,
    pub(crate) shell: Shell,
    pub(crate) tabs: Tabs,
    pub(crate) document: Document,
    pub(crate) log: Log,
    pub(crate) theme: ThemeChoice,
    /// The list's revision as the store last saw it.
    saved: u64,
    /// When the store was last caught up, on egui's own clock.
    saved_at: f64,
}

impl DrawbarApp {
    pub fn new(cc: &eframe::CreationContext<'_>) -> DrawbarApp {
        // Without this every `Glyph` draws as egui's broken-image warning.
        egui_extras::install_image_loaders(&cc.egui_ctx);
        cc.egui_ctx.set_fonts(fonts());
        // Both faces are dressed up front, so the system flipping from light to dark mid
        // session lands on this app's own colours rather than egui's defaults.
        cc.egui_ctx.set_visuals_of(egui::Theme::Dark, dark());
        cc.egui_ctx.set_visuals_of(egui::Theme::Light, light());
        // ⚠️ Both faces, not the one showing: egui keeps a `Style` per theme, and a
        // face that never learned the named text styles panics the frame that resolves
        // one.
        cc.egui_ctx.all_styles_mut(metrics);
        let theme = cc
            .storage
            .and_then(|storage| storage.get_string(ThemeChoice::KEY))
            .map_or(ThemeChoice::default(), |text| ThemeChoice::read(&text));
        cc.egui_ctx.set_theme(theme.preference());
        let mut app = DrawbarApp {
            workspace: Workspace::new(cc.egui_ctx.clone()),
            device: Device::new(cc.egui_ctx.clone()),
            browser: Browser::default(),
            library: Library::default(),
            keyboard: Keyboard::default(),
            queue: Queue::default(),
            shell: Shell::default(),
            tabs: Tabs::default(),
            document: Document::default(),
            log: Log::default(),
            theme,
            saved: 0,
            saved_at: 0.0,
        };
        if let Some(storage) = cc.storage {
            crate::store::load(storage, &mut app.workspace, &mut app.log);
            app.browser.restore(storage);
            app.shell.restore(storage);
            // Both stores are read; only now does the grouping know what survived.
            app.browser.settle(&app.workspace);
        }
        app.saved = app.workspace.revision();
        app
    }

    /// Ingest anything dropped on the window, or hand it to the New dialog while one is
    /// open and it is a WAV.
    ///
    /// The web backend fills `bytes` and the native backend fills `path`, so both are
    /// handled rather than cfg'd apart.
    fn take_dropped_files(&mut self, ctx: &egui::Context) {
        let dropped = ctx.input(|i| i.raw.dropped_files.clone());
        let drafting = self.workspace.draft_mut().is_some();
        let mut joining = Vec::new();
        for file in dropped {
            let name = match (file.name.is_empty(), &file.path) {
                (false, _) => file.name.clone(),
                (true, Some(path)) => path
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_else(|| path.display().to_string()),
                (true, None) => "dropped".to_string(),
            };
            let bytes = match (&file.bytes, &file.path) {
                (Some(bytes), _) => Some(bytes.to_vec()),
                (None, Some(path)) => match std::fs::read(path) {
                    Ok(bytes) => Some(bytes),
                    Err(e) => {
                        self.log.error(format!("{name}: {e}"));
                        self.log.trouble(format!("Could not read {name}."));
                        None
                    }
                },
                (None, None) => {
                    self.log
                        .trouble(format!("{name} arrived with no contents."));
                    None
                }
            };
            let Some(bytes) = bytes else { continue };
            match drafting && crate::newproject::is_wav_name(&name) {
                true => joining.push((name, bytes)),
                false => {
                    self.workspace
                        .ingest(name.clone(), Origin::File(name), bytes, &mut self.log);
                }
            }
        }
        if let Some(draft) = self.workspace.draft_mut() {
            draft.add(joining);
        }
    }

    /// Persist changes from their own frame; an idle egui window may not repaint.
    /// Writes are rate-limited because dragging changes the list every frame.
    fn keep_up(&mut self, ctx: &egui::Context, frame: &mut eframe::Frame) {
        /// How often the store is allowed to be rewritten, in seconds.
        const EVERY: f64 = 2.0;

        if self.workspace.revision() == self.saved {
            return;
        }
        let now = ctx.input(|i| i.time);
        if now - self.saved_at < EVERY {
            // Nothing else may be about to ask for a frame, and the write is still owed.
            ctx.request_repaint_after(std::time::Duration::from_secs(1));
            return;
        }
        let Some(storage) = frame.storage_mut() else {
            return;
        };
        crate::store::save(storage, &self.workspace, &self.queue, &mut self.log);
        self.saved = self.workspace.revision();
        self.saved_at = now;
    }
}

impl eframe::App for DrawbarApp {
    /// How long a change may sit unwritten.
    fn auto_save_interval(&self) -> std::time::Duration {
        std::time::Duration::from_secs(5)
    }

    /// eframe calls this on its own timer and on the way out, so an edit is kept
    /// without anyone asking for it to be.
    fn save(&mut self, storage: &mut dyn eframe::Storage) {
        crate::store::save(storage, &self.workspace, &self.queue, &mut self.log);
        storage.set_string(ThemeChoice::KEY, self.theme.stored().to_string());
        // Not written from the frame that changed it, the way the theme is: a divider
        // moves on every frame of a drag, and the whole store is rewritten each time.
        self.browser.keep(storage);
        self.shell.keep(storage);
        self.saved = self.workspace.revision();
    }

    fn update(&mut self, ctx: &egui::Context, frame: &mut eframe::Frame) {
        self.log.tick(ctx);
        self.workspace.poll(&mut self.log);
        self.device.poll(
            &mut self.log,
            &mut self.workspace,
            &mut self.tabs,
            &mut self.queue,
        );
        // An edit under a waiting entry changes what that write would do, and the queue
        // says so from the occupant it already read.
        crate::queue::follow(
            &self.workspace,
            &mut self.device,
            &mut self.queue,
            &mut self.log,
        );
        self.tabs.prune(&self.workspace);
        // Unedited views have no owner once their tab closes. An edited view is the only
        // copy of that edit and must survive.
        self.workspace.close_views(
            |id| self.tabs.holds(id),
            |id| self.document.pends(id),
            &self.queue,
            &mut self.log,
        );
        self.take_dropped_files(ctx);
        drop_hint(ctx);
        // Raised by a New pick of WAVs, and answered before anything else this frame
        // draws: it is a modal over the whole window.
        if let Some(made) = crate::newproject::dialog(ctx, &mut self.workspace, &mut self.log) {
            self.tabs.open(made);
        }

        // Before the panels, so an editor open in this frame still has the focus Escape
        // belongs to.
        self.browser.let_go(ctx);

        // Outside in. A panel claims its space from what the ones before it left.
        let mut acts = self
            .document
            .released(ctx, &mut self.workspace, &mut self.log);
        self.titlebar(ctx, frame, &mut acts);
        self.toolbar(ctx, &mut acts);
        self.status_bar(ctx, &mut acts);
        self.bottom_dock(ctx, &mut acts);
        self.browser_dock(ctx, &mut acts);
        self.inspector_dock(ctx, &mut acts);
        self.centre(ctx, &mut acts);

        // ⚠️ Between the panels and the acts they asked for: a piano library's plan is
        // not in its bytes yet, and whatever would carry those bytes waits here until it
        // is.
        let acts = self
            .document
            .settle(ctx, acts, &mut self.workspace, &mut self.log);
        browser::apply(
            &mut self.browser,
            &mut self.shell,
            acts,
            &mut self.workspace,
            &mut self.device,
            &mut self.tabs,
            &mut self.queue,
            &mut self.log,
        );
        // Last, so a command the user just asked for is ahead of the background read of
        // the tree in the one slot the protocol allows.
        self.device.pump();

        self.keep_up(ctx, frame);
    }
}

impl DrawbarApp {
    /// The tab strip, and whatever the tab in front is a view of.
    fn centre(&mut self, ctx: &egui::Context, acts: &mut Vec<browser::Act>) {
        let fill = ctx.style().visuals.panel_fill;
        egui::CentralPanel::default()
            .frame(egui::Frame::new().fill(fill))
            .show(ctx, |ui| {
                self.tabs.ui(ui, &self.workspace, acts);
                match self.tabs.showing() {
                    // The library is what the centre shows when no tab claims it.
                    None | Some(Spot::Library) => {
                        self.document.leave();
                        acts.extend(self.library.ui(
                            ui,
                            &mut self.browser,
                            &self.workspace,
                            &self.device,
                            &self.queue,
                            &self.shell,
                        ));
                    }
                    Some(Spot::Keyboard) => {
                        self.document.leave();
                        acts.extend(self.keyboard.ui(
                            ui,
                            &mut self.browser,
                            &self.workspace,
                            &self.device,
                            &self.queue,
                            &self.tabs,
                        ));
                    }
                    Some(Spot::Document(id)) => self.open_document(ui, id, acts),
                }
            });
    }

    /// A document owns its own room: the header is full bleed and the body inside it
    /// keeps the margin.
    fn open_document(&mut self, ui: &mut egui::Ui, id: u64, acts: &mut Vec<browser::Act>) {
        let around = crate::document::Around {
            queue: &self.queue,
            tags: self.browser.tags(),
        };
        let wants = self.document.ui(
            ui,
            id,
            &mut self.workspace,
            &mut self.device,
            &mut self.log,
            &around,
        );
        if let Some(send) = wants.send {
            acts.push(browser::Act::Send {
                id: send.id,
                class: send.class,
                at: send.at,
            });
        }
        if wants.keep {
            acts.push(browser::Act::Keep(id));
        }
        if let Some(item) = wants.open {
            acts.push(browser::Act::Open(item));
        }
    }
}

/// Dim the window while files hover, so a drop has somewhere it visibly lands.
fn drop_hint(ctx: &egui::Context) {
    if ctx.input(|i| i.raw.hovered_files.is_empty()) {
        return;
    }
    let painter = ctx.layer_painter(egui::LayerId::new(
        egui::Order::Foreground,
        egui::Id::new("drop_hint"),
    ));
    let screen = ctx.screen_rect();
    painter.rect_filled(screen, 0.0, egui::Color32::from_black_alpha(180));
    painter.text(
        screen.center(),
        egui::Align2::CENTER_CENTER,
        "Drop to open",
        egui::FontId::proportional(28.0),
        egui::Color32::WHITE,
    );
}

/// Dark, panel-like, with the accents kept for status alone.
fn dark() -> egui::Visuals {
    let mut visuals = egui::Visuals::dark();
    visuals.panel_fill = egui::Color32::from_rgb(0x16, 0x17, 0x19);
    visuals.window_fill = egui::Color32::from_rgb(0x1c, 0x1d, 0x20);
    visuals.faint_bg_color = egui::Color32::from_rgb(0x22, 0x23, 0x26);
    // A group's border is the only thing between one section and the next, so it is
    // lifted clear of egui's own hairline.
    visuals.widgets.noninteractive.bg_stroke.color = egui::Color32::from_gray(0x4e);
    // ⚠️ Both slots carry the body ink: `noninteractive` is what `Visuals::text_color`
    // answers, so a painted row and a button would otherwise disagree. The quieter
    // caption ink is `caption`.
    visuals.widgets.inactive.fg_stroke.color = egui::Color32::from_gray(0xc8);
    visuals.widgets.noninteractive.fg_stroke.color = egui::Color32::from_gray(0xc8);
    visuals.selection.bg_fill = egui::Color32::from_rgb(0x7a, 0x24, 0x24);
    // ⚠️ This also colors drop targets and focused knobs; inheriting egui's blue would
    // introduce a second accent.
    visuals.selection.stroke.color = egui::Color32::from_rgb(0xff, 0xdf, 0xd8);
    // Slot numbers and knob captions use the weak text color.
    visuals.weak_text_alpha = 0.85;
    visuals.hyperlink_color = bad(&visuals);
    visuals
}

/// The light theme, with stronger text and marks than egui's defaults.
fn light() -> egui::Visuals {
    let mut visuals = egui::Visuals::light();
    visuals.panel_fill = egui::Color32::from_rgb(0xf2, 0xf1, 0xee);
    visuals.window_fill = egui::Color32::from_rgb(0xfa, 0xf9, 0xf7);
    visuals.faint_bg_color = egui::Color32::from_rgb(0xdc, 0xd8, 0xce);
    visuals.selection.bg_fill = egui::Color32::from_rgb(0xe9, 0xa9, 0x9f);
    visuals.selection.stroke.color = egui::Color32::from_rgb(0x3a, 0x14, 0x10);
    visuals.widgets.noninteractive.fg_stroke.color = egui::Color32::from_gray(0x1c);
    visuals.widgets.inactive.fg_stroke.color = egui::Color32::from_gray(0x1c);
    visuals.widgets.noninteractive.bg_stroke.color = egui::Color32::from_gray(0x8a);
    visuals.weak_text_alpha = 0.9;
    visuals.hyperlink_color = bad(&visuals);
    visuals
}

/// The one family with weight in it, for the word-mark and nothing else.
pub fn bold() -> egui::FontFamily {
    egui::FontFamily::Name("bold".into())
}

/// Ubuntu Regular for the body and Ubuntu Bold beside it, over egui's own faces.
///
/// The files in `assets/fonts` are the Ubuntu font family 0.83 under the Ubuntu Font
/// Licence 1.0 beside them. egui bundles Ubuntu Light alone, so without these there is no
/// heavier weight to ask for and no 400 to set the body in.
pub(crate) fn fonts() -> egui::FontDefinitions {
    let mut fonts = egui::FontDefinitions::default();
    let bundled = fonts.families[&egui::FontFamily::Proportional].clone();
    for (family, face, ttf) in [
        (
            egui::FontFamily::Proportional,
            "Ubuntu",
            include_bytes!("../assets/fonts/Ubuntu-R.ttf").as_slice(),
        ),
        (
            bold(),
            "Ubuntu-Bold",
            include_bytes!("../assets/fonts/Ubuntu-B.ttf").as_slice(),
        ),
    ] {
        fonts.font_data.insert(
            face.to_owned(),
            std::sync::Arc::new(egui::FontData::from_static(ttf)),
        );
        let mut faces = bundled.clone();
        faces.insert(0, face.to_owned());
        fonts.families.insert(family, faces);
    }
    fonts
}

/// The text of the shell itself: menus, tabs, rail rows and cells.
///
/// A function rather than a const because [`egui::TextStyle::Name`] holds an `Arc<str>`.
pub fn ui() -> egui::TextStyle {
    egui::TextStyle::Name("ui".into())
}

/// The smallest text: panel headers and column heads, which are also uppercased.
pub fn micro() -> egui::TextStyle {
    egui::TextStyle::Name("micro".into())
}

/// The metrics both faces share: the room a control is given, and the room around it.
///
/// Theme-independent on purpose — flipping light to dark must not move anything.
pub(crate) fn metrics(style: &mut egui::Style) {
    let spacing = &mut style.spacing;
    spacing.item_spacing = egui::vec2(8.0, 4.0);
    // A button was 1px taller than its own text; a strip of them read as a solid bar.
    spacing.button_padding = egui::vec2(7.0, 3.0);
    // Panels own their inner padding, so the shared margin claims none of it.
    spacing.window_margin = egui::Margin::same(0);
    spacing.menu_margin = egui::Margin::same(4);
    spacing.indent = 18.0;
    spacing.interact_size.y = 18.0;
    spacing.scroll.bar_width = 8.0;
    style
        .text_styles
        .insert(ui(), egui::FontId::proportional(11.5));
    style
        .text_styles
        .insert(micro(), egui::FontId::proportional(9.5));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_theme_choice_round_trips_and_cycles_home() {
        let mut choice = ThemeChoice::System;
        let mut seen = Vec::new();
        for _ in 0..3 {
            seen.push(choice.stored());
            assert_eq!(ThemeChoice::read(choice.stored()), choice);
            choice = choice.next();
        }
        assert_eq!(seen, ["system", "light", "dark"]);
        assert_eq!(choice, ThemeChoice::System, "the cycle closes");
        // Anything the store cannot account for is the unset state, never a forced one.
        assert_eq!(ThemeChoice::read("moonlight"), ThemeChoice::System);
        assert_eq!(ThemeChoice::read(""), ThemeChoice::System);
    }

    #[test]
    fn each_theme_has_its_own_accents() {
        let (dark, light) = (dark(), light());
        assert!(dark.dark_mode && !light.dark_mode);
        for accent in [good, warn, bad, accent] {
            assert_ne!(accent(&dark), accent(&light));
        }
        // The panel is dark and the paper is light, whatever egui's own defaults do.
        assert!(dark.panel_fill.intensity() < 0.2);
        assert!(light.panel_fill.intensity() > 0.8);
    }

    fn luminance(color: egui::Color32) -> f32 {
        let channel = egui::ecolor::linear_f32_from_gamma_u8;
        0.2126 * channel(color.r()) + 0.7152 * channel(color.g()) + 0.0722 * channel(color.b())
    }

    // Color32 stores premultiplied alpha, so measure the rendered color.
    fn over(fg: egui::Color32, bg: egui::Color32) -> egui::Color32 {
        let rest = 1.0 - fg.a() as f32 / 255.0;
        let mix = |fg: u8, bg: u8| (fg as f32 + bg as f32 * rest).round() as u8;
        egui::Color32::from_rgb(
            mix(fg.r(), bg.r()),
            mix(fg.g(), bg.g()),
            mix(fg.b(), bg.b()),
        )
    }

    fn contrast(fg: egui::Color32, bg: egui::Color32) -> f32 {
        let (a, b) = (luminance(over(fg, bg)), luminance(bg));
        (a.max(b) + 0.05) / (a.min(b) + 0.05)
    }

    fn named(visuals: &egui::Visuals) -> &'static str {
        match visuals.dark_mode {
            true => "dark",
            false => "light",
        }
    }

    #[test]
    fn every_accent_carries_against_the_panel_it_is_painted_on() {
        for visuals in [dark(), light()] {
            let (where_, panel) = (named(&visuals), visuals.panel_fill);
            for signal in [good, warn, bad] {
                let ratio = contrast(signal(&visuals), panel);
                assert!(ratio >= 4.5, "{where_}: {ratio:.2}:1");
            }
            let lit = contrast(accent(&visuals), panel);
            assert!(lit >= 3.0, "{where_} accent: {lit:.2}:1");
            // The untravelled track is quieter than a mark but must remain visible.
            let track = contrast(unlit(&visuals), panel);
            assert!(track >= 2.4, "{where_} track: {track:.2}:1");
            // Selection needs legible text and a fill distinct from the panel.
            let selected = visuals.selection.bg_fill;
            let ink = contrast(visuals.selection.stroke.color, selected);
            assert!(ink >= 4.5, "{where_} selected text: {ink:.2}:1");
            assert!(contrast(selected, panel) >= 1.4, "{where_} selected fill");
        }
    }

    #[test]
    fn weak_text_stays_legible_in_both_themes() {
        for visuals in [dark(), light()] {
            let (where_, panel) = (named(&visuals), visuals.panel_fill);
            let weak = contrast(visuals.weak_text_color(), panel);
            assert!(weak >= 3.0, "{where_} weak: {weak:.2}:1");
            let body = contrast(visuals.text_color(), panel);
            assert!(body >= 4.5, "{where_} body: {body:.2}:1");
        }
    }

    /// The light face is read on paper, where a mid grey is a whisper. Its body ink is
    /// `#1c1c1c` and its captions `#282828`, and neither is allowed to drift back up.
    #[test]
    fn the_light_face_writes_in_ink_rather_than_pencil() {
        let light = light();
        let panel = light.panel_fill;
        assert_eq!(light.text_color(), egui::Color32::from_gray(0x1c));
        assert_eq!(caption(&light), egui::Color32::from_gray(0x28));

        let body = contrast(light.text_color(), panel);
        assert!(body >= 12.0, "light body: {body:.2}:1");
        let heading = contrast(caption(&light), panel);
        assert!(heading >= 12.0, "light caption: {heading:.2}:1");
        // Weak text carries a whole sentence in the inspector, so it holds body-text
        // contrast rather than the 3.0 a large mark would get away with.
        let weak = contrast(light.weak_text_color(), panel);
        assert!(weak >= 4.5, "light weak: {weak:.2}:1");
    }

    /// A caption sits over the same panel as the body it heads, and is quieter than it
    /// without becoming a grey nobody can read.
    #[test]
    fn a_caption_is_quieter_than_the_body_under_it_in_both_themes() {
        for visuals in [dark(), light()] {
            let (where_, panel) = (named(&visuals), visuals.panel_fill);
            let heading = contrast(caption(&visuals), panel);
            let body = contrast(visuals.text_color(), panel);
            assert!(heading >= 4.5, "{where_} caption: {heading:.2}:1");
            assert!(heading < body, "{where_}: {heading:.2}:1 vs {body:.2}:1");
        }
    }

    #[test]
    fn a_group_border_separates_it_from_the_panel_behind_it() {
        for visuals in [dark(), light()] {
            let (where_, panel) = (named(&visuals), visuals.panel_fill);
            let border = visuals.widgets.noninteractive.bg_stroke.color;
            let ratio = contrast(border, panel);
            assert!(ratio >= 1.9, "{where_} group border: {ratio:.2}:1");
        }
    }

    #[test]
    fn each_family_leads_with_its_ubuntu_face_over_the_same_fallbacks() {
        let fonts = fonts();
        assert!(fonts.font_data.contains_key("Ubuntu"));
        assert!(fonts.font_data.contains_key("Ubuntu-Bold"));
        let body = &fonts.families[&egui::FontFamily::Proportional];
        let mark = &fonts.families[&bold()];
        assert_eq!(body.first().map(String::as_str), Some("Ubuntu"));
        assert_eq!(mark.first().map(String::as_str), Some("Ubuntu-Bold"));
        assert_eq!(body[1..], mark[1..]);
        assert!(
            !body[1..].is_empty(),
            "a glyph Ubuntu lacks would draw as tofu"
        );
    }

    #[test]
    fn the_shared_metrics_do_not_depend_on_the_theme() {
        let mut style = egui::Style::default();
        metrics(&mut style);
        // Both faces read one style, so there is nothing here to disagree about.
        let spacing = &style.spacing;
        assert_eq!(spacing.item_spacing, egui::vec2(8.0, 4.0));
        assert_eq!(spacing.button_padding, egui::vec2(7.0, 3.0));
        assert_eq!(spacing.window_margin, egui::Margin::same(0));
        assert_eq!(spacing.menu_margin, egui::Margin::same(4));
        assert_eq!(spacing.indent, 18.0);
        assert_eq!(spacing.interact_size.y, 18.0);
        assert_eq!(spacing.scroll.bar_width, 8.0);
        // Both named styles must be registered, or resolving one panics mid-frame.
        assert_eq!(style.text_styles[&ui()], egui::FontId::proportional(11.5));
        assert_eq!(style.text_styles[&micro()], egui::FontId::proportional(9.5));
    }
}
