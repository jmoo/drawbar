//! The centre: one tab per open document, plus the library and the keyboard.
//!
//! A document tab is a view of an asset on this computer. Opening something off the
//! instrument copies it here first, so what a tab holds is always a working copy —
//! editing it changes nothing on the instrument until it is sent back. The library and
//! the keyboard are views of what is already there, so they hold nothing.

use eframe::egui;
use nord_usb::ObjectClass;

use crate::browser::{new_menu, Act, Kind};
use crate::icon::{painted, Glyph};
use crate::workspace::Workspace;

/// ⚠️ The strip's own scroll id. The strip and the document body are drawn into the same
/// `Ui`, and egui salts an unsalted `ScrollArea` with that `Ui` alone — two of them there
/// share one state, and a wheel over the body moves the strip instead of the document.
pub const SCROLL: &str = "tab_strip";

/// How tall the strip is.
pub const HEIGHT: f32 = 26.0;

/// Which of the centre's views a tab shows.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Spot {
    Document(u64),
    Library,
    Keyboard,
}

enum Tab {
    Document {
        id: u64,
        /// The bytes as the tab opened them: what Revert goes back to, and what the byte
        /// diff is measured against. Held per tab, so switching tabs does not lose it.
        opened: Vec<u8>,
    },
    Library,
    Keyboard,
}

impl Tab {
    fn spot(&self) -> Spot {
        match self {
            Tab::Document { id, .. } => Spot::Document(*id),
            Tab::Library => Spot::Library,
            Tab::Keyboard => Spot::Keyboard,
        }
    }
}

#[derive(Default)]
pub struct Tabs {
    open: Vec<Tab>,
    active: Option<Spot>,
    /// Which class the keyboard tab is switched to, as the tree last asked. There is one
    /// keyboard tab, so the class it is on is the tab's state rather than a tab of its
    /// own.
    keyboard: Option<ObjectClass>,
}

impl Tabs {
    /// Open a document, or bring the tab already on it forward.
    pub fn open(&mut self, id: u64, workspace: &Workspace) {
        if !self.holds(id) {
            let opened = workspace
                .get(id)
                .map(|e| e.bytes.clone())
                .unwrap_or_default();
            self.open.push(Tab::Document { id, opened });
        }
        self.active = Some(Spot::Document(id));
    }

    /// Bring a tab forward, making it if it is one of the singletons.
    ///
    /// ⚠️ There is one library and one keyboard, so showing either is opening it. A
    /// document is not made here: only [`Tabs::open`] has the bytes a document tab holds.
    pub fn show(&mut self, spot: Spot) {
        let held = self.open.iter().any(|tab| tab.spot() == spot);
        match (held, spot) {
            (true, _) => {}
            (false, Spot::Library) => self.open.push(Tab::Library),
            (false, Spot::Keyboard) => self.open.push(Tab::Keyboard),
            (false, Spot::Document(_)) => return,
        }
        self.active = Some(spot);
    }

    /// Switch the keyboard tab to a class. Bringing the tab forward is [`Tabs::show`];
    /// this says what it opens on.
    pub fn keyboard_on(&mut self, class: ObjectClass) {
        self.keyboard = Some(class);
    }

    /// The class the keyboard tab is switched to, if anything has asked for one.
    pub fn keyboard_class(&self) -> Option<ObjectClass> {
        self.keyboard
    }

    pub fn close(&mut self, spot: Spot) {
        self.open.retain(|tab| tab.spot() != spot);
        if self.active == Some(spot) {
            self.active = self.open.last().map(Tab::spot);
        }
    }

    /// What the centre is drawing.
    pub fn showing(&self) -> Option<Spot> {
        self.active
    }

    /// The document the centre is on, if it is on one.
    pub fn active(&self) -> Option<u64> {
        match self.active {
            Some(Spot::Document(id)) => Some(id),
            _ => None,
        }
    }

    /// The document tab nearest the front, whether or not it is showing.
    pub fn last_document(&self) -> Option<u64> {
        self.active().or_else(|| {
            self.open.iter().rev().find_map(|tab| match tab.spot() {
                Spot::Document(id) => Some(id),
                _ => None,
            })
        })
    }

    /// Whether a tab is open on this document, in front or behind.
    pub fn holds(&self, id: u64) -> bool {
        self.open.iter().any(|tab| tab.spot() == Spot::Document(id))
    }

