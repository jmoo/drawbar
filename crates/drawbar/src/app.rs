//! The app: the light and dark themes, and the routing between the regions.
//!
//! The regions themselves (the top bar, the two side cards, the status line) live in
//! [`crate::shell`]; `DrawbarApp::update` sets only the order they claim space in.

use eframe::egui;

use crate::browser::{self, Browser};
use crate::device::Device;
use crate::document::Document;
use crate::keyboard::Keyboard;
use crate::library::Library;
use crate::log::Log;
use crate::midi::{Midi, Played};
use crate::platform::Platform;
use crate::queue::Queue;
use crate::shell::{Dock, Shell};
use crate::store::{Pass, Store};
use crate::tabs::{Spot, Tabs};
use crate::workspace::Workspace;
use crate::zoom::{Room, Zoom};

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

/// The instrument's own red: a lit lamp, a knob's traveled arc, a selected row.
pub fn accent(visuals: &egui::Visuals) -> egui::Color32 {
    match visuals.dark_mode {
        true => egui::Color32::from_rgb(0xd6, 0x46, 0x3a),
        false => egui::Color32::from_rgb(0xb0, 0x1e, 0x12),
    }
}

/// The text color of a [`micro`] header, a step quieter than the body text under it.
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

/// The ivory of a white key or a unison drawbar stop.
///
/// Both themes share it: a key is the same color in either theme, and the stops are the
/// instrument's own plastic, not part of the app's styling.
pub const STOP_WHITE: egui::Color32 = rgb(0xe4e1da);

/// The ebony of a black key or a mutation drawbar stop.
pub const STOP_BLACK: egui::Color32 = rgb(0x303036);

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

    /// Every choice, in the order the theme button cycles through them.
    pub(crate) const ALL: [ThemeChoice; 3] =
        [ThemeChoice::System, ThemeChoice::Light, ThemeChoice::Dark];

    pub(crate) fn next(self) -> ThemeChoice {
        match self {
            ThemeChoice::System => ThemeChoice::Light,
            ThemeChoice::Light => ThemeChoice::Dark,
            ThemeChoice::Dark => ThemeChoice::System,
        }
    }

    /// The choice's name in the Theme menu.
    pub(crate) fn name(self) -> &'static str {
        match self {
            ThemeChoice::System => "Auto",
            ThemeChoice::Light => "Light",
            ThemeChoice::Dark => "Dark",
        }
    }

    pub(crate) fn hint(self) -> &'static str {
        match self {
            ThemeChoice::System => "following the system — click for light",
            ThemeChoice::Light => "always light — click for dark",
            ThemeChoice::Dark => "always dark — click to follow the system again",
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
/// `size` is the box it takes. The dot keeps a pixel of margin on each side, so a row of
/// dots does not merge into a line.
///
/// ⚠️ Painted, not drawn as text. The bundled fonts have no glyph for `●`, and a missing
/// glyph renders as an empty box that looks like a checkbox.
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
    pub(crate) zoom: Zoom,
    /// What the display and the window allowed the zoom to be, as of this frame.
    pub(crate) room: Room,
    /// The MIDI controllers listened to, whichever tab is in front.
    pub(crate) midi: Midi,
    pub(crate) splash: crate::splash::Splash,
    /// The About box while it is showing. Not kept between sessions.
    pub(crate) about: Option<crate::about::About>,
    /// The report sheet while it is showing. Not kept between sessions.
    pub(crate) report: Option<crate::report::Report>,
    /// Where this build runs, and who draws the window's frame on this run.
    pub(crate) platform: Platform,
    pub(crate) chrome: crate::platform::Frame,
    #[cfg(target_os = "macos")]
    menubar: Option<crate::menubar::MenuBar>,
    /// The library open in this window. `None` where the system names no folder for
    /// the default one.
    pub(crate) store: Option<Store>,
    /// The libraries opened lately.
    #[cfg(not(target_arch = "wasm32"))]
    recent: crate::libraries::Recent,
    #[cfg(not(target_arch = "wasm32"))]
    picker: crate::libraries::Picker,
    /// The libraries opened lately, and the browser's answers about them.
    #[cfg(target_arch = "wasm32")]
    libraries: crate::libraries::Libraries,
    /// The library to open once the open one has no answer outstanding, and whether the
    /// user agreed to lose what the open one cannot keep.
    #[cfg(target_arch = "wasm32")]
    waiting: Option<(crate::store::Root, bool)>,
    /// eframe's store still holds a library kept the old way, to empty at the first
    /// frame that can write it.
    leaving: bool,
    /// The list's revision when the index was last written.
    synced: u64,
    /// When the index was last written, in egui time.
    synced_at: f64,
    /// Where the pointer was last seen while files from outside hovered over the window.
    /// The desktop's windowing may not say, and a drop then lands at the top level.
    #[cfg(not(target_arch = "wasm32"))]
    dragged_at: Option<egui::Pos2>,
}

impl DrawbarApp {
    /// The app over the library open last, or the default library.
    #[cfg(not(target_arch = "wasm32"))]
    pub fn new(cc: &eframe::CreationContext<'_>) -> DrawbarApp {
        let recent = cc
            .storage
            .map(crate::libraries::Recent::restore)
            .unwrap_or_default();
        let root = recent
            .last()
            .map(std::path::Path::to_path_buf)
            .or_else(crate::store::default_root);
        let store = root.map(|root| library(&cc.egui_ctx, root));
        DrawbarApp::with_library(cc, store)
    }

    /// The app over the browser's own library, or, where the browser opens folders, the
    /// library open last once the list of them has been read back.
    #[cfg(target_arch = "wasm32")]
    pub fn new(cc: &eframe::CreationContext<'_>) -> DrawbarApp {
        if crate::libraries::picking() != crate::libraries::Picking::On {
            let store = crate::store::default_root().map(|root| library(&cc.egui_ctx, root));
            return DrawbarApp::with_library(cc, store);
        }
        let app = DrawbarApp::with_library(cc, None);
        app.libraries.restore(&cc.egui_ctx);
        app
    }

