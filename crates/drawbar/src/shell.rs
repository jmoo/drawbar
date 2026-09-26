//! The dock shell: the regions the window is divided into, which of them are collapsed,
//! and the menus, toolbar, and status bar around the center.
//!
//! Panels claim space in the order `DrawbarApp::update` adds them. Each of the three
//! open docks resizes by dragging the edge facing the center, between its minimum open
//! size and whatever leaves the center [`CENTER_WIDE`] by [`CENTER_TALL`]. A collapsed
//! rail is [`SHUT`] wide and does not resize. Each panel's contents pad themselves, so a
//! panel header can reach both edges.

use eframe::egui;
use nord_usb::ObjectClass;

use crate::app::{accent, bold, ui as ui_text, DrawbarApp, ThemeChoice};
use crate::browser::{new_menu, Act};
use crate::device::occupancy;
use crate::filter::Filter;
use crate::icon::{icon, sized, Glyph};
use crate::log::Level;
use crate::panel::{caps, chevron, dock_header, flat, strip, DOCK, GAP, GLYPH, PAD};
use crate::strings::folder;
use crate::tabs::Spot;

/// The title bar: the logo, the menus, the instrument, and the theme.
pub const TITLEBAR: f32 = 30.0;

/// The bar of actions under it.
pub const TOOLBAR: f32 = 32.0;

/// The status line at the bottom of the window.
pub const STATUS: f32 = 22.0;

/// The bottom dock's body height under its header, and its minimum open height.
pub const DOCK_BODY: f32 = 184.0;
const BODY_LEAST: f32 = 120.0;

/// The side docks' open and collapsed widths, and the minimum open width of either.
pub const BROWSER: f32 = 232.0;
pub const INSPECTOR: f32 = 244.0;
pub const SHUT: f32 = 30.0;
const SIDE_LEAST: f32 = 180.0;

/// The room the center keeps however far a dock is dragged. A dock's maximum size is
/// whatever leaves this much, so it is computed from the room left each frame and not
/// stored.
const CENTER_WIDE: f32 = 300.0;
const CENTER_TALL: f32 = 200.0;

/// The smallest screen the shell lays out in: the three docks at their minimum open
/// sizes, around a center that still keeps [`CENTER_WIDE`] by [`CENTER_TALL`].
///
/// A native window's minimum size keeps it above this. A browser tab can be any size, so
/// the web build shows [`too_small_notice`] instead of a shell that cannot fit.
pub const LEAST: egui::Vec2 = egui::vec2(
    SIDE_LEAST + CENTER_WIDE + SIDE_LEAST,
    TITLEBAR + TOOLBAR + STATUS + DOCK + BODY_LEAST + CENTER_TALL,
);

/// What that notice says, and what `index.html` says before the module has loaded.
const TOO_SMALL: &str = "drawbar needs a larger screen";
const TOO_SMALL_WHY: &str = "It is a desktop application: its panels need more room \
                             than a phone or a narrow window has. Open it on a larger \
                             screen, or make this window wider.";

/// The check beside a menu item, and the height of a control.
const CHECK: f32 = 12.0;
const BUTTON: f32 = 22.0;

/// The omnibox's minimum width. Any narrower and it cannot show a readable name.
const OMNIBOX: f32 = 300.0;

/// The omnibox's fixed widget id.
///
/// ⚠️ The controls before it come and go with the instrument. An id derived from its
/// position would change when one attaches, and the box would lose focus and the cursor
/// mid-word.
const SEARCH: &str = "omnibox";

/// The minimum width of a drop-down menu.
///
/// ⚠️ A menu sizes itself to its widest item, so without this its width depends on which
/// items are enabled, and the longest item, "Inspector panel" with ⌥⌘I, puts its key
/// text right against its label. This fits that item with a gap between the two.
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
    /// The two panels under the right dock's INSTRUMENT header, each collapsed
    /// separately. SELECTION cannot collapse.
    pub room_open: bool,
    pub info_open: bool,
    /// Each dock's last dragged size: a side dock's width, and the bottom dock's body
    /// height under its [`DOCK`] header.
    pub browser_width: f32,
    pub inspector_width: f32,
    pub dock_body: f32,
    pub page: Page,
    /// The text in the omnibox, which filters the library's table by name.
    pub omnibox: String,
    /// The library's other filters, set by the tree's kind, tag, and place rows.
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

    /// Open the bottom dock on a page. Asking for a page always opens the dock.
    pub fn show_page(&mut self, page: Page) {
        self.dock_open = true;
        self.page = page;
    }

    /// Restore the docks as they were left.
    ///
    /// ⚠️ An unknown version is refused, not guessed at: half a layout is a window nobody
    /// arranged.
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

