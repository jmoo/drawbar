//! The sidebar: the two places a sound can live, and the moving of sounds between them.
//!
//! Nothing here touches the instrument. Rendering reads the caches and answers with a
//! list of [`Act`]s, which [`apply`] then runs against the workspace, the device and the
//! tabs — so a row can be drawn while the thing it stands for is about to change.
//!
//! This file holds the state the two columns share — the selection, the in-place rename,
//! the one modal, the divider between them. The columns themselves are `computer` and
//! `instrument`; the drag vocabulary is `drag`, the row they are both painted from is
//! `row`, and the grouping of the local list lives outside the browser entirely, in
//! [`crate::folders`].

use std::sync::Arc;

use eframe::egui;
use nord_usb::{Location, ObjectClass};

use crate::device::Device;
use crate::folders::{self, Folders};
use crate::strings::place;
use crate::workspace::Workspace;

mod act;
#[cfg(test)]
mod bench;
mod computer;
mod drag;
mod instrument;
mod row;

pub use act::{apply, foreign_format, Act};
pub use drag::{landing, Carried, Item, Kind, Landing, Onto};
pub use row::{Cells, Drawn};

use act::{owed, write_warnings};
use drag::ghost;

/// An in-place rename, waiting on Enter or Esc.
struct Rename {
    what: Item,
    text: String,
    /// The first frame, in which the field takes focus and selects what is in it.
    fresh: bool,
}

/// A question that has to be answered before something is lost.
struct Ask {
    title: String,
    note: Option<String>,
    verb: &'static str,
    act: Act,
}

/// Whether a plain click starts a rename rather than moving the selection.
///
/// Both halves are needed. Selecting is the whole row's job, so a row that answers a
/// click anywhere would otherwise arm the editor on every second click.
pub fn arms_rename(selected: bool, on_name: bool) -> bool {
    selected && on_name
}

/// What Enter does to an in-place rename: nothing, or a new name.
///
/// A blank field is not a name and an unchanged one is not a rename, so both leave the
/// asset alone rather than sending an operation that would do nothing.
pub fn renamed(original: &str, typed: &str) -> Option<String> {
    let typed = typed.trim();
    match typed.is_empty() || typed == original.trim() {
        true => None,
        false => Some(typed.to_string()),
    }
}

pub struct Browser {
    selection: Option<Item>,
    rename: Option<Rename>,
    ask: Option<Ask>,
    /// Where the divider sits between the two columns, as a share of the dock.
    split: f32,
    folders: Folders,
    /// A slot to scroll to and select, once the list holding it has been drawn.
    jump: Option<(ObjectClass, Location)>,
}

impl Default for Browser {
    fn default() -> Browser {
        Browser {
            selection: None,
            rename: None,
            ask: None,
            split: EVEN,
            folders: Folders::default(),
            jump: None,
        }
    }
}

/// The divider's home: half the dock each.
const EVEN: f32 = 0.5;

/// Neither column may be dragged out of existence.
const LEAST: f32 = 0.15;

/// The strip the divider answers on.
const HANDLE: f32 = 7.0;

impl Browser {
    /// Where the divider is kept between sessions.
    pub const SPLIT: &'static str = "drawbar.dock_split";

    /// Put the divider and the folders back where they were left.
    ///
    /// Anything the store cannot account for is the even split: a fraction outside the
    /// stops would be one column showing and the other a sliver.
    pub fn restore(&mut self, storage: &dyn eframe::Storage) {
        self.split = storage
            .get_string(Browser::SPLIT)
            .and_then(|text| text.parse::<f32>().ok())
            .filter(|share| (LEAST..=1.0 - LEAST).contains(share))
            .unwrap_or(EVEN);
        self.folders = storage
            .get_string(folders::KEY)
            .map(|text| Folders::read(&text))
            .unwrap_or_default();
    }

    /// Reconcile the folders with the list that came back beside them. Call once, after
    /// both stores have been read.
    pub fn settle(&mut self, workspace: &Workspace) {
        self.folders.forget_missing(workspace);
    }

