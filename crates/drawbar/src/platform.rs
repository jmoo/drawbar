//! What differs between the four places drawbar runs: who draws the window's frame and
//! menus, and how a shortcut is written.
//!
//! The platform is chosen when the app is built. Only Linux decides anything at run time:
//! whether the desktop expects the app to draw its own header bar.

use eframe::egui;

/// Where this build runs.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Platform {
    /// Native menus in the system menu bar, and the traffic lights over the top bar.
    Mac,
    /// Menus in the top bar, and caption buttons the app draws.
    Windows,
    /// One menu button, and the window buttons when the desktop wants a header bar.
    Linux,
    /// One menu button. The browser owns the window.
    Web,
}

impl Platform {
    pub const fn current() -> Platform {
        if cfg!(target_arch = "wasm32") {
            Platform::Web
        } else if cfg!(target_os = "macos") {
            Platform::Mac
        } else if cfg!(target_os = "windows") {
            Platform::Windows
        } else {
            Platform::Linux
        }
    }

    /// Whether the menus are one button holding every menu as a section, rather than a row
    /// of titles.
    pub fn one_menu(self) -> bool {
        matches!(self, Platform::Linux | Platform::Web)
    }

    /// Whether the window's own Quit belongs in the File menu. The Mac keeps it in the app
    /// menu, and a browser tab cannot quit.
    pub fn quits_from_file(self) -> bool {
        matches!(self, Platform::Windows | Platform::Linux)
    }

    /// Whether this build may bind a key a browser tab keeps for itself: ⌘W, ⌘Q, ⌘N,
    /// ⌘T and ⌘1–⌘9 reach the tab, not the page.
    pub fn windowed(self) -> bool {
        self != Platform::Web
    }

    /// The window width under which the top bar's chips drop their labels. Windows needs
    /// more, for its caption buttons.
    pub fn narrow(self) -> f32 {
        match self {
            Platform::Windows => 1400.0,
            Platform::Mac | Platform::Linux | Platform::Web => 1280.0,
        }
    }
}

/// The window width under which the top bar drops its file tools.
pub const CRAMPED: f32 = 1060.0;

/// How the window's frame is drawn on this run.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Frame {
    /// The system draws it: the Mac's title bar under the top bar, a Linux desktop's own
    /// decorations, or the browser.
    System,
    /// drawbar draws the frame: Windows' caption buttons at the right edge.
    Captions,
    /// drawbar draws a GNOME header bar, with these buttons at each end.
    HeaderBar(Layout),
}

impl Frame {
    /// The frame for this platform. On Linux, a GNOME session draws client-side decorations,
    /// so the app draws its own header bar; other desktops decorate the window themselves.
    pub fn of(platform: Platform) -> Frame {
        match platform {
            Platform::Windows => Frame::Captions,
            Platform::Mac | Platform::Web => Frame::System,
            Platform::Linux => match header_bar_desktop() {
                true => Frame::HeaderBar(Layout::read(&gnome_button_layout())),
                false => Frame::System,
            },
        }
    }

    /// Whether the app draws the window frame, and so must not ask the system for one.
    pub fn undecorated(&self) -> bool {
        !matches!(self, Frame::System)
    }
}

/// A window button in a GNOME header bar.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Button {
    Minimize,
    Maximize,
    Close,
}

/// Which window buttons sit at each end of a header bar, read from GNOME's
/// `button-layout`, in the order they are drawn.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Layout {
    pub left: Vec<Button>,
    pub right: Vec<Button>,
}

impl Layout {
    /// Parse a GTK decoration layout such as `appmenu:minimize,maximize,close`. Names this
    /// app has no button for (`appmenu`, `icon`, `spacer`) are skipped. Text with no colon
    /// is not a layout, and GNOME's own default, a close button at the right, stands.
    pub fn read(text: &str) -> Layout {
        let text = text.trim().trim_matches('\'');
        let Some((left, right)) = text.split_once(':') else {
            return Layout {
                left: Vec::new(),
                right: vec![Button::Close],
            };
        };
        let buttons = |side: &str| {
            side.split(',')
                .filter_map(|name| match name.trim() {
                    "minimize" => Some(Button::Minimize),
                    "maximize" => Some(Button::Maximize),
                    "close" => Some(Button::Close),
                    _ => None,
                })
                .collect()
        };
        Layout {
            left: buttons(left),
            right: buttons(right),
        }
    }
}

/// Whether the desktop session expects client-side decorations: GNOME and the desktops
/// built on it.
#[cfg(not(target_arch = "wasm32"))]
fn header_bar_desktop() -> bool {
    std::env::var("XDG_CURRENT_DESKTOP").is_ok_and(|desktops| {
        desktops
            .split(':')
            .any(|desktop| matches!(desktop, "GNOME" | "GNOME-Classic" | "Budgie" | "Pantheon"))
    })
}

