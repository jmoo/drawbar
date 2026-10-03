//! The macOS menu bar: the app menu, the window menu, and the same menus the other
//! platforms draw in the window, built from [`crate::menu::menus`].
//!
//! A native item's key equivalent is handled by the system before the window sees the
//! key, so a command bound here arrives as a menu event, never as a key press.

use std::sync::mpsc::{channel, Receiver};

use eframe::egui;
use muda::accelerator::{Accelerator, Code, Modifiers};
use muda::{CheckMenuItem, IsMenuItem, Menu, MenuId, MenuItem, PredefinedMenuItem, Submenu};

use crate::menu::{label, menus, shortcut, Command, Entry, Offer};
use crate::platform::Platform;

/// One native item and what it last showed, so it is only touched when that changes.
struct Line {
    command: Command,
    item: Item,
    shown: Option<(String, bool, Option<bool>)>,
    /// The label while the command is not offered, when its offered label varies.
    resting: String,
}

enum Item {
    Plain(MenuItem),
    Check(CheckMenuItem),
}

/// The menu bar, kept alive for as long as the app runs.
pub struct MenuBar {
    /// ⚠️ Dropping the menu takes it out of the menu bar.
    _menu: Menu,
    lines: Vec<Line>,
    picked: Receiver<MenuId>,
}

impl MenuBar {
    /// Build the menus and hand them to the application. Call once, on the main thread,
    /// after the application has finished launching.
    pub fn install(ctx: &egui::Context) -> Result<MenuBar, muda::Error> {
        let menu = Menu::new();
        let mut lines = Vec::new();

        let app = Submenu::new("drawbar", true);
        append(&app, &plain(&mut lines, Command::About, "About drawbar"));
        append(&app, &PredefinedMenuItem::separator());
        append(&app, &PredefinedMenuItem::services(None));
        append(&app, &PredefinedMenuItem::separator());
        append(&app, &PredefinedMenuItem::hide(Some("Hide drawbar")));
        append(&app, &PredefinedMenuItem::hide_others(None));
        append(&app, &PredefinedMenuItem::show_all(None));
        append(&app, &PredefinedMenuItem::separator());
        append(&app, &plain(&mut lines, Command::Quit, "Quit drawbar"));
        menu.append(&app)?;

        let mut help = None;
        for drawn in menus(Platform::Mac) {
            let submenu = Submenu::new(drawn.title, true);
            fill(&submenu, &drawn.entries, &mut lines);
            menu.append(&submenu)?;
            if drawn.title == "Help" {
                help = Some(submenu);
            }
        }

        let window = Submenu::new("Window", true);
        append(&window, &PredefinedMenuItem::minimize(None));
        append(&window, &PredefinedMenuItem::maximize(Some("Zoom")));
        append(&window, &PredefinedMenuItem::separator());
        append(&window, &PredefinedMenuItem::fullscreen(None));
        menu.insert(&window, menu.items().len().saturating_sub(1))?;

        menu.init_for_nsapp();
        window.set_as_windows_menu_for_nsapp();
        if let Some(help) = help {
            help.set_as_help_menu_for_nsapp();
        }

        let (send, picked) = channel();
        let ctx = ctx.clone();
        muda::MenuEvent::set_event_handler(Some(move |event: muda::MenuEvent| {
            // The receiver lives as long as the app; a failed send means it is exiting.
            let _ = send.send(event.id);
            ctx.request_repaint();
        }));
        Ok(MenuBar {
            _menu: menu,
            lines,
            picked,
        })
    }

    /// The commands picked from the menu bar since the last frame.
    pub fn picked(&self) -> Vec<Command> {
        self.picked
            .try_iter()
            .filter_map(|id| {
                self.lines
                    .iter()
                    .find(|line| line.item.id() == &id)
                    .map(|line| line.command)
            })
            .collect()
    }

    /// Bring each item's label, enablement, and check up to date with `offer`.
    pub fn refresh(&mut self, offer: impl Fn(Command) -> Option<Offer>) {
        for line in &mut self.lines {
            let now = match offer(line.command) {
                Some(offer) => (offer.label, offer.enabled, offer.checked),
                None => (line.resting.clone(), false, Some(false)),
            };
            if line.shown.as_ref() == Some(&now) {
                continue;
            }
            let (label, enabled, checked) = &now;
            match &line.item {
                Item::Plain(item) => {
                    item.set_text(label);
                    item.set_enabled(*enabled);
                }
                Item::Check(item) => {
                    item.set_text(label);
                    item.set_enabled(*enabled);
                    item.set_checked(checked.unwrap_or(false));
                }
            }
            line.shown = Some(now);
        }
    }
}