    pub fn keep(&self, storage: &mut dyn eframe::Storage) {
        storage.set_string(Browser::SPLIT, self.split.to_string());
        storage.set_string(folders::KEY, self.folders.written());
    }

    /// Draw the places a sound can live and collect what the user asked for.
    /// The instrument gets a column once there is an instrument.
    pub fn ui(&mut self, ui: &mut egui::Ui, workspace: &Workspace, device: &Device) -> Vec<Act> {
        let mut acts = Vec::new();
        self.dialog(ui.ctx(), &mut acts);
        match device.state.connected() {
            true => self.dock(ui, workspace, device, &mut acts),
            false => self.computer(ui, workspace, device, &mut acts),
        }
        ghost(ui.ctx());
        acts
    }

    /// The two columns and the divider between them.
    ///
    /// ⚠️ A share of the width rather than a number of points. The sidebar the two live
    /// in is itself resizable, and a column pinned to points eats the other one as the
    /// sidebar narrows — at which point the divider has nothing left to give back.
    fn dock(
        &mut self,
        ui: &mut egui::Ui,
        workspace: &Workspace,
        device: &Device,
        acts: &mut Vec<Act>,
    ) {
        let whole = ui.available_rect_before_wrap();
        let usable = (whole.width() - HANDLE).max(1.0);
        let left = usable * self.split;
        let divider = egui::Rect::from_min_size(
            egui::pos2(whole.left() + left, whole.top()),
            egui::vec2(HANDLE, whole.height()),
        );

        let dragging = ui
            .interact(
                divider,
                ui.id().with("dock_divider"),
                egui::Sense::click_and_drag(),
            )
            .on_hover_and_drag_cursor(egui::CursorIcon::ResizeHorizontal);
        if dragging.dragged() {
            self.split = ((left + dragging.drag_delta().x) / usable).clamp(LEAST, 1.0 - LEAST);
        }
        // Somewhere to put it back to, for a divider that has been dragged into a corner.
        if dragging.double_clicked() {
            self.split = EVEN;
        }

        let ends = |from: f32, to: f32| {
            egui::Rect::from_min_max(
                egui::pos2(from, whole.top()),
                egui::pos2(to, whole.bottom()),
            )
        };
        ui.scope_builder(
            egui::UiBuilder::new().max_rect(ends(whole.left(), divider.left())),
            |ui| self.computer(ui, workspace, device, acts),
        );
        ui.scope_builder(
            egui::UiBuilder::new().max_rect(ends(divider.right(), whole.right())),
            |ui| self.instrument(ui, workspace, device, acts),
        );

        let visuals = ui.visuals();
        let stroke = match dragging.hovered() || dragging.dragged() {
            true => egui::Stroke::new(2.0_f32, visuals.selection.stroke.color),
            false => visuals.widgets.noninteractive.bg_stroke,
        };
        ui.painter()
            .vline(divider.center().x, whole.y_range(), stroke);
        ui.advance_cursor_after_rect(whole);
    }

    /// A column heading, and the strip of buttons beside it.
    ///
    /// Wrapped, because the strip is in a column the operator can drag to any width and
    /// a button pushed off the right edge is a button that is gone.
    fn heading(
        &mut self,
        ui: &mut egui::Ui,
        title: &str,
        buttons: impl FnOnce(&mut egui::Ui),
    ) -> egui::Response {
        let head = ui
            .horizontal_wrapped(|ui| {
                ui.label(egui::RichText::new(title).strong());
                buttons(ui);
            })
            .response;
        ui.separator();
        head
    }

    fn select(&mut self, item: Item) {
        let same = self.rename.as_ref().is_some_and(|r| r.what == item);
        if !same {
            self.rename = None;
        }
        self.selection = Some(item);
    }

    /// Take back an armed rename, because the row it belongs to is about to stop
    /// existing and no row will be drawn to close it.
    fn forget_rename(&mut self, what: Item) {
        if self.rename.as_ref().is_some_and(|r| r.what == what) {
            self.rename = None;
        }
        if self.selection == Some(what) {
            self.selection = None;
        }
    }