#[cfg(target_arch = "wasm32")]
fn header_bar_desktop() -> bool {
    false
}

/// GNOME's window-button layout, or nothing when it cannot be read.
#[cfg(not(target_arch = "wasm32"))]
fn gnome_button_layout() -> String {
    std::process::Command::new("gsettings")
        .args(["get", "org.gnome.desktop.wm.preferences", "button-layout"])
        .output()
        .ok()
        .filter(|output| output.status.success())
        .map(|output| String::from_utf8_lossy(&output.stdout).into_owned())
        .unwrap_or_default()
}

#[cfg(target_arch = "wasm32")]
fn gnome_button_layout() -> String {
    String::new()
}

/// A shortcut as the menus, tooltips, and hints write it: symbols run together on a Mac
/// (`⇧⌘S`), words joined by `+` everywhere else (`Ctrl+Shift+S`).
///
/// `mac` is the keyboard's platform, which in a browser is the host's, not the build's.
pub fn written(shortcut: egui::KeyboardShortcut, mac: bool) -> String {
    let with = shortcut.modifiers;
    let key = shortcut.logical_key.symbol_or_name();
    let command = with.command || with.mac_cmd;
    match mac {
        true => {
            let marks = [
                (with.ctrl && !with.command, "⌃"),
                (with.alt, "⌥"),
                (with.shift, "⇧"),
                (command, "⌘"),
            ];
            let mut text: String = marks
                .iter()
                .filter(|(held, _)| *held)
                .map(|(_, mark)| *mark)
                .collect();
            text.push_str(key);
            text
        }
        false => {
            let words = [
                (command || with.ctrl, "Ctrl"),
                (with.alt, "Alt"),
                (with.shift, "Shift"),
            ];
            let mut parts: Vec<&str> = words
                .iter()
                .filter(|(held, _)| *held)
                .map(|(_, word)| *word)
                .collect();
            parts.push(key);
            parts.join("+")
        }
    }
}

/// Whether the keyboard in use is a Mac's, which decides how shortcuts are written.
pub fn mac_keyboard(ctx: &egui::Context) -> bool {
    matches!(
        ctx.os(),
        egui::os::OperatingSystem::Mac | egui::os::OperatingSystem::IOS
    )
}

/// The width of one of Windows' caption buttons. They fill the bar's height.
pub const CAPTION: f32 = 46.0;

/// The red Windows uses under a hovered close button.
const CLOSE_HOVER: egui::Color32 = egui::Color32::from_rgb(0xc4, 0x2b, 0x1c);

/// Windows' minimize, maximize, and close, drawn from the right edge of `rect` leftward.
/// Returns the left edge of the space they took.
pub fn captions(ui: &mut egui::Ui, rect: egui::Rect) -> f32 {
    let maximized = ui.input(|input| input.viewport().maximized.unwrap_or(false));
    let mut right = rect.right();
    for button in [Button::Close, Button::Maximize, Button::Minimize] {
        let box_ = egui::Rect::from_min_max(
            egui::pos2(right - CAPTION, rect.top()),
            egui::pos2(right, rect.bottom()),
        );
        let response = ui.interact(
            box_,
            ui.id().with(("caption", button as u8)),
            egui::Sense::click(),
        );
        let visuals = ui.visuals();
        let (fill, ink) = match (response.hovered(), button) {
            (true, Button::Close) => (CLOSE_HOVER, egui::Color32::WHITE),
            (true, _) => (
                visuals.widgets.hovered.weak_bg_fill,
                visuals.widgets.hovered.fg_stroke.color,
            ),
            (false, _) => (
                egui::Color32::TRANSPARENT,
                visuals.widgets.inactive.fg_stroke.color,
            ),
        };
        ui.painter().rect_filled(box_, 0.0, fill);
        caption_glyph(ui.painter(), box_.center(), button, maximized, ink);
        if response.clicked() {
            press(ui.ctx(), button, maximized);
        }
        right = box_.left();
    }
    right
}

