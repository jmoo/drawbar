//! The menus: every command the app offers from them, one table of shortcuts, and the
//! in-window menus that draw them.
//!
//! The macOS menu bar ([`crate::menubar`]), the in-window menus, and the shortcuts all
//! read [`menus`], [`DrawbarApp::offer`], and [`shortcut`], so a command is named, bound,
//! and gated in one place.

use eframe::egui;
use nord_usb::ObjectClass;

use crate::app::{accent, DrawbarApp, ThemeChoice};
use crate::browser::Act;
use crate::icon::{sized, Glyph};
use crate::newproject::Making;
use crate::platform::{written, Platform};
use crate::shell::Dock;
use crate::strings::folder;
use crate::tabs::Spot;
use crate::workspace::Fresh;

/// Everything a menu item can do.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Command {
    Open,
    New(Fresh),
    FromWavs(Making),
    NewFolder,
    Save,
    Revert,
    Export,
    CloseTab,
    Quit,
    Keyboard,
    Document,
    Browser,
    Inspector,
    Activity,
    Theme(ThemeChoice),
    Connect,
    Disconnect,
    ReadEverything,
    ReadAgain,
    ReviewQueue,
    SendAll,
    ClearQueue,
    Unqueue,
    Listen,
    Guide,
    WhatsNew,
    Welcome,
    CopyLog,
    About,
}

/// One line of a menu.
pub enum Entry {
    Do(Command),
    Rule,
    Sub(&'static str, Vec<Entry>),
}

/// One menu: its title and its lines.
pub struct Menu {
    pub title: &'static str,
    pub entries: Vec<Entry>,
}

/// What a command looks like in a menu right now.
pub struct Offer {
    pub label: String,
    pub enabled: bool,
    /// Whether the item carries a check, and if so whether it is checked.
    pub checked: Option<bool>,
    /// Why a disabled item cannot be used.
    pub hint: Option<&'static str>,
}

impl Offer {
    fn plain(label: impl Into<String>) -> Offer {
        Offer {
            label: label.into(),
            enabled: true,
            checked: None,
            hint: None,
        }
    }

