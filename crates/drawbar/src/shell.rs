//! The dock shell: the regions the window is cut into, what is collapsed, and the
//! menus, toolbar and status bar that sit around the centre.
//!
//! Panels claim space in the order [`crate::app::DrawbarApp::update`] adds them. The
//! three open docks resize by dragging the edge they show the centre, between the least
//! each is worth opening to and whatever leaves the centre [`CENTRE_WIDE`] by
//! [`CENTRE_TALL`]; a collapsed rail is [`SHUT`] and does not. Each contents pads
//! itself, so a panel header can bleed to both edges.

use eframe::egui;
use nord_usb::ObjectClass;

use crate::app::{accent, bold, ui as ui_text, DrawbarApp, ThemeChoice};
use crate::browser::{new_menu, Act};
use crate::device::occupancy;
use crate::filter::Filter;
use crate::icon::{icon, sized, Glyph};
use crate::log::Level;
use crate::panel::{caps, chevron, dock_header, flat, strip, DOCK};
use crate::strings::folder;
use crate::tabs::Spot;

/// The window's own bar: the mark, the menus, the instrument, the theme.
pub const TITLEBAR: f32 = 30.0;

/// The bar of actions under it.
pub const TOOLBAR: f32 = 32.0;

/// One line of plain words at the foot of the window.
pub const STATUS: f32 = 22.0;

/// The bottom dock's body, under its header, and the least it is worth opening to.
pub const DOCK_BODY: f32 = 184.0;
const BODY_LEAST: f32 = 120.0;

/// The side docks open, shut, and the least either is worth opening to.
pub const BROWSER: f32 = 232.0;
pub const INSPECTOR: f32 = 244.0;
pub const SHUT: f32 = 30.0;
const SIDE_LEAST: f32 = 180.0;

/// What the centre keeps however far a dock is dragged. The most a dock may be dragged
/// to is whatever leaves this, so it is read off the room left rather than stored.
const CENTRE_WIDE: f32 = 300.0;
const CENTRE_TALL: f32 = 200.0;

/// The room the bars keep at each end, and the gap between their parts.
const PAD: f32 = 8.0;
const GAP: f32 = 6.0;

/// A glyph in a bar, the check beside a menu item, and the height of a control.
const GLYPH: f32 = 13.0;
const CHECK: f32 = 12.0;
const BUTTON: f32 = 22.0;

/// The omnibox's least width. Below this it is a box nobody can read a name in.
const OMNIBOX: f32 = 300.0;

/// The omnibox's own widget id.
///
/// ⚠️ The controls before it come and go with the instrument. An id counted off its
/// neighbours would change under it as one attaches, and the box would lose the focus
/// and the cursor mid-word.
const SEARCH: &str = "omnibox";

/// The least width a drop-down takes, whatever is in it.
///
/// ⚠️ A menu sizes itself to its widest item, so without this each one is as wide as
/// whatever happens to be enabled — and the longest item, "Inspector panel" with ⌥⌘I,
/// leaves its key text against its label. This is that item with a gap between the two.
const MENU: f32 = 240.0;

/// Which dock a toggle is about.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Dock {
    Browser,
    Inspector,
    Bottom,
}

/// Which of the bottom dock's two pages its body shows.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum Page {
    #[default]
    Queue,
    Log,
}

impl Page {
    fn title(self) -> &'static str {
        match self {
            Page::Queue => "send queue",
            Page::Log => "activity log",
        }
    }

    fn stored(self) -> &'static str {
        match self {
            Page::Queue => "queue",
            Page::Log => "log",
        }
    }
}

/// What is open, and what the omnibox holds.
pub struct Shell {
    pub browser_open: bool,
    pub inspector_open: bool,
    pub dock_open: bool,
    /// The two panels under the right dock's INSTRUMENT header, each collapsed on its
    /// own. SELECTION is flat and has none.
    pub room_open: bool,
    pub info_open: bool,
    /// How far each dock was last dragged. A side dock's is its width, the bottom
    /// dock's is its body under [`DOCK`].
    pub browser_width: f32,
    pub inspector_width: f32,
    pub dock_body: f32,
    pub page: Page,
    /// What has been typed into the omnibox: the name the library's table is narrowed
    /// by.
    pub omnibox: String,
    /// What else the library is narrowed to, as the tree's kind, tag and place rows ask
    /// for it.
    pub filter: Filter,
}

impl Default for Shell {
    fn default() -> Shell {
        Shell {
            browser_open: true,
            inspector_open: true,
            dock_open: false,
            room_open: true,
            info_open: false,
            browser_width: BROWSER,
            inspector_width: INSPECTOR,
            dock_body: DOCK_BODY,
            page: Page::default(),
            omnibox: String::new(),
            filter: Filter::default(),
        }
    }
}

impl Shell {
    /// Where the layout is kept between sessions, beside the browser's own keys.
    pub const KEY: &'static str = "drawbar.docks";

    const VERSION: &'static str = "drawbar docks 5";

    pub fn open(&self, dock: Dock) -> bool {
        match dock {
            Dock::Browser => self.browser_open,
            Dock::Inspector => self.inspector_open,
            Dock::Bottom => self.dock_open,
        }
    }

    pub fn toggle(&mut self, dock: Dock) {
        let held = !self.open(dock);
        match dock {
            Dock::Browser => self.browser_open = held,
            Dock::Inspector => self.inspector_open = held,
            Dock::Bottom => self.dock_open = held,
        }
    }

    /// Open the bottom dock on a page, which is what asking for either page means.
    pub fn show_page(&mut self, page: Page) {
        self.dock_open = true;
        self.page = page;
    }

    /// Put the docks back where they were left.
    ///
    /// ⚠️ A version this build does not know is refused rather than guessed at: half a
    /// layout is a window nobody arranged.
    pub fn restore(&mut self, storage: &dyn eframe::Storage) {
        let Some(text) = storage.get_string(Shell::KEY) else {
            return;
        };
        let mut lines = text.lines();
        if lines.next() != Some(Shell::VERSION) {
            return;
        }
        let mut held = Shell::default();
        for line in lines {
            let mut parts = line.split('\t');
            match (parts.next(), parts.next()) {
                (Some("browser"), Some(open)) => held.browser_open = open == "1",
                (Some("inspector"), Some(open)) => held.inspector_open = open == "1",
                (Some("dock"), Some(open)) => held.dock_open = open == "1",
                (Some("room"), Some(open)) => held.room_open = open == "1",
                (Some("info"), Some(open)) => held.info_open = open == "1",
                (Some("browser_width"), Some(text)) => {
                    held.browser_width = size(text, SIDE_LEAST, BROWSER)
                }
                (Some("inspector_width"), Some(text)) => {
                    held.inspector_width = size(text, SIDE_LEAST, INSPECTOR)
                }
                (Some("dock_body"), Some(text)) => {
                    held.dock_body = size(text, BODY_LEAST, DOCK_BODY)
                }
                (Some("page"), Some(page)) => {
                    held.page = match page == Page::Log.stored() {
                        true => Page::Log,
                        false => Page::Queue,
                    }
                }
                _ => {}
            }
        }
        self.browser_open = held.browser_open;
        self.inspector_open = held.inspector_open;
        self.dock_open = held.dock_open;
        self.room_open = held.room_open;
        self.info_open = held.info_open;
        self.browser_width = held.browser_width;
        self.inspector_width = held.inspector_width;
        self.dock_body = held.dock_body;
        self.page = held.page;
    }