/// The thin-stroke marks Windows draws on its caption buttons, 10 px across.
fn caption_glyph(
    painter: &egui::Painter,
    center: egui::Pos2,
    button: Button,
    maximized: bool,
    ink: egui::Color32,
) {
    let stroke = egui::Stroke::new(1.0_f32, ink);
    let half = 5.0;
    match (button, maximized) {
        (Button::Minimize, _) => {
            painter.hline((center.x - half)..=(center.x + half), center.y, stroke);
        }
        (Button::Maximize, false) => {
            let box_ = egui::Rect::from_center_size(center, egui::Vec2::splat(2.0 * half));
            painter.rect_stroke(box_, 1.0, stroke, egui::StrokeKind::Inside);
        }
        (Button::Maximize, true) => {
            let back = egui::Rect::from_center_size(
                center + egui::vec2(1.5, -1.5),
                egui::Vec2::splat(8.0),
            );
            let front = egui::Rect::from_center_size(
                center + egui::vec2(-1.5, 1.5),
                egui::Vec2::splat(8.0),
            );
            painter.rect_stroke(back, 1.0, stroke, egui::StrokeKind::Inside);
            painter.rect_filled(
                front,
                1.0,
                painter.ctx().style().visuals.panel_fill.gamma_multiply(0.0),
            );
            painter.rect_stroke(front, 1.0, stroke, egui::StrokeKind::Inside);
        }
        (Button::Close, _) => {
            let d = egui::vec2(half, half);
            painter.line_segment([center - d, center + d], stroke);
            painter.line_segment(
                [
                    center + egui::vec2(-half, half),
                    center + egui::vec2(half, -half),
                ],
                stroke,
            );
        }
    }
}

/// The diameter of a GNOME window button, and the gap between two.
pub const ROUND: f32 = 24.0;
pub const ROUND_GAP: f32 = 8.0;

/// The room `buttons` take in a header bar.
pub fn round_width(buttons: &[Button]) -> f32 {
    match buttons.len() {
        0 => 0.0,
        n => n as f32 * ROUND + (n - 1) as f32 * ROUND_GAP,
    }
}

/// GNOME's round window buttons, laid out left to right from `left`, centered on `middle`.
pub fn round_buttons(ui: &mut egui::Ui, buttons: &[Button], left: f32, middle: f32) {
    let maximized = ui.input(|input| input.viewport().maximized.unwrap_or(false));
    let mut x = left;
    for button in buttons.iter().copied() {
        let box_ = egui::Rect::from_min_size(
            egui::pos2(x, middle - ROUND / 2.0),
            egui::Vec2::splat(ROUND),
        );
        let response = ui.interact(
            box_,
            ui.id().with(("round", button as u8)),
            egui::Sense::click(),
        );
        let widgets = &ui.visuals().widgets;
        let (fill, ink) = match response.hovered() {
            true => (
                widgets.hovered.weak_bg_fill,
                widgets.hovered.fg_stroke.color,
            ),
            false => (
                widgets.inactive.weak_bg_fill,
                widgets.inactive.fg_stroke.color,
            ),
        };
        ui.painter().circle_filled(box_.center(), ROUND / 2.0, fill);
        let glyph = egui::Rect::from_center_size(box_.center(), egui::Vec2::splat(2.0 * 4.0));
        let stroke = egui::Stroke::new(1.5_f32, ink);
        match button {
            Button::Minimize => {
                ui.painter()
                    .hline(glyph.x_range(), glyph.bottom() - 1.0, stroke);
            }
            Button::Maximize => {
                ui.painter()
                    .rect_stroke(glyph, 1.0, stroke, egui::StrokeKind::Inside);
            }
            Button::Close => {
                ui.painter()
                    .line_segment([glyph.left_top(), glyph.right_bottom()], stroke);
                ui.painter()
                    .line_segment([glyph.left_bottom(), glyph.right_top()], stroke);
            }
        }
        if response.clicked() {
            press(ui.ctx(), button, maximized);
        }
        x = box_.right() + ROUND_GAP;
    }
}

/// What a window button does.
fn press(ctx: &egui::Context, button: Button, maximized: bool) {
    let command = match button {
        Button::Minimize => egui::ViewportCommand::Minimized(true),
        Button::Maximize => egui::ViewportCommand::Maximized(!maximized),
        Button::Close => egui::ViewportCommand::Close,
    };
    ctx.send_viewport_cmd(command);
}

/// Let the empty parts of the top bar move the window, and a double click on them
/// maximize or restore it, as a title bar does. `bar` is the top bar's response.
pub fn title_bar(ctx: &egui::Context, bar: &egui::Response) {
    if bar.double_clicked() {
        let maximized = ctx.input(|input| input.viewport().maximized.unwrap_or(false));
        ctx.send_viewport_cmd(egui::ViewportCommand::Maximized(!maximized));
    } else if bar.drag_started_by(egui::PointerButton::Primary) {
        ctx.send_viewport_cmd(egui::ViewportCommand::StartDrag);
    }
}

/// How far in from the window's edge a press resizes an undecorated window.
const GRIP: f32 = 5.0;