    fn check(label: impl Into<String>, on: bool) -> Offer {
        Offer {
            checked: Some(on),
            ..Offer::plain(label)
        }
    }
}

/// The menus in the order they are shown, as this platform arranges them.
pub fn menus(platform: Platform) -> Vec<Menu> {
    use Command as C;
    use Entry::{Do, Rule};

    let mut file = vec![Do(C::Open), Entry::Sub("New", new_entries()), Rule];
    file.extend([
        Do(C::Save),
        Do(C::Revert),
        Do(C::Export),
        Rule,
        Do(C::CloseTab),
    ]);
    if platform.quits_from_file() {
        file.push(Do(C::Quit));
    }
    let view = vec![
        Do(C::Keyboard),
        Do(C::Document),
        Rule,
        Do(C::Browser),
        Do(C::Inspector),
        Do(C::Activity),
        Rule,
        Entry::Sub(
            "Theme",
            ThemeChoice::ALL
                .iter()
                .map(|choice| Do(C::Theme(*choice)))
                .collect(),
        ),
    ];
    let instrument = vec![
        Do(C::Connect),
        Do(C::Disconnect),
        Rule,
        Do(C::ReadEverything),
        Do(C::ReadAgain),
        Rule,
        Do(C::ReviewQueue),
        Do(C::SendAll),
        Do(C::ClearQueue),
        Do(C::Unqueue),
        Rule,
        Do(C::Listen),
    ];
    let mut help = vec![
        Do(C::Guide),
        Do(C::WhatsNew),
        Do(C::Welcome),
        Rule,
        Do(C::CopyLog),
    ];
    // The Mac's About is the first item of the app menu.
    if platform != Platform::Mac {
        help.push(Do(C::About));
    }
    vec![
        Menu {
            title: "File",
            entries: file,
        },
        Menu {
            title: "View",
            entries: view,
        },
        Menu {
            title: "Instrument",
            entries: instrument,
        },
        Menu {
            title: "Help",
            entries: help,
        },
    ]
}

/// New's lines: each instrument's kinds under its name, what is made from WAVs, then what
/// no instrument holds.
fn new_entries() -> Vec<Entry> {
    use Entry::{Do, Rule};

    let mut entries: Vec<Entry> = Fresh::FAMILIES
        .iter()
        .map(|family| {
            Entry::Sub(
                family.label,
                family
                    .kinds
                    .iter()
                    .map(|kind| Do(Command::New(*kind)))
                    .collect(),
            )
        })
        .collect();
    let (held, loose): (Vec<Making>, Vec<Making>) = Making::FROM_WAVS
        .iter()
        .partition(|making| making.instrument_file());
    entries.extend(held.into_iter().map(|making| Do(Command::FromWavs(making))));
    entries.push(Rule);
    entries.extend(Fresh::LOOSE.into_iter().map(|kind| Do(Command::New(kind))));
    entries.extend(
        loose
            .into_iter()
            .map(|making| Do(Command::FromWavs(making))),
    );
    entries.push(Do(Command::NewFolder));
    entries
}

/// The key bound to a command on `platform`, if any. `mac` is whether the keyboard is a
/// Mac's, which on the web decides the modifier for the keys a browser keeps.
pub fn shortcut(command: Command, platform: Platform, mac: bool) -> Option<egui::KeyboardShortcut> {
    use egui::{Key, KeyboardShortcut as Shortcut, Modifiers as With};

    // ⚠️ A browser keeps ⌘W, ⌘Q and ⌘1–⌘9 for its tabs. The web build binds those
    // commands to ⌥ instead, except on a Mac, where a browser reports ⌥W as the "∑" it
    // types and egui never sees a W; there ⌃ stands in.
    let tab_key = |key| match (platform.windowed(), mac) {
        (true, _) => Shortcut::new(With::COMMAND, key),
        (false, true) => Shortcut::new(With::CTRL, key),
        (false, false) => Shortcut::new(With::ALT, key),
    };
    let command_and = |with, key| Shortcut::new(With::COMMAND.plus(with), key);
    Some(match command {
        Command::Open => Shortcut::new(With::COMMAND, Key::O),
        Command::Save => Shortcut::new(With::COMMAND, Key::S),
        Command::Export => command_and(With::SHIFT, Key::E),
        Command::CloseTab => tab_key(Key::W),
        Command::Quit if platform.windowed() && platform != Platform::Windows => {
            Shortcut::new(With::COMMAND, Key::Q)
        }
        Command::Keyboard => tab_key(Key::Num2),
        Command::Document => tab_key(Key::Num3),
        Command::Browser => command_and(With::ALT, Key::B),
        Command::Inspector => command_and(With::ALT, Key::I),
        Command::Activity => command_and(With::ALT, Key::L),
        Command::ReadEverything => Shortcut::new(With::COMMAND, Key::R),
        Command::ReviewQueue => command_and(With::SHIFT, Key::S),
        _ => return None,
    })
}

/// The key that puts the cursor in the search box.
pub fn search_key() -> egui::KeyboardShortcut {
    egui::KeyboardShortcut::new(egui::Modifiers::COMMAND, egui::Key::K)
}

/// The key text beside a command, as this platform writes it.
pub fn key_text(command: Command, platform: Platform, mac: bool) -> Option<String> {
    match (command, platform) {
        // Windows closes a window with its own key, which the app does not bind.
        (Command::Quit, Platform::Windows) => Some("Alt+F4".to_string()),
        _ => shortcut(command, platform, mac).map(|keys| written(keys, mac)),
    }
}

/// The commands with keys, in the order a key press is matched against them.
///
/// ⚠️ egui matches a shortcut's modifiers logically, so an extra Shift is ignored: ⇧⌘S
/// must be taken before ⌘S, or asking to review the send queue would save the open
/// document and lose its revert. A bound key is consumed whether or not its command is
/// offered now, so it never falls through to a shorter one, and ⌘R never reaches the
/// browser tab as a reload.
const KEYED: [Command; 13] = [
    Command::ReviewQueue,
    Command::Export,
    Command::Browser,
    Command::Inspector,
    Command::Activity,
    Command::Open,
    Command::Save,
    Command::ReadEverything,
    Command::CloseTab,
    Command::Quit,
    Command::Keyboard,
    Command::Document,
    Command::Unqueue,
];

/// The minimum width of a drop-down menu, so its width does not change with which items
/// are enabled and a long label keeps a gap before its key text.
const MENU: f32 = 260.0;

/// The height of a menu item, and of a section's title in the one-button menu.
const ITEM: f32 = 28.0;
const SECTION: f32 = 24.0;

/// The check column's glyph.
const CHECK: f32 = 13.0;

impl DrawbarApp {
    /// What `command` looks like in a menu now, or `None` when it is not offered: an
    /// in-window menu leaves the line out, and the Mac's menu bar disables it.
    pub(crate) fn offer(&self, command: Command) -> Option<Offer> {
        let attached = self.attached();
        let active = self.tabs.active();
        let waiting = self.queue.len();
        Some(match command {
            Command::Open => Offer::plain("Open…"),
            Command::New(kind) => Offer {
                hint: kind.note(),
                ..Offer::plain(kind.label())
            },
            Command::FromWavs(making) => {
                let (item, hint) = making.item();
                Offer {
                    hint: Some(hint),
                    ..Offer::plain(item)
                }
            }
            Command::NewFolder => Offer {
                hint: Some("groups the list on this computer; the instrument never sees it"),
                ..Offer::plain("New folder")
            },
            Command::Save => active.map(|_| Offer::plain("Save"))?,
            Command::Revert => {
                let unsaved = self
                    .workspace
                    .get(active?)
                    .is_some_and(crate::workspace::LocalEntity::is_unsaved);
                unsaved.then(|| Offer::plain("Revert to saved"))?
            }
            Command::Export => active.map(|_| Offer::plain("Export…"))?,
            Command::CloseTab => {
                (self.tabs.showing() != Spot::Library).then(|| Offer::plain("Close tab"))?
            }
            Command::Quit => Offer::plain("Quit"),
            Command::Keyboard => {
                attached.then(|| Offer::check("Keyboard", self.tabs.showing() == Spot::Keyboard))?
            }
            Command::Document => {
                let id = self.tabs.last_document()?;
                Offer::check("Document", self.tabs.showing() == Spot::Document(id))
            }
            Command::Browser => Offer::check("Browser panel", self.shell.open(Dock::Browser)),
            Command::Inspector => Offer::check("Inspector panel", self.shell.open(Dock::Inspector)),
            Command::Activity => Offer::check("Activity log", self.shell.log_open),
            Command::Theme(choice) => Offer::check(choice.name(), self.theme == choice),
            Command::Connect => (!attached).then(|| Offer::plain("Connect…"))?,
            Command::Disconnect => attached.then(|| Offer::plain("Disconnect"))?,
            Command::ReadEverything => attached.then(|| Offer::plain("Read everything"))?,
            Command::ReadAgain => {
                let class = self.open_class().filter(|_| attached)?;
                Offer::plain(format!("Read {} again", folder(class)))
            }
            Command::ReviewQueue => attached.then(|| Offer::plain("Review send queue…"))?,
            Command::SendAll => {
                (attached && waiting > 0).then(|| Offer::plain(format!("Send all ({waiting})")))?
            }
            Command::ClearQueue => {
                (attached && waiting > 0).then(|| Offer::plain("Clear send queue"))?
            }
            Command::Unqueue => (attached && !self.queued_picks().is_empty())
                .then(|| Offer::plain("Remove from queue"))?,
            Command::Listen => Offer {
                enabled: crate::midi::supported(),
                hint: Some(crate::midi::UNSUPPORTED),
                ..Offer::check("Listen to MIDI controllers", self.midi.on())
            },
            Command::Guide => Offer::plain("User guide"),
            Command::WhatsNew => Offer::plain("What's new"),
            Command::Welcome => Offer::plain("Welcome"),
            Command::CopyLog => Offer::plain("Copy activity log"),
            Command::About => Offer::plain("About drawbar"),
        })
    }

