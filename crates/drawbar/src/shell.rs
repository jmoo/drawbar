//! The shell: the top bar, the status line, and the cards between them.
//!
//! The window is the canvas. The top bar and the status line sit on it with no fill of
//! their own, and the browser, the document, and the inspector are rounded cards on it,
//! [`GUTTER`] apart and [`GUTTER`] in from the window's edges. A hidden side panel takes
//! no room at all. Each side card resizes by dragging its edge facing the center, between
//! its minimum width and whatever leaves the center [`CENTER_WIDE`] wide.
//!
//! The top bar is the one place the platforms differ; see [`crate::platform`].

use eframe::egui;
use nord_usb::ObjectClass;

use crate::app::{accent, bold, canvas, good, tint, ui as ui_text, warn, DrawbarApp, ThemeChoice};
use crate::browser::{new_menu, Act};
use crate::filter::Filter;
use crate::icon::{icon, painted, sized, Glyph};
use crate::log::Level;
use crate::menu::{key_text, search_key, Command};
use crate::panel::{flat, CARD_RADIUS, GUTTER};
use crate::platform::{Frame, Platform, CRAMPED};
use crate::strings::folder;
use crate::tabs::Spot;

/// The top bar: the platform's edges, the file tools, the search, and the instrument.
pub const TOPBAR: f32 = 46.0;

/// The status line at the bottom of the window.
pub const STATUS: f32 = 30.0;

/// The side cards' default widths, and the minimum width of either.
pub const BROWSER: f32 = 244.0;
pub const INSPECTOR: f32 = 268.0;
const SIDE_LEAST: f32 = 180.0;

/// The room the center keeps however far a side card is dragged. A card's maximum width
/// is whatever leaves this much, so it is computed from the room left each frame and not
/// stored.
const CENTER_WIDE: f32 = 300.0;
const CENTER_TALL: f32 = 240.0;

/// The smallest screen the shell lays out in: both side cards at their minimum widths,
/// around a center that still keeps [`CENTER_WIDE`] by [`CENTER_TALL`] under its tabs.
///
/// A native window's minimum size keeps it above this. A browser tab can be any size, so
/// the web build shows [`too_small_notice`] instead of a shell that cannot fit.
pub const LEAST: egui::Vec2 = egui::vec2(
    SIDE_LEAST + CENTER_WIDE + SIDE_LEAST + 4.0 * GUTTER,
    TOPBAR + STATUS + crate::tabs::HEIGHT + CENTER_TALL + GUTTER,
);

/// What that notice says, and what `index.html` says before the module has loaded.
const TOO_SMALL: &str = "drawbar needs a larger screen";
const TOO_SMALL_WHY: &str = "It is a desktop application: its panels need more room \
                             than a phone or a narrow window has. Open it on a larger \
                             screen, or make this window wider.";

/// The search box: its height, its narrowest and widest, and the space between it and
/// the bar's sides.
const SEARCH_TALL: f32 = 32.0;
const SEARCH_LEAST: f32 = 64.0;
const SEARCH_MOST: f32 = 420.0;
const SIDES_GAP: f32 = 16.0;

/// The search box's fixed widget id.
///
/// ⚠️ The controls before it come and go with the instrument. An id derived from its
/// position would change when one attaches, and the box would lose focus and the cursor
/// mid-word.
const SEARCH: &str = "omnibox";

/// The height of a chip or a button in the top bar, and the size of its glyph.
const CHIP: f32 = 30.0;
const BAR_GLYPH: f32 = 16.0;

/// The room the Mac's traffic lights take at the left of the bar.
const LIGHTS: f32 = 78.0;

/// The bar's padding at each end, where nothing of the platform's sits.
const BAR_PAD: f32 = 10.0;

/// The status line's padding at each end.
const STATUS_PAD: f32 = 16.0;

/// The height of the row of file tools at the top of the browser card on Windows.
const TOOLS_ROW: f32 = 40.0;

/// Which side panel a toggle is about.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Dock {
    Browser,
    Inspector,
}

/// What is open, and what the search box holds.
pub struct Shell {
    pub browser_open: bool,
    pub inspector_open: bool,
    /// The inspector's Room and Info cards, each collapsed separately.
    pub room_open: bool,
    pub info_open: bool,
    /// Each side panel's last dragged width, its gutter included.
    pub browser_width: f32,
    pub inspector_width: f32,
    /// The text in the search box, which filters the library's table by name.
    pub omnibox: String,
    /// The library's other filters, set by the tree's kind, tag, and place rows.
    pub filter: Filter,
    /// The activity popover over the status line, and whether it shows only problems.
    pub log_open: bool,
    pub log_problems: bool,
    /// The send queue's review, over the whole window.
    pub review_open: bool,
    /// Where the status line was last drawn: a click there toggles the popover, so the
    /// popover does not count it as a click outside itself.
    pub status_rect: egui::Rect,
}

impl Default for Shell {
    fn default() -> Shell {
        Shell {
            browser_open: true,
            inspector_open: true,
            room_open: true,
            info_open: false,
            browser_width: BROWSER + GUTTER,
            inspector_width: INSPECTOR + GUTTER,
            omnibox: String::new(),
            filter: Filter::default(),
            log_open: false,
            log_problems: false,
            review_open: false,
            status_rect: egui::Rect::NOTHING,
        }
    }
}