    pub fn keep(&self, storage: &mut dyn eframe::Storage) {
        let bit = |open: bool| match open {
            true => "1",
            false => "0",
        };
        storage.set_string(
            Shell::KEY,
            format!(
                "{}\nbrowser\t{}\ninspector\t{}\ndock\t{}\nroom\t{}\ninfo\t{}\n\
                 browser_width\t{}\ninspector_width\t{}\ndock_body\t{}\npage\t{}\n",
                Shell::VERSION,
                bit(self.browser_open),
                bit(self.inspector_open),
                bit(self.dock_open),
                bit(self.room_open),
                bit(self.info_open),
                self.browser_width,
                self.inspector_width,
                self.dock_body,
                self.page.stored(),
            ),
        );
    }
}

/// A size the last session left. One this build would not have laid out — under the
/// least the dock opens to, or not a number at all — is not a size, so the default
/// stands. The most is the screen's, and egui clamps to it every frame.
fn size(text: &str, least: f32, default: f32) -> f32 {
    match text.parse::<f32>() {
        Ok(size) if size.is_finite() && size >= least => size,
        _ => default,
    }
}

/// Which of a region's edges faces the centre.
enum Side {
    Top,
    Bottom,
    Left,
    Right,
}

/// The 1 px border a region shows the centre, drawn just inside its own rect.
fn edge(ui: &egui::Ui, side: Side) {
    let rect = ui.max_rect();
    let stroke = egui::Stroke::new(1.0_f32, ui.visuals().widgets.noninteractive.bg_stroke.color);
    let painter = ui.painter();
    match side {
        Side::Top => painter.hline(rect.x_range(), rect.top() + 0.5, stroke),
        Side::Bottom => painter.hline(rect.x_range(), rect.bottom() - 0.5, stroke),
        Side::Left => painter.vline(rect.left() + 0.5, rect.y_range(), stroke),
        Side::Right => painter.vline(rect.right() - 0.5, rect.y_range(), stroke),
    };
}

/// A panel frame that paints its fill and claims no margin of its own.
fn bare(fill: egui::Color32) -> egui::Frame {
    egui::Frame::new()
        .fill(fill)
        .inner_margin(egui::Margin::ZERO)
}

/// A 1 px vertical rule, for the gaps between groups of controls.
fn rule(ui: &mut egui::Ui, height: f32) {
    let (rect, _) = ui.allocate_exact_size(egui::vec2(1.0, height), egui::Sense::hover());
    let stroke = egui::Stroke::new(1.0_f32, ui.visuals().widgets.noninteractive.bg_stroke.color);
    ui.painter().vline(rect.center().x, rect.y_range(), stroke);
}

/// A bar's contents, laid out left to right inside its padding.
fn along<R>(ui: &mut egui::Ui, contents: impl FnOnce(&mut egui::Ui) -> R) -> R {
    let mut inner = ui.new_child(
        egui::UiBuilder::new()
            .max_rect(ui.max_rect().shrink2(egui::vec2(PAD, 0.0)))
            .layout(egui::Layout::left_to_right(egui::Align::Center)),
    );
    inner.spacing_mut().item_spacing.x = GAP;
    contents(&mut inner)
}

/// The keys the menus bind.
mod key {
    use eframe::egui::{Key, KeyboardShortcut as Shortcut, Modifiers as With};

    pub const OPEN: Shortcut = Shortcut::new(With::COMMAND, Key::O);
    pub const SAVE: Shortcut = Shortcut::new(With::COMMAND, Key::S);
    pub const EXPORT: Shortcut = Shortcut::new(With::COMMAND.plus(With::SHIFT), Key::E);
    pub const CLOSE: Shortcut = Shortcut::new(With::COMMAND, Key::W);
    pub const QUIT: Shortcut = Shortcut::new(With::COMMAND, Key::Q);
    pub const KEYBOARD: Shortcut = Shortcut::new(With::COMMAND, Key::Num2);
    pub const DOCUMENT: Shortcut = Shortcut::new(With::COMMAND, Key::Num3);
    pub const BROWSER: Shortcut = Shortcut::new(With::COMMAND.plus(With::ALT), Key::B);
    pub const INSPECTOR: Shortcut = Shortcut::new(With::COMMAND.plus(With::ALT), Key::I);
    pub const DOCK: Shortcut = Shortcut::new(With::COMMAND.plus(With::ALT), Key::L);
    pub const RESYNC: Shortcut = Shortcut::new(With::COMMAND, Key::R);
    pub const QUEUE: Shortcut = Shortcut::new(With::COMMAND.plus(With::SHIFT), Key::S);
}

/// Whether this build may bind a key a browser tab keeps for itself.
///
/// ⚠️ ⌘W, ⌘Q and ⌘2–⌘3 reach the tab, not the page. On the web those items work by
/// click alone.
const WINDOWED: bool = !cfg!(target_arch = "wasm32");

/// The user guide, published beside the browser build.
///
/// Relative in a tab, so the guide is the one on whichever host is serving the app; a
/// window has no page to be relative to and reaches for the published one.
#[cfg(target_arch = "wasm32")]
pub(crate) const GUIDE: &str = "docs/";
#[cfg(not(target_arch = "wasm32"))]
pub(crate) const GUIDE: &str = "https://jmoo.github.io/drawbar/docs/";

/// The key text beside a menu label — a window's, never a tab's.
fn keyed(ctx: &egui::Context, shortcut: egui::KeyboardShortcut) -> String {
    match WINDOWED {
        true => ctx.format_shortcut(&shortcut),
        false => String::new(),
    }
}

/// Whether a frame of the omnibox has to bring the library forward.
///
/// Any change to what is typed does. The box narrows the library's table and nothing
/// else, so a search run behind a document tab is a search nobody can see — including
/// the first keystroke into an empty box.
fn searched(before: &str, after: &str) -> bool {
    before != after
}

/// One of the title bar's drop-downs, no narrower than [`MENU`] however little is in it.
fn drop_down(ui: &mut egui::Ui, title: &str, contents: impl FnOnce(&mut egui::Ui)) {
    ui.menu_button(title, |ui| {
        ui.set_min_width(MENU);
        contents(ui);
    });
}

/// A menu item, closing the menu when it is picked.
fn item(ui: &mut egui::Ui, label: &str, shortcut: Option<egui::KeyboardShortcut>) -> bool {
    let mut button = egui::Button::new(label);
    if let Some(shortcut) = shortcut {
        button = button.shortcut_text(keyed(ui.ctx(), shortcut));
    }
    let clicked = ui.add(button).clicked();
    if clicked {
        ui.close();
    }
    clicked
}

/// What a click on one of the bottom dock's page titles asks for.
///
/// ⚠️ The title of the page already showing shuts the dock. It is the only way out that
/// is on the dock itself, and a title that answered a click by doing nothing reads as
/// broken.
fn page_click(page: Page, showing: bool) -> Act {
    match showing {
        true => Act::ToggleDock(Dock::Bottom),
        false => Act::ShowPage(page),
    }
}