    fn start_rename(&mut self, what: Item, from: &str) {
        self.selection = Some(what);
        self.rename = Some(Rename {
            what,
            text: from.to_string(),
            fresh: true,
        });
    }
    /// What a plain click on a row does.
    ///
    /// ⚠️ Arming the rename editor needs the click to land on the **name**, not merely
    /// on a row that was already selected. An editor armed by any second click sits
    /// there with the whole name selected, so the next keystroke — one meant for the
    /// document, or a stray one — replaces it, and the blur commits the replacement.
    fn clicked(&mut self, item: Item, response: &egui::Response, name: egui::Rect, from: &str) {
        let on_name = response
            .interact_pointer_pos()
            .is_some_and(|at| name.contains(at));
        match arms_rename(self.selection == Some(item), on_name) {
            true => self.start_rename(item, from),
            false => self.select(item),
        }
    }

    // ---- shared pieces ----------------------------------------------------------

    /// The in-place editor, prefilled and selected.
    ///
    /// ⚠️ **Only Enter renames.** Clicking away cancels. An editor that commits on blur
    /// turns a stray keystroke into a rename nobody asked for, and the name is the only
    /// record of what an object is — files store no name of their own.
    fn rename_row(&mut self, ui: &mut egui::Ui, original: &str) -> Option<String> {
        let rename = self.rename.as_mut()?;
        let output = ui
            .horizontal(|ui| {
                egui::TextEdit::singleline(&mut rename.text)
                    .desired_width(ui.available_width())
                    .show(ui)
            })
            .inner;
        if rename.fresh {
            rename.fresh = false;
            output.response.request_focus();
            let all = egui::text::CCursorRange::two(
                egui::text::CCursor::new(0),
                egui::text::CCursor::new(rename.text.chars().count()),
            );
            if let Some(mut state) = egui::TextEdit::load_state(ui.ctx(), output.response.id) {
                state.cursor.set_char_range(Some(all));
                state.store(ui.ctx(), output.response.id);
            }
            return None;
        }
        // ⚠️ Global Enter may belong to another editor. Commit only when this field lost
        // focus on the same keypress.
        let lost = output.response.lost_focus();
        let entered = lost && ui.input(|i| i.key_pressed(egui::Key::Enter));
        if !lost {
            return None;
        }
        let typed = std::mem::take(&mut rename.text);
        self.rename = None;
        entered.then(|| renamed(original, &typed)).flatten()
    }

    /// Take a drop, if this is somewhere the dragged thing can land.
    ///
    /// A target that would refuse does not light up; dropping on it anyway says why in
    /// the status strip rather than silently doing nothing.
    fn drop_zone(
        &mut self,
        ui: &egui::Ui,
        response: &egui::Response,
        onto: Onto,
        acts: &mut Vec<Act>,
    ) {
        if let Some(carried) = response.dnd_hover_payload::<Carried>() {
            if landing(&carried, onto).allowed() {
                ui.painter().rect_stroke(
                    response.rect,
                    3.0,
                    egui::Stroke::new(1.0_f32, ui.visuals().selection.stroke.color),
                    egui::StrokeKind::Inside,
                );
            }
        }
        let Some(carried) = response.dnd_release_payload::<Carried>() else {
            return;
        };
        self.land(&carried, onto, acts);
    }

    fn land(&mut self, carried: &Arc<Carried>, onto: Onto, acts: &mut Vec<Act>) {
        match (landing(carried, onto), carried.from, onto) {
            (Landing::Copy, Item::Slot { class, at }, _) => acts.push(Act::Copy { class, at }),
            (Landing::Rearrange, Item::Slot { at: from, .. }, Onto::Slot { class, at }) => acts
                .push(Act::Rearrange {
                    class,
                    from,
                    to: at,
                }),
            (Landing::Send, Item::Local(id), Onto::Slot { class, at }) => {
                acts.push(Act::Send { id, class, at })
            }
            (Landing::File, Item::Local(id), Onto::Group(folder)) => acts.push(Act::File {
                id,
                folder: Some(folder),
            }),
            (Landing::Unfile, Item::Local(id), Onto::Computer) => {
                acts.push(Act::File { id, folder: None })
            }
            (Landing::No(why), ..) => acts.push(Act::Refused(format!(
                "“{}” cannot go there — {why}.",
                carried.name
            ))),
            // Every allowed pairing is spelled out above; a shape that reaches here is a
            // verdict about a drag that did not come from where it says it did.
            _ => {}
        }
    }