    /// Do what `command` names, now.
    ///
    /// ⚠️ Listening to MIDI starts here, inside the click: a browser tab may ask for MIDI
    /// access only while the click's user activation is live.
    pub(crate) fn run(
        &mut self,
        ctx: &egui::Context,
        frame: &mut eframe::Frame,
        command: Command,
        acts: &mut Vec<Act>,
    ) {
        let active = self.tabs.active();
        match command {
            Command::Open => acts.push(Act::OpenFiles),
            Command::New(kind) => acts.push(Act::New(kind)),
            Command::FromWavs(making) => acts.push(Act::NewFromWavs(making)),
            Command::NewFolder => acts.push(Act::NewFolder),
            Command::Save => acts.extend(active.map(Act::SaveDoc)),
            Command::Revert => acts.extend(active.map(Act::Revert)),
            Command::Export => acts.extend(active.map(Act::Export)),
            Command::CloseTab => acts.push(Act::CloseTab),
            Command::Quit => acts.push(Act::Quit),
            Command::Keyboard => acts.push(Act::ShowTab(Spot::Keyboard)),
            Command::Document => acts.extend(
                self.tabs
                    .last_document()
                    .map(|id| Act::ShowTab(Spot::Document(id))),
            ),
            Command::Browser => acts.push(Act::ToggleDock(Dock::Browser)),
            Command::Inspector => acts.push(Act::ToggleDock(Dock::Inspector)),
            Command::Activity => acts.push(Act::ToggleLog),
            Command::Theme(choice) => self.pick_theme(ctx, frame, choice),
            Command::Connect => acts.push(Act::Connect),
            Command::Disconnect => acts.push(Act::Disconnect),
            Command::ReadEverything => acts.push(Act::Resync),
            Command::ReadAgain => acts.extend(self.open_class().map(Act::ReadAgain)),
            Command::ReviewQueue => acts.push(Act::ReviewQueue),
            Command::SendAll => acts.push(Act::AskSendAll),
            Command::ClearQueue => acts.push(Act::ClearQueue),
            Command::Unqueue => acts.extend(self.queued_picks().into_iter().map(Act::Unqueue)),
            Command::Listen => match self.midi.on() {
                true => self.midi.stop(),
                false => self.midi.listen(ctx),
            },
            Command::Guide => ctx.open_url(egui::OpenUrl::new_tab(crate::shell::GUIDE)),
            Command::WhatsNew => self.whats_new(ctx),
            Command::Welcome => self.splash.open_welcome(),
            Command::CopyLog => acts.push(Act::CopyLog),
            Command::About => {
                self.about = Some(crate::about::About::new(
                    &self.device.state,
                    &self.workspace,
                ))
            }
        }
    }

