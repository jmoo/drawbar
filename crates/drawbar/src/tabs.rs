//! The center: one tab per open document, plus the library and the keyboard.
//!
//! A document tab shows an asset on this computer. Opening something from the instrument
//! copies it here first, so a tab always holds a working copy: editing it changes nothing
//! on the instrument until it is sent back. The library and the keyboard show what is
//! already there, so they hold nothing.

use eframe::egui;
use nord_usb::ObjectClass;

use crate::browser::{Act, Kind};
use crate::icon::{painted, Glyph};
use crate::panel::{GAP, GLYPH, PAD};
use crate::shell::new_button;
use crate::workspace::Workspace;

/// ⚠️ The strip's scroll id. The strip and the document body are drawn into the same
/// `Ui`, and egui salts an unsalted `ScrollArea` with only that `Ui`, so two of them
/// there would share one state and a wheel over the body would scroll the strip.
pub const SCROLL: &str = "tab_strip";

/// How tall the strip is.
pub const HEIGHT: f32 = 26.0;

/// Which of the center's views a tab shows.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Spot {
    Document(u64),
    Library,
    Keyboard,
}

pub struct Tabs {
    /// ⚠️ [`Spot::Library`] is first and stays there: the center falls back to it, so
    /// nothing closes or moves it.
    open: Vec<Spot>,
    active: Option<Spot>,
    /// The class the keyboard tab shows, as the tree last set it. There is one keyboard
    /// tab, so the class is its state, not a separate tab.
    keyboard: Option<ObjectClass>,
}

impl Default for Tabs {
    fn default() -> Tabs {
        Tabs {
            open: vec![Spot::Library],
            active: Some(Spot::Library),
            keyboard: None,
        }
    }
}

impl Tabs {
    /// Open a document, or bring its existing tab forward.
    pub fn open(&mut self, id: u64) {
        if !self.holds(id) {
            self.open.push(Spot::Document(id));
        }
        self.active = Some(Spot::Document(id));
    }

    /// Bring a tab forward, opening the keyboard if needed.
    ///
    /// ⚠️ There is one keyboard, so showing it opens it. The library is always open, and
    /// only [`Tabs::open`] makes a document tab.
    pub fn show(&mut self, spot: Spot) {
        let held = self.open.contains(&spot);
        match (held, spot) {
            (true, _) => {}
            (false, Spot::Keyboard) => self.open.push(Spot::Keyboard),
            (false, Spot::Library | Spot::Document(_)) => return,
        }
        self.active = Some(spot);
    }

    /// Switch the keyboard tab to a class. [`Tabs::show`] brings the tab forward; this
    /// sets what it shows.
    pub fn keyboard_on(&mut self, class: ObjectClass) {
        self.keyboard = Some(class);
    }

    /// The class the keyboard tab is switched to, if one has been set.
    pub fn keyboard_class(&self) -> Option<ObjectClass> {
        self.keyboard
    }

    /// Move the tab at `from` to position `to`, shifting the rest to close the gap. An
    /// index out of range moves nothing.
    ///
    /// The keyboard moves like any other tab. ⚠️ The library is the first tab and stays
    /// there: it never moves, and a tab dropped on it lands after it.
    pub fn reorder(&mut self, from: usize, to: usize) {
        let to = to.max(1);
        if from == 0 || from == to || from >= self.open.len() || to >= self.open.len() {
            return;
        }
        let tab = self.open.remove(from);
        self.open.insert(to, tab);
    }

    /// Close a tab, falling back to the last tab in the strip.
    ///
    /// ⚠️ The library is the final fallback, so closing it does nothing.
    pub fn close(&mut self, spot: Spot) {
        if spot == Spot::Library {
            return;
        }
        self.open.retain(|held| *held != spot);
        if self.active == Some(spot) {
            self.active = self.open.last().copied();
        }
    }

    /// What the center is drawing.
    pub fn showing(&self) -> Option<Spot> {
        self.active
    }