    /// Ask before a slot is replaced or emptied. The only dialogs left in the app.
    fn dialog(&mut self, ctx: &egui::Context, acts: &mut Vec<Act>) {
        let Some(ask) = &self.ask else {
            return;
        };
        let mut decision = None;
        egui::Modal::new(egui::Id::new("browser_ask")).show(ctx, |ui| {
            ui.set_width(400.0);
            ui.heading(&ask.title);
            if let Some(note) = &ask.note {
                ui.add_space(4.0);
                ui.label(note);
            }
            ui.add_space(8.0);
            ui.separator();
            ui.horizontal(|ui| {
                if ui.button("Cancel").clicked() {
                    decision = Some(false);
                }
                if ui
                    .add(egui::Button::new(egui::RichText::new(ask.verb).strong()))
                    .clicked()
                {
                    decision = Some(true);
                }
            });
        });
        match decision {
            Some(true) => {
                if let Some(ask) = self.ask.take() {
                    acts.push(ask.act);
                }
            }
            Some(false) => self.ask = None,
            None => {}
        }
    }

    /// The one question a batch asks: everything it is about to write, and what it
    /// would replace.
    ///
    /// One question for every batch there is — the whole queue, or one folder's worth —
    /// so a folder cannot become a way of writing to the instrument without being asked.
    fn ask_send(
        &mut self,
        workspace: &Workspace,
        device: &Device,
        ids: &[u64],
        title: String,
        act: Act,
    ) {
        let mut lines = Vec::new();
        let mut warnings: Vec<String> = Vec::new();
        for entity in ids.iter().filter_map(|id| workspace.get(*id)) {
            let Some((class, at)) = owed(entity) else {
                continue;
            };
            let where_ = place(class, at);
            for warning in write_warnings(class, &entity.tag(), &device.state.formats_in(class)) {
                if !warnings.contains(&warning) {
                    warnings.push(warning);
                }
            }
            lines.push(match device.state.slot(class, at).flatten() {
                Some(info) => format!(
                    "“{}” replaces “{}” in {where_}",
                    entity.name,
                    info.name.trim()
                ),
                None => format!("“{}” goes into {where_}, which is empty", entity.name),
            });
        }
        if lines.is_empty() {
            return;
        }
        // The warnings first: they are the reason to say no.
        let mut note = warnings;
        if !note.is_empty() {
            note.push(String::new());
        }
        note.extend(lines);
        self.ask = Some(Ask {
            title,
            note: Some(note.join("\n")),
            verb: "Send",
            act,
        });
    }