    /// What the tab looked like when it opened.
    pub fn opened(&self, id: u64) -> &[u8] {
        self.open
            .iter()
            .find_map(|tab| match tab {
                Tab::Document { id: held, opened } if *held == id => Some(opened.as_slice()),
                _ => None,
            })
            .unwrap_or(&[])
    }

    /// Drop tabs whose asset is no longer on this computer.
    pub fn prune(&mut self, workspace: &Workspace) {
        self.open.retain(|tab| match tab {
            Tab::Document { id, .. } => workspace.entities().iter().any(|e| e.id == *id),
            Tab::Library | Tab::Keyboard => true,
        });
        if self
            .active
            .is_some_and(|spot| !self.open.iter().any(|tab| tab.spot() == spot))
        {
            self.active = self.open.last().map(Tab::spot);
        }
    }

    /// The strip. The open view draws itself below it.
    ///
    /// ⚠️ The scroll area is a direct child of the caller's `Ui`, and its salt is
    /// [`SCROLL`]: the document body below carries its own, and two unsalted areas in one
    /// `Ui` would share a state.
    pub fn ui(&mut self, ui: &mut egui::Ui, workspace: &Workspace, acts: &mut Vec<Act>) {
        let rect = egui::Rect::from_min_size(
            egui::pos2(ui.max_rect().left(), ui.cursor().top()),
            egui::vec2(ui.available_width(), HEIGHT),
        );
        ui.painter()
            .rect_filled(rect, 0.0, ui.visuals().window_fill);

        let mut close = None;
        let mut activate = None;
        egui::ScrollArea::horizontal()
            .id_salt(SCROLL)
            .max_height(HEIGHT)
            // A bar inside 26 px would take a third of the strip it is scrolling.
            .scroll_bar_visibility(egui::scroll_area::ScrollBarVisibility::AlwaysHidden)
            .auto_shrink([false; 2])
            .show(ui, |ui| {
                ui.spacing_mut().item_spacing = egui::Vec2::ZERO;
                ui.horizontal(|ui| {
                    let visuals = ui.visuals().clone();
                    for tab in &self.open {
                        let Some(face) = face(tab, workspace, &visuals) else {
                            continue;
                        };
                        let spot = tab.spot();
                        let drawn = paint(ui, &face, self.active == Some(spot));
                        let mut label = drawn.tab;
                        if let Some(hint) = face.hint {
                            label = label.on_hover_text(hint);
                        }
                        if label.clicked() {
                            activate = Some(spot);
                        }
                        if drawn.close.clicked() {
                            close = Some(spot);
                        }
                    }
                    plus(ui, acts);
                });
            });
        if let Some(spot) = activate {
            self.active = Some(spot);
        }
        if let Some(spot) = close {
            self.close(spot);
        }
    }
}