/// A menu item that also says whether what it names is on.
///
/// ⚠️ A check at the left rather than a selected button or a `selectable_label`: both
/// fill the row with `selection.bg_fill`, which is the instrument's red and reads as a
/// warning across a menu of ordinary items. **Every** checkable item in the app wears
/// this, wherever its menu is drawn.
pub fn marked(
    ui: &mut egui::Ui,
    label: &str,
    on: bool,
    shortcut: Option<egui::KeyboardShortcut>,
) -> bool {
    let tint = match on {
        true => accent(ui.visuals()),
        false => egui::Color32::TRANSPARENT,
    };
    let mut button = egui::Button::image_and_text(sized(Glyph::Check, CHECK, tint), label)
        .image_tint_follows_text_color(false);
    if let Some(shortcut) = shortcut {
        button = button.shortcut_text(keyed(ui.ctx(), shortcut));
    }
    let clicked = ui.add(button).clicked();
    if clicked {
        ui.close();
    }
    clicked
}

/// One of the toolbar's labelled actions.
///
/// ⚠️ An accented action carries the accent in its glyph alone: accent on panel measures
/// 4.1:1, which fails as 11 px text.
fn action(ui: &mut egui::Ui, glyph: Glyph, label: &str, accented: bool) -> egui::Response {
    ui.scope(|ui| {
        flat(ui);
        let visuals = ui.visuals();
        let quiet = visuals.widgets.inactive.fg_stroke.color;
        let (mark, ink) = match accented {
            true => (accent(visuals), visuals.widgets.active.fg_stroke.color),
            false => (quiet, quiet),
        };
        ui.add(
            egui::Button::image_and_text(
                sized(glyph, GLYPH, mark),
                egui::RichText::new(label).text_style(ui_text()).color(ink),
            )
            .image_tint_follows_text_color(false)
            .corner_radius(2.0)
            .min_size(egui::vec2(0.0, BUTTON)),
        )
    })
    .inner
}

/// A 24 × 22 button carrying one glyph. `on` is a toggle whose dock is open, which is
/// the one state that fills without the pointer on it.
fn glyph_button(ui: &mut egui::Ui, glyph: Glyph, on: bool, hint: &str) -> egui::Response {
    ui.scope(|ui| {
        flat(ui);
        let widgets = &mut ui.visuals_mut().widgets;
        if on {
            widgets.inactive.weak_bg_fill = widgets.active.weak_bg_fill;
        }
        let visuals = ui.visuals();
        let ink = match on {
            true => visuals.widgets.active.fg_stroke.color,
            false => visuals.widgets.inactive.fg_stroke.color,
        };
        ui.add(
            egui::Button::image(sized(glyph, GLYPH, ink))
                .image_tint_follows_text_color(false)
                .corner_radius(2.0)
                .min_size(egui::vec2(24.0, BUTTON)),
        )
    })
    .inner
    .on_hover_text(hint)
}

impl DrawbarApp {
    /// Whether an instrument is answering.
    ///
    /// ⚠️ The single gate on everything that only means something with one attached: the
    /// Read and Send actions, the send queue, the inspector's INSTRUMENT group, and the
    /// menu items naming any of them. A control for an instrument that is not there is a
    /// control that can only disappoint.
    pub(crate) fn attached(&self) -> bool {
        self.device.state.connected()
    }