    /// Raise the one Finder-style question a drop can need: the destination is taken.
    fn ask_replace(
        &mut self,
        occupant: &str,
        incoming: &str,
        at: String,
        warning: Option<String>,
        act: Act,
    ) {
        let note =
            format!("“{occupant}” is read back first and put where it was if anything goes wrong.");
        self.ask = Some(Ask {
            title: format!("Replace “{occupant}” in {at} with “{incoming}”?"),
            note: Some(match warning {
                Some(warning) => format!("{warning}\n\n{note}"),
                None => note,
            }),
            verb: "Replace",
            act,
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::browser::bench::bench;
    use crate::tabs::Tabs;
    use crate::workspace::Fresh;

    /// Paint the two columns headlessly. What this catches is a layout that panics or
    /// an id that collides, neither of which a unit test on the rules would see.
    fn paint(with_device: bool) {
        use crate::workspace::{Fresh, Origin};

        let ctx = egui::Context::default();
        let mut workspace = Workspace::new(ctx.clone());
        let mut device = Device::new(ctx.clone());
        let mut log = crate::log::Log::default();
        let mut tabs = Tabs::default();
        let mut browser = Browser::default();

        for kind in [Fresh::Program, Fresh::Live, Fresh::Settings] {
            workspace.create(kind, &mut log).unwrap();
        }
        // A folder with something in it, one with nothing, and a view of a slot: three
        // row shapes the list has no other way of reaching.
        let full = browser.folders.make();
        browser.folders.make();
        let filed = workspace.create(Fresh::Program, &mut log).unwrap();
        browser.folders.file(filed, Some(full));
        let bytes = workspace.get(filed).unwrap().bytes.clone();
        workspace.view(
            "Africa-Split.ne5p".into(),
            Origin::Device {
                class: ObjectClass::Program,
                at: Location { bank: 6, slot: 0 },
            },
            bytes,
            &mut log,
        );
        if with_device {
            // Every row shape a list can hold: a named slot, a vacant one, the slot the
            // panel is on, and a class that was never read.
            device.pretend_scanned(ObjectClass::Program, 7, &["Africa Split", "", "Squabble B"]);
            device.pretend_scanned(ObjectClass::Program, 8, &["Bass Manual"]);
            device.pretend_scanned(ObjectClass::SetList, 1, &["Sunday"]);
            device.pretend_focused(ObjectClass::Program, Location { bank: 6, slot: 2 });
            // Named banks, which is what a piano's categories arrive as.
            device.pretend_scanned(ObjectClass::Piano, 1, &["Royal Grand 3D"]);
            device.pretend_geometry(ObjectClass::Piano, &[("Grand", 1), ("Upright", 1)]);
        }

        // Twice: the second pass runs with the widget state the first left behind.
        for _ in 0..2 {
            let _ = ctx.run(egui::RawInput::default(), |ctx| {
                egui::SidePanel::left("places").show(ctx, |ui| {
                    let acts = browser.ui(ui, &workspace, &device);
                    apply(
                        &mut browser,
                        acts,
                        &mut workspace,
                        &mut device,
                        &mut tabs,
                        &mut log,
                    );
                });
            });
        }
    }

    /// The divider does what dragging it says, and stops before either column is gone.
    ///
    /// ⚠️ The share is what is kept, not a width: the sidebar the two live in is itself
    /// resizable, so the same fraction has to survive the dock changing size under it.
    #[test]
    fn the_divider_moves_and_stops_short_of_squeezing_a_column_out() {
        use crate::workspace::Fresh;

        let ctx = egui::Context::default();
        let mut workspace = Workspace::new(ctx.clone());
        let mut device = Device::new(ctx.clone());
        let mut log = crate::log::Log::default();
        let mut browser = Browser::default();
        workspace.create(Fresh::Program, &mut log).unwrap();
        device.pretend_scanned(ObjectClass::Program, 7, &["Africa Split"]);

        // Far enough left to ask for more than the stop allows.
        let travel = -400.0;
        let button = |pos, pressed| egui::Event::PointerButton {
            pos,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: egui::Modifiers::default(),
        };

        let mut divider = egui::pos2(0.0, 0.0);
        let mut frame = 0;
        while frame < 5 {
            let grip = divider;
            let moved = grip + egui::vec2(travel, 0.0);
            let input = egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(1280.0, 720.0),
                )),
                events: match frame {
                    0 => Vec::new(),
                    1 => vec![egui::Event::PointerMoved(grip)],
                    2 => vec![button(grip, true)],
                    3 => vec![egui::Event::PointerMoved(moved)],
                    _ => vec![button(moved, false)],
                },
                ..Default::default()
            };
            let _ = ctx.run(input, |ctx| {
                egui::SidePanel::left("places")
                    .exact_width(600.0)
                    .show(ctx, |ui| {
                        // The same rect the dock lays itself out in, so the test grabs
                        // the divider where the divider actually is.
                        let whole = ui.available_rect_before_wrap();
                        divider = egui::pos2(
                            whole.left() + (whole.width() - HANDLE) * browser.split + HANDLE / 2.0,
                            whole.center().y,
                        );
                        let _ = browser.ui(ui, &workspace, &device);
                    });
            });
            frame += 1;
        }

        assert!(browser.split < EVEN, "it moved: {}", browser.split);
        assert_eq!(browser.split, LEAST, "and stopped at the stop");
    }