/// The edge or corner a pointer at `at` would resize, in a window of `size`.
pub fn grip(size: egui::Vec2, at: egui::Pos2) -> Option<egui::ResizeDirection> {
    use egui::ResizeDirection as To;

    let (west, east) = (at.x < GRIP, at.x > size.x - GRIP);
    let (north, south) = (at.y < GRIP, at.y > size.y - GRIP);
    match (north, south, west, east) {
        (true, _, true, _) => Some(To::NorthWest),
        (true, _, _, true) => Some(To::NorthEast),
        (_, true, true, _) => Some(To::SouthWest),
        (_, true, _, true) => Some(To::SouthEast),
        (true, _, _, _) => Some(To::North),
        (_, true, _, _) => Some(To::South),
        (_, _, true, _) => Some(To::West),
        (_, _, _, true) => Some(To::East),
        _ => None,
    }
}

/// The resize border an undecorated window lacks: the cursor at its edges, and a press
/// there hands the resize to the system. Nothing while maximized.
pub fn edges(ctx: &egui::Context) {
    let (maximized, at, pressed) = ctx.input(|input| {
        (
            input.viewport().maximized.unwrap_or(false),
            input.pointer.hover_pos(),
            input.pointer.primary_pressed(),
        )
    });
    let (false, Some(at)) = (maximized, at) else {
        return;
    };
    let Some(direction) = grip(ctx.screen_rect().size(), at) else {
        return;
    };
    use egui::ResizeDirection as To;
    ctx.set_cursor_icon(match direction {
        To::North | To::South => egui::CursorIcon::ResizeVertical,
        To::East | To::West => egui::CursorIcon::ResizeHorizontal,
        To::NorthWest | To::SouthEast => egui::CursorIcon::ResizeNwSe,
        To::NorthEast | To::SouthWest => egui::CursorIcon::ResizeNeSw,
    });
    if pressed {
        ctx.send_viewport_cmd(egui::ViewportCommand::BeginResize(direction));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use egui::{Key, KeyboardShortcut as Shortcut, Modifiers as With};

    #[test]
    fn a_shortcut_is_written_in_symbols_on_a_mac_and_in_words_elsewhere() {
        let cases = [
            (Shortcut::new(With::COMMAND, Key::O), "⌘O", "Ctrl+O"),
            (
                Shortcut::new(With::COMMAND.plus(With::SHIFT), Key::S),
                "⇧⌘S",
                "Ctrl+Shift+S",
            ),
            (
                Shortcut::new(With::COMMAND.plus(With::ALT), Key::B),
                "⌥⌘B",
                "Ctrl+Alt+B",
            ),
            (Shortcut::new(With::ALT, Key::W), "⌥W", "Alt+W"),
            (Shortcut::new(With::CTRL, Key::Num2), "⌃2", "Ctrl+2"),
        ];
        for (shortcut, mac, other) in cases {
            assert_eq!(written(shortcut, true), mac);
            assert_eq!(written(shortcut, false), other);
        }
    }

    #[test]
    fn a_gnome_layout_puts_each_button_on_its_side_in_order() {
        assert_eq!(
            Layout::read("'appmenu:minimize,maximize,close'\n"),
            Layout {
                left: Vec::new(),
                right: vec![Button::Minimize, Button::Maximize, Button::Close],
            }
        );
        assert_eq!(
            Layout::read("close,minimize:appmenu"),
            Layout {
                left: vec![Button::Close, Button::Minimize],
                right: Vec::new(),
            }
        );
    }

    #[test]
    fn an_unreadable_layout_leaves_gnomes_default_close_button() {
        for text in ["", "not a layout"] {
            assert_eq!(
                Layout::read(text),
                Layout {
                    left: Vec::new(),
                    right: vec![Button::Close],
                },
                "{text:?}"
            );
        }
    }

    #[test]
    fn only_the_edges_of_an_undecorated_window_resize_it() {
        use egui::ResizeDirection as To;

        let size = egui::vec2(1000.0, 600.0);
        assert_eq!(grip(size, egui::pos2(500.0, 300.0)), None);
        assert_eq!(grip(size, egui::pos2(1.0, 300.0)), Some(To::West));
        assert_eq!(grip(size, egui::pos2(999.0, 599.0)), Some(To::SouthEast));
        assert_eq!(grip(size, egui::pos2(500.0, 2.0)), Some(To::North));
        assert_eq!(grip(size, egui::pos2(998.0, 1.0)), Some(To::NorthEast));
    }

    #[test]
    fn only_windows_and_linux_quit_from_the_file_menu() {
        assert!(Platform::Windows.quits_from_file());
        assert!(Platform::Linux.quits_from_file());
        assert!(!Platform::Mac.quits_from_file());
        assert!(!Platform::Web.quits_from_file());
    }
}