    /// Run whatever command this frame's keys ask for.
    pub(crate) fn shortcuts(
        &mut self,
        ctx: &egui::Context,
        frame: &mut eframe::Frame,
        acts: &mut Vec<Act>,
    ) {
        let (platform, mac) = (self.platform, crate::platform::mac_keyboard(ctx));
        for command in KEYED {
            let Some(keys) = shortcut(command, platform, mac) else {
                continue;
            };
            if !ctx.input_mut(|input| input.consume_shortcut(&keys)) {
                continue;
            }
            if self.offer(command).is_some_and(|offer| offer.enabled) {
                self.run(ctx, frame, command, acts);
            }
        }
    }

    /// The class of the slot the open document came off, for the item that reads that
    /// folder again. A document that did not come off a slot has none.
    pub(crate) fn open_class(&self) -> Option<ObjectClass> {
        let id = self.tabs.active()?;
        let (class, _) = self.workspace.get(id)?.origin.slot()?;
        Some(class)
    }

    /// The selected assets that are waiting to be sent.
    fn queued_picks(&self) -> Vec<u64> {
        self.browser
            .picked()
            .locals()
            .into_iter()
            .filter(|id| self.queue.holds(*id))
            .collect()
    }

    /// Windows' menus: a row of titles, each opening its drop-down.
    pub(crate) fn menu_bar(
        &mut self,
        ui: &mut egui::Ui,
        frame: &mut eframe::Frame,
        acts: &mut Vec<Act>,
    ) {
        let menus = menus(self.platform);
        egui::containers::menu::MenuBar::new().ui(ui, |ui| {
            for menu in &menus {
                title_style(ui);
                ui.menu_button(menu.title, |ui| {
                    drop_down_style(ui);
                    self.entries(ui, frame, &menu.entries, acts);
                });
            }
        });
    }