    /// A share the store cannot account for is the even split, never one column and a
    /// sliver of the other.
    #[test]
    fn a_divider_comes_back_where_it_was_left_or_not_at_all() {
        let restored = |held: Option<&str>| {
            let mut store = Fake::default();
            if let Some(held) = held {
                eframe::Storage::set_string(&mut store, Browser::SPLIT, held.to_string());
            }
            let mut browser = Browser::default();
            browser.restore(&store);
            browser.split
        };
        assert_eq!(restored(Some("0.3")), 0.3);
        assert_eq!(restored(None), EVEN);
        for nonsense in ["0.0", "1.0", "-3", "wide", "", "NaN"] {
            assert_eq!(restored(Some(nonsense)), EVEN, "{nonsense:?}");
        }

        // And what is written comes back as itself.
        let mut store = Fake::default();
        let browser = Browser {
            split: 0.42,
            ..Browser::default()
        };
        browser.keep(&mut store);
        let mut after = Browser::default();
        after.restore(&store);
        assert_eq!(after.split, 0.42);
    }

    /// A store that answers for one key at a time, which is what the two things the
    /// browser keeps need it to be.
    #[derive(Default)]
    struct Fake(std::collections::HashMap<String, String>);

    impl eframe::Storage for Fake {
        fn get_string(&self, key: &str) -> Option<String> {
            self.0.get(key).cloned()
        }
        fn set_string(&mut self, key: &str, value: String) {
            self.0.insert(key.to_string(), value);
        }
        fn flush(&mut self) {}
    }

    #[test]
    fn the_two_columns_paint_with_nothing_attached() {
        paint(false);
    }

    #[test]
    fn the_two_columns_paint_with_a_tree_to_show() {
        paint(true);
    }

    /// ⚠️ The gesture that lost a program its name. An editor armed by any second click
    /// on a selected row sits there with everything selected, so the next keystroke
    /// replaces the name and the blur commits it.
    #[test]
    fn a_click_away_from_the_name_selects_rather_than_arming_a_rename() {
        // The row is the click target, so most of it must be safe to click.
        assert!(!arms_rename(true, false), "past the name on a selected row");
        assert!(
            !arms_rename(false, true),
            "on the name of an unselected row"
        );
        assert!(!arms_rename(false, false));
        assert!(arms_rename(true, true), "the one gesture that renames");
    }

    /// The gesture end to end: arm the editor, type, press Enter, and the new name comes
    /// back as an act.
    ///
    /// What this catches is a rename that has stopped committing at all — the unit tests
    /// on [`renamed`] cannot see the field it is fed from, and the field's own idea of
    /// "the operator pressed Enter" is the part that is easy to get wrong.
    #[test]
    fn typing_a_name_and_pressing_enter_renames_the_row() {
        use crate::workspace::Fresh;

        let ctx = egui::Context::default();
        let mut workspace = Workspace::new(ctx.clone());
        let device = Device::new(ctx.clone());
        let mut log = crate::log::Log::default();
        let mut browser = Browser::default();
        let id = workspace.create(Fresh::Program, &mut log).unwrap();

        let key = |key| egui::Event::Key {
            key,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: egui::Modifiers::default(),
        };
        // The editor opens on the first frame and takes the focus; the second types over
        // what it opened with, selected; the third commits.
        let frames: [Vec<egui::Event>; 3] = [
            Vec::new(),
            vec![egui::Event::Text("LA Grand".into())],
            vec![key(egui::Key::Enter)],
        ];
        browser.start_rename(Item::Local(id), "Africa Split");

        let mut named = None;
        for events in frames {
            let input = egui::RawInput {
                events,
                ..Default::default()
            };
            let _ = ctx.run(input, |ctx| {
                egui::SidePanel::left("places").show(ctx, |ui| {
                    for act in browser.ui(ui, &workspace, &device) {
                        if let Act::RenameLocal { name, .. } = act {
                            named = Some(name);
                        }
                    }
                });
            });
        }
        assert_eq!(named.as_deref(), Some("LA Grand"));
        assert!(browser.rename.is_none(), "and the editor is done with");
    }