    /// 30 px: the mark, the menu bar, the instrument, the theme.
    pub(crate) fn titlebar(
        &mut self,
        ctx: &egui::Context,
        frame: &mut eframe::Frame,
        acts: &mut Vec<Act>,
    ) {
        let fill = ctx.style().visuals.window_fill;
        egui::TopBottomPanel::top("titlebar")
            .resizable(false)
            .exact_height(TITLEBAR)
            .frame(bare(fill))
            .show(ctx, |ui| {
                edge(ui, Side::Bottom);
                along(ui, |ui| {
                    icon(ui, Glyph::SlidersVertical, 14.0, accent(ui.visuals()));
                    ui.label(egui::RichText::new("drawbar").font(egui::FontId::new(12.0, bold())));
                    self.shortcuts(ui, acts);
                    ui.scope(|ui| {
                        flat(ui);
                        self.menus(ui, frame, acts);
                    });
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        flat(ui);
                        self.theme_chip(ui, frame);
                        self.instrument_chip(ui);
                    });
                });
            });
    }

    /// The instrument, and that it is answering.
    fn instrument_chip(&self, ui: &mut egui::Ui) {
        let Some(product) = self.device.state.product() else {
            return;
        };
        let visuals = ui.visuals();
        let ink = visuals.widgets.inactive.fg_stroke.color;
        let lit = crate::app::good(visuals);
        egui::Frame::new()
            .inner_margin(egui::Margin::symmetric(6, 2))
            .show(ui, |ui| {
                ui.spacing_mut().item_spacing.x = GAP;
                icon(ui, Glyph::Keyboard, GLYPH, ink);
                ui.label(
                    egui::RichText::new(product)
                        .text_style(ui_text())
                        .color(ink),
                );
                crate::app::dot(ui, lit).on_hover_text("attached");
            });
    }

    fn theme_chip(&mut self, ui: &mut egui::Ui, frame: &mut eframe::Frame) {
        let glyph = match ui.visuals().dark_mode {
            true => Glyph::Moon,
            false => Glyph::Sun,
        };
        let ink = ui.visuals().widgets.inactive.fg_stroke.color;
        let picked = ui
            .add(
                egui::Button::image_and_text(
                    sized(glyph, GLYPH, ink),
                    egui::RichText::new(self.theme.label())
                        .text_style(ui_text())
                        .color(ink),
                )
                .image_tint_follows_text_color(false)
                .corner_radius(2.0)
                .min_size(egui::vec2(0.0, BUTTON)),
            )
            .on_hover_text(self.theme.hint())
            .clicked();
        if picked {
            self.pick_theme(ui.ctx(), frame, self.theme.next());
        }
    }

    /// Hold the theme, and write it where the next session reads it.
    fn pick_theme(&mut self, ctx: &egui::Context, frame: &mut eframe::Frame, theme: ThemeChoice) {
        self.theme = theme;
        ctx.set_theme(theme.preference());
        // Written now; eframe's own persistence otherwise waits for another frame.
        if let Some(storage) = frame.storage_mut() {
            storage.set_string(ThemeChoice::KEY, theme.stored().to_string());
        }
    }

    /// Every key a menu item binds, answered whether or not a menu is open.
    fn shortcuts(&mut self, ui: &mut egui::Ui, acts: &mut Vec<Act>) {
        let hit = |shortcut: &egui::KeyboardShortcut| {
            ui.input_mut(|input| input.consume_shortcut(shortcut))
        };
        if hit(&key::OPEN) {
            acts.push(Act::OpenFiles);
        }
        if hit(&key::EXPORT) {
            if let Some(id) = self.tabs.active() {
                acts.push(Act::Export(id));
            }
        }
        if hit(&key::BROWSER) {
            acts.push(Act::ToggleDock(Dock::Browser));
        }
        if hit(&key::INSPECTOR) {
            acts.push(Act::ToggleDock(Dock::Inspector));
        }
        if hit(&key::DOCK) {
            acts.push(Act::ToggleDock(Dock::Bottom));
        }
        if self.attached() && hit(&key::QUEUE) {
            acts.push(Act::ShowPage(Page::Queue));
        }
        if self.attached() && hit(&key::RESYNC) {
            acts.push(Act::Resync);
        }
        if WINDOWED && hit(&key::CLOSE) {
            acts.push(Act::CloseTab);
        }
        if WINDOWED && hit(&key::QUIT) {
            acts.push(Act::Quit);
        }
        if WINDOWED && self.attached() && hit(&key::KEYBOARD) {
            acts.push(Act::ShowTab(Spot::Keyboard));
        }
        if WINDOWED && hit(&key::DOCUMENT) {
            if let Some(id) = self.tabs.last_document() {
                acts.push(Act::ShowTab(Spot::Document(id)));
            }
        }
        // ⚠️ Never a file export. ⌘S means "save what I did", which for a view of a slot
        // is the write back to that slot. The library and the keyboard are views of
        // what is already there, so neither has anything to save.
        if hit(&key::SAVE) {
            if let Some(id) = self.tabs.active() {
                acts.push(Act::SaveDoc(id));
            }
        }
    }

    fn menus(&mut self, ui: &mut egui::Ui, frame: &mut eframe::Frame, acts: &mut Vec<Act>) {
        drop_down(ui, "File", |ui| self.file_menu(ui, acts));
        drop_down(ui, "View", |ui| self.view_menu(ui, frame, acts));
        drop_down(ui, "Instrument", |ui| self.instrument_menu(ui, acts));
        drop_down(ui, "Help", |ui| {
            if item(ui, "User guide", None) {
                ui.ctx().open_url(egui::OpenUrl::new_tab(GUIDE));
            }
            if item(ui, "What's new", None) {
                self.whats_new(ui.ctx());
            }
            ui.separator();
            if item(ui, "Copy activity log", None) {
                acts.push(Act::CopyLog);
            }
            if item(ui, "About drawbar", None) {
                self.about_open = true;
            }
        });
    }

    fn file_menu(&mut self, ui: &mut egui::Ui, acts: &mut Vec<Act>) {
        if item(ui, "Open…", Some(key::OPEN)) {
            acts.push(Act::OpenFiles);
        }
        ui.menu_button("New", |ui| new_menu(ui, acts));
        ui.separator();
        if let Some(id) = self.tabs.active() {
            if item(ui, "Save", Some(key::SAVE)) {
                acts.push(Act::SaveDoc(id));
            }
            let unsaved = self
                .workspace
                .get(id)
                .is_some_and(crate::workspace::LocalEntity::is_unsaved);
            if unsaved && item(ui, "Revert to saved", None) {
                acts.push(Act::Revert(id));
            }
            if item(ui, "Export…", Some(key::EXPORT)) {
                acts.push(Act::Export(id));
            }
            ui.separator();
        }
        let closable = self
            .tabs
            .showing()
            .is_some_and(|spot| spot != Spot::Library);
        if closable && item(ui, "Close tab", Some(key::CLOSE)) {
            acts.push(Act::CloseTab);
        }
        if WINDOWED && item(ui, "Quit", Some(key::QUIT)) {
            acts.push(Act::Quit);
        }
    }

    /// ⚠️ No Library item. The library is always open and always the first tab, so the
    /// menu would offer a view that is one click away and can never be missing.
    fn view_menu(&mut self, ui: &mut egui::Ui, frame: &mut eframe::Frame, acts: &mut Vec<Act>) {
        let showing = self.tabs.showing();
        if self.attached()
            && marked(
                ui,
                "Keyboard",
                showing == Some(Spot::Keyboard),
                Some(key::KEYBOARD),
            )
        {
            acts.push(Act::ShowTab(Spot::Keyboard));
        }
        if let Some(id) = self.tabs.last_document() {
            if marked(
                ui,
                "Document",
                showing == Some(Spot::Document(id)),
                Some(key::DOCUMENT),
            ) {
                acts.push(Act::ShowTab(Spot::Document(id)));
            }
        }
        ui.separator();
        for (label, dock, shortcut) in [
            ("Browser panel", Dock::Browser, key::BROWSER),
            ("Inspector panel", Dock::Inspector, key::INSPECTOR),
            ("Bottom dock", Dock::Bottom, key::DOCK),
        ] {
            if marked(ui, label, self.shell.open(dock), Some(shortcut)) {
                acts.push(Act::ToggleDock(dock));
            }
        }
        ui.separator();
        ui.menu_button("Theme", |ui| {
            for choice in [ThemeChoice::System, ThemeChoice::Light, ThemeChoice::Dark] {
                if marked(ui, choice.label(), self.theme == choice, None) {
                    self.pick_theme(ui.ctx(), frame, choice);
                }
            }
        });
    }

    /// ⚠️ Nothing but Connect… until one answers. Every other item here acts on an
    /// instrument, and the send queue is only ever owed to one.
    fn instrument_menu(&mut self, ui: &mut egui::Ui, acts: &mut Vec<Act>) {
        if !self.attached() {
            if item(ui, "Connect…", None) {
                acts.push(Act::Connect);
            }
            return;
        }
        if item(ui, "Disconnect", None) {
            acts.push(Act::Disconnect);
        }
        ui.separator();
        if item(ui, "Read everything", Some(key::RESYNC)) {
            acts.push(Act::Resync);
        }
        if let Some(class) = self.open_class() {
            if item(ui, &format!("Read {} again", folder(class)), None) {
                acts.push(Act::ReadAgain(class));
            }
        }
        ui.separator();
        if item(ui, "Review send queue…", Some(key::QUEUE)) {
            acts.push(Act::ShowPage(Page::Queue));
        }
        let waiting = self.queue.len();
        if waiting > 0 && item(ui, &format!("Send all ({waiting})"), None) {
            acts.push(Act::AskSendAll);
        }
        if waiting > 0 && item(ui, "Clear send queue", None) {
            acts.push(Act::ClearQueue);
        }
        let queued: Vec<u64> = self
            .browser
            .picked()
            .locals()
            .into_iter()
            .filter(|id| self.queue.holds(*id))
            .collect();
        if !queued.is_empty() && item(ui, "Remove from queue", None) {
            acts.extend(queued.into_iter().map(Act::Unqueue));
        }
    }

    /// The class the open document belongs to, for the menu that offers to read it
    /// again. Nothing on this computer came off a slot means nothing to name.
    fn open_class(&self) -> Option<ObjectClass> {
        let id = self.tabs.active()?;
        let (class, _) = self.workspace.get(id)?.origin.slot()?;
        Some(class)
    }

    /// 32 px: three groups of actions, the omnibox, and the three dock toggles.
    pub(crate) fn toolbar(&mut self, ctx: &egui::Context, acts: &mut Vec<Act>) {
        let fill = ctx.style().visuals.panel_fill;
        egui::TopBottomPanel::top("toolbar")
            .resizable(false)
            .exact_height(TOOLBAR)
            .frame(bare(fill))
            .show(ctx, |ui| {
                edge(ui, Side::Bottom);
                along(ui, |ui| {
                    if glyph_button(ui, Glyph::FolderOpen, false, "open files…").clicked() {
                        acts.push(Act::OpenFiles);
                    }
                    ui.scope(|ui| {
                        flat(ui);
                        let ink = ui.visuals().widgets.inactive.fg_stroke.color;
                        ui.menu_image_button(sized(Glyph::FilePlus2, GLYPH, ink), |ui| {
                            new_menu(ui, acts);
                        })
                        .response
                        .on_hover_text("something new on this computer");
                    });
                    let open = self.tabs.active();
                    if glyph_button(ui, Glyph::Save, false, "save the open document").clicked() {
                        if let Some(id) = open {
                            acts.push(Act::SaveDoc(id));
                        }
                    }
                    rule(ui, 16.0);
                    self.instrument_actions(ui, acts);

                    self.omnibox(ui, acts);
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        for (glyph, dock, hint) in [
                            (Glyph::PanelRight, Dock::Inspector, "the inspector"),
                            (Glyph::PanelBottom, Dock::Bottom, "the bottom dock"),
                            (Glyph::PanelLeft, Dock::Browser, "the browser"),
                        ] {
                            if glyph_button(ui, glyph, self.shell.open(dock), hint).clicked() {
                                acts.push(Act::ToggleDock(dock));
                            }
                        }
                    });
                });
            });
    }

    /// Read, and what is owed. Both act on an instrument, so neither is drawn without
    /// one, and the rule that would separate them from the omnibox goes with them.
    fn instrument_actions(&mut self, ui: &mut egui::Ui, acts: &mut Vec<Act>) {
        if !self.attached() {
            return;
        }
        if action(ui, Glyph::RefreshCw, "Read", false).clicked() {
            acts.push(Act::Resync);
        }
        let waiting = self.queue.len();
        let label = match waiting {
            0 => "Send".to_string(),
            n => format!("Send {n}"),
        };
        if action(ui, Glyph::Upload, &label, waiting > 0).clicked() && waiting > 0 {
            acts.push(Act::AskSendAll);
        }
        rule(ui, 16.0);
    }

    /// The name search the library's table is narrowed by.
    fn omnibox(&mut self, ui: &mut egui::Ui, acts: &mut Vec<Act>) {
        // Three toggles at 24, their gaps, and the padding they keep from the edge.
        const TOGGLES: f32 = 3.0 * 24.0 + 3.0 * GAP + PAD;
        let width = (ui.available_width() - TOGGLES).max(OMNIBOX);
        let border = ui.visuals().widgets.noninteractive.bg_stroke.color;
        let paper = ui.visuals().extreme_bg_color;
        let before = self.shell.omnibox.clone();
        ui.scope(|ui| {
            ui.visuals_mut().widgets.inactive.bg_stroke = egui::Stroke::new(1.0_f32, border);
            ui.add_sized(
                egui::vec2(width, BUTTON),
                egui::TextEdit::singleline(&mut self.shell.omnibox)
                    .id(egui::Id::new(SEARCH))
                    .background_color(paper)
                    // What is typed and the hint under it share one line down the middle
                    // of a box a third taller than the text in it.
                    .vertical_align(egui::Align::Center)
                    .hint_text(egui::RichText::new("Search…").text_style(ui_text())),
            );
        });
        if searched(&before, &self.shell.omnibox) {
            acts.push(Act::ShowTab(Spot::Library));
        }
    }

    /// 22 px: what just happened, and how much room is left.
    pub(crate) fn status_bar(&mut self, ctx: &egui::Context, acts: &mut Vec<Act>) {
        let fill = ctx.style().visuals.panel_fill;
        egui::TopBottomPanel::bottom("status")
            .resizable(false)
            .exact_height(STATUS)
            .frame(bare(fill))
            .show(ctx, |ui| {
                edge(ui, Side::Top);
                along(ui, |ui| {
                    let said = match &self.device.state.in_flight {
                        Some(words) => {
                            ui.spinner();
                            egui::RichText::new(&words.doing).size(11.0)
                        }
                        None => {
                            let (level, text) = self.log.status();
                            let tint = level.color(ui.visuals());
                            let glyph = match level {
                                Level::Info => Glyph::CircleCheck,
                                Level::Warn | Level::Error => Glyph::CircleAlert,
                            };
                            icon(ui, glyph, GLYPH, tint);
                            egui::RichText::new(text).size(11.0).color(tint)
                        }
                    };
                    if ui
                        .add(egui::Label::new(said).sense(egui::Sense::click()))
                        .on_hover_text("the whole activity log")
                        .clicked()
                    {
                        acts.push(Act::ShowPage(Page::Log));
                    }
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        let mut first = true;
                        for class in [ObjectClass::Sample, ObjectClass::Program] {
                            let unit = self.device.state.allocation_unit(class);
                            let Some(room) = occupancy(class, &self.device.state.inventory, unit)
                            else {
                                continue;
                            };
                            if !first {
                                rule(ui, 12.0);
                            }
                            first = false;
                            ui.label(
                                egui::RichText::new(format!("{} {room}", folder(class)))
                                    .monospace()
                                    .size(10.0)
                                    .weak(),
                            );
                        }
                    });
                });
            });
    }

    /// The bottom dock: a header that picks a page, and the page under it.
    pub(crate) fn bottom_dock(&mut self, ctx: &egui::Context, acts: &mut Vec<Act>) {
        let open = self.shell.dock_open;
        let fill = ctx.style().visuals.panel_fill;
        let shut = egui::TopBottomPanel::bottom("dock_shut")
            .resizable(false)
            .exact_height(DOCK)
            .frame(bare(fill));
        let most = (ctx.available_rect().height() - CENTRE_TALL).max(DOCK + BODY_LEAST);
        let full = egui::TopBottomPanel::bottom("dock")
            .resizable(true)
            .default_height(DOCK + self.shell.dock_body)
            .height_range((DOCK + BODY_LEAST)..=most)
            .frame(bare(fill));
        egui::TopBottomPanel::show_animated_between(ctx, open, shut, full, |ui, how| {
            claim(ui);
            edge(ui, Side::Top);
            self.dock_header(ui, acts);
            if how < 1.0 {
                return;
            }
            match self.page() {
                Page::Queue => self.queue_page(ui, acts),
                Page::Log => self.log.ui(ui),
            }
        });
        if let Some(rect) = laid_out(ctx, "dock") {
            self.shell.dock_body = rect.height() - DOCK;
        }
    }

    /// The dock's own header. [`panel_header`]'s geometry, with two titles to pick
    /// between rather than one.
    fn dock_header(&mut self, ui: &mut egui::Ui, acts: &mut Vec<Act>) {
        let waiting = self.queue.len();
        let pages: &[Page] = match self.attached() {
            true => &[Page::Queue, Page::Log],
            false => &[Page::Log],
        };
        let mut picked = None;
        let mut clear = false;
        strip(ui, |ui| {
            flat(ui);
            if chevron(ui, self.shell.dock_open).clicked() {
                acts.push(Act::ToggleDock(Dock::Bottom));
            }
            let ink = crate::app::caption(ui.visuals());
            for page in pages.iter().copied() {
                let on = self.shell.dock_open && self.shell.page == page;
                if ui
                    .selectable_label(on, caps(page.title()).color(ink))
                    .clicked()
                {
                    picked = Some(page_click(page, on));
                }
            }
            // An edit does not queue itself, so what a send would carry stands beside
            // what it would walk past, in one line, next to the button that closes the
            // gap between them. Each part wears the ink and the words of the mark it
            // stands for, which makes the line the legend for every dot in the window.
            let behind = crate::queue::Behind::of(&self.workspace, &self.device.state, &self.queue);
            ui.scope(|ui| {
                ui.spacing_mut().item_spacing.x = 4.0;
                for (index, (said, mark)) in behind.parts().into_iter().enumerate() {
                    if index > 0 {
                        ui.label(crate::queue::aside("·", crate::app::caption(ui.visuals())));
                    }
                    ui.label(crate::queue::aside(
                        &said,
                        crate::library::mark_ink(mark, ui.visuals()),
                    ))
                    .on_hover_text(crate::library::mark_words(mark));
                }
            });
            if ui
                .add_enabled(
                    behind.changed > 0,
                    egui::Button::new(behind.action()).small(),
                )
                .on_disabled_hover_text("nothing here differs from what the instrument holds")
                .clicked()
            {
                acts.push(Act::QueueChanged);
            }
            ui.with_layout(
                egui::Layout::right_to_left(egui::Align::Center),
                |ui| match self.page() {
                    Page::Log => clear = ui.small_button("Clear").clicked(),
                    Page::Queue => {
                        if waiting > 0 {
                            if action(ui, Glyph::Upload, "Send all", true).clicked() {
                                acts.push(Act::AskSendAll);
                            }
                            if ui
                                .small_button("Clear")
                                .on_hover_text("stop waiting to send any of it")
                                .clicked()
                            {
                                acts.push(Act::ClearQueue);
                            }
                        }
                    }
                },
            );
        });
        acts.extend(picked);
        if clear {
            self.log.clear();
        }
    }

    /// Which page the bottom dock is on. Nothing is owed to an instrument that is not
    /// there, so with none attached the log is the only page there is.
    fn page(&self) -> Page {
        match self.attached() {
            true => self.shell.page,
            false => Page::Log,
        }
    }

    /// Everything owed to the instrument, and what each of it runs into.
    fn queue_page(&mut self, ui: &mut egui::Ui, acts: &mut Vec<Act>) {
        crate::queue::page(
            ui,
            &mut self.queue,
            &self.workspace,
            &self.device.state,
            acts,
        );
    }

    /// The browser dock: this computer and the instrument, under one header.
    pub(crate) fn browser_dock(&mut self, ctx: &egui::Context, acts: &mut Vec<Act>) {
        self.shell.filter.keep_kinds(&crate::browser::kinds_present(
            &self.workspace,
            &self.device.state,
        ));
        let open = self.shell.browser_open;
        let fill = ctx.style().visuals.panel_fill;
        let shut = egui::SidePanel::left("browser_shut")
            .resizable(false)
            .exact_width(SHUT)
            .frame(bare(fill));
        let most =
            (ctx.available_rect().width() - self.inspector_room() - CENTRE_WIDE).max(SIDE_LEAST);
        let full = egui::SidePanel::left("browser")
            .resizable(true)
            .default_width(self.shell.browser_width)
            .width_range(SIDE_LEAST..=most)
            .frame(bare(fill));
        egui::SidePanel::show_animated_between(ctx, open, shut, full, |ui, how| {
            claim(ui);
            edge(ui, Side::Right);
            if how < 1.0 {
                if reopen(ui, Glyph::PanelLeftOpen, "show the browser").clicked() {
                    acts.push(Act::ToggleDock(Dock::Browser));
                }
                return;
            }
            dock_header(ui, "browser");
            acts.extend(self.browser.ui(
                ui,
                &self.workspace,
                &self.device,
                &self.queue,
                &self.shell.filter,
            ));
        });
        if let Some(rect) = laid_out(ctx, "browser") {
            self.shell.browser_width = rect.width();
        }
    }

    /// What the inspector will claim once the browser has taken its own: the browser's
    /// most is read before that dock is added, so it has to be asked for.
    fn inspector_room(&self) -> f32 {
        match self.shell.inspector_open {
            true => self.shell.inspector_width,
            false => SHUT,
        }
    }

    /// The inspector dock: what is picked, and — while one is attached — the instrument.
    pub(crate) fn inspector_dock(&mut self, ctx: &egui::Context, acts: &mut Vec<Act>) {
        let open = self.shell.inspector_open;
        let fill = ctx.style().visuals.panel_fill;
        let shut = egui::SidePanel::right("inspector_shut")
            .resizable(false)
            .exact_width(SHUT)
            .frame(bare(fill));
        let most = (ctx.available_rect().width() - CENTRE_WIDE).max(SIDE_LEAST);
        let full = egui::SidePanel::right("inspector")
            .resizable(true)
            .default_width(self.shell.inspector_width)
            .width_range(SIDE_LEAST..=most)
            .frame(bare(fill));
        egui::SidePanel::show_animated_between(ctx, open, shut, full, |ui, how| {
            claim(ui);
            edge(ui, Side::Left);
            if how < 1.0 {
                if reopen(ui, Glyph::PanelRightOpen, "show the inspector").clicked() {
                    acts.push(Act::ToggleDock(Dock::Inspector));
                }
                return;
            }
            acts.extend(crate::inspector::ui(
                ui,
                &mut self.shell,
                &mut self.browser,
                &self.workspace,
                &self.device,
                &self.queue,
            ));
        });
        if let Some(rect) = laid_out(ctx, "inspector") {
            self.shell.inspector_width = rect.width();
        }
    }
}

