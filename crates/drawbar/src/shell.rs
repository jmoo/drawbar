//! The dock shell: the regions the window is cut into, what is collapsed, and the
//! menus, toolbar and status bar that sit around the centre.
//!
//! Panels claim space in the order [`crate::app::DrawbarApp::update`] adds them, and
//! every one of them is an exact size that does not resize: the layout is the design's,
//! not a drag's. Each contents pads itself, so a panel header can bleed to both edges.

use eframe::egui;
use nord_usb::ObjectClass;

use crate::app::{accent, ui as ui_text, DrawbarApp, ThemeChoice};
use crate::browser::{new_menu, Act};
use crate::device::occupancy;
use crate::filter::Filter;
use crate::icon::{icon, sized, Glyph};
use crate::log::Level;
use crate::panel::{caps, chevron, panel_header, strip, HEADER};
use crate::strings::folder;
use crate::tabs::Spot;

/// The window's own bar: the mark, the menus, the instrument, the theme.
pub const TITLEBAR: f32 = 30.0;

/// The bar of actions under it.
pub const TOOLBAR: f32 = 32.0;

/// One line of plain words at the foot of the window.
pub const STATUS: f32 = 22.0;

/// The bottom dock's body, under its header.
pub const DOCK_BODY: f32 = 184.0;

/// The side docks, open and shut. Fixed widths: the design's proportions are the point.
pub const BROWSER: f32 = 232.0;
pub const INSPECTOR: f32 = 244.0;
pub const SHUT: f32 = 30.0;

/// The room the bars keep at each end, and the gap between their parts.
const PAD: f32 = 8.0;
const GAP: f32 = 6.0;

/// A glyph in a bar, and the height of a control beside it.
const GLYPH: f32 = 13.0;
const BUTTON: f32 = 22.0;

/// The omnibox's least width. Below this it is a box nobody can read a name in.
const OMNIBOX: f32 = 300.0;

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
    /// The inspector's three panels, each collapsed on its own.
    pub room_open: bool,
    pub deps_open: bool,
    pub tags_open: bool,
    pub page: Page,
    /// What has been typed into the omnibox. Filtering the library by it is stage 6;
    /// nothing reads this yet.
    pub omnibox: String,
    /// What the library is narrowed to. The tree's kind and tag rows set it; stage 6's
    /// table reads it.
    pub filter: Filter,
}

impl Default for Shell {
    fn default() -> Shell {
        Shell {
            browser_open: true,
            inspector_open: true,
            dock_open: false,
            room_open: true,
            deps_open: true,
            tags_open: true,
            page: Page::default(),
            omnibox: String::new(),
            filter: Filter::default(),
        }
    }
}

impl Shell {
    /// Where the layout is kept between sessions, beside the browser's own keys.
    pub const KEY: &'static str = "drawbar.docks";

    const VERSION: &'static str = "drawbar docks 2";

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
                (Some("deps"), Some(open)) => held.deps_open = open == "1",
                (Some("tags"), Some(open)) => held.tags_open = open == "1",
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
        self.deps_open = held.deps_open;
        self.tags_open = held.tags_open;
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
                "{}\nbrowser\t{}\ninspector\t{}\ndock\t{}\nroom\t{}\ndeps\t{}\n\
                 tags\t{}\npage\t{}\n",
                Shell::VERSION,
                bit(self.browser_open),
                bit(self.inspector_open),
                bit(self.dock_open),
                bit(self.room_open),
                bit(self.deps_open),
                bit(self.tags_open),
                self.page.stored(),
            ),
        );
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
    pub const KEEP: Shortcut = Shortcut::new(With::COMMAND, Key::S);
    pub const EXPORT: Shortcut = Shortcut::new(With::COMMAND.plus(With::SHIFT), Key::E);
    pub const CLOSE: Shortcut = Shortcut::new(With::COMMAND, Key::W);
    pub const QUIT: Shortcut = Shortcut::new(With::COMMAND, Key::Q);
    pub const LIBRARY: Shortcut = Shortcut::new(With::COMMAND, Key::Num1);
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
/// ⚠️ ⌘W, ⌘Q and ⌘1–⌘3 reach the tab, not the page. On the web those items work by
/// click alone.
const WINDOWED: bool = !cfg!(target_arch = "wasm32");