    /// The app over `store`, or over no library at all.
    pub(crate) fn with_library(
        cc: &eframe::CreationContext<'_>,
        store: Option<Store>,
    ) -> DrawbarApp {
        // Without this every `Glyph` draws as egui's broken-image warning.
        egui_extras::install_image_loaders(&cc.egui_ctx);
        cc.egui_ctx.set_fonts(fonts());
        // Both themes are set up front, so a system switch between light and dark
        // mid-session uses this app's colors, not egui's defaults.
        cc.egui_ctx.set_visuals_of(egui::Theme::Dark, dark());
        cc.egui_ctx.set_visuals_of(egui::Theme::Light, light());
        // ⚠️ Both themes, not only the visible one: egui keeps a `Style` per theme, and
        // resolving a named text style that a theme lacks panics.
        cc.egui_ctx.all_styles_mut(metrics);
        let theme = cc
            .storage
            .and_then(|storage| storage.get_string(ThemeChoice::KEY))
            .map_or(ThemeChoice::default(), |text| ThemeChoice::read(&text));
        cc.egui_ctx.set_theme(theme.preference());
        let zoom = cc
            .storage
            .and_then(|storage| storage.get_string(Zoom::KEY))
            .map_or(Zoom::default(), |text| Zoom::read(&text));
        cc.egui_ctx.set_zoom_factor(zoom.factor());
        // egui's own keys step by a tenth and reset to 100%, off the steps drawbar offers.
        cc.egui_ctx
            .options_mut(|options| options.zoom_with_keyboard = false);
        let leaving = cc.storage.is_some_and(crate::store::left_behind);
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
            zoom,
            room: Room::of(&cc.egui_ctx),
            midi: Midi::default(),
            splash: crate::splash::Splash::new(&cc.egui_ctx),
            about: None,
            report: None,
            platform: Platform::current(),
            chrome: crate::platform::Frame::of(Platform::current()),
            #[cfg(target_os = "macos")]
            menubar: None,
            store,
            #[cfg(not(target_arch = "wasm32"))]
            recent: cc
                .storage
                .map(crate::libraries::Recent::restore)
                .unwrap_or_default(),
            #[cfg(not(target_arch = "wasm32"))]
            picker: crate::libraries::Picker::default(),
            #[cfg(target_arch = "wasm32")]
            libraries: crate::libraries::Libraries::default(),
            #[cfg(target_arch = "wasm32")]
            waiting: None,
            leaving,
            synced: 0,
            synced_at: 0.0,
            #[cfg(not(target_arch = "wasm32"))]
            dragged_at: None,
        };
        if let Some(storage) = cc.storage {
            app.shell.restore(storage);
            app.library.restore(storage);
            app.browser.folders.all_files =
                storage.get_string(crate::folders::ALL_FILES_KEY).as_deref() == Some("true");
            #[cfg(not(target_arch = "wasm32"))]
            app.midi.restore(storage, &cc.egui_ctx);
        }
        #[cfg(not(target_arch = "wasm32"))]
        app.opened();
        app.synced = app.workspace.revision();
        app
    }

    /// The app with the macOS menu bar installed and its window's title bar fitted to the
    /// top bar. Only the window's own app, never a test, may own the one menu bar the
    /// system has.
    #[cfg(target_os = "macos")]
    pub fn in_mac_window(mut self, cc: &eframe::CreationContext<'_>) -> DrawbarApp {
        match crate::menubar::MenuBar::install(&cc.egui_ctx) {
            Ok(bar) => self.menubar = Some(bar),
            Err(e) => {
                self.log.error(format!("the menu bar: {e}"));
                self.log
                    .trouble("drawbar could not build its menu bar; the menus are not there.");
            }
        }
        crate::platform::center_traffic_lights(cc);
        self
    }

    /// Run what was picked from the macOS menu bar, of what `admit` lets through, and bring
    /// its items up to date.
    #[cfg(target_os = "macos")]
    fn menu_bar_events(
        &mut self,
        ctx: &egui::Context,
        frame: &mut eframe::Frame,
        admit: fn(crate::menu::Command) -> bool,
        acts: &mut Vec<browser::Act>,
    ) {
        let Some(mut bar) = self.menubar.take() else {
            return;
        };
        let picked = bar.picked();
        // ⚠️ A pick from the menu bar is not an egui event, so nothing else asks for the
        // frame that shows what it did.
        if !picked.is_empty() {
            ctx.request_repaint();
        }
        for command in picked {
            if admit(command)
                && self
                    .offer_now(ctx, command)
                    .is_some_and(|offer| offer.enabled)
            {
                self.run(ctx, frame, command, acts);
            }
        }
        bar.refresh(|command| self.offer_now(ctx, command));
        self.menubar = Some(bar);
    }

    /// A screen too small for the shell: a browser tab of any size, or a window too small
    /// at this zoom.
    ///
    /// Only the notice draws, so no input reaches a shell with no room to lay out, and its
    /// state is untouched. The zoom alone still answers its keys and menu items.
    fn gated(&mut self, ctx: &egui::Context, frame: &mut eframe::Frame) {
        let zooms =
            |command: crate::menu::Command| matches!(command, crate::menu::Command::Zoom(_));
        let mut acts = Vec::new();
        #[cfg(target_os = "macos")]
        self.menu_bar_events(ctx, frame, zooms, &mut acts);
        self.shortcuts(ctx, frame, zooms, &mut acts);
        let smaller = self.zoom.after(crate::zoom::Step::Out, self.room).ok();
        if let Some(zoom) = crate::shell::too_small_notice(ctx, smaller) {
            self.pick_zoom(ctx, frame, zoom);
        }
    }

    /// Copy anything dropped on the window into the library, in the folder it was dropped
    /// on, or hand it to the New dialog while one is open and it is a WAV. A library that
    /// cannot take a copy holds it in memory.
    ///
    /// The native backend fills `path` and the web backend fills `bytes`, which in the
    /// browser only a drop the page did not catch carries.
    fn take_dropped_files(&mut self, ctx: &egui::Context) -> Vec<browser::Act> {
        #[cfg(not(target_arch = "wasm32"))]
        let at = self.dropped_at(ctx);
        let dropped = ctx.input(|i| i.raw.dropped_files.clone());
        let drafting = self.workspace.draft_mut().is_some();
        let takes = !drafting && self.store.as_ref().is_some_and(Store::takes_files);
        let mut joining = Vec::new();
        let mut imports = Vec::new();
        for file in dropped {
            let name = match (file.name.is_empty(), &file.path) {
                (false, _) => file.name.clone(),
                (true, Some(path)) => path
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_else(|| path.display().to_string()),
                (true, None) => "dropped".to_string(),
            };
            #[cfg(not(target_arch = "wasm32"))]
            if let (true, Some(path)) = (takes, &file.path) {
                imports.push(browser::Act::Take {
                    from: path.clone(),
                    dir: self.browser.landing_dir(at),
                    name,
                });
                continue;
            }
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
                false => imports.push(browser::Act::Import { name, bytes }),
            }
        }
        if let Some(draft) = self.workspace.draft_mut() {
            draft.add(joining);
        }
        #[cfg(target_arch = "wasm32")]
        {
            for (from, at) in crate::dropped::take() {
                imports.extend(self.take_in(from, self.browser.landing_dir(Some(at)), takes));
            }
            crate::dropped::catch(ctx, takes);
        }
        imports
    }

    /// Where files dropped this frame landed: the pointer as last seen while they hovered,
    /// where the windowing reported it moving then.
    #[cfg(not(target_arch = "wasm32"))]
    fn dropped_at(&mut self, ctx: &egui::Context) -> Option<egui::Pos2> {
        let (hovering, dropped, moved) = ctx.input(|i| {
            let moved = i.raw.events.iter().rev().find_map(|event| match event {
                egui::Event::PointerMoved(at) => Some(*at),
                _ => None,
            });
            let hovering = !i.raw.hovered_files.is_empty();
            (hovering, !i.raw.dropped_files.is_empty(), moved)
        });
        if hovering {
            self.dragged_at = moved.or(self.dragged_at);
        }
        let at = self.dragged_at.filter(|_| dropped);
        if !hovering {
            self.dragged_at = None;
        }
        at
    }

    /// The files File ▸ Open… picked, copied into the top level of the library, or held
    /// in memory by a library that cannot take a copy.
    fn take_picked(&mut self) -> Vec<browser::Act> {
        let takes = self.store.as_ref().is_some_and(Store::takes_files);
        let picked = self.workspace.take_picked();
        picked
            .into_iter()
            .filter_map(|from| self.take_in(from, crate::store::LibPath::root(), takes))
            .collect()
    }

    /// Copy `from` into the folder `dir`, where the library `takes` a copy, and otherwise
    /// read it into memory.
    fn take_in(
        &mut self,
        from: crate::store::Outside,
        dir: crate::store::LibPath,
        takes: bool,
    ) -> Option<browser::Act> {
        let name = crate::store::outside_name(&from);
        if takes {
            return Some(browser::Act::Take { from, dir, name });
        }
        self.workspace.read_outside(name, from);
        None
    }

    /// Hold a send to the instrument until the library's files have been checked for
    /// changes made outside this app. The store lets it go once they have.
    fn hold_sends(&mut self, acts: Vec<browser::Act>) -> Vec<browser::Act> {
        let sending = acts.iter().any(|act| matches!(act, browser::Act::SendAll));
        let Some(store) = self.store.as_mut().filter(|_| sending) else {
            return acts;
        };
        if !store.hold_send() {
            return acts;
        }
        acts.into_iter()
            .filter(|act| !matches!(act, browser::Act::SendAll))
            .collect()
    }

    /// What changed in the running version. The web build shows the change list in the
    /// app, where it can fetch the notes; the native build opens the release page.
    #[cfg(target_arch = "wasm32")]
    pub(crate) fn whats_new(&mut self, ctx: &egui::Context) {
        self.splash.open_news(ctx);
    }

    #[cfg(not(target_arch = "wasm32"))]
    pub(crate) fn whats_new(&mut self, ctx: &egui::Context) {
        let page = crate::about::release_page(crate::sheet::VERSION);
        ctx.open_url(egui::OpenUrl::new_tab(page));
    }

    /// Bring the library level with the list from the frame that changed it; an idle
    /// egui window may not repaint. Files are written at once, and the working copies
    /// and the index at most every few seconds, because dragging changes the list every
    /// frame.
    fn keep_up(&mut self, ctx: &egui::Context) {
        /// The least time between writes of the index, in seconds.
        const EVERY: f64 = 2.0;

        let Some(store) = &mut self.store else {
            return;
        };
        if self.workspace.revision() == self.synced && !self.browser.folders.changed() {
            return;
        }
        let now = ctx.input(|i| i.time);
        let pass = match now - self.synced_at >= EVERY {
            true => Pass::Full,
            false => Pass::Files,
        };
        let synced = store.sync(&mut self.workspace, &mut self.browser, &self.queue, pass);
        if synced && pass == Pass::Full {
            self.synced = self.workspace.revision();
            self.synced_at = now;
            return;
        }
        // Nothing else may request a frame, and the index is still to be written.
        ctx.request_repaint_after(std::time::Duration::from_millis(500));
    }
}