    /// Only Enter renames: an armed editor that commits on blur turns a stray keystroke
    /// into a rename nobody asked for.
    #[test]
    fn a_rename_needs_enter_and_a_real_change() {
        assert_eq!(renamed("Africa Split", "LA Grand"), Some("LA Grand".into()));
        // What blur hands back is nothing at all — see `rename_row`.
        assert_eq!(renamed("Africa Split", "Africa Split"), None);
    }

    /// Enter on an untouched field, or on an empty one, leaves the asset alone.
    #[test]
    fn a_rename_that_changes_nothing_is_not_a_rename() {
        assert_eq!(renamed("Africa Split", "Africa Split"), None);
        assert_eq!(renamed("Africa Split", "  Africa Split "), None);
        assert_eq!(renamed("Africa Split", "   "), None);
        assert_eq!(renamed("Africa Split", ""), None);
    }

    /// What is typed is what the asset is called, with the spaces around it dropped.
    #[test]
    fn a_rename_takes_the_typed_name_trimmed() {
        assert_eq!(
            renamed("Africa Split", "  LA Grand  "),
            Some("LA Grand".into())
        );
    }

    /// The two stores are read separately and only the asset one decides what survived —
    /// anything too big to keep, or dropped for want of room, would otherwise leave its
    /// membership behind to accumulate for as long as the app is installed.
    #[test]
    fn a_grouping_forgets_the_assets_the_list_came_back_without() {
        let (mut browser, mut workspace, _device, _tabs, mut log) = bench();
        let here = workspace.create(Fresh::Program, &mut log).unwrap();
        let folder = browser.folders.make();
        browser.folders.file(here, Some(folder));
        // As a store that could not keep everything reads back: a membership for an
        // asset the list does not hold.
        browser.folders.file(here + 99, Some(folder));

        browser.settle(&workspace);
        assert_eq!(browser.folders.holding(here), Some(folder));
        assert_eq!(browser.folders.holding(here + 99), None);
        assert_eq!(browser.folders.all().len(), 1, "the folder itself stays");
    }

    /// The warning reaches the modal a batch raises, once per format however many items
    /// carry it — and it goes above the list of destinations, which is what the eye
    /// slides past.
    #[test]
    fn the_modal_says_when_a_batch_is_of_another_model() {
        use crate::workspace::Origin;

        let (mut browser, mut workspace, mut device, _tabs, mut log) = bench();
        let class = ObjectClass::Program;
        device.pretend_scanned(class, 7, &["Africa Split", "Squabble B"]);

        let mut ids = Vec::new();
        for slot in 0..2 {
            let stage = workspace.create(Fresh::Stage4Program, &mut log).unwrap();
            let bytes = workspace.get(stage).unwrap().bytes.clone();
            workspace.remove(stage, &mut log);
            ids.push(workspace.ingest(
                format!("stage-{slot}.ns4p"),
                Origin::Device {
                    class,
                    at: Location { bank: 6, slot },
                },
                bytes,
                &mut log,
            ));
        }

        browser.ask_send(&workspace, &device, &ids, "Send?".into(), Act::SendAll);
        let note = browser.ask.as_ref().and_then(|ask| ask.note.clone());
        let note = note.expect("the modal has a note");
        assert_eq!(note.matches("This file is ns4p").count(), 1, "{note}");
        let warned = note.find("ns4p").expect("the warning is there");
        let listed = note.find("replaces").expect("and so are the destinations");
        assert!(warned < listed, "the warning comes first:\n{note}");
    }
}
