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
    /// A command as its item looks when nothing about the moment changes it.
    fn of(command: Command) -> Offer {
        Offer {
            label: label(command).to_string(),
            enabled: true,
            checked: None,
            hint: hint(command),
        }
    }
}

/// The words of a command's item, before anything about the moment changes them.
pub fn label(command: Command) -> &'static str {
    match command {
        Command::Open => "Open…",
        Command::New(kind) => kind.label(),
        Command::FromWavs(making) => making.item().0,
        Command::NewFolder => "New folder",
        Command::Save => "Save",
        Command::Revert => "Revert to saved",
        Command::Export => "Export…",
        Command::CloseTab => "Close tab",
        Command::Quit => "Quit",
        Command::Keyboard => "Keyboard",
        Command::Document => "Document",
        Command::Browser => "Browser panel",
        Command::Inspector => "Inspector panel",
        Command::Activity => "Activity log",
        Command::Theme(choice) => choice.name(),
        Command::Connect => "Connect…",
        Command::Disconnect => "Disconnect",
        Command::ReadEverything => "Read everything",
        Command::ReadAgain => "Read again",
        Command::ReviewQueue => "Review send queue…",
        Command::SendAll => "Send all",
        Command::ClearQueue => "Clear send queue",
        Command::Unqueue => "Remove from queue",
        Command::Listen => "Listen to MIDI controllers",
        Command::Guide => "User guide",
        Command::WhatsNew => "What's new",
        Command::Welcome => "Welcome",
        Command::CopyLog => "Copy activity log",
        Command::About => "About drawbar",
    }
}

/// What a command's item says on hover, when it has more to say than its label.
fn hint(command: Command) -> Option<&'static str> {
    match command {
        Command::New(kind) => kind.note(),
        Command::FromWavs(making) => Some(making.item().1),
        Command::NewFolder => {
            Some("groups the list on this computer; the instrument never sees it")
        }
        _ => None,
    }
}

/// The New menu. Above the separator are files an instrument holds: each family's
/// defaults, and the two instrument files built from audio. Below it are files only this
/// computer keeps: a note, a Sample Editor project, and a folder.
///
/// ⚠️ One menu, used everywhere. The tree's context menu, the File menu, the top bar and
/// the tab row all offer "New", and four different menus of one name would be four
/// things to learn. Connecting an instrument makes nothing on this computer, so it is on
/// the tree's instrument row instead.
pub fn new_menu(ui: &mut egui::Ui, acts: &mut Vec<Act>) {
    drop_down_style(ui);
    new_lines(ui, &new_entries(), acts);
}

fn new_lines(ui: &mut egui::Ui, entries: &[Entry], acts: &mut Vec<Act>) {
    for entry in entries {
        match entry {
            Entry::Rule => {
                ui.separator();
            }
            Entry::Sub(title, inner) => {
                ui.menu_button(*title, |ui| {
                    drop_down_style(ui);
                    new_lines(ui, inner, acts);
                });
            }
            Entry::Do(command) => {
                if item_button(ui, &Offer::of(*command), None) {
                    acts.extend(made(*command));
                    ui.close();
                }
            }
        }
    }
}