impl Shell {
    /// Where the layout is kept between sessions, beside the browser's own keys.
    pub const KEY: &'static str = "drawbar.docks";

    const VERSION: &'static str = "drawbar docks 6";

    pub fn open(&self, dock: Dock) -> bool {
        match dock {
            Dock::Browser => self.browser_open,
            Dock::Inspector => self.inspector_open,
        }
    }

    pub fn toggle(&mut self, dock: Dock) {
        match dock {
            Dock::Browser => self.browser_open = !self.browser_open,
            Dock::Inspector => self.inspector_open = !self.inspector_open,
        }
    }

    /// Restore the panels as they were left.
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
                (Some("room"), Some(open)) => held.room_open = open == "1",
                (Some("info"), Some(open)) => held.info_open = open == "1",
                (Some("browser_width"), Some(text)) => {
                    held.browser_width = size(text, SIDE_LEAST, BROWSER + GUTTER)
                }
                (Some("inspector_width"), Some(text)) => {
                    held.inspector_width = size(text, SIDE_LEAST, INSPECTOR + GUTTER)
                }
                _ => {}
            }
        }
        *self = Shell {
            omnibox: std::mem::take(&mut self.omnibox),
            filter: std::mem::take(&mut self.filter),
            ..held
        };
    }

    pub fn keep(&self, storage: &mut dyn eframe::Storage) {
        let bit = |open: bool| match open {
            true => "1",
            false => "0",
        };
        storage.set_string(
            Shell::KEY,
            format!(
                "{}\nbrowser\t{}\ninspector\t{}\nroom\t{}\ninfo\t{}\n\
                 browser_width\t{}\ninspector_width\t{}\n",
                Shell::VERSION,
                bit(self.browser_open),
                bit(self.inspector_open),
                bit(self.room_open),
                bit(self.info_open),
                self.browser_width,
                self.inspector_width,
            ),
        );
    }
}

/// A size the last session left. A value this build would not have laid out (below the
/// panel's minimum, or not a number) is ignored and the default stands. egui clamps the
/// maximum to the screen every frame.
fn size(text: &str, least: f32, default: f32) -> f32 {
    match text.parse::<f32>() {
        Ok(size) if size.is_finite() && size >= least => size,
        _ => default,
    }
}

/// The user guide, published beside the browser build.
///
/// Relative in a browser tab, so the guide comes from whichever host serves the app. A
/// native window has no page to be relative to and uses the published guide.
#[cfg(target_arch = "wasm32")]
pub(crate) const GUIDE: &str = "docs/";
#[cfg(not(target_arch = "wasm32"))]
pub(crate) const GUIDE: &str = "https://drawbar.app/docs/";

/// A rounded card on the canvas: `rect` filled with the panel color, and a child `Ui`
/// clipped to it for its contents.
pub(crate) fn card(ui: &mut egui::Ui, rect: egui::Rect) -> egui::Ui {
    ui.painter()
        .rect_filled(rect, CARD_RADIUS, ui.visuals().panel_fill);
    let mut inside = ui.new_child(
        egui::UiBuilder::new()
            .max_rect(rect)
            .layout(egui::Layout::top_down(egui::Align::Min)),
    );
    inside.set_clip_rect(rect.intersect(ui.clip_rect()));
    inside
}

/// Round off whatever a card's contents painted into its corners, by painting the canvas
/// back over them. Call after the contents, with the card's own `rect`.
///
/// ⚠️ A clip rect is square, so a header or a selected row that fills its card's width
/// would otherwise show square corners on a round card.
pub(crate) fn round_off(ui: &egui::Ui, rect: egui::Rect) {
    let reach = f32::from(CARD_RADIUS);
    let painter = ui.painter().with_clip_rect(rect);
    painter.rect_stroke(
        rect.expand(reach / 2.0),
        CARD_RADIUS + CARD_RADIUS / 2,
        egui::Stroke::new(reach, canvas(ui.visuals())),
        egui::StrokeKind::Middle,
    );
}

/// A glyph button in a bar: no fill until the pointer is on it.
fn glyph_button(
    ui: &mut egui::Ui,
    glyph: Glyph,
    size: f32,
    box_: f32,
    hint: &str,
) -> egui::Response {
    ui.scope(|ui| {
        flat(ui);
        let ink = crate::app::caption(ui.visuals());
        ui.add(
            egui::Button::image(sized(glyph, size, ink))
                .image_tint_follows_text_color(false)
                .corner_radius(7.0)
                .min_size(egui::Vec2::splat(box_)),
        )
    })
    .inner
    .on_hover_text(hint)
}

/// The New menu behind a glyph button. The top bar and the tab row share this button,
/// so New offers one list from both places.
pub(crate) fn new_button(
    ui: &mut egui::Ui,
    glyph: Glyph,
    size: f32,
    box_: f32,
    acts: &mut Vec<Act>,
) {
    ui.scope(|ui| {
        flat(ui);
        let ink = crate::app::caption(ui.visuals());
        let button = egui::Button::image(sized(glyph, size, ink))
            .image_tint_follows_text_color(false)
            .corner_radius(7.0)
            .min_size(egui::Vec2::splat(box_));
        egui::containers::menu::MenuButton::from_button(button)
            .ui(ui, |ui| new_menu(ui, acts))
            .0
            .on_hover_text("something new on this computer");
    });
}