    /// Linux's and the web's menus: one button at the right of the bar, holding every
    /// menu as a titled section.
    pub(crate) fn menu_button(
        &mut self,
        ui: &mut egui::Ui,
        frame: &mut eframe::Frame,
        acts: &mut Vec<Act>,
    ) {
        let menus = menus(self.platform);
        let ink = ui.visuals().widgets.inactive.fg_stroke.color;
        let button = egui::Button::image(sized(Glyph::Menu, 16.0, ink))
            .image_tint_follows_text_color(false)
            .frame(false)
            .min_size(egui::Vec2::splat(30.0));
        // F10 opens it, as it opens a GTK app's primary menu.
        let f10 = ui.input_mut(|input| input.consume_key(egui::Modifiers::NONE, egui::Key::F10));
        let response = egui::containers::menu::MenuButton::from_button(button)
            .ui(ui, |ui| {
                drop_down_style(ui);
                for (index, menu) in menus.iter().enumerate() {
                    if index > 0 {
                        ui.separator();
                    }
                    section_title(ui, menu.title);
                    self.entries(ui, frame, &menu.entries, acts);
                }
            })
            .0;
        if f10 {
            egui::Popup::open_id(ui.ctx(), egui::Popup::default_response_id(&response));
        }
        response.on_hover_text("Menu  F10");
    }

    /// A menu's lines, leaving out what is not offered now and any rule left with
    /// nothing between it and the last.
    fn entries(
        &mut self,
        ui: &mut egui::Ui,
        frame: &mut eframe::Frame,
        entries: &[Entry],
        acts: &mut Vec<Act>,
    ) {
        let mut drawn = false;
        let mut owed_rule = false;
        for entry in entries {
            match entry {
                Entry::Rule => owed_rule = drawn,
                Entry::Sub(title, inner) => {
                    rule_if(ui, &mut owed_rule);
                    ui.menu_button(*title, |ui| {
                        drop_down_style(ui);
                        self.entries(ui, frame, inner, acts);
                    });
                    drawn = true;
                }
                Entry::Do(command) => {
                    let Some(offer) = self.offer(*command) else {
                        continue;
                    };
                    rule_if(ui, &mut owed_rule);
                    if self.item(ui, *command, &offer) {
                        self.run(ui.ctx(), frame, *command, acts);
                        ui.close();
                    }
                    drawn = true;
                }
            }
        }
    }

