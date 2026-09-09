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
use crate::queue::Queue;
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
    Document { id: u64 },
    Library,
    Keyboard,
}

impl Tab {
    fn spot(&self) -> Spot {
        match self {
            Tab::Document { id } => Spot::Document(*id),
            Tab::Library => Spot::Library,
            Tab::Keyboard => Spot::Keyboard,
        }
    }
}

pub struct Tabs {
    /// ⚠️ [`Tab::Library`] is the first of these and stays there: it is what the centre
    /// falls back to, so nothing closes it and nothing moves it.
    open: Vec<Tab>,
    active: Option<Spot>,
    /// Which class the keyboard tab is switched to, as the tree last asked. There is one
    /// keyboard tab, so the class it is on is the tab's state rather than a tab of its
    /// own.
    keyboard: Option<ObjectClass>,
}

impl Default for Tabs {
    fn default() -> Tabs {
        Tabs {
            open: vec![Tab::Library],
            active: Some(Spot::Library),
            keyboard: None,
        }
    }
}

impl Tabs {
    /// Open a document, or bring the tab already on it forward.
    pub fn open(&mut self, id: u64) {
        if !self.holds(id) {
            self.open.push(Tab::Document { id });
        }
        self.active = Some(Spot::Document(id));
    }

    /// Bring a tab forward, opening the keyboard if that is what is asked for.
    ///
    /// ⚠️ There is one keyboard, so showing it is opening it. The library is always
    /// open, and a document tab is made by [`Tabs::open`] alone.
    pub fn show(&mut self, spot: Spot) {
        let held = self.open.iter().any(|tab| tab.spot() == spot);
        match (held, spot) {
            (true, _) => {}
            (false, Spot::Keyboard) => self.open.push(Tab::Keyboard),
            (false, Spot::Library | Spot::Document(_)) => return,
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

    /// Move the tab at `from` to sit where the one at `to` is, the rest closing up
    /// behind it. An index the strip does not hold moves nothing.
    ///
    /// The keyboard moves like any other tab. ⚠️ The library is the first tab and stays
    /// there: it is never what moves, and a tab let go over it lands after it.
    pub fn reorder(&mut self, from: usize, to: usize) {
        let to = to.max(1);
        if from == 0 || from == to || from >= self.open.len() || to >= self.open.len() {
            return;
        }
        let tab = self.open.remove(from);
        self.open.insert(to, tab);
    }

    /// Shut a tab, falling back to whatever is nearest the front.
    ///
    /// ⚠️ The library is where every close lands, so asking to close it does nothing.
    pub fn close(&mut self, spot: Spot) {
        if spot == Spot::Library {
            return;
        }
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

    /// Drop tabs whose asset is no longer on this computer.
    pub fn prune(&mut self, workspace: &Workspace) {
        self.open.retain(|tab| match tab {
            Tab::Document { id } => workspace.entities().iter().any(|e| e.id == *id),
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
    pub fn ui(
        &mut self,
        ui: &mut egui::Ui,
        workspace: &Workspace,
        queue: &Queue,
        acts: &mut Vec<Act>,
    ) {
        let rect = egui::Rect::from_min_size(
            egui::pos2(ui.max_rect().left(), ui.cursor().top()),
            egui::vec2(ui.available_width(), HEIGHT),
        );
        ui.painter()
            .rect_filled(rect, 0.0, ui.visuals().window_fill);

        let mut close = None;
        let mut activate = None;
        let mut dropped = None;
        let mut painted: Vec<(usize, egui::Rect)> = Vec::new();
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
                    for (index, tab) in self.open.iter().enumerate() {
                        let Some(face) = face(tab, workspace, queue, &visuals) else {
                            continue;
                        };
                        let spot = tab.spot();
                        let drawn = paint(ui, &face, self.active == Some(spot));
                        painted.push((index, drawn.tab.rect));
                        let mut label = drawn.tab;
                        if label.dragged() {
                            ui.ctx().set_cursor_icon(egui::CursorIcon::Grabbing);
                        }
                        if let Some(at) = label
                            .drag_stopped()
                            .then(|| ui.ctx().pointer_interact_pos())
                            .flatten()
                        {
                            dropped = Some((index, at.x));
                        }
                        if let Some(hint) = face.hint {
                            label = label.on_hover_text(hint);
                        }
                        if label.clicked() {
                            activate = Some(spot);
                        }
                        if drawn.close.is_some_and(|shut| shut.clicked()) {
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
        if let Some((from, x)) = dropped {
            if let Some(to) = landing(&painted, x) {
                self.reorder(from, to);
            }
        }
    }
}

/// Which tab a drag was let go over: the one the pointer is inside, or the tab at
/// whichever end it was carried past.
fn landing(painted: &[(usize, egui::Rect)], x: f32) -> Option<usize> {
    let (first, left) = painted.first()?;
    let (last, right) = painted.last()?;
    if x < left.left() {
        return Some(*first);
    }
    if x > right.right() {
        return Some(*last);
    }
    painted
        .iter()
        .find(|(_, rect)| rect.x_range().contains(x))
        .map(|(index, _)| *index)
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
    /// The × at the end. The library has none: it is what a close falls back to.
    shut: bool,
}

fn face(tab: &Tab, workspace: &Workspace, queue: &Queue, visuals: &egui::Visuals) -> Option<Face> {
    match tab {
        Tab::Library => Some(Face {
            glyph: Glyph::LibraryBig,
            name: "Library".into(),
            borrowed: false,
            mark: None,
            hint: None,
            shut: false,
        }),
        Tab::Keyboard => Some(Face {
            glyph: Glyph::Keyboard,
            name: "Keyboard".into(),
            borrowed: false,
            mark: None,
            hint: None,
            shut: true,
        }),
        Tab::Document { id } => {
            let entity = workspace.get(*id)?;
            let mark = match (queue.holds(*id), entity.is_unsaved()) {
                (true, _) => Some((crate::app::warn(visuals), "waiting to be sent")),
                (false, true) => Some((crate::app::good(visuals), "not saved")),
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
                shut: true,
            })
        }
    }
}

/// What a click on a drawn tab landed on.
struct Drawn {
    tab: egui::Response,
    /// The ×, on every tab that has one.
    close: Option<egui::Response>,
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
        false => crate::app::caption(&visuals),
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
        true => GAP + DOT,
        false => 0.0,
    };
    let closes = match face.shut {
        true => GAP + SHUT,
        false => 0.0,
    };
    let width = PAD + GLYPH + GAP + galley.size().x + marked + closes + PAD;
    let (rect, tab) =
        ui.allocate_exact_size(egui::vec2(width, HEIGHT), egui::Sense::click_and_drag());
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
    x += galley.size().x;
    if let Some((color, why)) = face.mark {
        x += GAP;
        let at = box_(x, DOT);
        painter.circle_filled(at.center(), DOT / 2.0, color);
        ui.interact(at, tab.id.with("mark"), egui::Sense::hover())
            .on_hover_text(why);
        x += DOT;
    }
    let close = face.shut.then(|| {
        let shut = box_(x + GAP, SHUT);
        painted(ui, Glyph::X, shut, ink.gamma_multiply(0.5));
        ui.interact(shut, tab.id.with("close"), egui::Sense::click())
    });
    Drawn { tab, close }
}

/// The one after the last tab: whatever the File menu's New offers.
fn plus(ui: &mut egui::Ui, acts: &mut Vec<Act>) {
    ui.scope(|ui| {
        crate::panel::flat(ui);
        let ink = crate::app::caption(ui.visuals());
        ui.menu_image_button(crate::icon::sized(Glyph::Plus, GLYPH, ink), |ui| {
            new_menu(ui, acts);
        })
        .response
        .on_hover_text("something new on this computer");
    });
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
        let mut tabs = Tabs::default();
        tabs.open(1);
        tabs.open(2);
        tabs.open(1);
        assert_eq!(tabs.open.len(), 3, "the library, and the two documents");
        assert_eq!(tabs.active(), Some(1));
    }

    /// Closing what is in front falls back to another tab, and closing the last document
    /// falls back to the library.
    #[test]
    fn closing_the_active_tab_falls_back_to_another() {
        let mut tabs = Tabs::default();
        tabs.open(1);
        tabs.open(2);
        tabs.close(Spot::Document(2));
        assert_eq!(tabs.active(), Some(1));
        tabs.close(Spot::Document(1));
        assert_eq!(tabs.active(), None);
        assert_eq!(tabs.showing(), Some(Spot::Library));
    }

    /// ⚠️ The library is where a close lands, so it is not itself closable — and the
    /// strip is never empty, whatever is asked of it.
    #[test]
    fn closing_the_library_does_nothing_and_leaves_the_strip_standing() {
        let mut tabs = Tabs::default();
        tabs.open(1);
        tabs.show(Spot::Keyboard);
        for spot in [
            Spot::Library,
            Spot::Document(1),
            Spot::Keyboard,
            Spot::Library,
        ] {
            tabs.close(spot);
        }
        let left: Vec<Spot> = tabs.open.iter().map(Tab::spot).collect();
        assert_eq!(left, vec![Spot::Library]);
        assert_eq!(tabs.showing(), Some(Spot::Library));
    }

    /// Closing a tab that is not in front leaves the front one showing.
    #[test]
    fn closing_a_background_tab_leaves_the_front_one_showing() {
        let mut tabs = Tabs::default();
        tabs.open(1);
        tabs.open(2);
        tabs.close(Spot::Document(1));
        assert_eq!(tabs.active(), Some(2));
    }

    /// Whether a tab is open is its own question, not one inferred from what it is over
    /// — a document over nothing at all is still open.
    #[test]
    fn a_tab_says_whether_it_is_open_whatever_it_holds() {
        let mut tabs = Tabs::default();
        assert!(!tabs.holds(1));
        // Nothing in the workspace under this id, so the tab stands for nothing.
        tabs.open(1);
        tabs.open(2);
        assert!(tabs.holds(1) && tabs.holds(2), "both are open");
        assert!(!tabs.holds(3));

        // Behind the front one still counts.
        tabs.close(Spot::Document(2));
        assert!(tabs.holds(1) && !tabs.holds(2));
        tabs.close(Spot::Document(1));
        assert!(!tabs.holds(1));
    }

    /// There is one library and one keyboard, so asking for either twice is one tab
    /// brought forward — and a document opened between them does not make a second.
    #[test]
    fn the_library_and_the_keyboard_are_each_one_tab() {
        let mut tabs = Tabs::default();
        tabs.open(1);
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

    /// Moving a tab closes the strip up behind it, wherever it came from and wherever it
    /// lands. ⚠️ The library stays first: neither end of a move may be it.
    #[test]
    fn reordering_moves_one_tab_and_closes_the_strip_up_behind_it() {
        let mut tabs = Tabs::default();
        tabs.open(1);
        tabs.open(2);
        tabs.show(Spot::Keyboard);
        let order = |tabs: &Tabs| tabs.open.iter().map(Tab::spot).collect::<Vec<_>>();

        tabs.reorder(1, 3);
        assert_eq!(
            order(&tabs),
            vec![
                Spot::Library,
                Spot::Document(2),
                Spot::Keyboard,
                Spot::Document(1)
            ]
        );
        tabs.reorder(3, 1);
        assert_eq!(
            order(&tabs),
            vec![
                Spot::Library,
                Spot::Document(1),
                Spot::Document(2),
                Spot::Keyboard
            ]
        );
        tabs.reorder(0, 2);
        assert_eq!(
            order(&tabs),
            vec![
                Spot::Library,
                Spot::Document(1),
                Spot::Document(2),
                Spot::Keyboard
            ],
            "the library itself never moves"
        );
        tabs.reorder(2, 0);
        assert_eq!(
            order(&tabs),
            vec![
                Spot::Library,
                Spot::Document(2),
                Spot::Document(1),
                Spot::Keyboard
            ],
            "a tab let go over the library lands after it"
        );
        assert_eq!(
            tabs.showing(),
            Some(Spot::Keyboard),
            "moving is not showing"
        );
    }

    /// An index the strip does not hold is not a move, so nothing is dropped and nothing
    /// panics on the way.
    #[test]
    fn reordering_past_the_end_of_the_strip_moves_nothing() {
        let mut tabs = Tabs::default();
        tabs.open(1);
        tabs.open(2);
        let before = tabs.open.iter().map(Tab::spot).collect::<Vec<_>>();
        for (from, to) in [(0, 0), (0, 2), (5, 1), (9, 9)] {
            tabs.reorder(from, to);
        }
        assert_eq!(tabs.open.iter().map(Tab::spot).collect::<Vec<_>>(), before);
    }

    /// Where a drop lands: the tab under the pointer, or the tab at whichever end it was
    /// carried past.
    #[test]
    fn a_drop_lands_on_the_tab_under_it_or_on_the_end_it_passed() {
        let box_ = |left: f32, right: f32| {
            egui::Rect::from_min_max(egui::pos2(left, 0.0), egui::pos2(right, HEIGHT))
        };
        let painted = [(0, box_(0.0, 60.0)), (1, box_(60.0, 130.0))];
        assert_eq!(landing(&painted, 30.0), Some(0));
        assert_eq!(landing(&painted, 100.0), Some(1));
        assert_eq!(landing(&painted, -40.0), Some(0), "carried off the left");
        assert_eq!(landing(&painted, 900.0), Some(1), "carried off the right");
        assert_eq!(landing(&[], 30.0), None, "an empty strip takes no drop");
    }

    /// Dragging a tab across its neighbour and letting go swaps the two. Nothing is
    /// activated by it: a release that moved is a drop, not a click.
    #[test]
    fn dragging_a_tab_across_its_neighbour_swaps_them() {
        let ctx = egui::Context::default();
        ctx.all_styles_mut(crate::app::metrics);
        let mut ws = Workspace::new(ctx.clone());
        let mut log = crate::log::Log::default();
        let first = ws
            .create(crate::workspace::Fresh::Program, &mut log)
            .unwrap();
        let second = ws.create(crate::workspace::Fresh::Live, &mut log).unwrap();
        // Long enough that the tab reaches well past the library's own, which is the one
        // tab a drag may not start on.
        ws.rename(
            first,
            "Africa Split, the one with the long tail".to_string(),
        );
        let mut tabs = Tabs::default();
        tabs.open(first);
        tabs.open(second);

        // Inside the first document's tab, and far past the right of the last one.
        let (from, to) = (
            egui::pos2(200.0, HEIGHT / 2.0),
            egui::pos2(4_000.0, HEIGHT / 2.0),
        );
        let button = |pos, pressed| egui::Event::PointerButton {
            pos,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: egui::Modifiers::NONE,
        };
        let frames: [Vec<egui::Event>; 5] = [
            vec![egui::Event::PointerMoved(from)],
            vec![button(from, true)],
            vec![egui::Event::PointerMoved(to)],
            vec![button(to, false)],
            Vec::new(),
        ];
        for events in frames {
            let input = egui::RawInput {
                events,
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(600.0, 300.0),
                )),
                ..Default::default()
            };
            let _ = ctx.run(input, |ctx| {
                egui::CentralPanel::default()
                    .frame(egui::Frame::new())
                    .show(ctx, |ui| {
                        tabs.ui(ui, &ws, &Queue::default(), &mut Vec::new());
                    });
            });
        }

        assert_eq!(
            tabs.open.iter().map(Tab::spot).collect::<Vec<_>>(),
            vec![Spot::Library, Spot::Document(second), Spot::Document(first)],
            "the dragged tab landed past its neighbour"
        );
        assert_eq!(
            tabs.showing(),
            Some(Spot::Document(second)),
            "a drop is not a click, so what was in front stayed in front"
        );
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
        tabs.open(id);
        ws.remove(id, &mut log);
        tabs.prune(&ws);
        assert!(!tabs.holds(id));
        assert_eq!(tabs.showing(), Some(Spot::Library));
    }
}