/// What a New line asks for.
fn made(command: Command) -> Option<Act> {
    match command {
        Command::New(kind) => Some(Act::New(kind)),
        Command::FromWavs(making) => Some(Act::NewFromWavs(making)),
        Command::NewFolder => Some(Act::NewFolder),
        _ => None,
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
/// offered now, so it never falls through to a shorter one. In a browser, `index.html`
/// also keeps each of these keys from the browser's own action.
const KEYED: [Command; 12] = [
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
];

/// The minimum width of a drop-down menu, so its width does not change with which items
/// are enabled and a long label keeps a gap before its key text.
const MENU: f32 = 260.0;

/// The height of a menu item, and of a section's title in the one-button menu.
const ITEM: f32 = 28.0;
const SECTION: f32 = 24.0;

/// The padding at each end of a title in Windows' menu bar.
const TITLE_PAD: f32 = 9.0;

/// The check column's glyph.
const CHECK: f32 = 13.0;

impl DrawbarApp {
    /// What `command` looks like in a menu now, or `None` when it is not offered: an
    /// in-window menu leaves the line out, and the Mac's menu bar disables it.
    pub(crate) fn offer(&self, command: Command) -> Option<Offer> {
        let attached = self.attached();
        let active = self.tabs.active();
        let waiting = self.queue.len();
        let plain = Offer::of(command);
        let check = |on| Offer {
            checked: Some(on),
            ..Offer::of(command)
        };
        let relabeled = |label: String| Offer {
            label,
            ..Offer::of(command)
        };
        Some(match command {
            Command::Open
            | Command::New(_)
            | Command::FromWavs(_)
            | Command::NewFolder
            | Command::Guide
            | Command::WhatsNew
            | Command::Welcome
            | Command::CopyLog
            | Command::About => plain,
            Command::Save | Command::Export => active.map(|_| plain)?,
            Command::Revert => {
                let unsaved = self
                    .workspace
                    .get(active?)
                    .is_some_and(crate::workspace::LocalEntity::is_unsaved);
                unsaved.then_some(plain)?
            }
            Command::CloseTab => (self.tabs.showing() != Spot::Library).then_some(plain)?,
            Command::Quit => match self.platform {
                Platform::Mac => relabeled("Quit drawbar".to_string()),
                Platform::Windows | Platform::Linux | Platform::Web => plain,
            },
            Command::Keyboard => attached.then(|| check(self.tabs.showing() == Spot::Keyboard))?,
            Command::Document => {
                let id = self.tabs.last_document()?;
                check(self.tabs.showing() == Spot::Document(id))
            }
            Command::Browser => check(self.shell.open(Dock::Browser)),
            Command::Inspector => check(self.shell.open(Dock::Inspector)),
            Command::Activity => check(self.shell.log_open),
            Command::Theme(choice) => check(self.theme == choice),
            Command::Connect => (!attached).then_some(plain)?,
            Command::Disconnect | Command::ReadEverything | Command::ReviewQueue => {
                attached.then_some(plain)?
            }
            Command::ReadAgain => {
                let class = self.open_class().filter(|_| attached)?;
                relabeled(format!("Read {} again", folder(class)))
            }
            Command::SendAll => {
                (attached && waiting > 0).then(|| relabeled(format!("Send all ({waiting})")))?
            }
            Command::ClearQueue => (attached && waiting > 0).then_some(plain)?,
            Command::Unqueue => (attached && !self.queued_picks().is_empty()).then_some(plain)?,
            Command::Listen => {
                let supported = crate::midi::supported();
                Offer {
                    enabled: supported,
                    hint: (!supported).then_some(crate::midi::UNSUPPORTED),
                    ..check(self.midi.on())
                }
            }
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
            Command::New(_) | Command::FromWavs(_) | Command::NewFolder => {
                acts.extend(made(command))
            }
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
        // ⚠️ A menu bar takes all the width it is given, so it is given only what its
        // titles need. Otherwise it would leave the search no room in the top bar.
        let font = crate::app::ui().resolve(ui.style());
        let gap = ui.spacing().item_spacing.x;
        let titles: f32 = menus
            .iter()
            .map(|menu| {
                let text = ui.painter().layout_no_wrap(
                    menu.title.to_owned(),
                    font.clone(),
                    egui::Color32::PLACEHOLDER,
                );
                text.size().x + 2.0 * TITLE_PAD + gap
            })
            .sum();
        ui.allocate_ui(egui::vec2(titles, ITEM), |ui| {
            egui::containers::menu::MenuBar::new().ui(ui, |ui| {
                for menu in &menus {
                    title_style(ui);
                    ui.menu_button(menu.title, |ui| {
                        drop_down_style(ui);
                        self.entries(ui, frame, &menu.entries, acts);
                    });
                }
            });
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
        item_button(ui, offer, key_text(command, self.platform, mac))
    }
}

/// One menu item: the check column, the label, and `keys` at the right. Its hint shows
/// on hover, enabled or not.
fn item_button(ui: &mut egui::Ui, offer: &Offer, keys: Option<String>) -> bool {
    let mut button = check(ui, &offer.label, offer.checked == Some(true));
    if let Some(keys) = keys {
        button =
            button.shortcut_text(egui::RichText::new(keys).font(egui::FontId::monospace(10.5)));
    }
    let response = ui.add_enabled(offer.enabled, button);
    let response = match offer.hint {
        Some(hint) => response.on_hover_text(hint).on_disabled_hover_text(hint),
        None => response,
    };
    response.clicked()
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
    style.spacing.button_padding = egui::vec2(TITLE_PAD, 5.0);
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
    use crate::testing::{self, context, words};

    /// ⚠️ Everything built from audio is on the New menu. One pick of WAVs can make any
    /// of them, and a menu offering only some would hide what the dialog does.
    #[test]
    fn the_new_menu_offers_everything_a_pick_of_wavs_makes() {
        let output = testing::run(&context(), egui::RawInput::default(), |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| new_menu(ui, &mut Vec::new()));
        });
        let said = words(&output);
        for making in Making::FROM_WAVS {
            let item = making.item().0;
            assert!(said.iter().any(|word| word == item), "{item} is missing");
        }
        assert!(said.iter().any(|word| word == "New folder"));
    }

    /// ⚠️ The New menu's separator splits files an instrument holds from files only this
    /// computer keeps. A kind on the wrong side would misstate where the new file can go.
    #[test]
    fn the_new_menu_parts_instrument_files_from_the_rest() {
        let output = testing::run(&context(), egui::RawInput::default(), |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| new_menu(ui, &mut Vec::new()));
        });
        let said = words(&output);
        let at = |word: &str| {
            said.iter()
                .position(|held| held == word)
                .unwrap_or_else(|| panic!("{word} is missing: {said:?}"))
        };
        let rule = Fresh::FAMILIES
            .iter()
            .map(|family| at(family.label))
            .chain(
                Making::FROM_WAVS
                    .iter()
                    .filter(|making| making.instrument_file())
                    .map(|making| at(making.item().0)),
            )
            .max()
            .expect("the instrument files are above it");
        let below: Vec<&str> = Fresh::LOOSE
            .iter()
            .map(|kind| kind.label())
            .chain(
                Making::FROM_WAVS
                    .iter()
                    .filter(|making| !making.instrument_file())
                    .map(|making| making.item().0),
            )
            .chain(["New folder"])
            .collect();
        for item in below {
            assert!(at(item) > rule, "{item} belongs below the rule: {said:?}");
        }
    }

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

    /// The page stops the browser acting on a key, by its modifiers and its key's name.
    fn page_name(keys: egui::KeyboardShortcut) -> String {
        let with = keys.modifiers;
        let mut held: Vec<String> = [
            (with.command, "mod"),
            (with.ctrl && !with.command, "ctrl"),
            (with.alt, "alt"),
            (with.shift, "shift"),
        ]
        .iter()
        .filter(|(on, _)| *on)
        .map(|(_, name)| name.to_string())
        .collect();
        held.push(keys.logical_key.name().to_lowercase());
        held.join("+")
    }

    /// ⚠️ A browser acts on ⌘R and the rest even when the app takes them: it would reload
    /// the page out from under unsaved work. Every key the web build binds is kept from
    /// it.
    #[test]
    fn the_page_keeps_every_key_the_web_build_binds_from_the_browser() {
        let page = include_str!("../index.html");
        for mac in [true, false] {
            let bound = KEYED
                .iter()
                .filter_map(|command| shortcut(*command, Platform::Web, mac))
                .chain([search_key()]);
            for keys in bound {
                let name = format!("\"{}\"", page_name(keys));
                assert!(page.contains(&name), "index.html does not take {name}");
            }
        }
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