    /// One menu item: the check column, the label, and the key text at the right.
    fn item(&self, ui: &mut egui::Ui, command: Command, offer: &Offer) -> bool {
        let mac = crate::platform::mac_keyboard(ui.ctx());
        let mut button = check(ui, &offer.label, offer.checked == Some(true));
        if let Some(keys) = key_text(command, self.platform, mac) {
            button =
                button.shortcut_text(egui::RichText::new(keys).font(egui::FontId::monospace(10.5)));
        }
        let response = ui.add_enabled(offer.enabled, button);
        let response = match (offer.enabled, offer.hint) {
            (false, Some(why)) => response.on_disabled_hover_text(why),
            (true, Some(note)) if !matches!(command, Command::Listen) => {
                response.on_hover_text(note)
            }
            _ => response,
        };
        response.clicked()
    }
}

/// A menu item with a check mark showing whether what it names is on, closing the menu
/// when it is picked.
///
/// ⚠️ A check at the left, not a selected button or a `selectable_label`: both fill the
/// row with `selection.bg_fill`, the instrument's red, which reads as a warning in a menu
/// of ordinary items. **Every** checkable menu item in the app uses this or [`check`].
pub fn marked(ui: &mut egui::Ui, label: &str, on: bool) -> bool {
    let clicked = ui.add(check(ui, label, on)).clicked();
    if clicked {
        ui.close();
    }
    clicked
}

/// The button behind a checkable item: the check column, then the label.
fn check<'a>(ui: &egui::Ui, label: &'a str, on: bool) -> egui::Button<'a> {
    let tint = match on {
        true => accent(ui.visuals()),
        false => egui::Color32::TRANSPARENT,
    };
    egui::Button::image_and_text(sized(Glyph::Check, CHECK, tint), label)
        .image_tint_follows_text_color(false)
        .min_size(egui::vec2(0.0, ITEM))
}

/// A rule owed before the next drawn line.
fn rule_if(ui: &mut egui::Ui, owed: &mut bool) {
    if std::mem::take(owed) {
        ui.separator();
    }
}

/// A menu title in Windows' bar: 12.5 px, 28 px tall, rounded.
fn title_style(ui: &mut egui::Ui) {
    let style = ui.style_mut();
    style.override_text_style = Some(crate::app::ui());
    style.spacing.button_padding = egui::vec2(9.0, 5.0);
    for state in [
        &mut style.visuals.widgets.inactive,
        &mut style.visuals.widgets.hovered,
        &mut style.visuals.widgets.active,
        &mut style.visuals.widgets.open,
    ] {
        state.corner_radius = egui::CornerRadius::same(7);
        state.bg_stroke = egui::Stroke::NONE;
    }
    style.visuals.widgets.inactive.weak_bg_fill = egui::Color32::TRANSPARENT;
}

/// The inside of a drop-down: at least [`MENU`] wide, 28 px items rounded at 6, filled
/// only under the pointer.
fn drop_down_style(ui: &mut egui::Ui) {
    ui.set_min_width(MENU);
    let style = ui.style_mut();
    style.override_text_style = Some(crate::app::ui());
    style.spacing.button_padding = egui::vec2(8.0, 4.0);
    for state in [
        &mut style.visuals.widgets.inactive,
        &mut style.visuals.widgets.hovered,
        &mut style.visuals.widgets.active,
        &mut style.visuals.widgets.open,
    ] {
        state.corner_radius = egui::CornerRadius::same(6);
        state.bg_stroke = egui::Stroke::NONE;
    }
    style.visuals.widgets.inactive.weak_bg_fill = egui::Color32::TRANSPARENT;
}