    /// The document the center shows, if any.
    pub fn active(&self) -> Option<u64> {
        match self.active {
            Some(Spot::Document(id)) => Some(id),
            _ => None,
        }
    }

    /// The active document, or else the last document tab in the strip.
    pub fn last_document(&self) -> Option<u64> {
        self.active().or_else(|| {
            self.open.iter().rev().find_map(|spot| match spot {
                Spot::Document(id) => Some(*id),
                Spot::Library | Spot::Keyboard => None,
            })
        })
    }

    /// Whether a tab is open on this document, active or not.
    pub fn holds(&self, id: u64) -> bool {
        self.open.contains(&Spot::Document(id))
    }

    /// Drop tabs whose asset is no longer on this computer.
    pub fn prune(&mut self, workspace: &Workspace) {
        self.open.retain(|spot| match spot {
            Spot::Document(id) => workspace.entities().iter().any(|e| e.id == *id),
            Spot::Library | Spot::Keyboard => true,
        });
        if self.active.is_some_and(|spot| !self.open.contains(&spot)) {
            self.active = self.open.last().copied();
        }
    }

    /// The strip. The open view draws itself below it.
    ///
    /// ⚠️ The scroll area is a direct child of the caller's `Ui`, and its salt is
    /// [`SCROLL`]: the document body below has its own salt, and two unsalted areas in
    /// one `Ui` would share a state.
    pub fn ui(&mut self, ui: &mut egui::Ui, workspace: &Workspace, acts: &mut Vec<Act>) {
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
            // A scroll bar would take a third of the 26 px strip.
            .scroll_bar_visibility(egui::scroll_area::ScrollBarVisibility::AlwaysHidden)
            .auto_shrink([false; 2])
            .show(ui, |ui| {
                ui.spacing_mut().item_spacing = egui::Vec2::ZERO;
                ui.horizontal(|ui| {
                    for (index, spot) in self.open.iter().copied().enumerate() {
                        let Some(face) = face(spot, workspace) else {
                            continue;
                        };
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
                        if let Some(hint) = &face.hint {
                            label = label.on_hover_text(hint);
                        }
                        if label.clicked() {
                            activate = Some(spot);
                        }
                        if drawn.close.is_some_and(|shut| shut.clicked()) {
                            close = Some(spot);
                        }
                    }
                    let ink = crate::app::caption(ui.visuals());
                    new_button(ui, Glyph::Plus, ink, acts);
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

/// The tab a drag was dropped on: the one under the pointer, or the tab at whichever end
/// it went past.
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

/// A tab as drawn: its glyph, its name, and whether it is saved.
struct Face {
    glyph: Glyph,
    name: String,
    /// It differs from what was last saved, shown by an italic name with a star, as
    /// everywhere else.
    unsaved: bool,
    /// The hover text: the full name, and anything else to know about this tab.
    hint: Option<String>,
    /// Whether the tab has a × at the end. The library has none: closes fall back to it.
    shut: bool,
}

fn face(spot: Spot, workspace: &Workspace) -> Option<Face> {
    match spot {
        Spot::Library => Some(Face {
            glyph: Glyph::LibraryBig,
            name: "Library".into(),
            unsaved: false,
            hint: None,
            shut: false,
        }),
        Spot::Keyboard => Some(Face {
            glyph: Glyph::Keyboard,
            name: "Keyboard".into(),
            unsaved: false,
            hint: None,
            shut: true,
        }),
        Spot::Document(id) => {
            let entity = workspace.get(id)?;
            Some(Face {
                glyph: Kind::of(entity).glyph(),
                name: entity.name.clone(),
                unsaved: entity.is_unsaved(),
                hint: Some(match workspace.is_view(id) {
                    true => format!("{}: the instrument's copy, viewed in place", entity.name),
                    false => entity.name.clone(),
                }),
                shut: true,
            })
        }
    }
}

/// The responses of a drawn tab.
struct Drawn {
    tab: egui::Response,
    /// The ×, on every tab that has one.
    close: Option<egui::Response>,
}

/// The size of the × at the end of a tab.
const SHUT: f32 = 11.0;

fn paint(ui: &mut egui::Ui, face: &Face, active: bool) -> Drawn {
    let visuals = ui.visuals().clone();
    let ink = match active {
        true => visuals.widgets.active.fg_stroke.color,
        false => crate::app::caption(&visuals),
    };
    let mut text = egui::RichText::new(crate::browser::starred(&face.name, face.unsaved))
        .text_style(crate::app::ui());
    if face.unsaved {
        text = text.italics();
    }
    let galley = egui::WidgetText::from(text.color(ink)).into_galley(
        ui,
        Some(egui::TextWrapMode::Extend),
        f32::INFINITY,
        crate::app::ui(),
    );

    let closes = match face.shut {
        true => GAP + SHUT,
        false => 0.0,
    };
    let width = PAD + GLYPH + GAP + galley.size().x + closes + PAD;
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
    let close = face.shut.then(|| {
        let shut = box_(x + GAP, SHUT);
        painted(ui, Glyph::X, shut, ink.gamma_multiply(0.5));
        ui.interact(shut, tab.id.with("close"), egui::Sense::click())
    });
    Drawn { tab, close }
}

/// Every string a frame painted, headers and button labels included.
#[cfg(test)]
pub(crate) fn words(output: &egui::FullOutput) -> Vec<String> {
    fn walk(shape: &egui::Shape, into: &mut Vec<String>) {
        match shape {
            egui::Shape::Text(text) => into.push(text.galley.text().to_string()),
            egui::Shape::Vec(shapes) => shapes.iter().for_each(|shape| walk(shape, into)),
            _ => {}
        }
    }
    let mut said = Vec::new();
    for clipped in &output.shapes {
        walk(&clipped.shape, &mut said);
    }
    said
}

#[cfg(test)]
mod tests {
    use super::*;

    fn workspace() -> Workspace {
        Workspace::new(egui::Context::default())
    }

    #[test]
    fn opening_an_asset_that_is_already_open_just_activates_it() {
        let mut tabs = Tabs::default();
        tabs.open(1);
        tabs.open(2);
        tabs.open(1);
        assert_eq!(tabs.open.len(), 3, "the library, and the two documents");
        assert_eq!(tabs.active(), Some(1));
    }

    /// Closing the last document falls back to the library.
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

    /// The strip is never empty.
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
        assert_eq!(tabs.open, vec![Spot::Library]);
        assert_eq!(tabs.showing(), Some(Spot::Library));
    }

    #[test]
    fn closing_a_background_tab_leaves_the_front_one_showing() {
        let mut tabs = Tabs::default();
        tabs.open(1);
        tabs.open(2);
        tabs.close(Spot::Document(1));
        assert_eq!(tabs.active(), Some(2));
    }

    /// Whether a tab is open does not depend on the workspace: a tab over a missing
    /// asset is still open.
    #[test]
    fn a_tab_says_whether_it_is_open_whatever_it_holds() {
        let mut tabs = Tabs::default();
        assert!(!tabs.holds(1));
        // No asset has this id.
        tabs.open(1);
        tabs.open(2);
        assert!(tabs.holds(1) && tabs.holds(2), "both are open");
        assert!(!tabs.holds(3));

        // A background tab still counts.
        tabs.close(Spot::Document(2));
        assert!(tabs.holds(1) && !tabs.holds(2));
        tabs.close(Spot::Document(1));
        assert!(!tabs.holds(1));
    }

    /// An unsaved document's name takes a star, as in the tree and the table.
    #[test]
    fn a_tab_over_an_unsaved_document_wears_a_star() {
        let ctx = egui::Context::default();
        ctx.all_styles_mut(crate::app::metrics);
        let mut ws = Workspace::new(ctx.clone());
        let mut log = crate::log::Log::default();
        let id = ws
            .create(crate::workspace::Fresh::Program, &mut log)
            .unwrap();
        ws.rename(id, "Africa Split".into());
        let mut tabs = Tabs::default();
        tabs.open(id);

        let strip = |tabs: &mut Tabs, ws: &Workspace| {
            let input = egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(600.0, 300.0),
                )),
                ..Default::default()
            };
            let output = ctx.run(input, |ctx| {
                egui::CentralPanel::default()
                    .frame(egui::Frame::new())
                    .show(ctx, |ui| {
                        tabs.ui(ui, ws, &mut Vec::new());
                    });
            });
            words(&output)
        };

        let said = strip(&mut tabs, &ws);
        assert!(said.contains(&"Africa Split".to_string()), "{said:?}");

        let bytes = ws.get(id).unwrap().bytes.clone();
        let (_, edited) =
            crate::fields::apply(&bytes, &[("center_panel.gain".into(), "96".into())]).unwrap();
        ws.replace_bytes(id, edited, &mut log);

        let said = strip(&mut tabs, &ws);
        assert!(said.contains(&"Africa Split*".to_string()), "{said:?}");
    }

    #[test]
    fn the_library_and_the_keyboard_are_each_one_tab() {
        let mut tabs = Tabs::default();
        tabs.open(1);
        tabs.show(Spot::Keyboard);
        tabs.show(Spot::Library);
        assert_eq!(tabs.open.len(), 3);
        assert_eq!(tabs.showing(), Some(Spot::Library));
        // The center shows the library, so no document is active.
        assert_eq!(tabs.active(), None);
        assert_eq!(tabs.last_document(), Some(1));
    }

    /// Only `open` makes a document tab; `show` leaves the front tab unchanged.
    #[test]
    fn showing_a_document_that_no_tab_holds_changes_nothing() {
        let mut tabs = Tabs::default();
        tabs.show(Spot::Library);
        tabs.show(Spot::Document(7));
        assert_eq!(tabs.showing(), Some(Spot::Library));
        assert!(!tabs.holds(7));
    }

    /// ⚠️ The library stays first: it can be neither end of a move.
    #[test]
    fn reordering_moves_one_tab_and_closes_the_strip_up_behind_it() {
        let mut tabs = Tabs::default();
        tabs.open(1);
        tabs.open(2);
        tabs.show(Spot::Keyboard);
        let order = |tabs: &Tabs| tabs.open.clone();

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
            "a tab dropped on the library lands after it"
        );
        assert_eq!(
            tabs.showing(),
            Some(Spot::Keyboard),
            "moving is not showing"
        );
    }

    /// An out-of-range index moves nothing and does not panic.
    #[test]
    fn reordering_past_the_end_of_the_strip_moves_nothing() {
        let mut tabs = Tabs::default();
        tabs.open(1);
        tabs.open(2);
        let before = tabs.open.clone();
        for (from, to) in [(0, 0), (0, 2), (5, 1), (9, 9)] {
            tabs.reorder(from, to);
        }
        assert_eq!(tabs.open, before);
    }

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

    /// A release after a drag is a drop, not a click, so nothing is activated.
    #[test]
    fn dragging_a_tab_across_its_neighbor_swaps_them() {
        let ctx = egui::Context::default();
        ctx.all_styles_mut(crate::app::metrics);
        let mut ws = Workspace::new(ctx.clone());
        let mut log = crate::log::Log::default();
        let first = ws
            .create(crate::workspace::Fresh::Program, &mut log)
            .unwrap();
        let second = ws.create(crate::workspace::Fresh::Live, &mut log).unwrap();
        // Long enough that the tab extends well past the library's tab, which a drag may
        // not start on.
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
                        tabs.ui(ui, &ws, &mut Vec::new());
                    });
            });
        }

        assert_eq!(
            tabs.open,
            vec![Spot::Library, Spot::Document(second), Spot::Document(first)],
            "the dragged tab landed past its neighbor"
        );
        assert_eq!(
            tabs.showing(),
            Some(Spot::Document(second)),
            "a drop is not a click, so what was in front stayed in front"
        );
    }

    /// The library and keyboard tabs show no single asset, so pruning never removes them.
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