/// The store over the library at `root`, named for the browser unless it is the default
/// library.
fn library(ctx: &egui::Context, root: crate::store::Root) -> Store {
    #[cfg(not(target_arch = "wasm32"))]
    let name = crate::libraries::name(&root, crate::store::default_root().as_deref());
    #[cfg(target_arch = "wasm32")]
    let name = root.name();
    let cache = crate::store::Cache::open(ctx, &root);
    let store = Store::start(crate::store::Backend::start(ctx, root)).remembering(cache);
    #[cfg(not(target_arch = "wasm32"))]
    let store = store.shelving(crate::device::shelf());
    match name {
        Some(name) => store.named(name),
        None => store,
    }
}

impl DrawbarApp {
    /// Run the acts that pick or open a library, and pass the rest on.
    fn switch_libraries(
        &mut self,
        ctx: &egui::Context,
        acts: Vec<browser::Act>,
    ) -> Vec<browser::Act> {
        let mut rest = Vec::new();
        for act in acts {
            match act {
                #[cfg(not(target_arch = "wasm32"))]
                browser::Act::PickLibrary => self.picker.pick(ctx),
                #[cfg(not(target_arch = "wasm32"))]
                browser::Act::OpenLibrary(root) => self.open_library(ctx, root, false),
                #[cfg(not(target_arch = "wasm32"))]
                browser::Act::OpenLibraryDiscarding(root) => self.open_library(ctx, root, true),
                #[cfg(target_arch = "wasm32")]
                browser::Act::PickLibrary => self.libraries.pick(ctx),
                #[cfg(target_arch = "wasm32")]
                browser::Act::OpenLibrary(root) => self.libraries.allow(ctx, root),
                #[cfg(target_arch = "wasm32")]
                browser::Act::OpenLibraryDiscarding(root) => self.waiting = Some((root, true)),
                act => rest.push(act),
            }
        }
        rest
    }

    /// Run what the user chose to do with the slots' former occupants interrupted writes
    /// left, and pass the rest on.
    fn rescue(&mut self, ctx: &egui::Context, acts: Vec<browser::Act>) -> Vec<browser::Act> {
        let mut rest = Vec::new();
        for act in acts {
            let browser::Act::Rescue(rescue, what) = act else {
                rest.push(act);
                continue;
            };
            let Some(store) = self.store.as_mut() else {
                continue;
            };
            let shown = store.rescue(
                rescue,
                what,
                &mut self.workspace,
                &mut self.browser,
                &mut self.log,
            );
            if let Some(url) = shown {
                ctx.open_url(egui::OpenUrl::new_tab(url));
            }
        }
        rest
    }

    /// Open the library at `root` in place of the one open now.
    ///
    /// The open library is written first, its unsaved edits as working copies in its own
    /// `.drawbar/`, where they come back when it is opened again. Its assets then leave
    /// the window; the views of slots stay. Where the open library cannot keep something
    /// unsaved, it asks first unless `discard` says the user already agreed to lose it.
    ///
    /// ⚠️ On the desktop it waits for the library's saves in flight to land, as quitting
    /// does. The browser cannot wait, so there it is called once [`Store::settled`].
    pub(crate) fn open_library(
        &mut self,
        ctx: &egui::Context,
        root: crate::store::Root,
        discard: bool,
    ) {
        if self
            .store
            .as_ref()
            .is_some_and(|store| *store.root() == root)
        {
            return;
        }
        #[cfg(not(target_arch = "wasm32"))]
        if !root.is_dir() {
            self.log.trouble(format!(
                "{} is not a folder drawbar can open as the library.",
                root.display()
            ));
            return;
        }
        let unkept = self
            .store
            .as_ref()
            .map(|store| store.unkept(&self.workspace))
            .unwrap_or_default();
        if !discard && !unkept.is_empty() {
            self.browser.ask_leave(&unkept, root);
            return;
        }
        if let Some(store) = self.store.take() {
            store.hand_over(
                &mut self.workspace,
                &mut self.browser,
                &self.queue,
                &mut self.log,
            );
        }
        let gone = self.workspace.close_library();
        let unqueued = gone.iter().filter(|id| self.queue.holds(**id)).count();
        for id in gone {
            self.queue.forget(id);
            self.document.forget(id);
        }
        if unqueued > 0 {
            self.log.say(format!(
                "{unqueued} waiting to be sent left the queue with the library they are in."
            ));
        }
        for tag in self.browser.tags.all() {
            self.shell.filter.forget_tag(tag.id);
        }
        self.browser.leave_library();
        self.tabs.prune(&self.workspace);
        self.store = Some(library(ctx, root));
        self.opened();
        self.synced = self.workspace.revision();
    }

    /// Put the library just opened first among the recent ones.
    #[cfg(not(target_arch = "wasm32"))]
    fn opened(&mut self) {
        let default = crate::store::default_root();
        let open = self.store.as_ref().map(|store| store.root().to_path_buf());
        if let Some(open) = &open {
            self.recent.opened(open);
        }
        self.browser.folders.libraries = self.recent.offered(default.as_deref(), open.as_deref());
    }

    /// Put the library just opened first among the recent ones.
    #[cfg(target_arch = "wasm32")]
    fn opened(&mut self) {
        let open = self.store.as_ref().map(|store| store.root().clone());
        if let Some(notice) = open.as_ref().and_then(|open| self.libraries.opened(open)) {
            self.log.say(notice);
        }
        self.browser.folders.libraries = self.libraries.offered(open.as_ref());
    }