/// A tab as it is drawn: what it wears, what it says, and what it is owed.
struct Face {
    glyph: Glyph,
    name: String,
    /// A view of the instrument's copy reads differently from a tab holding this
    /// computer's own.
    borrowed: bool,
    /// The dot, and what it means.
    mark: Option<(egui::Color32, &'static str)>,
    hint: Option<&'static str>,
}

fn face(tab: &Tab, workspace: &Workspace, visuals: &egui::Visuals) -> Option<Face> {
    match tab {
        Tab::Library => Some(Face {
            glyph: Glyph::LibraryBig,
            name: "Library".into(),
            borrowed: false,
            mark: None,
            hint: None,
        }),
        Tab::Keyboard => Some(Face {
            glyph: Glyph::Keyboard,
            name: "Keyboard".into(),
            borrowed: false,
            mark: None,
            hint: None,
        }),
        Tab::Document { id, .. } => {
            let entity = workspace.get(*id)?;
            let mark = match (entity.pending, entity.dirty) {
                (true, _) => Some((crate::app::warn(visuals), "waiting to be sent")),
                (false, true) => Some((crate::app::good(visuals), "changed since it was opened")),
                (false, false) => None,
            };
            Some(Face {
                glyph: Kind::of(entity.entity.as_ref()).glyph(),
                name: entity.name.clone(),
                borrowed: workspace.is_view(*id),
                mark,
                hint: workspace
                    .is_view(*id)
                    .then_some("the instrument's copy, viewed in place"),
            })
        }
    }
}

/// What a click on a drawn tab landed on.
struct Drawn {
    tab: egui::Response,
    close: egui::Response,
}

/// The kind glyph's box.
const GLYPH: f32 = 13.0;

/// The × at the end of every tab.
const SHUT: f32 = 11.0;

/// A tab's own padding, and the gap between its parts.
const PAD: f32 = 8.0;
const GAP: f32 = 6.0;

/// The dot's diameter when a tab owes something.
const DOT: f32 = 6.0;

fn paint(ui: &mut egui::Ui, face: &Face, active: bool) -> Drawn {
    let visuals = ui.visuals().clone();
    let ink = match active {
        true => visuals.widgets.active.fg_stroke.color,
        false => visuals.widgets.noninteractive.fg_stroke.color,
    };
    let mut text = egui::RichText::new(&face.name).text_style(crate::app::ui());
    if face.borrowed {
        text = text.italics();
    }
    let galley = egui::WidgetText::from(text.color(ink)).into_galley(
        ui,
        Some(egui::TextWrapMode::Extend),
        f32::INFINITY,
        crate::app::ui(),
    );

    let marked = match face.mark.is_some() {
        true => DOT + GAP,
        false => 0.0,
    };
    let width = PAD + GLYPH + GAP + galley.size().x + GAP + marked + SHUT + PAD;
    let (rect, tab) = ui.allocate_exact_size(egui::vec2(width, HEIGHT), egui::Sense::click());
    let painter = ui.painter().clone();

    if active {
        painter.rect_filled(rect, 0.0, visuals.panel_fill);
        painter.hline(
            rect.x_range(),
            rect.bottom() - 1.0,
            egui::Stroke::new(2.0_f32, crate::app::accent(&visuals)),
        );
    }
    painter.vline(
        rect.right() - 0.5,
        rect.y_range(),
        egui::Stroke::new(1.0_f32, visuals.widgets.noninteractive.bg_stroke.color),
    );

    let mut x = rect.left() + PAD;
    let box_ = |x: f32, size: f32| {
        egui::Rect::from_min_size(
            egui::pos2(x, rect.center().y - size / 2.0),
            egui::vec2(size, size),
        )
    };
    painted(ui, face.glyph, box_(x, GLYPH), ink);
    x += GLYPH + GAP;
    painter.galley(
        egui::pos2(x, rect.center().y - galley.size().y / 2.0),
        galley.clone(),
        egui::Color32::PLACEHOLDER,
    );
    x += galley.size().x + GAP;
    if let Some((color, why)) = face.mark {
        let at = box_(x, DOT);
        painter.circle_filled(at.center(), DOT / 2.0, color);
        ui.interact(at, tab.id.with("mark"), egui::Sense::hover())
            .on_hover_text(why);
        x += DOT + GAP;
    }
    let shut = box_(x, SHUT);
    painted(ui, Glyph::X, shut, ink.gamma_multiply(0.5));
    let close = ui.interact(shut, tab.id.with("close"), egui::Sense::click());
    Drawn { tab, close }
}

/// The one after the last tab: whatever the File menu's New offers.
fn plus(ui: &mut egui::Ui, acts: &mut Vec<Act>) {
    let ink = ui.visuals().widgets.noninteractive.fg_stroke.color;
    ui.menu_image_button(crate::icon::sized(Glyph::Plus, GLYPH, ink), |ui| {
        new_menu(ui, acts);
    })
    .response
    .on_hover_text("something new on this computer");
}

#[cfg(test)]
mod tests {
    use super::*;

    fn workspace() -> Workspace {
        Workspace::new(egui::Context::default())
    }

    /// Opening the same asset twice is the same tab, brought forward.
    #[test]
    fn opening_an_asset_that_is_already_open_just_activates_it() {
        let (mut tabs, ws) = (Tabs::default(), workspace());
        tabs.open(1, &ws);
        tabs.open(2, &ws);
        tabs.open(1, &ws);
        assert_eq!(tabs.open.len(), 2);
        assert_eq!(tabs.active(), Some(1));
    }