/// A size the last session left. A value this build would not have laid out (below the
/// dock's minimum, or not a number) is ignored and the default stands. egui clamps the
/// maximum to the screen every frame.
fn size(text: &str, least: f32, default: f32) -> f32 {
    match text.parse::<f32>() {
        Ok(size) if size.is_finite() && size >= least => size,
        _ => default,
    }
}

/// Which of a region's edges faces the center.
enum Side {
    Top,
    Bottom,
    Left,
    Right,
}

/// The 1 px border on a region's side facing the center, drawn just inside its rect.
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

/// A panel frame that paints its fill and has no margin.
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
/// Relative in a browser tab, so the guide comes from whichever host serves the app. A
/// native window has no page to be relative to and uses the published guide.
#[cfg(target_arch = "wasm32")]
pub(crate) const GUIDE: &str = "docs/";
#[cfg(not(target_arch = "wasm32"))]
pub(crate) const GUIDE: &str = "https://drawbar.app/docs/";

/// The key text beside a menu label, shown only in a native window.
fn keyed(ctx: &egui::Context, shortcut: egui::KeyboardShortcut) -> String {
    match WINDOWED {
        true => ctx.format_shortcut(&shortcut),
        false => String::new(),
    }
}

/// Whether an omnibox frame must bring the library forward.
///
/// Any change to the text does, including the first keystroke into an empty box. The box
/// filters only the library's table, so a search behind a document tab would be
/// invisible.
fn searched(before: &str, after: &str) -> bool {
    before != after
}

/// One of the title bar's drop-down menus, at least [`MENU`] wide.
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

/// The title bar's MIDI input status: a short label, the lamp beside it, and the details
/// on hover. `None` while MIDI is off.
///
/// ⚠️ A failure is only named here. Its message comes from the browser or the driver,
/// and it goes to the activity log.
fn midi_reading(
    state: &crate::midi::State,
    visuals: &egui::Visuals,
) -> Option<(String, egui::Color32, String)> {
    use crate::midi::State;

    match state {
        State::Off => None,
        State::Asking => Some((
            "MIDI".to_string(),
            crate::app::warn(visuals),
            "Asking this browser for access to MIDI controllers.".to_string(),
        )),
        State::Failed(_) => Some((
            "MIDI failed".to_string(),
            crate::app::bad(visuals),
            "drawbar could not listen to MIDI controllers. The activity log says why.".to_string(),
        )),
        State::On { ports, refused } => Some(listening(ports, refused, visuals)),
    }
}

/// What listening has found: the ports open, and the ones that would not open.
fn listening(
    ports: &[String],
    refused: &[String],
    visuals: &egui::Visuals,
) -> (String, egui::Color32, String) {
    let label = match ports {
        [] if refused.is_empty() => "No MIDI input".to_string(),
        [] => "MIDI input busy".to_string(),
        [one] => one.clone(),
        many => format!("{} MIDI inputs", many.len()),
    };
    let lamp = match (ports.is_empty(), refused.is_empty()) {
        (false, true) => crate::app::good(visuals),
        (true, _) | (false, false) => crate::app::warn(visuals),
    };
    let heard = match ports.is_empty() {
        true => "Listening, but no MIDI input is open. A controller is heard once plugged in."
            .to_string(),
        false => format!("Listening to {}.", ports.join(", ")),
    };
    let busy = match refused.is_empty() {
        true => String::new(),
        false => format!(
            " Could not open {}; another program may be using it.",
            refused.join(", ")
        ),
    };
    (label, lamp, format!("{heard}{busy}"))
}

/// What a click on one of the bottom dock's page titles does.
///
/// ⚠️ Clicking the title of the page already showing collapses the dock. The collapse
/// triangle is a small target, and a title that ignored a click would look broken.
fn page_click(page: Page, showing: bool) -> Act {
    match showing {
        true => Act::ToggleDock(Dock::Bottom),
        false => Act::ShowPage(page),
    }
}

/// A menu item with a check mark showing whether what it names is on.
///
/// ⚠️ A check at the left, not a selected button or a `selectable_label`: both fill the
/// row with `selection.bg_fill`, the instrument's red, which reads as a warning in a menu
/// of ordinary items. **Every** checkable menu item in the app uses this.
pub fn marked(
    ui: &mut egui::Ui,
    label: &str,
    on: bool,
    shortcut: Option<egui::KeyboardShortcut>,
) -> bool {
    let clicked = ui.add(check(ui, label, on, shortcut)).clicked();
    if clicked {
        ui.close();
    }
    clicked
}