    /// Fold in what the browser answered about libraries, and open the one waiting once
    /// the open library has no answer outstanding.
    #[cfg(target_arch = "wasm32")]
    fn follow_libraries(&mut self, ctx: &egui::Context) {
        use crate::libraries::Heard;
        use crate::store::Root;

        while let Some(heard) = self.libraries.heard() {
            match heard {
                Heard::Restored { recent, permitted } => {
                    self.libraries.restored(recent);
                    let last = self.libraries.last().cloned();
                    let (root, reconnect) = match last {
                        Some(last @ Root::Picked(_)) if !permitted => (Root::Private, Some(last)),
                        last => (last.unwrap_or(Root::Private), None),
                    };
                    self.store = Some(library(ctx, root));
                    self.opened();
                    if let Some(root) = reconnect {
                        let name = root.name().unwrap_or_default();
                        self.log.say(format!(
                            "{name} opens again once you let this browser into it: choose \
                             File ▸ Reconnect {name}. Until then the library is {}.",
                            crate::libraries::THIS_COMPUTER
                        ));
                        self.browser.folders.reconnect = Some(crate::folders::Library {
                            root,
                            name,
                            open: false,
                        });
                    }
                }
                Heard::Open(root) => self.waiting = Some((root, false)),
                Heard::Trouble(why) => self.log.trouble(why),
            }
        }
        if self.waiting.is_none() {
            return;
        }
        let settled = match &mut self.store {
            Some(store) => store.settled(&mut self.workspace, &mut self.browser, &self.queue),
            None => true,
        };
        if let Some((root, discard)) = self.waiting.take_if(|_| settled) {
            self.open_library(ctx, root, discard);
        }
    }
}

impl eframe::App for DrawbarApp {
    /// How long a change may sit unwritten.
    fn auto_save_interval(&self) -> std::time::Duration {
        std::time::Duration::from_secs(5)
    }

    /// eframe calls this on a timer and at exit, so edits are kept without an explicit
    /// save.
    fn save(&mut self, storage: &mut dyn eframe::Storage) {
        if let Some(store) = &mut self.store {
            if store.sync(
                &mut self.workspace,
                &mut self.browser,
                &self.queue,
                Pass::Full,
            ) {
                self.synced = self.workspace.revision();
            }
        }
        storage.set_string(ThemeChoice::KEY, self.theme.stored().to_string());
        storage.set_string(Zoom::KEY, self.zoom.percent().to_string());
        #[cfg(not(target_arch = "wasm32"))]
        self.recent.keep(storage);
        storage.set_string(
            crate::folders::ALL_FILES_KEY,
            self.browser.folders.all_files.to_string(),
        );
        // Unlike the theme, these are not written from the frame that changed them: a
        // divider moves on every frame of a drag, and each write rewrites the whole
        // store.
        self.shell.keep(storage);
        self.library.keep(storage);
        #[cfg(not(target_arch = "wasm32"))]
        self.midi.keep(storage);
    }

    /// eframe calls this once at exit, after `save`.
    #[cfg(not(target_arch = "wasm32"))]
    fn on_exit(&mut self, _gl: Option<&eframe::glow::Context>) {
        self.device.release();
        if let Some(store) = &mut self.store {
            store.close(
                &mut self.workspace,
                &mut self.browser,
                &self.queue,
                &mut self.log,
            );
        }
    }

    fn update(&mut self, ctx: &egui::Context, frame: &mut eframe::Frame) {
        self.room = Room::of(ctx);
        #[cfg(target_arch = "wasm32")]
        crate::telemetry::visit(
            !crate::shell::too_small(ctx.screen_rect().size()),
            self.device.usb(),
        );
        if crate::shell::too_small(ctx.screen_rect().size()) {
            self.gated(ctx, frame);
            return;
        }
        self.log.tick(ctx);
        let mut arrived: Vec<browser::Act> = self
            .workspace
            .poll(&mut self.log)
            .into_iter()
            .map(|(name, bytes)| browser::Act::Import { name, bytes })
            .collect();
        // What is open or picked is needed whatever is drawn, so the library keeps it.
        let needed = self.tabs.documents().chain(self.browser.picked().locals());
        for id in needed {
            self.workspace.hurry(id);
        }
        let released = match &mut self.store {
            Some(store) => {
                store.focus(ctx.input(|input| input.focused));
                store.poll(
                    &mut self.workspace,
                    &mut self.browser,
                    &self.queue,
                    &mut self.log,
                )
            }
            None => false,
        };
        if self.leaving {
            if let Some(storage) = frame.storage_mut() {
                crate::store::leave_behind(storage);
                self.leaving = false;
            }
        }
        self.device.poll(
            &mut self.log,
            &mut self.workspace,
            &mut self.tabs,
            &mut self.queue,
        );
        // An edit to an asset with a waiting entry changes what that write would do; the
        // queue diffs it again against the occupant it already read.
        crate::queue::follow(
            &self.workspace,
            &mut self.device,
            &mut self.queue,
            &mut self.log,
        );
        self.tabs.prune(&self.workspace);
        // Unedited views have no owner once their tab closes. An edited view is the only
        // copy of that edit and must survive.
        self.workspace
            .close_views(|id| self.tabs.holds(id), &self.queue, &mut self.log);
        self.midi.report(&mut self.log);
        // ⚠️ Drained every frame, whatever is in front, so keys played over the library
        // or the keyboard are dropped instead of sounding when a document comes forward.
        let played = self.midi.played(ctx.input(|input| input.time));
        arrived.extend(self.take_dropped_files(ctx));
        arrived.extend(self.take_picked());
        self.browser.forget_targets();
        #[cfg(not(target_arch = "wasm32"))]
        arrived.extend(self.picker.picked().map(browser::Act::OpenLibrary));
        #[cfg(target_arch = "wasm32")]
        self.follow_libraries(ctx);
        if released {
            arrived.push(browser::Act::SendAll);
        }
        drop_hint(ctx);
        // Opened by choosing WAVs under New. It is drawn before anything else this frame
        // because it is a modal over the whole window.
        if let Some(made) = crate::newproject::dialog(ctx, &mut self.workspace, &mut self.log) {
            self.tabs.open(made);
        }
        let asked = self.splash.show(ctx, self.device.usb());
        crate::about::dialog(ctx, &mut self.about, &self.log);
        crate::report::dialog(ctx, &mut self.report, &self.log);

        // Before the panels, so an editor open this frame still has focus when Escape is
        // handled. An overlay takes Escape for itself: the activity log, the zoom popover,
        // or any modal up last frame, which an Escape this frame has already closed.
        if !crate::menu::covered(ctx) && !self.shell.log_open && !self.shell.zoom_open {
            self.browser.let_go(ctx);
        }

        // Outside in: each panel claims its space from what the earlier ones left.
        let mut acts = self
            .document
            .released(ctx, &mut self.workspace, &mut self.log);
        acts.extend(arrived);
        acts.extend(asked);
        // A library still being listed may hold a demo it has not listed yet.
        if !self.store.as_ref().is_some_and(Store::listing) {
            acts.extend(self.workspace.take_demos().map(browser::Act::Demos));
        }
        #[cfg(target_os = "macos")]
        self.menu_bar_events(ctx, frame, |_| true, &mut acts);
        self.shortcuts(ctx, frame, |_| true, &mut acts);
        self.browser.dialog(ctx, &mut acts);
        self.backdrop(ctx);
        self.top_bar(ctx, frame, &mut acts);
        self.status_line(ctx, &mut acts);
        self.browser_card(ctx, &mut acts);
        self.inspector_card(ctx, &mut acts);
        self.center(ctx, &played, &mut acts);
        // An instrument that goes away takes its queue's review with it.
        self.shell.review_open &= self.attached();
        if self.shell.review_open {
            self.shell.review_open = crate::queue::review(
                ctx,
                &mut self.queue,
                &self.workspace,
                &self.device.state,
                &mut acts,
            );
        }
        if self.shell.log_open {
            self.shell.log_open =
                self.log
                    .popover(ctx, &mut self.shell.log_problems, self.shell.status_rect);
        }
        if self.shell.zoom_open {
            self.shell.zoom_open = self.zoom_popover(ctx, frame);
        }

        // ⚠️ Between the panels and the acts they requested: a piano library's plan is
        // not in its bytes yet, and any act that would carry those bytes waits here until
        // it is.
        let acts = self
            .document
            .settle(ctx, acts, &mut self.workspace, &self.queue, &mut self.log);
        let acts = match released {
            true => acts,
            false => self.hold_sends(acts),
        };
        let acts = self.switch_libraries(ctx, acts);
        let acts = self.rescue(ctx, acts);
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
        #[cfg(not(target_arch = "wasm32"))]
        self.device
            .keep_occupants_in(self.store.as_ref().and_then(Store::tmp));
        // Last, so a command the user just requested takes the protocol's single slot
        // ahead of the background tree read.
        self.device.pump();

        self.keep_up(ctx);
    }
}