/// A pill in the top bar: a glyph, an optional label, and an optional lamp, on `fill`
/// with an optional `border`. The response senses clicks.
struct Pill<'a> {
    glyph: Glyph,
    mark: egui::Color32,
    label: Option<&'a str>,
    ink: egui::Color32,
    fill: egui::Color32,
    border: Option<egui::Stroke>,
    dashed: bool,
    lamp: Option<egui::Color32>,
    radius: f32,
}

impl Pill<'_> {
    fn show(self, ui: &mut egui::Ui) -> egui::Response {
        let font = egui::FontId::proportional(12.5);
        let galley = self.label.map(|label| {
            ui.painter()
                .layout_no_wrap(label.to_owned(), font, self.ink)
        });
        let pad = 10.0;
        let mut width = pad + BAR_GLYPH + pad;
        if let Some(galley) = &galley {
            width += 7.0 + galley.size().x;
        }
        if self.lamp.is_some() {
            width += 8.0 + 7.0;
        }
        let (rect, response) =
            ui.allocate_exact_size(egui::vec2(width, CHIP), egui::Sense::click());
        let painter = ui.painter();
        let fill = match response.hovered() {
            true => ui.visuals().widgets.hovered.weak_bg_fill,
            false => self.fill,
        };
        painter.rect_filled(rect, self.radius, fill);
        match (self.border, self.dashed) {
            (Some(stroke), false) => {
                painter.rect_stroke(rect, self.radius, stroke, egui::StrokeKind::Inside);
            }
            (Some(stroke), true) => crate::panel::dashed_rect(painter, rect.shrink(0.5), stroke),
            (None, _) => {}
        }
        let mut x = rect.left() + pad;
        painted(
            ui,
            self.glyph,
            egui::Rect::from_min_size(
                egui::pos2(x, rect.center().y - BAR_GLYPH / 2.0),
                egui::Vec2::splat(BAR_GLYPH),
            ),
            self.mark,
        );
        x += BAR_GLYPH;
        if let Some(galley) = galley {
            x += 7.0;
            let size = galley.size();
            ui.painter().galley(
                egui::pos2(x, rect.center().y - size.y / 2.0),
                galley,
                self.ink,
            );
            x += size.x;
        }
        if let Some(lamp) = self.lamp {
            let center = egui::pos2(x + 8.0 + 3.5, rect.center().y);
            ui.painter()
                .circle_filled(center, 3.5 + 3.0, tint(lamp, 0.22));
            ui.painter().circle_filled(center, 3.5, lamp);
        }
        response
    }
}