/// The button behind [`marked`], for an item that is sometimes offered disabled.
fn check<'a>(
    ui: &egui::Ui,
    label: &'a str,
    on: bool,
    shortcut: Option<egui::KeyboardShortcut>,
) -> egui::Button<'a> {
    let tint = match on {
        true => accent(ui.visuals()),
        false => egui::Color32::TRANSPARENT,
    };
    let button = egui::Button::image_and_text(sized(Glyph::Check, CHECK, tint), label)
        .image_tint_follows_text_color(false);
    match shortcut {
        Some(shortcut) => button.shortcut_text(keyed(ui.ctx(), shortcut)),
        None => button,
    }
}

/// One of the toolbar's labeled actions.
///
/// ⚠️ An accented action shows the accent in its glyph only: accent on the panel color
/// measures 4.1:1 contrast, which fails for 11 px text.
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

/// The New menu behind a glyph button. The toolbar and the tab strip share this button,
/// so New offers one list from both places.
pub(crate) fn new_button(ui: &mut egui::Ui, glyph: Glyph, ink: egui::Color32, acts: &mut Vec<Act>) {
    ui.scope(|ui| {
        flat(ui);
        ui.menu_image_button(sized(glyph, GLYPH, ink), |ui| new_menu(ui, acts))
            .response
            .on_hover_text("something new on this computer");
    });
}