    /// Closing what is in front falls back to another tab rather than to nothing, and
    /// closing the last one leaves nothing showing.
    #[test]
    fn closing_the_active_tab_falls_back_to_another() {
        let (mut tabs, ws) = (Tabs::default(), workspace());
        tabs.open(1, &ws);
        tabs.open(2, &ws);
        tabs.close(Spot::Document(2));
        assert_eq!(tabs.active(), Some(1));
        tabs.close(Spot::Document(1));
        assert_eq!(tabs.active(), None);
    }

    /// Closing a tab that is not in front leaves the front one showing.
    #[test]
    fn closing_a_background_tab_leaves_the_front_one_showing() {
        let (mut tabs, ws) = (Tabs::default(), workspace());
        tabs.open(1, &ws);
        tabs.open(2, &ws);
        tabs.close(Spot::Document(1));
        assert_eq!(tabs.active(), Some(2));
    }

    /// Whether a tab is open is its own question, not one inferred from the bytes it
    /// opened with — a document that opened empty is still open.
    #[test]
    fn a_tab_says_whether_it_is_open_whatever_it_holds() {
        let (mut tabs, ws) = (Tabs::default(), workspace());
        assert!(!tabs.holds(1));
        // Nothing in the workspace under this id, so the tab opens with no bytes at all.
        tabs.open(1, &ws);
        tabs.open(2, &ws);
        assert!(tabs.opened(1).is_empty());
        assert!(tabs.holds(1) && tabs.holds(2), "both are open");
        assert!(!tabs.holds(3));

        // Behind the front one still counts.
        tabs.close(Spot::Document(2));
        assert!(tabs.holds(1) && !tabs.holds(2));
        tabs.close(Spot::Document(1));
        assert!(!tabs.holds(1));
    }

    /// Each tab keeps the bytes it opened with, so Revert in one is not Revert in
    /// another.
    #[test]
    fn each_tab_keeps_the_bytes_it_opened_with() {
        let (mut tabs, mut ws) = (Tabs::default(), workspace());
        let mut log = crate::log::Log::default();
        let first = ws
            .create(crate::workspace::Fresh::Program, &mut log)
            .unwrap();
        let second = ws
            .create(crate::workspace::Fresh::Settings, &mut log)
            .unwrap();
        tabs.open(first, &ws);
        tabs.open(second, &ws);

        assert_eq!(tabs.opened(first), ws.get(first).unwrap().bytes.as_slice());
        assert_ne!(tabs.opened(first), tabs.opened(second));
        // A tab that was never opened has nothing to go back to.
        assert!(tabs.opened(999).is_empty());
    }

    /// There is one library and one keyboard, so asking for either twice is one tab
    /// brought forward — and a document opened between them does not make a second.
    #[test]
    fn the_library_and_the_keyboard_are_each_one_tab() {
        let (mut tabs, ws) = (Tabs::default(), workspace());
        tabs.show(Spot::Library);
        tabs.open(1, &ws);
        tabs.show(Spot::Keyboard);
        tabs.show(Spot::Library);
        assert_eq!(tabs.open.len(), 3);
        assert_eq!(tabs.showing(), Some(Spot::Library));
        // The centre is on the library, so no document is open in it.
        assert_eq!(tabs.active(), None);
        assert_eq!(tabs.last_document(), Some(1));
    }

    /// A document is opened with its bytes or not at all: `show` cannot make one, and
    /// asking it to leaves what was in front where it was.
    #[test]
    fn showing_a_document_that_no_tab_holds_changes_nothing() {
        let mut tabs = Tabs::default();
        tabs.show(Spot::Library);
        tabs.show(Spot::Document(7));
        assert_eq!(tabs.showing(), Some(Spot::Library));
        assert!(!tabs.holds(7));
    }

    /// Pruning drops documents the list no longer holds; the two singletons are views of
    /// what is there rather than of an asset, so nothing prunes them.
    #[test]
    fn pruning_takes_documents_and_leaves_the_singletons() {
        let (mut tabs, mut ws) = (Tabs::default(), workspace());
        let mut log = crate::log::Log::default();
        let id = ws
            .create(crate::workspace::Fresh::Program, &mut log)
            .unwrap();
        tabs.show(Spot::Library);
        tabs.open(id, &ws);
        ws.remove(id, &mut log);
        tabs.prune(&ws);
        assert!(!tabs.holds(id));
        assert_eq!(tabs.showing(), Some(Spot::Library));
    }
}