/// Claim the whole of the panel being drawn.
///
/// ⚠️ egui remembers a resizable panel's size as the size of what was put into it, so a
/// dock holding less than it shows would come back the least it is allowed to be.
fn claim(ui: &mut egui::Ui) {
    ui.set_min_size(ui.max_rect().size());
}

/// Where a dock ended up this frame. A collapsed dock draws under another id, so what
/// this answers is the size it will open back to.
fn laid_out(ctx: &egui::Context, id: &str) -> Option<egui::Rect> {
    egui::containers::panel::PanelState::load(ctx, egui::Id::new(id)).map(|state| state.rect)
}

/// A shut side dock: the width of one glyph, and the glyph that opens it again.
fn reopen(ui: &mut egui::Ui, glyph: Glyph, hint: &str) -> egui::Response {
    let ink = ui.visuals().widgets.inactive.fg_stroke.color;
    let rect = ui.max_rect();
    let box_ = egui::Rect::from_center_size(
        egui::pos2(rect.center().x, rect.top() + DOCK / 2.0),
        egui::Vec2::splat(GLYPH),
    );
    crate::icon::painted(ui, glyph, box_, ink);
    ui.interact(box_, ui.id().with("reopen"), egui::Sense::click())
        .on_hover_text(hint)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::Fake;
    use eframe::{App, Storage};

    /// The window the design is drawn to, and the smallest this shell claims to hold.
    const SCREEN: egui::Vec2 = egui::vec2(900.0, 540.0);

    /// The regions, in the order they claim space.
    const REGIONS: [&str; 6] = [
        "titlebar",
        "toolbar",
        "status",
        "dock",
        "browser",
        "inspector",
    ];

    fn app(ctx: &egui::Context, storage: Option<&dyn eframe::Storage>) -> DrawbarApp {
        let mut cc = eframe::CreationContext::_new_kittest(ctx.clone());
        cc.storage = storage;
        DrawbarApp::new(&cc)
    }

    /// An instrument answering, which is what the full layout needs to draw.
    fn attach(app: &mut DrawbarApp) {
        app.device
            .pretend_scanned(ObjectClass::Program, 1, &["Africa Split"]);
    }

    /// What one frame at 900 × 540 laid out and what it wrote.
    struct Painted {
        centre: egui::Rect,
        panels: Vec<(String, egui::Rect)>,
        /// Every string the frame painted, headers and button labels included.
        words: Vec<String>,
    }

    impl Painted {
        fn wrote(&self, word: &str) -> bool {
            self.words.iter().any(|said| said == word)
        }

        fn region(&self, want: &str) -> Option<egui::Rect> {
            self.panels
                .iter()
                .find(|(id, _)| id == want)
                .map(|(_, rect)| *rect)
        }
    }

    /// One frame at 900 × 540, answering with the centre's rect, every panel's, and the
    /// text the frame put on screen.
    fn drawn(ctx: &egui::Context, app: &mut DrawbarApp) -> Painted {
        frame_of(ctx, app, Vec::new())
    }

    /// One frame with something arriving in it.
    fn frame_of(ctx: &egui::Context, app: &mut DrawbarApp, events: Vec<egui::Event>) -> Painted {
        fn words(shape: &egui::Shape, into: &mut Vec<String>) {
            match shape {
                egui::Shape::Text(text) => into.push(text.galley.text().to_string()),
                egui::Shape::Vec(shapes) => shapes.iter().for_each(|shape| words(shape, into)),
                _ => {}
            }
        }

        let mut frame = eframe::Frame::_new_kittest();
        let input = egui::RawInput {
            events,
            screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, SCREEN)),
            ..Default::default()
        };
        let mut centre = egui::Rect::NOTHING;
        let output = ctx.run(input, |ctx| {
            app.update(ctx, &mut frame);
            // Panels shrink this as they are added; the central panel does not.
            centre = ctx.available_rect();
        });
        let mut said = Vec::new();
        for clipped in &output.shapes {
            words(&clipped.shape, &mut said);
        }
        let panels = REGIONS
            .iter()
            .filter_map(|id| {
                let state = egui::containers::panel::PanelState::load(ctx, egui::Id::new(*id))?;
                Some((id.to_string(), state.rect))
            })
            .collect();
        Painted {
            centre,
            panels,
            words: said,
        }
    }

    /// Every fixed region fits inside the window the design is drawn to, and the centre
    /// still has room left after all of them have taken theirs.
    #[test]
    fn at_900_by_540_every_region_fits_and_the_centre_survives() {
        let ctx = egui::Context::default();
        let mut app = app(&ctx, None);
        app.shell.dock_open = true;
        attach(&mut app);
        // Twice: the first frame is what the second lays itself out against.
        let _ = drawn(&ctx, &mut app);
        let painted = drawn(&ctx, &mut app);
        let (centre, panels) = (painted.centre, &painted.panels);

        let screen = egui::Rect::from_min_size(egui::Pos2::ZERO, SCREEN);
        assert_eq!(panels.len(), REGIONS.len(), "every region drew: {panels:?}");
        for (id, rect) in panels {
            assert!(
                screen.contains_rect(*rect),
                "{id} is outside the window: {rect:?}"
            );
        }
        let at = |want: &str| painted.region(want).unwrap();
        assert_eq!(at("titlebar").height(), TITLEBAR);
        assert_eq!(at("toolbar").height(), TOOLBAR);
        assert_eq!(at("status").height(), STATUS);
        assert_eq!(at("dock").height(), DOCK + DOCK_BODY);
        assert_eq!(at("browser").width(), BROWSER);
        assert_eq!(at("inspector").width(), INSPECTOR);
        // The dock's header is its top [`DOCK`], inside the window with the rest.
        let header = at("dock").split_top_bottom_at_y(at("dock").top() + DOCK).0;
        assert!(screen.contains_rect(header), "the dock header: {header:?}");

        assert!(centre.width() > 0.0, "the centre: {centre:?}");
        assert!(centre.height() > 0.0, "the centre: {centre:?}");
    }

    /// Flipping the theme moves colours and nothing else: the metrics live on the style
    /// both faces share, so every region lands in the same place.
    #[test]
    fn flipping_the_theme_leaves_every_region_where_it_was() {
        let ctx = egui::Context::default();
        let mut app = app(&ctx, None);
        app.shell.dock_open = true;
        attach(&mut app);

        ctx.set_theme(egui::ThemePreference::Dark);
        let _ = drawn(&ctx, &mut app);
        let dark = drawn(&ctx, &mut app);

        ctx.set_theme(egui::ThemePreference::Light);
        let _ = drawn(&ctx, &mut app);
        let light = drawn(&ctx, &mut app);

        assert_eq!(dark.centre, light.centre);
        assert_eq!(dark.panels, light.panels);
    }

    /// ⚠️ With nothing attached there is nothing to read from, nothing to send to and no
    /// room to report. Every control that acts on an instrument is absent rather than
    /// dead — the inspector's INSTRUMENT group with them — while SELECTION, which is
    /// about what is picked here, stays whatever is on the bus.
    #[test]
    fn no_instrument_means_no_instrument_controls() {
        const ONLY_WITH_ONE: [&str; 6] =
            ["Read", "Send", "SEND QUEUE", "INSTRUMENT", "ROOM", "INFO"];

        let ctx = egui::Context::default();
        let mut app = app(&ctx, None);
        app.shell.dock_open = true;
        let _ = drawn(&ctx, &mut app);
        let alone = drawn(&ctx, &mut app);

        assert!(!app.attached(), "nothing was attached");
        for control in ONLY_WITH_ONE {
            assert!(
                !alone.wrote(control),
                "{control} is painted with none attached"
            );
        }
        assert!(alone.wrote("ACTIVITY LOG"), "the log is the one page left");
        assert!(alone.wrote("SELECTION"), "the inspector still answers");
        assert!(alone.region("inspector").is_some(), "{:?}", alone.panels);

        attach(&mut app);
        let _ = drawn(&ctx, &mut app);
        let answering = drawn(&ctx, &mut app);
        for control in ONLY_WITH_ONE {
            assert!(
                answering.wrote(control),
                "{control} is missing with one attached"
            );
        }
        assert!(answering.wrote("SELECTION"));
        assert_eq!(
            answering.region("inspector"),
            alone.region("inspector"),
            "the dock is the same width either way"
        );
    }

    /// Every change to what is typed brings the library forward, the first keystroke
    /// into an empty box and clearing it included. A frame that typed nothing is not a
    /// change.
    #[test]
    fn a_change_in_the_omnibox_is_what_brings_the_library_forward() {
        assert!(searched("", "a"), "the first keystroke");
        assert!(searched("afr", "afri"));
        assert!(searched("afri", "afr"), "and a backspace");
        assert!(searched("afr", ""), "and clearing it");
        assert!(!searched("afr", "afr"));
        assert!(!searched("", ""));
    }

    /// ⚠️ The omnibox narrows the library's table and nothing else. Typing into it with a
    /// document in front would otherwise search where nobody can see the result.
    #[test]
    fn typing_a_search_with_a_document_in_front_brings_the_library_forward() {
        let ctx = egui::Context::default();
        let mut app = app(&ctx, None);
        let id = app
            .workspace
            .create(crate::workspace::Fresh::Program, &mut app.log)
            .unwrap();
        app.tabs.open(id);
        let _ = drawn(&ctx, &mut app);
        assert_eq!(app.tabs.showing(), Some(Spot::Document(id)));

        ctx.memory_mut(|memory| memory.request_focus(egui::Id::new(SEARCH)));
        let _ = frame_of(&ctx, &mut app, vec![egui::Event::Text("afr".into())]);
        assert_eq!(app.shell.omnibox, "afr");
        assert_eq!(app.tabs.showing(), Some(Spot::Library));

        // And the frame after it, which typed nothing, leaves the tab where the user put
        // it.
        app.tabs.show(Spot::Document(id));
        let _ = drawn(&ctx, &mut app);
        assert_eq!(app.tabs.showing(), Some(Spot::Document(id)));
    }

    /// What was collapsed comes back collapsed in the next session's window.
    #[test]
    fn the_docks_come_back_where_the_last_session_left_them() {
        let mut store = Fake::default();
        {
            let ctx = egui::Context::default();
            let mut before = app(&ctx, None);
            before.shell.browser_open = false;
            before.shell.show_page(Page::Log);
            before.save(&mut store);
        }
        let ctx = egui::Context::default();
        let after = app(&ctx, Some(&store));
        assert!(!after.shell.browser_open);
        assert!(after.shell.inspector_open);
        assert!(after.shell.dock_open);
        assert_eq!(after.shell.page, Page::Log);
    }

    #[test]
    fn a_dock_toggles_and_a_page_opens_the_dock_it_is_on() {
        let mut shell = Shell::default();
        for dock in [Dock::Browser, Dock::Inspector, Dock::Bottom] {
            let was = shell.open(dock);
            shell.toggle(dock);
            assert_eq!(shell.open(dock), !was, "{dock:?}");
        }
        // Asking for a page is asking to read it, which means opening the dock too.
        shell.dock_open = false;
        shell.show_page(Page::Log);
        assert!(shell.dock_open && shell.page == Page::Log);
    }

    /// ⚠️ The title of the page already showing is the way back out of the dock. A title
    /// that answered a click by doing nothing reads as broken, and the collapse triangle
    /// is 8 px of the header.
    #[test]
    fn the_title_of_the_page_showing_shuts_the_dock() {
        assert!(matches!(
            page_click(Page::Queue, true),
            Act::ToggleDock(Dock::Bottom)
        ));
        assert!(matches!(
            page_click(Page::Queue, false),
            Act::ShowPage(Page::Queue)
        ));
    }

    /// What was collapsed is collapsed again next session, on the page it was left on.
    #[test]
    fn the_layout_comes_back_as_it_was_left() {
        let mut store = Fake::default();
        let before = Shell {
            browser_open: false,
            inspector_open: true,
            dock_open: true,
            room_open: false,
            info_open: true,
            browser_width: 301.0,
            inspector_width: 199.0,
            dock_body: 260.0,
            page: Page::Log,
            omnibox: "typed and not kept".into(),
            filter: Filter::default(),
        };
        before.keep(&mut store);

        let mut after = Shell::default();
        after.restore(&store);
        assert!(!after.browser_open);
        assert!(after.inspector_open);
        assert!(after.dock_open);
        assert!(!after.room_open, "a shut inspector panel comes back shut");
        assert!(after.info_open, "and an open one comes back open");
        assert_eq!(after.browser_width, 301.0);
        assert_eq!(after.inspector_width, 199.0);
        assert_eq!(after.dock_body, 260.0);
        assert_eq!(after.page, Page::Log);
        assert!(after.omnibox.is_empty(), "a search is not a layout");
    }

    /// A stored size this build would never have laid out is not a size, so the dock
    /// opens to the width the design gives it rather than to a sliver or to nonsense.
    #[test]
    fn a_size_outside_what_a_dock_opens_to_comes_back_as_the_default() {
        let mut store = Fake::default();
        store.set_string(
            Shell::KEY,
            format!(
                "{}\nbrowser_width\t12\ninspector_width\twide\ndock_body\tNaN\n",
                Shell::VERSION
            ),
        );
        let mut shell = Shell::default();
        shell.restore(&store);
        assert_eq!(shell.browser_width, BROWSER, "under the least it opens to");
        assert_eq!(shell.inspector_width, INSPECTOR, "not a number");
        assert_eq!(shell.dock_body, DOCK_BODY, "not a size");
    }

    /// However far a dock was dragged last session, the centre keeps its own room: the
    /// most a dock may claim is read off the window every frame.
    #[test]
    fn docks_wider_than_the_window_still_leave_the_centre_its_room() {
        let ctx = egui::Context::default();
        let mut app = app(&ctx, None);
        app.shell.dock_open = true;
        app.shell.browser_width = 5_000.0;
        app.shell.inspector_width = 5_000.0;
        app.shell.dock_body = 5_000.0;
        attach(&mut app);
        let _ = drawn(&ctx, &mut app);
        let painted = drawn(&ctx, &mut app);

        assert!(
            painted.centre.width() >= CENTRE_WIDE,
            "the centre: {:?}",
            painted.centre
        );
        assert!(
            painted.centre.height() >= CENTRE_TALL,
            "the centre: {:?}",
            painted.centre
        );
        assert!(app.shell.browser_width >= SIDE_LEAST);
        assert!(app.shell.dock_body >= BODY_LEAST);
    }

    /// A dock keeps the size it was left at across a frame, so what is written out is
    /// what the window actually showed rather than the design's default.
    #[test]
    fn a_dock_drawn_narrower_writes_the_size_it_was_drawn_at() {
        let ctx = egui::Context::default();
        let mut app = app(&ctx, None);
        app.shell.dock_open = true;
        app.shell.browser_width = 190.0;
        app.shell.dock_body = 130.0;
        attach(&mut app);
        let _ = drawn(&ctx, &mut app);
        let painted = drawn(&ctx, &mut app);

        assert_eq!(painted.region("browser").unwrap().width(), 190.0);
        assert_eq!(painted.region("dock").unwrap().height(), DOCK + 130.0);
        assert_eq!(app.shell.browser_width, 190.0);
        assert_eq!(app.shell.dock_body, 130.0);
    }

    /// A version nobody wrote is a layout nobody can explain, so the defaults stand.
    #[test]
    fn an_unknown_version_leaves_the_default_layout() {
        let mut store = Fake::default();
        store.set_string(
            Shell::KEY,
            "drawbar docks 99\nbrowser\t0\ndock\t1\n".to_string(),
        );
        let mut shell = Shell::default();
        shell.restore(&store);
        assert!(shell.browser_open, "the default stands");
        assert!(!shell.dock_open);
    }

    /// A store holding nothing about the docks is not an error, and neither is one
    /// holding lines this build has no field for.
    #[test]
    fn an_empty_or_partial_store_is_a_store() {
        let mut shell = Shell::default();
        shell.restore(&Fake::default());
        assert!(shell.browser_open && shell.inspector_open && !shell.dock_open);

        let mut store = Fake::default();
        store.set_string(
            Shell::KEY,
            format!("{}\ninspector\t0\nrail\t1\n", Shell::VERSION),
        );
        shell.restore(&store);
        assert!(!shell.inspector_open);
        assert!(shell.browser_open, "a field nobody wrote keeps its default");
    }
}