impl DrawbarApp {
    /// The row of tabs between the two panel toggles, and the front tab's view in the
    /// document card under it.
    ///
    /// Keys `played` on a MIDI controller reach only the key map of the front document.
    fn center(&mut self, ctx: &egui::Context, played: &Played, acts: &mut Vec<browser::Act>) {
        let gutter = crate::panel::GUTTER as i8;
        let margin = egui::Margin {
            left: gutter,
            right: gutter,
            top: 0,
            bottom: gutter,
        };
        egui::CentralPanel::default()
            .frame(egui::Frame::NONE.inner_margin(margin))
            .show(ctx, |ui| {
                self.tab_row(ui, acts);
                let rect = ui.available_rect_before_wrap();
                let mut inside = crate::shell::card(ui, rect);
                let ui_ = &mut inside;
                match self.tabs.showing() {
                    Spot::Library => {
                        self.document.leave();
                        acts.extend(self.library.ui(
                            ui_,
                            &mut self.browser,
                            &self.workspace,
                            &self.device,
                            &self.queue,
                            &self.shell,
                        ));
                    }
                    Spot::Keyboard => {
                        self.document.leave();
                        acts.extend(self.keyboard.ui(
                            ui_,
                            &mut self.browser,
                            &self.workspace,
                            &self.device,
                            &self.queue,
                            &self.tabs,
                        ));
                    }
                    Spot::Document(id) => self.open_document(ui_, id, played, acts),
                }
                crate::shell::round_off(ui, rect);
            });
    }

    /// The browser's toggle, the tabs, and the inspector's toggle, across the top of the
    /// center.
    fn tab_row(&mut self, ui: &mut egui::Ui, acts: &mut Vec<browser::Act>) {
        let row = egui::Rect::from_min_size(
            ui.cursor().min,
            egui::vec2(ui.available_width(), crate::tabs::HEIGHT),
        );
        let along = |ui: &mut egui::Ui, layout| {
            ui.new_child(egui::UiBuilder::new().max_rect(row).layout(layout))
        };
        let mut start = along(ui, egui::Layout::left_to_right(egui::Align::Center));
        self.panel_toggle(&mut start, Dock::Browser, acts);
        let mut end = along(ui, egui::Layout::right_to_left(egui::Align::Center));
        self.panel_toggle(&mut end, Dock::Inspector, acts);
        let strip = egui::Rect::from_min_max(
            egui::pos2(start.min_rect().right() + 4.0, row.top()),
            egui::pos2(end.min_rect().left() - 4.0, row.bottom()),
        );
        let mut tabs = ui.new_child(
            egui::UiBuilder::new()
                .max_rect(strip)
                .layout(egui::Layout::left_to_right(egui::Align::Center)),
        );
        self.tabs.ui(&mut tabs, &self.workspace, acts);
        ui.advance_cursor_after_rect(row);
    }