impl Item {
    fn id(&self) -> &MenuId {
        match self {
            Item::Plain(item) => item.id(),
            Item::Check(item) => item.id(),
        }
    }
}

/// Append an item. A failure here means the item was already in this menu, which this
/// module never does.
fn append(submenu: &Submenu, item: &dyn IsMenuItem) {
    let _ = submenu.append(item);
}

/// Fill a native menu with `entries`.
fn fill(submenu: &Submenu, entries: &[Entry], lines: &mut Vec<Line>) {
    for entry in entries {
        match entry {
            Entry::Rule => append(submenu, &PredefinedMenuItem::separator()),
            Entry::Sub(title, inner) => {
                let child = Submenu::new(*title, true);
                fill(&child, inner, lines);
                append(submenu, &child);
            }
            Entry::Do(command) => {
                let resting = label(*command);
                match checkable(*command) {
                    true => {
                        let item = CheckMenuItem::new(resting, true, false, accelerator(*command));
                        append(submenu, &item);
                        lines.push(Line {
                            command: *command,
                            item: Item::Check(item),
                            shown: None,
                            resting: resting.to_string(),
                        });
                    }
                    false => append(submenu, &plain(lines, *command, resting)),
                }
            }
        }
    }
}

/// A plain item for `command`, recorded in `lines`.
fn plain(lines: &mut Vec<Line>, command: Command, label: &str) -> MenuItem {
    let item = MenuItem::new(label, true, accelerator(command));
    lines.push(Line {
        command,
        item: Item::Plain(item.clone()),
        shown: None,
        resting: label.to_string(),
    });
    item
}

/// Whether the command's item carries a check.
fn checkable(command: Command) -> bool {
    matches!(
        command,
        Command::Keyboard
            | Command::Document
            | Command::Browser
            | Command::Inspector
            | Command::Activity
            | Command::Theme(_)
            | Command::Listen
    )
}

/// The native key equivalent of a command's shortcut.
fn accelerator(command: Command) -> Option<Accelerator> {
    let keys = shortcut(command, Platform::Mac, true)?;
    let code = match keys.logical_key {
        egui::Key::B => Code::KeyB,
        egui::Key::E => Code::KeyE,
        egui::Key::I => Code::KeyI,
        egui::Key::L => Code::KeyL,
        egui::Key::O => Code::KeyO,
        egui::Key::Q => Code::KeyQ,
        egui::Key::R => Code::KeyR,
        egui::Key::S => Code::KeyS,
        egui::Key::W => Code::KeyW,
        egui::Key::Num2 => Code::Digit2,
        egui::Key::Num3 => Code::Digit3,
        _ => return None,
    };
    let with = keys.modifiers;
    let mut modifiers = Modifiers::empty();
    for (held, modifier) in [
        (with.command || with.mac_cmd, Modifiers::META),
        (with.alt, Modifiers::ALT),
        (with.shift, Modifiers::SHIFT),
        (with.ctrl && !with.command, Modifiers::CONTROL),
    ] {
        if held {
            modifiers |= modifier;
        }
    }
    Some(Accelerator::new(modifiers, code))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A command the Mac binds and the menu bar cannot express would leave its key dead:
    /// the key never reaches the window while a native item claims it, and no item would.
    #[test]
    fn every_key_the_mac_binds_has_a_native_key_equivalent() {
        let mut all = Vec::new();
        fn walk(entries: &[Entry], into: &mut Vec<Command>) {
            for entry in entries {
                match entry {
                    Entry::Do(command) => into.push(*command),
                    Entry::Sub(_, inner) => walk(inner, into),
                    Entry::Rule => {}
                }
            }
        }
        for menu in menus(Platform::Mac) {
            walk(&menu.entries, &mut all);
        }
        all.push(Command::Quit);
        for command in all {
            assert_eq!(
                shortcut(command, Platform::Mac, true).is_some(),
                accelerator(command).is_some(),
                "{command:?}"
            );
        }
    }
}