/// A 24 × 22 button with one glyph. `on` marks a toggle whose dock is open, the only
/// state that is filled without the pointer over it.
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
    /// ⚠️ The single gate on everything that needs an attached instrument: the Read and
    /// Send actions, the send queue, the inspector's INSTRUMENT group, and the menu items
    /// for them. A control for an absent instrument could only fail.
    pub(crate) fn attached(&self) -> bool {
        self.device.state.connected()
    }

    /// The title bar: the logo, the menus, the instrument, and the theme.
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
                        self.midi_chip(ui);
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
                crate::app::dot(ui, lit, 9.0).on_hover_text("attached");
            });
    }

    /// The MIDI controllers being listened to and whether they answer, with details on
    /// hover. Nothing while MIDI is off.
    fn midi_chip(&self, ui: &mut egui::Ui) {
        let Some((label, lamp, detail)) = midi_reading(&self.midi.state(), ui.visuals()) else {
            return;
        };
        let ink = ui.visuals().widgets.inactive.fg_stroke.color;
        egui::Frame::new()
            .inner_margin(egui::Margin::symmetric(6, 2))
            .show(ui, |ui| {
                ui.spacing_mut().item_spacing.x = GAP;
                icon(ui, Glyph::Piano, GLYPH, ink);
                ui.label(egui::RichText::new(label).text_style(ui_text()).color(ink));
                crate::app::dot(ui, lamp, 9.0);
            })
            .response
            .on_hover_text(detail);
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

    /// Apply the theme, and store it for the next session.
    fn pick_theme(&mut self, ctx: &egui::Context, frame: &mut eframe::Frame, theme: ThemeChoice) {
        self.theme = theme;
        ctx.set_theme(theme.preference());
        // Written now; eframe's own persistence otherwise waits for another frame.
        if let Some(storage) = frame.storage_mut() {
            storage.set_string(ThemeChoice::KEY, theme.stored().to_string());
        }
    }

    /// Handle every key a menu item binds, whether or not a menu is open.
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
        // ⚠️ Consumed whether or not an instrument is attached, and before ⌘S below. egui
        // matches a shortcut's modifiers logically, so an unconsumed ⌘⇧S would go on to
        // match ⌘S and save instead of showing the queue.
        if hit(&key::QUEUE) && self.attached() {
            acts.push(Act::ShowPage(Page::Queue));
        }
        // ⚠️ Likewise: an unconsumed ⌘R reloads the browser tab this build runs in.
        if hit(&key::RESYNC) && self.attached() {
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
            if item(ui, "Welcome", None) {
                self.splash.open_welcome();
            }
            ui.separator();
            if item(ui, "Copy activity log", None) {
                acts.push(Act::CopyLog);
            }
            if item(ui, "About drawbar", None) {
                self.about = Some(crate::about::About::new(
                    &self.device.state,
                    &self.workspace,
                ));
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

    /// ⚠️ No Library item. The library is always open as the first tab, so an item for it
    /// would add nothing.
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

    /// The instrument items, then the MIDI controllers that play drawbar's audition.
    fn instrument_menu(&mut self, ui: &mut egui::Ui, acts: &mut Vec<Act>) {
        self.usb_items(ui, acts);
        ui.separator();
        self.midi_item(ui);
    }

    /// The item that turns listening to MIDI controllers on or off for the whole app.
    ///
    /// ⚠️ Listening starts inside the click handler. A browser tab may ask the user for
    /// MIDI access only while the click's user activation is live.
    fn midi_item(&mut self, ui: &mut egui::Ui) {
        let on = self.midi.on();
        let button = check(ui, "Listen to MIDI controllers", on, None);
        let picked = ui
            .add_enabled(crate::midi::supported(), button)
            .on_disabled_hover_text(crate::midi::UNSUPPORTED)
            .clicked();
        if !picked {
            return;
        }
        ui.close();
        match on {
            true => self.midi.stop(),
            false => self.midi.listen(ui.ctx()),
        }
    }

    /// ⚠️ Only Connect… until an instrument answers. Every other item here acts on an
    /// instrument, and the send queue always belongs to one.
    fn usb_items(&mut self, ui: &mut egui::Ui, acts: &mut Vec<Act>) {
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

    /// The class of the slot the open document came off, for the menu item that reads
    /// that folder again. A document that did not come off a slot has none.
    fn open_class(&self) -> Option<ObjectClass> {
        let id = self.tabs.active()?;
        let (class, _) = self.workspace.get(id)?.origin.slot()?;
        Some(class)
    }

    /// The toolbar: three groups of actions, the omnibox, and the three dock toggles.
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
                    new_button(ui, Glyph::FilePlus2, ink, acts);
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

    /// Read and Send, and the rule separating them from the omnibox. Both actions act on
    /// an instrument, so none of the three is drawn without one.
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
        // Offer to queue saved changes that a send would otherwise skip.
        let offer = crate::queue::offer(&self.workspace, &self.device.state, &self.queue);
        if let Some((label, hint)) = offer {
            if action(ui, Glyph::Plus, &label, false)
                .on_hover_text(hint)
                .clicked()
            {
                acts.push(Act::QueueChanged);
            }
        }
        rule(ui, 16.0);
    }

    /// The name search that filters the library's table.
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
                    // Center the text and the hint vertically in a box taller than the
                    // text.
                    .vertical_align(egui::Align::Center)
                    .hint_text(egui::RichText::new("Search…").text_style(ui_text())),
            );
        });
        if searched(&before, &self.shell.omnibox) {
            acts.push(Act::ShowTab(Spot::Library));
        }
    }

    /// The status bar: what just happened, and how much room is left on the instrument.
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
        let most = (ctx.available_rect().height() - CENTER_TALL).max(DOCK + BODY_LEAST);
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

    /// The bottom dock's header: [`crate::panel::dock_header`]'s geometry, with two page
    /// titles to pick from.
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
                let on = self.shell.dock_open && self.page() == page;
                if ui
                    .selectable_label(on, caps(page.title()).color(ink))
                    .clicked()
                {
                    picked = Some(page_click(page, on));
                }
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

    /// Which page the bottom dock shows. With no instrument attached there is no queue,
    /// so the log is the only page.
    fn page(&self) -> Page {
        match self.attached() {
            true => self.shell.page,
            false => Page::Log,
        }
    }

    /// The send queue page.
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
            (ctx.available_rect().width() - self.inspector_room() - CENTER_WIDE).max(SIDE_LEAST);
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

    /// The width the inspector will take. The browser's maximum is computed before the
    /// inspector is added, so it must be asked for here.
    fn inspector_room(&self) -> f32 {
        match self.shell.inspector_open {
            true => self.shell.inspector_width,
            false => SHUT,
        }
    }

    /// The inspector dock: the selection, and the instrument while one is attached.
    pub(crate) fn inspector_dock(&mut self, ctx: &egui::Context, acts: &mut Vec<Act>) {
        let open = self.shell.inspector_open;
        let fill = ctx.style().visuals.panel_fill;
        let shut = egui::SidePanel::right("inspector_shut")
            .resizable(false)
            .exact_width(SHUT)
            .frame(bare(fill));
        let most = (ctx.available_rect().width() - CENTER_WIDE).max(SIDE_LEAST);
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

/// Claim the whole panel being drawn.
///
/// ⚠️ egui remembers a resizable panel's size as the size of its contents, so a dock
/// holding less than it shows would shrink to its minimum.
fn claim(ui: &mut egui::Ui) {
    ui.set_min_size(ui.max_rect().size());
}

/// Where a dock ended up this frame. A collapsed dock draws under another id, so this
/// returns the size it will reopen at.
fn laid_out(ctx: &egui::Context, id: &str) -> Option<egui::Rect> {
    egui::containers::panel::PanelState::load(ctx, egui::Id::new(id)).map(|state| state.rect)
}

/// A collapsed side dock: one glyph wide, with the glyph that reopens it.
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

/// Whether a screen this size leaves the shell too little room to lay out in.
pub fn too_small(screen: egui::Vec2) -> bool {
    screen.x < LEAST.x || screen.y < LEAST.y
}

/// What a screen too small for the shell shows instead of it.
pub fn too_small_notice(ctx: &egui::Context) {
    let fill = ctx.style().visuals.panel_fill;
    egui::CentralPanel::default()
        .frame(
            egui::Frame::new()
                .fill(fill)
                .inner_margin(egui::Margin::symmetric(16, 0)),
        )
        .show(ctx, |ui| {
            ui.vertical_centered(|ui| {
                ui.add_space(ui.available_height() / 3.0);
                ui.label(egui::RichText::new(TOO_SMALL).size(18.0).strong());
                ui.add_space(GAP);
                ui.label(TOO_SMALL_WHY);
                ui.add_space(GAP * 2.0);
                crate::sheet::link(ui, "User guide", GUIDE);
            });
        });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::Fake;
    use eframe::{App, Storage};

    /// The window size the design is drawn for.
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

    /// Attach an instrument, which the full layout needs.
    fn attach(app: &mut DrawbarApp) {
        app.device
            .pretend_scanned(ObjectClass::Program, 1, &["Africa Split"]);
    }

    /// What one frame laid out and painted.
    struct Painted {
        center: egui::Rect,
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

    /// One frame at 900 × 540: the center's rect, every panel's rect, and the text
    /// painted.
    fn drawn(ctx: &egui::Context, app: &mut DrawbarApp) -> Painted {
        drawn_at(ctx, app, SCREEN)
    }

    /// One frame on a screen of some other size.
    fn drawn_at(ctx: &egui::Context, app: &mut DrawbarApp, screen: egui::Vec2) -> Painted {
        frame_of(ctx, app, screen, Vec::new())
    }

    /// One frame with something arriving in it.
    fn frame_of(
        ctx: &egui::Context,
        app: &mut DrawbarApp,
        screen: egui::Vec2,
        events: Vec<egui::Event>,
    ) -> Painted {
        let mut frame = eframe::Frame::_new_kittest();
        let input = egui::RawInput {
            events,
            screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, screen)),
            ..Default::default()
        };
        let mut center = egui::Rect::NOTHING;
        let output = ctx.run(input, |ctx| {
            app.update(ctx, &mut frame);
            // Panels shrink this as they are added; the central panel does not.
            center = ctx.available_rect();
        });
        let panels = REGIONS
            .iter()
            .filter_map(|id| {
                let state = egui::containers::panel::PanelState::load(ctx, egui::Id::new(*id))?;
                Some((id.to_string(), state.rect))
            })
            .collect();
        Painted {
            center,
            panels,
            words: crate::tabs::words(&output),
        }
    }

    /// The gate depends only on the screen size and the layout metrics: collapsed docks
    /// and the kind of device do not make room the shell does not have.
    #[test]
    fn a_screen_short_of_the_least_room_is_gated_in_either_dimension_alone() {
        assert!(
            !too_small(LEAST),
            "the smallest screen the shell lays out in"
        );
        assert!(!too_small(SCREEN), "the window the design is drawn for");
        assert!(
            too_small(LEAST - egui::vec2(1.0, 0.0)),
            "a point too narrow"
        );
        assert!(too_small(LEAST - egui::vec2(0.0, 1.0)), "a point too short");
        // A phone in either orientation, and an empty canvas.
        assert!(too_small(egui::vec2(390.0, 844.0)));
        assert!(too_small(egui::vec2(844.0, 390.0)));
        assert!(too_small(egui::Vec2::ZERO));
    }

    /// At [`LEAST`], the three docks and the bars fit, and the center keeps the room no
    /// dock may take from it.
    #[test]
    fn at_the_least_room_it_claims_every_region_fits_and_the_center_keeps_its_own() {
        let ctx = egui::Context::default();
        let mut app = app(&ctx, None);
        app.shell.dock_open = true;
        attach(&mut app);
        let _ = drawn_at(&ctx, &mut app, LEAST);
        let painted = drawn_at(&ctx, &mut app, LEAST);

        let screen = egui::Rect::from_min_size(egui::Pos2::ZERO, LEAST);
        assert_eq!(painted.panels.len(), REGIONS.len(), "every region drew");
        for (id, rect) in &painted.panels {
            assert!(
                screen.contains_rect(*rect),
                "{id} is outside the window: {rect:?}"
            );
        }
        let center = painted.center;
        assert!(center.width() >= CENTER_WIDE, "the center: {center:?}");
        assert!(center.height() >= CENTER_TALL, "the center: {center:?}");
    }

    /// ⚠️ The page must turn a phone away before the module has loaded, so `index.html`
    /// repeats the threshold and the words. The shell's layout points are CSS pixels.
    #[test]
    fn the_page_gates_where_the_shell_does_and_says_the_same_thing() {
        let page = include_str!("../index.html");
        let gate = format!("@media (width < {}px), (height < {}px)", LEAST.x, LEAST.y);
        assert!(page.contains(&gate), "index.html does not gate at `{gate}`");
        assert!(page.contains(TOO_SMALL), "index.html: {TOO_SMALL:?}");
        assert!(
            page.contains(TOO_SMALL_WHY),
            "index.html: {TOO_SMALL_WHY:?}"
        );
    }

    #[test]
    fn the_favicon_is_the_titlebar_mark_in_both_accents() {
        let favicon = include_str!("../favicon.svg");
        let mark = include_str!("../assets/icons/sliders-vertical.svg");
        for line in mark.lines().filter(|l| l.trim_start().starts_with("<line")) {
            assert!(
                favicon.contains(line),
                "favicon.svg lacks `{}`",
                line.trim()
            );
        }
        let hex = |visuals: egui::Visuals| {
            let [r, g, b, _] = accent(&visuals).to_array();
            format!("#{r:02x}{g:02x}{b:02x}")
        };
        let light = format!("stroke=\"{}\"", hex(egui::Visuals::light()));
        let dark = format!("stroke: {};", hex(egui::Visuals::dark()));
        assert!(favicon.contains(&light), "favicon.svg lacks `{light}`");
        assert!(favicon.contains(&dark), "favicon.svg lacks `{dark}`");
    }

    /// A gated frame draws only the notice: what is wrong, and a link to the guide.
    #[test]
    fn the_notice_says_what_is_wrong_and_offers_the_guide() {
        let ctx = egui::Context::default();
        let input = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(390.0, 844.0),
            )),
            ..Default::default()
        };
        let output = ctx.run(input, too_small_notice);
        let said = crate::tabs::words(&output);

        assert!(said.iter().any(|word| word == TOO_SMALL), "{said:?}");
        assert!(said.iter().any(|word| word == TOO_SMALL_WHY), "{said:?}");
        assert!(said.iter().any(|word| word == "User guide"), "{said:?}");
    }

    /// Every fixed region fits inside the window the design is drawn for, and the center
    /// still has room left after all of them have taken theirs.
    #[test]
    fn at_900_by_540_every_region_fits_and_the_center_survives() {
        let ctx = egui::Context::default();
        let mut app = app(&ctx, None);
        app.shell.dock_open = true;
        attach(&mut app);
        // Twice: the second frame lays out against the first.
        let _ = drawn(&ctx, &mut app);
        let painted = drawn(&ctx, &mut app);
        let (center, panels) = (painted.center, &painted.panels);

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
        // The dock's header is its top [`DOCK`] points, inside the window.
        let header = at("dock").split_top_bottom_at_y(at("dock").top() + DOCK).0;
        assert!(screen.contains_rect(header), "the dock header: {header:?}");

        assert!(center.width() > 0.0, "the center: {center:?}");
        assert!(center.height() > 0.0, "the center: {center:?}");
    }

    /// Switching the theme changes only colors: the metrics live on the style both themes
    /// share, so every region stays in place.
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

        assert_eq!(dark.center, light.center);
        assert_eq!(dark.panels, light.panels);
    }

    /// The toolbar's Queue button counts the changed set, and is not drawn when nothing
    /// has changed.
    #[test]
    fn the_queue_button_offers_what_a_send_would_skip() {
        use nord_usb::Location;

        let class = ObjectClass::Program;
        let at = Location { bank: 6, slot: 0 };
        let ctx = egui::Context::default();
        let mut app = app(&ctx, None);
        attach(&mut app);

        let fresh = app
            .workspace
            .create(crate::workspace::Fresh::Program, &mut app.log)
            .unwrap();
        let bytes = app.workspace.get(fresh).unwrap().bytes.clone();
        app.workspace.remove(fresh, &mut app.log);
        let id = app.workspace.ingest(
            "Africa-Split.ne5p".into(),
            crate::workspace::Origin::Device { class, at },
            bytes.clone(),
            &mut app.log,
        );
        let held = app.workspace.get(id).unwrap().saved.crc32.unwrap();
        app.device
            .pretend_bodies(class, 7, &[Some(("Africa Split", held))]);
        app.device.relink(&mut app.workspace);

        let _ = drawn(&ctx, &mut app);
        assert!(
            !drawn(&ctx, &mut app).wrote("Queue 1"),
            "the slot holds what this is saved as"
        );

        // Saved on this computer and nowhere else: the slot holds the older body.
        let (_, edited) =
            crate::fields::apply(&bytes, &[("center_panel.gain".into(), "96".into())]).unwrap();
        app.workspace.replace_bytes(id, edited, &mut app.log);
        app.workspace.mark_saved(id);
        app.device.relink(&mut app.workspace);

        assert_eq!(
            crate::queue::changed(&app.workspace, &app.device.state, &app.queue).len(),
            1
        );
        let _ = drawn(&ctx, &mut app);
        assert!(drawn(&ctx, &mut app).wrote("Queue 1"), "one to offer");

        crate::queue::queue_changed(
            &app.workspace,
            &mut app.device,
            &mut app.queue,
            &mut app.log,
        );
        let _ = drawn(&ctx, &mut app);
        let painted = drawn(&ctx, &mut app);
        assert!(!painted.wrote("Queue 1"), "nothing is left to offer");
        assert!(painted.wrote("Send 1"), "and the change is waiting");
    }

    /// ⚠️ With nothing attached there is nothing to read, nothing to send to, and no room
    /// to report. Every control that acts on an instrument, including the inspector's
    /// INSTRUMENT group, is hidden, not disabled. SELECTION, which is about what is selected
    /// here, stays either way.
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
        assert!(alone.wrote("ACTIVITY LOG"), "the log is the only page left");
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

    /// ⚠️ The omnibox filters only the library's table, so typing into it with a document
    /// in front must bring the library forward.
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
        let _ = frame_of(
            &ctx,
            &mut app,
            SCREEN,
            vec![egui::Event::Text("afr".into())],
        );
        assert_eq!(app.shell.omnibox, "afr");
        assert_eq!(app.tabs.showing(), Some(Spot::Library));

        // The next frame types nothing and leaves the tab where the user put it.
        app.tabs.show(Spot::Document(id));
        let _ = drawn(&ctx, &mut app);
        assert_eq!(app.tabs.showing(), Some(Spot::Document(id)));
    }

    /// ⚠️ egui matches a shortcut's modifiers logically, so an extra Shift is ignored and
    /// an unconsumed ⌘⇧S goes on to match ⌘S. Asking to review the send queue would then
    /// mark the open document saved and lose its revert.
    #[test]
    fn the_send_queue_shortcut_never_falls_through_to_save() {
        let ctx = egui::Context::default();
        let mut app = app(&ctx, None);
        let id = app
            .workspace
            .create(crate::workspace::Fresh::Program, &mut app.log)
            .unwrap();
        let bytes = app.workspace.get(id).unwrap().bytes.clone();
        let (_, edited) =
            crate::fields::apply(&bytes, &[("center_panel.gain".into(), "96".into())]).unwrap();
        app.workspace.replace_bytes(id, edited, &mut app.log);
        app.tabs.open(id);
        assert!(app.workspace.get(id).unwrap().is_unsaved());

        let pressed = || egui::Event::Key {
            key: egui::Key::S,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: egui::Modifiers::COMMAND.plus(egui::Modifiers::SHIFT),
        };
        let _ = drawn(&ctx, &mut app);
        let _ = frame_of(&ctx, &mut app, SCREEN, vec![pressed()]);
        assert!(!app.attached(), "nothing was attached");
        assert!(
            app.workspace.get(id).unwrap().is_unsaved(),
            "there is no queue to review, and there is no save either"
        );

        attach(&mut app);
        let _ = drawn(&ctx, &mut app);
        let _ = frame_of(&ctx, &mut app, SCREEN, vec![pressed()]);
        assert!(app.shell.dock_open && app.shell.page == Page::Queue);
        assert!(
            app.workspace.get(id).unwrap().is_unsaved(),
            "reviewing the queue is not saving"
        );
    }

    /// ⚠️ ⌘R is the browser tab's reload. Left unconsumed, it reloads the page out from
    /// under whatever is open, so it is consumed whether or not an instrument is attached.
    #[test]
    fn the_read_everything_shortcut_is_taken_with_nothing_attached() {
        let ctx = egui::Context::default();
        let mut app = app(&ctx, None);
        let pressed = || egui::Event::Key {
            key: egui::Key::R,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: egui::Modifiers::COMMAND,
        };
        let reload = |event: &egui::Event| match event {
            egui::Event::Key { key, .. } => *key == egui::Key::R,
            _ => false,
        };
        let left = |ctx: &egui::Context| ctx.input(|input| input.events.iter().any(reload));

        let _ = drawn(&ctx, &mut app);
        let _ = frame_of(&ctx, &mut app, SCREEN, vec![pressed()]);
        assert!(!app.attached(), "nothing was attached");
        assert!(!left(&ctx), "⌘R reached the tab with nothing attached");

        attach(&mut app);
        let _ = drawn(&ctx, &mut app);
        let _ = frame_of(&ctx, &mut app, SCREEN, vec![pressed()]);
        assert!(!left(&ctx), "⌘R reached the tab with one attached");
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
        // Asking for a page opens the dock too.
        shell.dock_open = false;
        shell.show_page(Page::Log);
        assert!(shell.dock_open && shell.page == Page::Log);
    }

    /// ⚠️ Clicking the title of the page already showing collapses the dock. A title that
    /// ignored a click would look broken, and the collapse triangle is only 8 px of the header.
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
        assert!(
            !after.room_open,
            "a collapsed inspector panel comes back collapsed"
        );
        assert!(after.info_open, "and an open one comes back open");
        assert_eq!(after.browser_width, 301.0);
        assert_eq!(after.inspector_width, 199.0);
        assert_eq!(after.dock_body, 260.0);
        assert_eq!(after.page, Page::Log);
        assert!(after.omnibox.is_empty(), "a search is not a layout");
    }

    /// A stored size this build would never have laid out is ignored, so the dock opens
    /// at its default size, not as a sliver or at a nonsense size.
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
        assert_eq!(shell.browser_width, BROWSER, "below its minimum");
        assert_eq!(shell.inspector_width, INSPECTOR, "not a number");
        assert_eq!(shell.dock_body, DOCK_BODY, "not a size");
    }

    /// However far a dock was dragged last session, the center keeps its room: a dock's
    /// maximum is computed from the window every frame.
    #[test]
    fn docks_wider_than_the_window_still_leave_the_center_its_room() {
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
            painted.center.width() >= CENTER_WIDE,
            "the center: {:?}",
            painted.center
        );
        assert!(
            painted.center.height() >= CENTER_TALL,
            "the center: {:?}",
            painted.center
        );
        assert!(app.shell.browser_width >= SIDE_LEAST);
        assert!(app.shell.dock_body >= BODY_LEAST);
    }

    /// A dock keeps the size it was left at across frames, so the stored size is what
    /// the window showed, not the design's default.
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

    /// An unknown version is not read, so the defaults stand.
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

    #[test]
    fn the_title_bar_names_the_controllers_heard_and_the_ports_that_would_not_open() {
        let visuals = egui::Visuals::dark();
        let names = |names: &[&str]| {
            names
                .iter()
                .map(|name| name.to_string())
                .collect::<Vec<_>>()
        };
        assert!(
            midi_reading(&crate::midi::State::Off, &visuals).is_none(),
            "nothing while MIDI is off"
        );

        let (label, lamp, detail) = listening(&names(&["Launchkey"]), &[], &visuals);
        assert_eq!(
            (label.as_str(), detail.as_str()),
            ("Launchkey", "Listening to Launchkey.")
        );
        assert_eq!(lamp, crate::app::good(&visuals));

        let (label, lamp, detail) = listening(
            &names(&["Launchkey", "Pads"]),
            &names(&["Keystation"]),
            &visuals,
        );
        assert_eq!(label, "2 MIDI inputs");
        assert_eq!(
            detail,
            "Listening to Launchkey, Pads. Could not open Keystation; another program may \
             be using it."
        );
        assert_eq!(lamp, crate::app::warn(&visuals));

        let (label, lamp, _) = listening(&[], &[], &visuals);
        assert_eq!(label, "No MIDI input");
        assert_eq!(lamp, crate::app::warn(&visuals));
    }

    #[test]
    fn a_failure_to_listen_is_named_and_its_cause_left_to_the_log() {
        let visuals = egui::Visuals::dark();
        let raw = "TypeError: getObject(arg0).requestMIDIAccess is not a function";
        let (label, lamp, detail) =
            midi_reading(&crate::midi::State::Failed(raw.to_string()), &visuals)
                .expect("a failure is shown");
        assert_eq!(label, "MIDI failed");
        assert_eq!(lamp, crate::app::bad(&visuals));
        assert!(!detail.contains("TypeError"), "{detail}");
    }
}