    /// A document owns its own room: the header is full bleed and the body inside it
    /// keeps the margin.
    fn open_document(
        &mut self,
        ui: &mut egui::Ui,
        id: u64,
        played: &Played,
        acts: &mut Vec<browser::Act>,
    ) {
        let around = crate::document::Around {
            queue: &self.queue,
            tags: self.browser.tags(),
            played,
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
        if let Some((class, at)) = wants.load {
            acts.push(browser::Act::LoadOnInstrument { class, at });
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

/// The dark theme, like the instrument panel, with accent colors reserved for status.
fn dark() -> egui::Visuals {
    let mut visuals = egui::Visuals::dark();
    visuals.panel_fill = rgb(0x18191c);
    visuals.window_fill = rgb(0x1f2024);
    visuals.faint_bg_color = rgb(0x26272b);
    visuals.extreme_bg_color = rgb(0x121316);
    let widgets = &mut visuals.widgets;
    widgets.noninteractive.bg_fill = rgb(0x1d1e21);
    widgets.noninteractive.bg_stroke.color = rgb(0x2c2d31);
    // ⚠️ Both slots use the body text color: `Visuals::text_color` returns
    // `noninteractive`, so a painted row and a button would otherwise differ. The quieter
    // caption color is `caption`.
    widgets.noninteractive.fg_stroke.color = egui::Color32::from_gray(0xc8);
    widgets.inactive.fg_stroke.color = egui::Color32::from_gray(0xc8);
    fill(&mut widgets.inactive, rgb(0x2b2c30));
    fill(&mut widgets.hovered, rgb(0x35363b));
    widgets.hovered.bg_stroke.color = rgb(0x4d4e54);
    widgets.hovered.fg_stroke.color = rgb(0xf2f2f4);
    fill(&mut widgets.active, rgb(0x3d3e44));
    widgets.active.bg_stroke.color = rgb(0x6a6b72);
    widgets.active.fg_stroke.color = rgb(0xf6f6f7);
    fill(&mut widgets.open, rgb(0x2a2b2f));
    widgets.open.bg_stroke.color = rgb(0x36373c);
    widgets.open.fg_stroke.color = rgb(0xd6d7db);
    visuals.selection.bg_fill = rgb(0x4b2420);
    // ⚠️ This also colors drop targets and focused knobs; inheriting egui's blue would
    // introduce a second accent.
    visuals.selection.stroke.color = rgb(0xffd9d1);
    // Slot numbers and knob captions use the weak text color.
    visuals.weak_text_alpha = 0.85;
    visuals.hyperlink_color = bad(&visuals);
    softened(visuals, egui::Color32::from_black_alpha(107))
}

/// The light theme, with stronger text and marks than egui's defaults.
fn light() -> egui::Visuals {
    let mut visuals = egui::Visuals::light();
    visuals.panel_fill = rgb(0xf4f3f0);
    visuals.window_fill = rgb(0xfbfaf8);
    visuals.faint_bg_color = rgb(0xe9e6df);
    visuals.selection.bg_fill = rgb(0xf3d4cd);
    visuals.selection.stroke.color = rgb(0x3a1410);
    let widgets = &mut visuals.widgets;
    widgets.noninteractive.fg_stroke.color = egui::Color32::from_gray(0x1c);
    widgets.inactive.fg_stroke.color = egui::Color32::from_gray(0x1c);
    widgets.noninteractive.bg_stroke.color = rgb(0xdedad2);
    fill(&mut widgets.inactive, rgb(0xeae8e3));
    fill(&mut widgets.hovered, rgb(0xe0ddd6));
    widgets.hovered.bg_stroke.color = rgb(0xb9b5ac);
    fill(&mut widgets.active, rgb(0xd4d0c7));
    widgets.active.bg_stroke.color = rgb(0x8d8980);
    fill(&mut widgets.open, rgb(0xe6e3dd));
    widgets.open.bg_stroke.color = rgb(0xd2cec6);
    widgets.open.fg_stroke.color = rgb(0x4a4a4c);
    visuals.weak_text_alpha = 0.9;
    visuals.hyperlink_color = bad(&visuals);
    softened(
        visuals,
        egui::Color32::from_rgba_unmultiplied(40, 30, 20, 41),
    )
}

/// An opaque color written as `0xrrggbb`.
const fn rgb(hex: u32) -> egui::Color32 {
    egui::Color32::from_rgb((hex >> 16) as u8, (hex >> 8) as u8, hex as u8)
}

/// A widget state's resting fill, for both the filled and the frameless widgets.
fn fill(state: &mut egui::style::WidgetVisuals, color: egui::Color32) {
    state.bg_fill = color;
    state.weak_bg_fill = color;
}

/// The shape both themes share: rounded controls, and menus and sheets that float on a
/// soft shadow of `shade`.
fn softened(mut visuals: egui::Visuals, shade: egui::Color32) -> egui::Visuals {
    let widgets = &mut visuals.widgets;
    for state in [
        &mut widgets.noninteractive,
        &mut widgets.inactive,
        &mut widgets.hovered,
        &mut widgets.active,
        &mut widgets.open,
    ] {
        state.corner_radius = egui::CornerRadius::same(CONTROL_RADIUS);
    }
    visuals.window_corner_radius = egui::CornerRadius::same(POPUP_RADIUS);
    visuals.menu_corner_radius = egui::CornerRadius::same(POPUP_RADIUS);
    let shadow = egui::Shadow {
        offset: [0, 14],
        blur: 36,
        spread: 0,
        color: shade,
    };
    visuals.popup_shadow = shadow;
    visuals.window_shadow = shadow;
    visuals
}

/// The rounding of a button, a field, or any other control.
pub const CONTROL_RADIUS: u8 = 5;

/// The rounding of a menu, a popover, or a sheet.
pub const POPUP_RADIUS: u8 = 10;

/// The window behind the cards: the gaps between the panels, and the bars at the top and
/// bottom, which sit on it with no fill of their own.
pub fn canvas(visuals: &egui::Visuals) -> egui::Color32 {
    match visuals.dark_mode {
        true => rgb(0x0f1013),
        false => rgb(0xe7e4dd),
    }
}

/// `color` thinned to `alpha` over whatever is under it: the fill of a chip, a pill, or a
/// row that carries a signal. Text on it keeps the full-strength color.
pub fn tint(color: egui::Color32, alpha: f32) -> egui::Color32 {
    color.gamma_multiply(alpha)
}

/// The bold family, used only for the word mark.
pub fn bold() -> egui::FontFamily {
    egui::FontFamily::Name("bold".into())
}

/// Ubuntu Regular for the body and Ubuntu Bold beside it, Hack for the monospace runs,
/// and drawbar's own glyphs under every family.
///
/// The Ubuntu files in `assets/fonts` are the Ubuntu font family 0.83 under the Ubuntu
/// Font Licence 1.0 beside them. Hack comes from egui's `epaint_default_fonts`.
/// `drawbar-glyphs.ttf` draws the characters drawbar's text uses that no other face has;
/// `scripts/glyphs.py` generates it.
///
/// ⚠️ These four faces are all the app ships. egui's default set would add Ubuntu Light
/// and two emoji faces, a megabyte of glyphs, so a character none of these covers draws
/// as Hack's `◻`, emoji included.
pub(crate) fn fonts() -> egui::FontDefinitions {
    const UBUNTU: &str = "Ubuntu";
    const UBUNTU_BOLD: &str = "Ubuntu-Bold";
    const HACK: &str = "Hack";
    const GLYPHS: &str = "drawbar-glyphs";
    let mut fonts = egui::FontDefinitions::empty();
    for (face, ttf) in [
        (
            UBUNTU,
            include_bytes!("../assets/fonts/Ubuntu-R.ttf").as_slice(),
        ),
        (
            UBUNTU_BOLD,
            include_bytes!("../assets/fonts/Ubuntu-B.ttf").as_slice(),
        ),
        (HACK, epaint_default_fonts::HACK_REGULAR),
        (
            GLYPHS,
            include_bytes!("../assets/fonts/drawbar-glyphs.ttf").as_slice(),
        ),
    ] {
        fonts.font_data.insert(
            face.to_owned(),
            std::sync::Arc::new(egui::FontData::from_static(ttf)),
        );
    }
    // drawbar's glyphs come before Hack in the body families, so its arrows and key
    // symbols keep Ubuntu's size and weight. Hack covers what other text brings, like `─`.
    for (family, faces) in [
        (egui::FontFamily::Proportional, [UBUNTU, GLYPHS, HACK]),
        (bold(), [UBUNTU_BOLD, GLYPHS, HACK]),
        (egui::FontFamily::Monospace, [HACK, UBUNTU, GLYPHS]),
    ] {
        fonts
            .families
            .insert(family, faces.map(str::to_owned).to_vec());
    }
    fonts
}

/// The text of the shell itself: menus, tabs, tree rows and cells.
///
/// A function rather than a const because [`egui::TextStyle::Name`] holds an `Arc<str>`.
pub fn ui() -> egui::TextStyle {
    egui::TextStyle::Name("ui".into())
}

/// The smallest text: the document editors' captions, which are also uppercased.
pub fn micro() -> egui::TextStyle {
    egui::TextStyle::Name("micro".into())
}

/// The text of a section label or a column head in the shell: bold, in sentence case.
pub fn section() -> egui::TextStyle {
    egui::TextStyle::Name("section".into())
}

/// The spacing both themes share: the size of a control and the space around it.
///
/// It is independent of the theme, so switching between light and dark moves nothing.
pub(crate) fn metrics(style: &mut egui::Style) {
    let spacing = &mut style.spacing;
    spacing.item_spacing = egui::vec2(8.0, 4.0);
    spacing.button_padding = egui::vec2(8.0, 2.0);
    // Panels own their inner padding, so the shared margin claims none of it.
    spacing.window_margin = egui::Margin::same(0);
    spacing.menu_margin = egui::Margin::same(5);
    spacing.indent = 18.0;
    spacing.interact_size.y = 22.0;
    spacing.slider_rail_height = 4.0;
    // ⚠️ Floating only so the track can be transparent: the bar claims the width a solid
    // one would, and content laid out beside it never runs under the thumb.
    let solid = egui::style::ScrollStyle {
        bar_width: 8.0,
        ..egui::style::ScrollStyle::solid()
    };
    spacing.scroll = egui::style::ScrollStyle {
        floating: true,
        floating_width: solid.bar_width,
        floating_allocated_width: solid.allocated_width(),
        dormant_background_opacity: 0.0,
        active_background_opacity: 0.0,
        interact_background_opacity: 0.0,
        dormant_handle_opacity: 1.0,
        active_handle_opacity: 1.0,
        interact_handle_opacity: 1.0,
        ..solid
    };
    style.animation_time = 0.14;
    for (style_, font) in [
        (egui::TextStyle::Body, egui::FontId::proportional(13.0)),
        (egui::TextStyle::Button, egui::FontId::proportional(13.0)),
        (egui::TextStyle::Small, egui::FontId::proportional(10.0)),
        (ui(), egui::FontId::proportional(12.5)),
        (micro(), egui::FontId::proportional(9.5)),
        (section(), egui::FontId::new(11.5, bold())),
    ] {
        style.text_styles.insert(style_, font);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::workspace::Origin;

    /// A library kept the old way is not read. Its keys are emptied at the first frame,
    /// and preferences beside them stay.
    #[test]
    fn a_library_left_behind_is_emptied_quietly() {
        let mut storage = crate::testing::Fake::default();
        eframe::Storage::set_string(
            &mut storage,
            "drawbar.this_computer",
            "drawbar 2\n9\n".into(),
        );
        eframe::Storage::set_string(&mut storage, "drawbar.tags", "drawbar tags 1\n".into());
        eframe::Storage::set_string(&mut storage, ThemeChoice::KEY, "dark".into());

        let first = open(&storage);
        assert!(first.leaving);
        crate::store::leave_behind(&mut storage);
        for key in ["drawbar.this_computer", "drawbar.tags"] {
            let held = eframe::Storage::get_string(&storage, key);
            assert_eq!(held.as_deref(), Some(""), "{key}");
        }
        assert_eq!(
            eframe::Storage::get_string(&storage, ThemeChoice::KEY).as_deref(),
            Some("dark"),
            "a preference stays"
        );

        let second = open(&storage);
        assert!(!second.leaving, "emptied keys are not left behind again");
    }

    /// The app over a library in `root`, once it has opened.
    #[cfg(not(target_arch = "wasm32"))]
    fn opened_over(root: &crate::testing::Temp) -> (egui::Context, DrawbarApp) {
        let ctx = egui::Context::default();
        let cc = eframe::CreationContext::_new_kittest(ctx.clone());
        let backend = crate::store::Backend::start(&ctx, root.0.clone());
        let mut app = DrawbarApp::with_library(&cc, Some(Store::start(backend)));
        for _ in 0..500 {
            frame(&ctx, &mut app);
            if app
                .browser
                .folders
                .place
                .as_ref()
                .is_some_and(|at| !at.opening)
            {
                return (ctx, app);
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        panic!("the library did not open");
    }

    #[cfg(not(target_arch = "wasm32"))]
    fn frame(ctx: &egui::Context, app: &mut DrawbarApp) {
        use eframe::App as _;

        let mut frame = eframe::Frame::_new_kittest();
        crate::testing::run(
            ctx,
            crate::testing::screen(egui::vec2(1280.0, 720.0), Vec::new()),
            |ctx| app.update(ctx, &mut frame),
        );
    }

    /// End to end through the app's own frames: New makes a file, quitting writes the
    /// index, and the next run finds the asset where it was left.
    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    fn what_is_made_in_one_run_is_a_file_the_next_run_opens() {
        use eframe::App as _;

        let root = crate::testing::Temp::new();
        let (ctx, mut app) = opened_over(&root);
        app.browser.folders.take_ops();
        let id = app
            .workspace
            .create(crate::workspace::Fresh::Program, &mut app.log)
            .unwrap();
        let bytes = app.workspace.get(id).unwrap().bytes.to_vec();
        frame(&ctx, &mut app);
        app.save(&mut crate::testing::Fake::default());
        app.on_exit(None);
        assert_eq!(root.read("untitled.ne5p"), bytes);
        assert!(root.at(".drawbar/library.ron").is_file());

        let (_, again) = opened_over(&root);
        let names: Vec<&str> = again
            .workspace
            .listed()
            .map(|entity| entity.name.as_str())
            .collect();
        assert_eq!(names, ["untitled.ne5p"]);
    }

    /// Run frames until the library just asked for has opened.
    #[cfg(not(target_arch = "wasm32"))]
    fn until_open(ctx: &egui::Context, app: &mut DrawbarApp) {
        for _ in 0..500 {
            frame(ctx, app);
            let place = app.browser.folders.place.as_ref();
            if place.is_some_and(|at| !at.opening) {
                return;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        panic!("the library did not open");
    }

    /// Switching writes the library open until then, its unsaved edit as a working copy
    /// in its own sidecar, and opening it again brings the edit back. A view of a slot
    /// stays in the window and leaves nothing in the library it was open over.
    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    fn switching_libraries_keeps_each_ones_unsaved_edits_in_its_own_folder() {
        let (first, second) = (crate::testing::Temp::new(), crate::testing::Temp::new());
        let (ctx, mut app) = opened_over(&first);
        let id = app
            .workspace
            .create(crate::workspace::Fresh::Program, &mut app.log)
            .unwrap();
        frame(&ctx, &mut app);
        let saved = app.workspace.get(id).unwrap().bytes.to_vec();
        let edit = |bytes: &[u8]| {
            crate::fields::apply(bytes, &[("center_panel.gain".into(), "96".into())])
                .unwrap()
                .1
        };
        let edited = edit(&saved);
        app.workspace
            .replace_bytes(id, edited.clone(), &mut app.log);
        let at = nord_usb::Location { bank: 6, slot: 3 };
        let view = app.workspace.view(
            "Africa Split.ne5p".into(),
            Origin::Device {
                class: nord_usb::ObjectClass::Program,
                at,
            },
            saved.clone(),
            &mut app.log,
        );
        app.workspace
            .replace_bytes(view, edited.clone(), &mut app.log);
        app.tabs.open(view);

        app.open_library(&ctx, second.0.clone(), false);
        until_open(&ctx, &mut app);
        assert_eq!(
            first.read("untitled.ne5p"),
            saved,
            "the file is as last saved"
        );
        assert_eq!(
            first.names(".drawbar/working").len(),
            1,
            "the asset's edit, not the view's"
        );
        assert!(app.workspace.get(id).is_none(), "it left with its library");
        assert!(app.workspace.get(view).is_some(), "the view stays");
        assert_eq!(app.workspace.listed().count(), 0);
        let libraries = &app.browser.folders.libraries;
        assert_eq!(libraries[0].root, second.0, "most recent first");
        assert!(libraries[0].open);

        app.open_library(&ctx, first.0.clone(), false);
        until_open(&ctx, &mut app);
        let back = app.workspace.listed().next().expect("the asset is back");
        assert_eq!(back.bytes, edited, "with its edit");
        assert!(back.is_unsaved());
        assert!(back.id > view, "under an id this session has not given out");
    }

    /// A library nothing may be written to cannot keep what is unsaved in it, so leaving
    /// it asks first, and only a yes lets the edit go.
    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    fn leaving_a_read_only_library_asks_before_its_unsaved_edits_are_lost() {
        let (first, second) = (crate::testing::Temp::new(), crate::testing::Temp::new());
        std::fs::create_dir(first.at(".drawbar")).unwrap();
        std::fs::write(first.at(".drawbar/library.ron"), "(version: 99)").unwrap();
        let (ctx, mut app) = opened_over(&first);
        let id = app
            .workspace
            .create(crate::workspace::Fresh::Program, &mut app.log)
            .unwrap();

        app.open_library(&ctx, second.0.clone(), false);
        let (title, answers) = app.browser.asking().expect("a question");
        assert_eq!(title, "Discard what this library cannot keep?");
        assert_eq!(answers, ["Cancel", "Discard"]);
        assert!(app.workspace.get(id).is_some(), "nothing left yet");
        let open = app.store.as_ref().map(|store| store.root().to_path_buf());
        assert_eq!(open, Some(first.0.clone()), "still the library open");

        let acts = app.browser.answer("Discard");
        assert!(app.switch_libraries(&ctx, acts).is_empty());
        until_open(&ctx, &mut app);
        let open = app.store.as_ref().map(|store| store.root().to_path_buf());
        assert_eq!(open, Some(second.0.clone()));
        assert!(app.workspace.get(id).is_none(), "discarded, as agreed");
    }

    fn open(storage: &dyn eframe::Storage) -> DrawbarApp {
        let mut cc = eframe::CreationContext::_new_kittest(egui::Context::default());
        cc.storage = Some(storage);
        DrawbarApp::with_library(&cc, None)
    }

    #[test]
    fn a_linked_documents_load_on_instrument_is_the_trees_act() {
        let ctx = egui::Context::default();
        let cc = eframe::CreationContext::_new_kittest(ctx.clone());
        let mut app = DrawbarApp::with_library(&cc, None);
        let at = nord_usb::Location { bank: 6, slot: 3 };
        let fresh = app
            .workspace
            .create(crate::workspace::Fresh::Program, &mut app.log)
            .expect("a fresh default");
        let bytes = app.workspace.get(fresh).expect("just made").bytes.to_vec();
        let id = app.workspace.ingest(
            "Africa Split.ne5p".into(),
            Origin::Device {
                class: nord_usb::ObjectClass::Program,
                at,
            },
            bytes,
            &mut app.log,
        );
        app.device.pretend_scanned(
            nord_usb::ObjectClass::Program,
            7,
            &["", "", "", "Africa Split"],
        );
        app.device.relink(&mut app.workspace);

        let frame = |app: &mut DrawbarApp, events: Vec<egui::Event>| {
            let mut acts = Vec::new();
            let input = egui::RawInput {
                events,
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(1280.0, 720.0),
                )),
                ..Default::default()
            };
            let output = ctx.run(input, |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| {
                    app.open_document(ui, id, &Played::default(), &mut acts)
                });
            });
            (output, acts)
        };
        let (output, _) = frame(&mut app, Vec::new());
        let button = output
            .shapes
            .iter()
            .find_map(|clipped| match &clipped.shape {
                egui::Shape::Text(text) if text.galley.text() == browser::LOAD_ON_INSTRUMENT => {
                    Some(text.pos + text.galley.size() / 2.0)
                }
                _ => None,
            })
            .expect("the header offers Load on instrument");

        let press = |pressed| egui::Event::PointerButton {
            pos: button,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: egui::Modifiers::NONE,
        };
        frame(&mut app, vec![egui::Event::PointerMoved(button)]);
        frame(&mut app, vec![press(true)]);
        let (_, acts) = frame(&mut app, vec![press(false)]);
        assert!(
            matches!(
                acts.as_slice(),
                [browser::Act::LoadOnInstrument {
                    class: nord_usb::ObjectClass::Program,
                    at: asked,
                }] if *asked == at
            ),
            "one act, the tree's, for Programs 7:4; got {} acts",
            acts.len()
        );
    }

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
        // An unrecognized stored value reads as the unset choice.
        assert_eq!(ThemeChoice::read("moonlight"), ThemeChoice::System);
        assert_eq!(ThemeChoice::read(""), ThemeChoice::System);
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
            // The untraveled track is quieter than a mark but must remain visible.
            let track = contrast(unlit(&visuals), panel);
            assert!(track >= 2.4, "{where_} track: {track:.2}:1");
            // Selection needs legible text and a fill distinct from the panel.
            let selected = visuals.selection.bg_fill;
            let ink = contrast(visuals.selection.stroke.color, selected);
            assert!(ink >= 4.5, "{where_} selected text: {ink:.2}:1");
            // The fill is quiet on purpose: a selected row's ink and weight carry it too.
            let fill = contrast(selected, panel);
            assert!(fill >= 1.25, "{where_} selected fill: {fill:.2}:1");
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

    /// A mid gray is too faint on the light panel.
    #[test]
    fn the_light_theme_uses_near_black_text() {
        let light = light();
        let panel = light.panel_fill;
        let body = contrast(light.text_color(), panel);
        assert!(body >= 12.0, "light body: {body:.2}:1");
        let heading = contrast(caption(&light), panel);
        assert!(heading >= 12.0, "light caption: {heading:.2}:1");
        // Weak text carries whole sentences in the inspector, so it needs body-text
        // contrast, not the 3.0 a large mark could get by with.
        let weak = contrast(light.weak_text_color(), panel);
        assert!(weak >= 4.5, "light weak: {weak:.2}:1");
    }

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
    fn every_character_in_drawbar_text_has_a_glyph_in_every_family() {
        let faces = egui::epaint::text::Fonts::new(1.0, 2048, Default::default(), fonts());
        let written = written();
        assert!(written.contains_key(&'→'), "{written:?}");
        for (char, at) in written {
            for font in [
                egui::FontId::proportional(12.0),
                egui::FontId::new(12.0, bold()),
                egui::FontId::monospace(12.0),
            ] {
                assert!(
                    faces.has_glyph(&font, char),
                    "{char:?} (U+{:04X}), written at {at}, draws as an empty box in {:?}",
                    u32::from(char),
                    font.family
                );
            }
        }
    }

    /// Every non-ASCII character of a string or character literal drawbar compiles outside
    /// its tests, with the first `file:line` that writes it. Attributes, doc comments
    /// among them, are not drawn, so they are left out.
    fn written() -> std::collections::BTreeMap<char, String> {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
        let mut found = std::collections::BTreeMap::new();
        let mut files = vec![root.join("src/lib.rs"), root.join("src/main.rs")];
        while let Some(path) = files.pop() {
            let source = std::fs::read_to_string(&path)
                .unwrap_or_else(|error| panic!("{}: {error}", path.display()));
            let tokens = source
                .parse()
                .unwrap_or_else(|error| panic!("{}: {error}", path.display()));
            let file = path
                .strip_prefix(root)
                .unwrap_or(&path)
                .display()
                .to_string();
            let mut modules = Vec::new();
            literals(tokens, &file, &mut found, &mut modules);
            let owner = match path.file_name().and_then(|name| name.to_str()) {
                Some("lib.rs" | "main.rs" | "mod.rs") => path.with_file_name(""),
                _ => path.with_extension(""),
            };
            files.extend(modules.into_iter().map(|name| {
                let flat = owner.join(format!("{name}.rs"));
                match flat.exists() {
                    true => flat,
                    false => owner.join(name).join("mod.rs"),
                }
            }));
        }
        found
    }

    /// Record the literals in `tokens` into `found`, and the names of the modules they
    /// declare in their own files into `modules`, skipping items built only for tests.
    fn literals(
        tokens: proc_macro2::TokenStream,
        file: &str,
        found: &mut std::collections::BTreeMap<char, String>,
        modules: &mut Vec<String>,
    ) {
        use proc_macro2::{Delimiter, TokenTree};
        let ends_item = |token: &TokenTree| match token {
            TokenTree::Group(group) => group.delimiter() == Delimiter::Brace,
            token => is_punct(Some(token), ';'),
        };
        let mut tokens = tokens.into_iter().peekable();
        while let Some(token) = tokens.next() {
            match token {
                TokenTree::Punct(hash) if hash.as_char() == '#' => {
                    let attribute = tokens.by_ref().find_map(|token| match token {
                        TokenTree::Group(group) => Some(group.stream().to_string()),
                        _ => None,
                    });
                    // ⚠️ The gated item runs to its first `;` or `{}` block, as every item
                    // drawbar builds only for tests does.
                    if attribute.as_deref().is_some_and(tests_only) {
                        tokens.by_ref().find(ends_item);
                    }
                }
                TokenTree::Ident(word) if word == "mod" => {
                    if let Some(TokenTree::Ident(name)) = tokens.next() {
                        if is_punct(tokens.peek(), ';') {
                            modules.push(name.to_string());
                        }
                    }
                }
                TokenTree::Group(group) => literals(group.stream(), file, found, modules),
                TokenTree::Literal(literal) => {
                    let line = literal.span().start().line;
                    let text = match syn::Lit::new(literal) {
                        syn::Lit::Str(text) => text.value(),
                        syn::Lit::Char(char) => char.value().to_string(),
                        _ => continue,
                    };
                    for char in text.chars().filter(|char| !char.is_ascii()) {
                        found
                            .entry(char)
                            .or_insert_with(|| format!("{file}:{line}"));
                    }
                }
                _ => {}
            }
        }
    }

    fn is_punct(token: Option<&proc_macro2::TokenTree>, char: char) -> bool {
        matches!(token, Some(proc_macro2::TokenTree::Punct(punct)) if punct.as_char() == char)
    }

    /// Whether an attribute builds its item only for tests.
    fn tests_only(attribute: &str) -> bool {
        let attribute = attribute.replace(' ', "");
        attribute == "cfg(test)" || attribute.starts_with("cfg(all(test,")
    }
}