/// The key text beside a menu label — a window's, never a tab's.
fn keyed(ctx: &egui::Context, shortcut: egui::KeyboardShortcut) -> String {
    match WINDOWED {
        true => ctx.format_shortcut(&shortcut),
        false => String::new(),
    }
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

/// A menu item that also says whether what it names is showing.
fn marked(
    ui: &mut egui::Ui,
    label: &str,
    on: bool,
    shortcut: Option<egui::KeyboardShortcut>,
) -> bool {
    let mut button = egui::Button::new(label).selected(on);
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
/// ⚠️ An accented action carries the accent in its glyph and its border and nowhere
/// else: accent on panel measures 4.1:1, which fails as 11 px text.
fn action(ui: &mut egui::Ui, glyph: Glyph, label: &str, accented: bool) -> egui::Response {
    let visuals = ui.visuals();
    let quiet = visuals.widgets.inactive.fg_stroke.color;
    let (mark, border, ink) = match accented {
        true => (
            accent(visuals),
            accent(visuals),
            visuals.widgets.active.fg_stroke.color,
        ),
        false => (quiet, visuals.widgets.noninteractive.bg_stroke.color, quiet),
    };
    ui.add(
        egui::Button::image_and_text(
            sized(glyph, GLYPH, mark),
            egui::RichText::new(label).text_style(ui_text()).color(ink),
        )
        .image_tint_follows_text_color(false)
        .stroke(egui::Stroke::new(1.0_f32, border))
        .corner_radius(2.0)
        .min_size(egui::vec2(0.0, BUTTON)),
    )
}

/// A 24 × 22 button carrying one glyph.
fn glyph_button(ui: &mut egui::Ui, glyph: Glyph, on: bool, hint: &str) -> egui::Response {
    let visuals = ui.visuals();
    let ink = match on {
        true => visuals.widgets.active.fg_stroke.color,
        false => visuals.widgets.inactive.fg_stroke.color,
    };
    let held = on.then_some(visuals.widgets.active.bg_fill);
    let mut button = egui::Button::image(sized(glyph, GLYPH, ink))
        .image_tint_follows_text_color(false)
        .corner_radius(2.0)
        .min_size(egui::vec2(24.0, BUTTON));
    if let Some(fill) = held {
        button = button.fill(fill);
    }
    ui.add(button).on_hover_text(hint)
}

impl DrawbarApp {
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
                    ui.label(egui::RichText::new("drawbar").size(12.0).strong());
                    self.shortcuts(ui, acts);
                    self.menus(ui, frame, acts);
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
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
        let border = visuals.widgets.noninteractive.bg_stroke.color;
        let lit = crate::app::good(visuals);
        egui::Frame::new()
            .stroke(egui::Stroke::new(1.0_f32, border))
            .corner_radius(2.0)
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
                acts.push(Act::Save(id));
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
        if hit(&key::QUEUE) {
            acts.push(Act::ShowPage(Page::Queue));
        }
        if self.device.state.connected() && hit(&key::RESYNC) {
            acts.push(Act::Resync);
        }
        if WINDOWED && hit(&key::CLOSE) {
            acts.push(Act::CloseTab);
        }
        if WINDOWED && hit(&key::QUIT) {
            acts.push(Act::Quit);
        }
        if WINDOWED && hit(&key::LIBRARY) {
            acts.push(Act::ShowTab(Spot::Library));
        }
        if WINDOWED && self.device.state.connected() && hit(&key::KEYBOARD) {
            acts.push(Act::ShowTab(Spot::Keyboard));
        }
        if WINDOWED && hit(&key::DOCUMENT) {
            if let Some(id) = self.tabs.last_document() {
                acts.push(Act::ShowTab(Spot::Document(id)));
            }
        }
        // ⚠️ Never a file export. ⌘S means "keep what I did", which for something read
        // off the instrument is a promise to send it back.
        if hit(&key::KEEP) {
            self.stage_open();
        }
    }

    /// Queue the open document for the slot it came off, or say why there is none.
    fn stage_open(&mut self) {
        let Some(id) = self.tabs.active() else {
            return;
        };
        self.document.stage(
            id,
            &self.workspace,
            &mut self.device,
            &mut self.queue,
            &mut self.log,
        );
    }

    fn menus(&mut self, ui: &mut egui::Ui, frame: &mut eframe::Frame, acts: &mut Vec<Act>) {
        ui.menu_button("File", |ui| self.file_menu(ui, acts));
        ui.menu_button("View", |ui| self.view_menu(ui, frame, acts));
        ui.menu_button("Instrument", |ui| self.instrument_menu(ui, acts));
        ui.menu_button("Help", |ui| {
            if item(ui, "Copy activity log", None) {
                acts.push(Act::CopyLog);
            }
        });
    }

    fn file_menu(&mut self, ui: &mut egui::Ui, acts: &mut Vec<Act>) {
        if item(ui, "Open…", Some(key::OPEN)) {
            acts.push(Act::OpenFiles);
        }
        ui.menu_button("New", |ui| {
            new_menu(ui, acts);
            ui.separator();
            if item(ui, "New folder", None) {
                acts.push(Act::NewFolder);
            }
        });
        ui.separator();
        if let Some(id) = self.tabs.active() {
            if item(ui, "Keep", Some(key::KEEP)) {
                self.stage_open();
            }
            let opened = self.tabs.opened(id).to_vec();
            let changed = self
                .workspace
                .get(id)
                .is_some_and(|entity| entity.bytes != opened);
            if changed && item(ui, "Revert to opened", None) {
                acts.push(Act::Revert(id));
            }
            if item(ui, "Export…", Some(key::EXPORT)) {
                acts.push(Act::Save(id));
            }
            ui.separator();
        }
        if self.tabs.showing().is_some() && item(ui, "Close tab", Some(key::CLOSE)) {
            acts.push(Act::CloseTab);
        }
        if WINDOWED && item(ui, "Quit", Some(key::QUIT)) {
            acts.push(Act::Quit);
        }
    }

    fn view_menu(&mut self, ui: &mut egui::Ui, frame: &mut eframe::Frame, acts: &mut Vec<Act>) {
        let showing = self.tabs.showing();
        if marked(
            ui,
            "Library",
            showing == Some(Spot::Library),
            Some(key::LIBRARY),
        ) {
            acts.push(Act::ShowTab(Spot::Library));
        }
        if self.device.state.connected()
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

    fn instrument_menu(&mut self, ui: &mut egui::Ui, acts: &mut Vec<Act>) {
        match self.device.state.connected() {
            false => {
                if item(ui, "Connect…", None) {
                    acts.push(Act::Connect);
                }
            }
            true => {
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
                    let ink = ui.visuals().widgets.inactive.fg_stroke.color;
                    ui.menu_image_button(sized(Glyph::FilePlus2, GLYPH, ink), |ui| {
                        new_menu(ui, acts);
                    })
                    .response
                    .on_hover_text("something new on this computer");
                    let open = self.tabs.active();
                    if glyph_button(ui, Glyph::Save, false, "export the open document…").clicked()
                    {
                        if let Some(id) = open {
                            acts.push(Act::Save(id));
                        }
                    }
                    rule(ui, 16.0);

                    let attached = self.device.state.connected();
                    if action(ui, Glyph::RefreshCw, "Read", false).clicked() && attached {
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

                    self.omnibox(ui);
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

    /// ⚠️ It holds what is typed and nothing else. Filtering the library by it is stage
    /// 6; a box that looked like it filtered and did not would be worse than none.
    fn omnibox(&mut self, ui: &mut egui::Ui) {
        // Three toggles at 24, their gaps, and the padding they keep from the edge.
        const TOGGLES: f32 = 3.0 * 24.0 + 3.0 * GAP + PAD;
        let width = (ui.available_width() - TOGGLES).max(OMNIBOX);
        let border = ui.visuals().widgets.noninteractive.bg_stroke.color;
        let paper = ui.visuals().extreme_bg_color;
        ui.scope(|ui| {
            ui.visuals_mut().widgets.inactive.bg_stroke = egui::Stroke::new(1.0_f32, border);
            ui.add_sized(
                egui::vec2(width, BUTTON),
                egui::TextEdit::singleline(&mut self.shell.omnibox)
                    .background_color(paper)
                    .hint_text(egui::RichText::new("Find a sound").text_style(ui_text())),
            );
        });
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
            .exact_height(HEADER)
            .frame(bare(fill));
        let full = egui::TopBottomPanel::bottom("dock")
            .resizable(false)
            .exact_height(HEADER + DOCK_BODY)
            .frame(bare(fill));
        egui::TopBottomPanel::show_animated_between(ctx, open, shut, full, |ui, how| {
            edge(ui, Side::Top);
            self.dock_header(ui, acts);
            if how < 1.0 {
                return;
            }
            match self.shell.page {
                Page::Queue => self.queue_page(ui),
                Page::Log => self.log.ui(ui),
            }
        });
    }

    /// The dock's own header. [`panel_header`]'s geometry, with two titles to pick
    /// between rather than one.
    fn dock_header(&mut self, ui: &mut egui::Ui, acts: &mut Vec<Act>) {
        let waiting = self.queue.len();
        let mut picked = None;
        let mut clear = false;
        strip(ui, |ui| {
            if chevron(ui, self.shell.dock_open).clicked() {
                acts.push(Act::ToggleDock(Dock::Bottom));
            }
            let ink = ui.visuals().widgets.noninteractive.fg_stroke.color;
            icon(ui, Glyph::GitCompareArrows, GLYPH, ink);
            for page in [Page::Queue, Page::Log] {
                let on = self.shell.dock_open && self.shell.page == page;
                if ui
                    .selectable_label(on, caps(page.title()).color(ink))
                    .clicked()
                {
                    picked = Some(page);
                }
            }
            // What the queue amounts to, wherever the dock is: the summary is the
            // reason to open it.
            if waiting > 0 {
                ui.label(crate::queue::heading(&self.queue, ui.visuals()));
            }
            ui.with_layout(
                egui::Layout::right_to_left(egui::Align::Center),
                |ui| match self.shell.page {
                    Page::Log => clear = ui.small_button("Clear").clicked(),
                    Page::Queue => {
                        if waiting > 0 {
                            let sent = action(ui, Glyph::Upload, "Send all", true).clicked();
                            if sent {
                                acts.push(Act::AskSendAll);
                            }
                        }
                    }
                },
            );
        });
        if let Some(page) = picked {
            acts.push(Act::ShowPage(page));
        }
        if clear {
            self.log.clear();
        }
    }

    /// Everything owed to the instrument, and what each of it runs into.
    fn queue_page(&mut self, ui: &mut egui::Ui) {
        crate::queue::page(ui, &mut self.queue, &self.workspace);
    }

    /// The browser dock: this computer and the instrument, under one header.
    pub(crate) fn browser_dock(&mut self, ctx: &egui::Context, acts: &mut Vec<Act>) {
        let open = self.shell.browser_open;
        let fill = ctx.style().visuals.panel_fill;
        let shut = egui::SidePanel::left("browser_shut")
            .resizable(false)
            .exact_width(SHUT)
            .frame(bare(fill));
        let full = egui::SidePanel::left("browser")
            .resizable(false)
            .exact_width(BROWSER)
            .frame(bare(fill));
        egui::SidePanel::show_animated_between(ctx, open, shut, full, |ui, how| {
            edge(ui, Side::Right);
            if how < 1.0 {
                if reopen(ui, Glyph::PanelLeftOpen, "show the browser").clicked() {
                    acts.push(Act::ToggleDock(Dock::Browser));
                }
                return;
            }
            panel_header(ui, "browser", Some(&mut self.shell.browser_open), None);
            acts.extend(self.browser.ui(
                ui,
                &self.workspace,
                &self.device,
                &self.queue,
                &self.shell.filter,
            ));
        });
    }

    /// The inspector dock: how much room there is, what the selection needs, and what it
    /// is labelled with.
    pub(crate) fn inspector_dock(&mut self, ctx: &egui::Context, acts: &mut Vec<Act>) {
        let open = self.shell.inspector_open;
        let fill = ctx.style().visuals.panel_fill;
        let shut = egui::SidePanel::right("inspector_shut")
            .resizable(false)
            .exact_width(SHUT)
            .frame(bare(fill));
        let full = egui::SidePanel::right("inspector")
            .resizable(false)
            .exact_width(INSPECTOR)
            .frame(bare(fill));
        egui::SidePanel::show_animated_between(ctx, open, shut, full, |ui, how| {
            edge(ui, Side::Left);
            if how < 1.0 {
                if reopen(ui, Glyph::PanelRightOpen, "show the inspector").clicked() {
                    acts.push(Act::ToggleDock(Dock::Inspector));
                }
                return;
            }
            // The dock's own header keeps the collapse gesture every dock has; the three
            // under it collapse only themselves.
            panel_header(ui, "inspector", Some(&mut self.shell.inspector_open), None);
            acts.extend(crate::inspector::ui(
                ui,
                &mut self.shell,
                &mut self.browser,
                &self.workspace,
                &self.device,
                &self.queue,
            ));
        });
    }
}

/// A shut side dock: the width of one glyph, and the glyph that opens it again.
fn reopen(ui: &mut egui::Ui, glyph: Glyph, hint: &str) -> egui::Response {
    let ink = ui.visuals().widgets.inactive.fg_stroke.color;
    let rect = ui.max_rect();
    let box_ = egui::Rect::from_center_size(
        egui::pos2(rect.center().x, rect.top() + HEADER / 2.0),
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

    /// One frame at 900 × 540, answering with the centre's rect and every panel's.
    fn drawn(ctx: &egui::Context, app: &mut DrawbarApp) -> (egui::Rect, Vec<(String, egui::Rect)>) {
        let mut frame = eframe::Frame::_new_kittest();
        let input = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, SCREEN)),
            ..Default::default()
        };
        let mut centre = egui::Rect::NOTHING;
        let _ = ctx.run(input, |ctx| {
            app.update(ctx, &mut frame);
            // Panels shrink this as they are added; the central panel does not.
            centre = ctx.available_rect();
        });
        let panels = REGIONS
            .iter()
            .filter_map(|id| {
                let state = egui::containers::panel::PanelState::load(ctx, egui::Id::new(*id))?;
                Some((id.to_string(), state.rect))
            })
            .collect();
        (centre, panels)
    }

    /// Every fixed region fits inside the window the design is drawn to, and the centre
    /// still has room left after all of them have taken theirs.
    #[test]
    fn at_900_by_540_every_region_fits_and_the_centre_survives() {
        let ctx = egui::Context::default();
        let mut app = app(&ctx, None);
        app.shell.dock_open = true;
        // Twice: the first frame is what the second lays itself out against.
        let _ = drawn(&ctx, &mut app);
        let (centre, panels) = drawn(&ctx, &mut app);

        let screen = egui::Rect::from_min_size(egui::Pos2::ZERO, SCREEN);
        assert_eq!(panels.len(), REGIONS.len(), "every region drew: {panels:?}");
        for (id, rect) in &panels {
            assert!(
                screen.contains_rect(*rect),
                "{id} is outside the window: {rect:?}"
            );
        }
        let at = |want: &str| {
            panels
                .iter()
                .find(|(id, _)| id == want)
                .map(|(_, rect)| *rect)
                .unwrap()
        };
        assert_eq!(at("titlebar").height(), TITLEBAR);
        assert_eq!(at("toolbar").height(), TOOLBAR);
        assert_eq!(at("status").height(), STATUS);
        assert_eq!(at("dock").height(), HEADER + DOCK_BODY);
        assert_eq!(at("browser").width(), BROWSER);
        assert_eq!(at("inspector").width(), INSPECTOR);
        // The dock's header is its top 24 px, and it is inside the window with the rest.
        let header = at("dock")
            .split_top_bottom_at_y(at("dock").top() + HEADER)
            .0;
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

        ctx.set_theme(egui::ThemePreference::Dark);
        let _ = drawn(&ctx, &mut app);
        let (dark_centre, dark_panels) = drawn(&ctx, &mut app);

        ctx.set_theme(egui::ThemePreference::Light);
        let _ = drawn(&ctx, &mut app);
        let (light_centre, light_panels) = drawn(&ctx, &mut app);

        assert_eq!(dark_centre, light_centre);
        assert_eq!(dark_panels, light_panels);
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

    /// What was collapsed is collapsed again next session, on the page it was left on.
    #[test]
    fn the_layout_comes_back_as_it_was_left() {
        let mut store = Fake::default();
        let before = Shell {
            browser_open: false,
            inspector_open: true,
            dock_open: true,
            room_open: true,
            deps_open: false,
            tags_open: true,
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
        assert!(after.room_open && after.tags_open);
        assert!(!after.deps_open, "a shut inspector panel comes back shut");
        assert_eq!(after.page, Page::Log);
        assert!(after.omnibox.is_empty(), "a search is not a layout");
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