/// The MIDI controllers' reading: a short label, its signal, and the details on hover.
/// `None` while MIDI is off.
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
            warn(visuals),
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
        (false, true) => good(visuals),
        (true, _) | (false, false) => warn(visuals),
    };
    let heard = match ports.is_empty() {
        true => "No MIDI input is open. Plug in a controller to start playing.".to_string(),
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

/// Where the search box sits across a bar of `width`, given the room its two sides take:
/// centered on the bar while both sides fit beside it, and otherwise between them, never
/// narrower than [`SEARCH_LEAST`].
fn search_span(width: f32, left: f32, right: f32) -> std::ops::Range<f32> {
    let side = left.max(right);
    let centered = width - 2.0 * (side + SIDES_GAP);
    if centered >= SEARCH_LEAST {
        let wide = centered.min(SEARCH_MOST);
        let start = (width - wide) / 2.0;
        return start..start + wide;
    }
    let start = left + SIDES_GAP;
    let wide = (width - right - SIDES_GAP - start).clamp(SEARCH_LEAST, SEARCH_MOST);
    start..start + wide
}

impl DrawbarApp {
    /// Whether an instrument is answering.
    ///
    /// ⚠️ The single gate on everything that needs an attached instrument: Send, the send
    /// queue, the inspector's Room and Info, and the menu items for them. A control for an
    /// absent instrument could only fail.
    pub(crate) fn attached(&self) -> bool {
        self.device.state.connected()
    }

    /// Apply the theme, and store it for the next session.
    pub(crate) fn pick_theme(
        &mut self,
        ctx: &egui::Context,
        frame: &mut eframe::Frame,
        theme: ThemeChoice,
    ) {
        self.theme = theme;
        ctx.set_theme(theme.preference());
        // Written now; eframe's own persistence otherwise waits for another frame.
        if let Some(storage) = frame.storage_mut() {
            storage.set_string(ThemeChoice::KEY, theme.stored().to_string());
        }
    }

    /// The canvas the cards sit on, under everything else.
    pub(crate) fn backdrop(&self, ctx: &egui::Context) {
        let screen = ctx.screen_rect();
        ctx.layer_painter(egui::LayerId::background()).rect_filled(
            screen,
            0.0,
            canvas(&ctx.style().visuals),
        );
        if self.chrome.undecorated() {
            crate::platform::edges(ctx);
        }
    }

    /// The top bar, as this platform arranges it.
    pub(crate) fn top_bar(
        &mut self,
        ctx: &egui::Context,
        frame: &mut eframe::Frame,
        acts: &mut Vec<Act>,
    ) {
        egui::TopBottomPanel::top("topbar")
            .resizable(false)
            .exact_height(TOPBAR)
            .show_separator_line(false)
            .frame(egui::Frame::NONE)
            .show(ctx, |ui| {
                let bar = ui.max_rect();
                // First, so every control drawn after it takes the pointer over it.
                let empty = ui.interact(bar, ui.id().with("bar"), egui::Sense::click_and_drag());
                if self.platform.windowed() {
                    crate::platform::title_bar(ui.ctx(), &empty);
                }
                self.focus_search(ui.ctx());

                let mut left = ui.new_child(
                    egui::UiBuilder::new()
                        .max_rect(bar)
                        .layout(egui::Layout::left_to_right(egui::Align::Center)),
                );
                left.spacing_mut().item_spacing.x = 4.0;
                self.left_edge(&mut left, frame, acts);
                let mut right = ui.new_child(
                    egui::UiBuilder::new()
                        .max_rect(bar)
                        .layout(egui::Layout::right_to_left(egui::Align::Center)),
                );
                right.spacing_mut().item_spacing.x = 6.0;
                self.right_edge(&mut right, frame, acts);

                let span = search_span(
                    bar.width(),
                    left.min_rect().right() - bar.left(),
                    bar.right() - right.min_rect().left(),
                );
                let field = egui::Rect::from_min_size(
                    egui::pos2(bar.left() + span.start, bar.center().y - SEARCH_TALL / 2.0),
                    egui::vec2(span.end - span.start, SEARCH_TALL),
                );
                self.search(ui, field, acts);
            });
    }

    /// The bar's left end: what the platform puts there, then the file tools.
    fn left_edge(&mut self, ui: &mut egui::Ui, frame: &mut eframe::Frame, acts: &mut Vec<Act>) {
        match (&self.chrome, self.platform) {
            (_, Platform::Mac) => ui.add_space(LIGHTS),
            (_, Platform::Windows) => {
                ui.add_space(BAR_PAD);
                logo(ui);
                ui.add_space(4.0);
                self.menu_bar(ui, frame, acts);
            }
            (Frame::HeaderBar(layout), Platform::Linux) if !layout.left.is_empty() => {
                ui.add_space(BAR_PAD);
                let left = ui.cursor().left();
                let width = crate::platform::round_width(&layout.left);
                let middle = ui.max_rect().center().y;
                let buttons = layout.left.clone();
                crate::platform::round_buttons(ui, &buttons, left, middle);
                ui.add_space(width + 8.0);
            }
            (_, Platform::Linux) => ui.add_space(BAR_PAD),
            (_, Platform::Web) => {
                ui.add_space(BAR_PAD);
                logo(ui);
                ui.label(
                    egui::RichText::new("drawbar")
                        .font(egui::FontId::new(13.0, bold()))
                        .color(ui.visuals().widgets.active.fg_stroke.color),
                );
                ui.add_space(6.0);
            }
        }
        let roomy = ui.ctx().screen_rect().width() >= CRAMPED;
        if self.platform != Platform::Windows && roomy {
            self.file_tools(ui, acts);
        }
    }

    /// Open, New, and Save.
    fn file_tools(&mut self, ui: &mut egui::Ui, acts: &mut Vec<Act>) {
        let mac = crate::platform::mac_keyboard(ui.ctx());
        let hint = |label: &str, command| match key_text(command, self.platform, mac) {
            Some(keys) => format!("{label}  {keys}"),
            None => label.to_string(),
        };
        if glyph_button(
            ui,
            Glyph::FolderOpen,
            BAR_GLYPH,
            CHIP,
            &hint("Open files…", Command::Open),
        )
        .clicked()
        {
            acts.push(Act::OpenFiles);
        }
        new_button(ui, Glyph::FilePlus2, BAR_GLYPH, CHIP, acts);
        let open = self.tabs.active();
        if glyph_button(
            ui,
            Glyph::Save,
            BAR_GLYPH,
            CHIP,
            &hint("Save the open document", Command::Save),
        )
        .clicked()
        {
            acts.extend(open.map(Act::SaveDoc));
        }
    }

    /// The bar's right end, laid out from the edge inward: what the platform puts at the
    /// edge, the menu where the menus are one button, the theme, the instrument, the MIDI
    /// controllers, and Send.
    fn right_edge(&mut self, ui: &mut egui::Ui, frame: &mut eframe::Frame, acts: &mut Vec<Act>) {
        match &self.chrome {
            Frame::Captions => {
                let left = crate::platform::captions(ui, ui.max_rect());
                ui.add_space(ui.max_rect().right() - left + 4.0);
            }
            Frame::HeaderBar(layout) if !layout.right.is_empty() => {
                ui.add_space(BAR_PAD);
                let width = crate::platform::round_width(&layout.right);
                let left = ui.cursor().right() - width;
                let middle = ui.max_rect().center().y;
                let buttons = layout.right.clone();
                crate::platform::round_buttons(ui, &buttons, left, middle);
                ui.add_space(width + 8.0);
            }
            Frame::System | Frame::HeaderBar(_) => ui.add_space(BAR_PAD),
        }
        if self.platform.one_menu() {
            self.menu_button(ui, frame, acts);
        }
        let narrow = ui.ctx().screen_rect().width() < self.platform.narrow();
        self.theme_button(ui, frame);
        self.instrument_pill(ui, narrow, acts);
        self.midi_chip(ui, narrow);
        self.send_button(ui, narrow, acts);
    }

    /// The theme, as one glyph that cycles auto → light → dark.
    fn theme_button(&mut self, ui: &mut egui::Ui, frame: &mut eframe::Frame) {
        let glyph = match self.theme {
            ThemeChoice::System => Glyph::SunMoon,
            ThemeChoice::Light => Glyph::Sun,
            ThemeChoice::Dark => Glyph::Moon,
        };
        let face = match ui.visuals().dark_mode {
            true => "dark",
            false => "light",
        };
        let hint = match self.theme {
            ThemeChoice::System => format!("Theme: auto, {face} now — {}", self.theme.hint()),
            choice => format!(
                "Theme: {} — {}",
                choice.name().to_lowercase(),
                choice.hint()
            ),
        };
        if glyph_button(ui, glyph, BAR_GLYPH, CHIP, &hint).clicked() {
            self.pick_theme(ui.ctx(), frame, self.theme.next());
        }
    }

    /// The instrument as a pill with a lamp, or, with none attached, a dashed pill that
    /// connects one.
    fn instrument_pill(&self, ui: &mut egui::Ui, narrow: bool, acts: &mut Vec<Act>) {
        let visuals = ui.visuals().clone();
        let ink = visuals.widgets.inactive.fg_stroke.color;
        let label = |text: &str| (!narrow).then(|| text.to_string());
        match self.device.state.product() {
            Some(product) => {
                let label = label(product);
                Pill {
                    glyph: Glyph::Keyboard,
                    mark: good(&visuals),
                    label: label.as_deref(),
                    ink,
                    fill: visuals.panel_fill,
                    border: None,
                    dashed: false,
                    lamp: Some(good(&visuals)),
                    radius: CHIP / 2.0,
                }
                .show(ui)
                .on_hover_text(format!("{product}: attached"));
            }
            None => {
                let label = label("Connect instrument…");
                let connect = Pill {
                    glyph: Glyph::Plug,
                    mark: ink,
                    label: label.as_deref(),
                    ink,
                    fill: egui::Color32::TRANSPARENT,
                    border: Some(egui::Stroke::new(
                        1.0_f32,
                        visuals.widgets.hovered.bg_stroke.color,
                    )),
                    dashed: true,
                    lamp: None,
                    radius: CHIP / 2.0,
                }
                .show(ui)
                .on_hover_text("Find a Nord on USB and read what it holds");
                if connect.clicked() {
                    acts.push(Act::Connect);
                }
            }
        }
    }

    /// The MIDI controllers being listened to, with details on hover. Nothing while MIDI
    /// is off.
    fn midi_chip(&self, ui: &mut egui::Ui, narrow: bool) {
        let visuals = ui.visuals().clone();
        let Some((label, signal, detail)) = midi_reading(&self.midi.state(), &visuals) else {
            return;
        };
        let mark = match signal == good(&visuals) {
            true => visuals.widgets.inactive.fg_stroke.color,
            false => signal,
        };
        Pill {
            glyph: Glyph::Cable,
            mark,
            label: (!narrow).then_some(label.as_str()),
            ink: visuals.widgets.inactive.fg_stroke.color,
            fill: visuals.panel_fill,
            border: None,
            dashed: false,
            lamp: None,
            radius: CHIP / 2.0,
        }
        .show(ui)
        .on_hover_text(detail);
    }

    /// Send, which opens the review of what is waiting, with a warn dot while something
    /// waiting cannot go as it is. Before it, the offer to queue saved changes a send
    /// would otherwise skip. Neither is drawn without an instrument.
    fn send_button(&self, ui: &mut egui::Ui, narrow: bool, acts: &mut Vec<Act>) {
        if !self.attached() {
            return;
        }
        let visuals = ui.visuals().clone();
        let lit = accent(&visuals);
        let waiting = self.queue.len();
        let label = match waiting {
            0 => "Send".to_string(),
            n => format!("Send {n}"),
        };
        let troubled = self
            .queue
            .entries()
            .iter()
            .any(|held| held.failure.is_some());
        let mac = crate::platform::mac_keyboard(ui.ctx());
        let keys = key_text(Command::ReviewQueue, self.platform, mac).unwrap_or_default();
        let send = Pill {
            glyph: Glyph::Upload,
            mark: lit,
            label: (!narrow || waiting > 0).then_some(label.as_str()),
            ink: visuals.widgets.active.fg_stroke.color,
            fill: tint(lit, 0.16),
            border: Some(egui::Stroke::new(1.0_f32, tint(lit, 0.55))),
            dashed: false,
            lamp: troubled.then(|| warn(&visuals)),
            radius: 8.0,
        }
        .show(ui)
        .on_hover_text(format!("Review the send queue  {keys}"));
        if send.clicked() {
            acts.push(Act::ReviewQueue);
        }
        let offer = crate::queue::offer(&self.workspace, &self.device.state, &self.queue);
        if let Some((label, hint)) = offer {
            let queue = Pill {
                glyph: Glyph::Plus,
                mark: visuals.widgets.inactive.fg_stroke.color,
                label: Some(&label),
                ink: visuals.widgets.inactive.fg_stroke.color,
                fill: visuals.widgets.inactive.weak_bg_fill,
                border: None,
                dashed: false,
                lamp: None,
                radius: 8.0,
            }
            .show(ui)
            .on_hover_text(hint);
            if queue.clicked() {
                acts.push(Act::QueueChanged);
            }
        }
    }

    /// Put the cursor in the search box when its key is pressed.
    fn focus_search(&self, ctx: &egui::Context) {
        if ctx.input_mut(|input| input.consume_shortcut(&search_key())) {
            ctx.memory_mut(|memory| memory.request_focus(egui::Id::new(SEARCH)));
        }
    }

    /// The name search that filters the library's table, in `field`.
    fn search(&mut self, ui: &mut egui::Ui, field: egui::Rect, acts: &mut Vec<Act>) {
        let visuals = ui.visuals().clone();
        let hovered = ui.rect_contains_pointer(field);
        let border = match hovered {
            true => visuals.widgets.hovered.bg_stroke.color,
            false => visuals.widgets.noninteractive.bg_stroke.color,
        };
        ui.painter().rect(
            field,
            9.0,
            visuals.panel_fill,
            egui::Stroke::new(1.0_f32, border),
            egui::StrokeKind::Inside,
        );
        let quiet = crate::app::caption(&visuals);
        let glyph = egui::Rect::from_center_size(
            egui::pos2(field.left() + 12.0 + 7.0, field.center().y),
            egui::Vec2::splat(14.0),
        );
        painted(ui, Glyph::Search, glyph, quiet);

        let mac = crate::platform::mac_keyboard(ui.ctx());
        let keys = crate::platform::written(search_key(), mac);
        let galley = ui
            .painter()
            .layout_no_wrap(keys, egui::FontId::monospace(10.5), quiet);
        let badge = egui::Rect::from_min_size(
            egui::pos2(
                field.right() - 8.0 - galley.size().x - 10.0,
                field.center().y - 9.0,
            ),
            egui::vec2(galley.size().x + 10.0, 18.0),
        );
        let fits = badge.left() > glyph.right() + 60.0;
        if fits {
            ui.painter()
                .rect_filled(badge, 5.0, visuals.widgets.inactive.weak_bg_fill);
            ui.painter()
                .galley(badge.center() - galley.size() / 2.0, galley, quiet);
        }
        let text_right = match fits {
            true => badge.left() - 6.0,
            false => field.right() - 8.0,
        };
        let text = egui::Rect::from_min_max(
            egui::pos2(glyph.right() + 8.0, field.top()),
            egui::pos2(text_right, field.bottom()),
        );
        let before = self.shell.omnibox.clone();
        ui.put(
            text,
            egui::TextEdit::singleline(&mut self.shell.omnibox)
                .id(egui::Id::new(SEARCH))
                .frame(false)
                .vertical_align(egui::Align::Center)
                .font(ui_text())
                .hint_text(
                    egui::RichText::new("Search the library by name, or drop files here")
                        .text_style(ui_text()),
                ),
        );
        // The box filters only the library's table, so any change to the text, the first
        // keystroke included, brings the library forward.
        if before != self.shell.omnibox {
            acts.push(Act::ShowTab(Spot::Library));
        }
    }

    /// The status line: what just happened, whether anything went wrong, and how much
    /// room is left on the instrument. The first two open the activity popover.
    pub(crate) fn status_line(&mut self, ctx: &egui::Context, acts: &mut Vec<Act>) {
        egui::TopBottomPanel::bottom("status")
            .resizable(false)
            .exact_height(STATUS)
            .show_separator_line(false)
            .frame(egui::Frame::NONE)
            .show(ctx, |ui| {
                self.shell.status_rect = ui.max_rect();
                let mut row = ui.new_child(
                    egui::UiBuilder::new()
                        .max_rect(ui.max_rect().shrink2(egui::vec2(STATUS_PAD - 6.0, 0.0)))
                        .layout(egui::Layout::left_to_right(egui::Align::Center)),
                );
                row.spacing_mut().item_spacing.x = 6.0;
                self.said(&mut row, acts);
                let mut right = ui.new_child(
                    egui::UiBuilder::new()
                        .max_rect(ui.max_rect().shrink2(egui::vec2(STATUS_PAD, 0.0)))
                        .layout(egui::Layout::right_to_left(egui::Align::Center)),
                );
                self.room_figures(&mut right);
            });
    }

    /// The newest sentence, or what the instrument is doing, then the count of problems.
    fn said(&mut self, ui: &mut egui::Ui, acts: &mut Vec<Act>) {
        let visuals = ui.visuals().clone();
        let (glyph, tint_, text) = match &self.device.state.in_flight {
            Some(words) => (None, visuals.text_color(), words.doing.clone()),
            None => {
                let (level, text) = self.log.status();
                let glyph = match level {
                    Level::Info => Glyph::CircleCheck,
                    Level::Warn | Level::Error => Glyph::CircleAlert,
                };
                (Some(glyph), level.color(&visuals), text.to_string())
            }
        };
        let galley = ui
            .painter()
            .layout_no_wrap(text, egui::FontId::proportional(12.0), tint_);
        let width = (galley.size().x + 6.0 + 14.0 + 14.0).min(ui.available_width() * 0.7);
        let (rect, response) =
            ui.allocate_exact_size(egui::vec2(width, 22.0), egui::Sense::click());
        if response.hovered() {
            ui.painter()
                .rect_filled(rect, 5.0, visuals.widgets.hovered.weak_bg_fill);
        }
        let mark = egui::Rect::from_center_size(
            egui::pos2(rect.left() + 6.0 + 6.5, rect.center().y),
            egui::Vec2::splat(13.0),
        );
        match glyph {
            Some(glyph) => painted(ui, glyph, mark, tint_),
            None => {
                ui.put(mark, egui::Spinner::new().size(12.0));
            }
        }
        crate::panel::cut(
            ui.painter(),
            mark.right() + 6.0,
            rect.center().y,
            rect.right() - 6.0 - mark.right() - 6.0,
            galley.text(),
            egui::TextFormat::simple(egui::FontId::proportional(12.0), tint_),
        );
        if response.on_hover_text("the whole activity log").clicked() {
            acts.push(Act::ToggleLog);
        }
        let problems = self.log.problems();
        if problems == 0 {
            return;
        }
        let ink = warn(&visuals);
        let label = match problems {
            1 => "1 problem".to_string(),
            n => format!("{n} problems"),
        };
        let galley = ui
            .painter()
            .layout_no_wrap(label, egui::FontId::proportional(11.5), ink);
        let size = egui::vec2(8.0 + 11.0 + 5.0 + galley.size().x + 9.0, 20.0);
        let (rect, response) = ui.allocate_exact_size(size, egui::Sense::click());
        let alpha = match response.hovered() {
            true => 0.26,
            false => 0.16,
        };
        ui.painter().rect_filled(rect, 10.0, tint(ink, alpha));
        let mark = egui::Rect::from_center_size(
            egui::pos2(rect.left() + 8.0 + 5.5, rect.center().y),
            egui::Vec2::splat(11.0),
        );
        painted(ui, Glyph::ScrollText, mark, ink);
        ui.painter().galley(
            egui::pos2(mark.right() + 5.0, rect.center().y - galley.size().y / 2.0),
            galley,
            ink,
        );
        if response
            .on_hover_text("what went wrong, in the activity log")
            .clicked()
        {
            acts.push(Act::ShowProblems);
        }
    }

    /// How full the instrument is, as figures separated by dots. A class nearly full is in
    /// the warning color.
    fn room_figures(&self, ui: &mut egui::Ui) {
        let visuals = ui.visuals().clone();
        let quiet = visuals.weak_text_color();
        let mut first = true;
        for class in [ObjectClass::Program, ObjectClass::Sample] {
            let state = &self.device.state;
            let unit = state.allocation_unit(class);
            let Some(room) = crate::device::occupancy(class, &state.inventory, unit) else {
                continue;
            };
            let full = state
                .inventory
                .iter()
                .find(|status| status.class == class)
                .is_some_and(|status| status.used_percent() >= NEARLY_FULL);
            if !first {
                let (dot, _) = ui.allocate_exact_size(egui::vec2(9.0, 9.0), egui::Sense::hover());
                ui.painter().circle_filled(dot.center(), 1.5, quiet);
            }
            first = false;
            let ink = match full {
                true => warn(&visuals),
                false => quiet,
            };
            ui.label(
                egui::RichText::new(format!("{room} {}", folder(class).to_lowercase()))
                    .monospace()
                    .size(10.5)
                    .color(ink),
            );
        }
    }

    /// The browser card: this computer and the instrument. Not drawn at all while hidden.
    pub(crate) fn browser_card(&mut self, ctx: &egui::Context, acts: &mut Vec<Act>) {
        self.shell.filter.keep_kinds(&crate::browser::kinds_present(
            &self.workspace,
            &self.device.state,
        ));
        if !self.shell.browser_open {
            return;
        }
        let most =
            (ctx.available_rect().width() - self.inspector_room() - CENTER_WIDE).max(SIDE_LEAST);
        egui::SidePanel::left("browser")
            .resizable(true)
            .show_separator_line(false)
            .default_width(self.shell.browser_width)
            .width_range(SIDE_LEAST..=most)
            .frame(egui::Frame::NONE)
            .show(ctx, |ui| {
                claim(ui);
                let mut rect = ui.max_rect();
                rect.min.x += GUTTER;
                rect.max.y -= GUTTER;
                let mut inside = card(ui, rect);
                inside.add_space(GUTTER);
                if self.platform == Platform::Windows {
                    self.tools_row(&mut inside, acts);
                }
                acts.extend(self.browser.ui(
                    &mut inside,
                    &self.workspace,
                    &self.device,
                    &self.queue,
                    &self.shell.filter,
                ));
                round_off(ui, rect);
            });
        if let Some(rect) = laid_out(ctx, "browser") {
            self.shell.browser_width = rect.width();
        }
    }

    /// Windows' file tools, at the top of the browser card rather than in the bar its
    /// menus fill.
    fn tools_row(&mut self, ui: &mut egui::Ui, acts: &mut Vec<Act>) {
        let row = egui::Rect::from_min_size(
            ui.cursor().min,
            egui::vec2(ui.available_width(), TOOLS_ROW - GUTTER),
        );
        let mut tools = ui.new_child(
            egui::UiBuilder::new()
                .max_rect(row.shrink2(egui::vec2(6.0, 0.0)))
                .layout(egui::Layout::left_to_right(egui::Align::Center)),
        );
        tools.spacing_mut().item_spacing.x = 4.0;
        self.file_tools(&mut tools, acts);
        ui.advance_cursor_after_rect(row);
    }

    /// The width the inspector will take. The browser's maximum is computed before the
    /// inspector is added, so it must be asked for here.
    fn inspector_room(&self) -> f32 {
        match self.shell.inspector_open {
            true => self.shell.inspector_width,
            false => GUTTER,
        }
    }

    /// The inspector card: the selection, and the instrument while one is attached. Not
    /// drawn at all while hidden.
    pub(crate) fn inspector_card(&mut self, ctx: &egui::Context, acts: &mut Vec<Act>) {
        if !self.shell.inspector_open {
            return;
        }
        let most = (ctx.available_rect().width() - CENTER_WIDE).max(SIDE_LEAST);
        egui::SidePanel::right("inspector")
            .resizable(true)
            .show_separator_line(false)
            .default_width(self.shell.inspector_width)
            .width_range(SIDE_LEAST..=most)
            .frame(egui::Frame::NONE)
            .show(ctx, |ui| {
                claim(ui);
                let mut rect = ui.max_rect();
                rect.max.x -= GUTTER;
                rect.max.y -= GUTTER;
                let mut inside = card(ui, rect);
                acts.extend(crate::inspector::ui(
                    &mut inside,
                    &mut self.shell,
                    &mut self.browser,
                    &self.workspace,
                    &self.device,
                    &self.queue,
                ));
                round_off(ui, rect);
            });
        if let Some(rect) = laid_out(ctx, "inspector") {
            self.shell.inspector_width = rect.width();
        }
    }

    /// The toggle at one end of the tab row, beside the edge of the panel it shows or
    /// hides. It stays where it is either way.
    pub(crate) fn panel_toggle(&self, ui: &mut egui::Ui, dock: Dock, acts: &mut Vec<Act>) {
        let open = self.shell.open(dock);
        let (glyph, name, command) = match (dock, open) {
            (Dock::Browser, true) => (Glyph::PanelLeftClose, "Hide the browser", Command::Browser),
            (Dock::Browser, false) => (Glyph::PanelLeftOpen, "Show the browser", Command::Browser),
            (Dock::Inspector, true) => (
                Glyph::PanelRightClose,
                "Hide the inspector",
                Command::Inspector,
            ),
            (Dock::Inspector, false) => (
                Glyph::PanelRightOpen,
                "Show the inspector",
                Command::Inspector,
            ),
        };
        let mac = crate::platform::mac_keyboard(ui.ctx());
        let hint = match key_text(command, self.platform, mac) {
            Some(keys) => format!("{name}  {keys}"),
            None => name.to_string(),
        };
        if glyph_button(ui, glyph, BAR_GLYPH, 28.0, &hint).clicked() {
            acts.push(Act::ToggleDock(dock));
        }
    }
}

/// A class this full is shown in the warning color.
const NEARLY_FULL: f32 = 90.0;

/// The mark at the left of the bar on Windows and the web: the slider glyph on an
/// accent-tinted tile.
fn logo(ui: &mut egui::Ui) {
    let lit = accent(ui.visuals());
    let (tile, _) = ui.allocate_exact_size(egui::Vec2::splat(24.0), egui::Sense::hover());
    ui.painter().rect_filled(tile, 7.0, tint(lit, 0.18));
    painted(ui, Glyph::SlidersVertical, tile.shrink(5.0), lit);
}

/// Claim the whole panel being drawn.
///
/// ⚠️ egui remembers a resizable panel's size as the size of its contents, so a panel
/// holding less than it shows would shrink to its minimum.
fn claim(ui: &mut egui::Ui) {
    ui.set_min_size(ui.max_rect().size());
}

/// Where a panel ended up this frame.
fn laid_out(ctx: &egui::Context, id: &str) -> Option<egui::Rect> {
    egui::containers::panel::PanelState::load(ctx, egui::Id::new(id)).map(|state| state.rect)
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
                ui.add_space(6.0);
                ui.label(TOO_SMALL_WHY);
                ui.add_space(12.0);
                crate::sheet::link(ui, "User guide", GUIDE);
            });
        });
}

/// A glyph drawn at a fixed size in a layout.
#[allow(dead_code)]
fn glyph(ui: &mut egui::Ui, glyph: Glyph, size: f32, tint: egui::Color32) {
    icon(ui, glyph, size, tint);
}