/// A section's title in the one-button menu: quiet, bold, and not a control.
fn section_title(ui: &mut egui::Ui, title: &str) {
    let ink = crate::app::caption(ui.visuals());
    let (rect, _) = ui.allocate_exact_size(
        egui::vec2(ui.available_width(), SECTION),
        egui::Sense::hover(),
    );
    ui.painter().text(
        egui::pos2(rect.left() + 8.0, rect.center().y),
        egui::Align2::LEFT_CENTER,
        title,
        egui::FontId::new(11.5, crate::app::bold()),
        ink,
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    fn commands(entries: &[Entry], into: &mut Vec<Command>) {
        for entry in entries {
            match entry {
                Entry::Do(command) => into.push(*command),
                Entry::Sub(_, inner) => commands(inner, into),
                Entry::Rule => {}
            }
        }
    }

    fn every(platform: Platform) -> Vec<Command> {
        let mut all = Vec::new();
        for menu in menus(platform) {
            commands(&menu.entries, &mut all);
        }
        all
    }

    #[test]
    fn quit_is_in_the_file_menu_only_where_the_window_is_the_apps_to_close() {
        assert!(every(Platform::Windows).contains(&Command::Quit));
        assert!(every(Platform::Linux).contains(&Command::Quit));
        assert!(
            !every(Platform::Mac).contains(&Command::Quit),
            "the app menu has it"
        );
        assert!(
            !every(Platform::Web).contains(&Command::Quit),
            "a tab cannot quit"
        );
    }

    #[test]
    fn the_mac_says_about_from_its_app_menu_and_not_from_help() {
        assert!(!every(Platform::Mac).contains(&Command::About));
        assert!(every(Platform::Windows).contains(&Command::About));
    }

    #[test]
    fn every_kind_new_can_make_is_offered_once() {
        let all = every(Platform::Linux);
        for kind in Fresh::ALL {
            let times = all.iter().filter(|it| **it == Command::New(kind)).count();
            assert_eq!(times, 1, "{kind:?}");
        }
        for making in Making::FROM_WAVS {
            let times = all
                .iter()
                .filter(|it| **it == Command::FromWavs(making))
                .count();
            assert_eq!(times, 1, "{making:?}");
        }
    }

    #[test]
    fn the_web_moves_the_keys_a_browser_keeps_and_leaves_the_rest() {
        use egui::{Key, KeyboardShortcut as Shortcut, Modifiers as With};

        let web = |command, mac| shortcut(command, Platform::Web, mac);
        assert_eq!(
            web(Command::CloseTab, false),
            Some(Shortcut::new(With::ALT, Key::W))
        );
        assert_eq!(
            web(Command::Keyboard, false),
            Some(Shortcut::new(With::ALT, Key::Num2))
        );
        assert_eq!(
            web(Command::CloseTab, true),
            Some(Shortcut::new(With::CTRL, Key::W))
        );
        assert_eq!(web(Command::Quit, true), None, "a tab cannot quit");
        assert_eq!(
            web(Command::Open, true),
            shortcut(Command::Open, Platform::Mac, true),
            "a page may take ⌘O"
        );
        assert_eq!(
            key_text(Command::CloseTab, Platform::Mac, true).as_deref(),
            Some("⌘W")
        );
    }

    #[test]
    fn windows_writes_its_own_close_key_for_quit_and_binds_none() {
        assert_eq!(
            key_text(Command::Quit, Platform::Windows, false).as_deref(),
            Some("Alt+F4")
        );
        assert_eq!(shortcut(Command::Quit, Platform::Windows, false), None);
        assert_eq!(
            key_text(Command::Quit, Platform::Linux, false).as_deref(),
            Some("Ctrl+Q")
        );
    }

    /// ⚠️ A key with more modifiers is matched before any key it contains, or egui's
    /// logical matching would hand ⇧⌘S to ⌘S.
    #[test]
    fn a_key_is_matched_before_any_shorter_key_it_contains() {
        for platform in [
            Platform::Mac,
            Platform::Windows,
            Platform::Linux,
            Platform::Web,
        ] {
            let keys: Vec<_> = KEYED
                .iter()
                .filter_map(|command| shortcut(*command, platform, true))
                .collect();
            for (at, later) in keys.iter().enumerate() {
                for earlier in &keys[..at] {
                    assert!(
                        !(later.logical_key == earlier.logical_key
                            && later.modifiers.contains(earlier.modifiers)
                            && later.modifiers != earlier.modifiers),
                        "{platform:?}: {later:?} is matched after {earlier:?}, which it contains"
                    );
                }
            }
        }
    }
}
