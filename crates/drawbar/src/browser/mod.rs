//! The browser dock: one tree with three sections, for the places a sound can be, the
//! kinds of sound, and the tags on the local list.
//!
//! Nothing here touches the instrument. Drawing reads the caches and returns a list of
//! [`Act`]s, which [`apply`] then runs against the workspace, the device and the tabs. A
//! row can therefore be drawn while the thing it stands for is about to change.
//!
//! This file holds the state rows share: the selection, the in-place rename, the
//! confirmation modal, and which branches are open. The tree is drawn in `tree`, the drag
//! rules are in `drag`, and a single row is drawn in `row`. Folders and tags for the
//! local list live outside the browser, in [`crate::folders`] and [`crate::tags`], and
//! [`crate::store`] keeps both on disk.

use std::collections::{BTreeSet, VecDeque};
use std::sync::Arc;

use eframe::egui;
use nord_usb::{Location, ObjectClass};

use crate::device::{read_only, Device, DeviceState};
use crate::filter::Filter;
use crate::folders::Folders;
use crate::icon::Glyph;
use crate::queue::Queue;
use crate::sheet;
use crate::store::{LibPath, Rescue};
use crate::tags::Tags;
use crate::workspace::Workspace;

mod act;
mod drag;
mod instrument;
mod row;
mod selection;
mod tree;

pub use act::{
    apply, bulk, foreign_format, send_warnings, Act, Bulk, Rescuing, LOAD_ON_INSTRUMENT,
};
#[cfg(test)]
pub(crate) use drag::TAGGED;
pub use drag::{
    kinds_present, landing, qualifier, tagged, Carried, Held, Item, Kept, Kind, Onto, Qualifier,
};
pub use instrument::about;
pub use row::{cell_ink, starred, Cells};
pub use selection::Selection;
pub use tree::{library_items, offers_libraries};

use drag::ghost;
use selection::{gesture, Gesture};
use tree::{Branch, Rows, Sections};

/// An in-place rename, waiting on Enter or Esc.
struct Rename {
    what: Item,
    text: String,
    /// True on the first frame, when the field takes focus and selects its text.
    fresh: bool,
}

/// A click on a row, and the list a ⇧-click extends across.
struct Click<'a> {
    item: Item,
    /// The rows of the list this one sits in, in the order they are drawn.
    list: &'a [Item],
}

/// A confirmation asked before something is lost, and the acts a yes runs. One question
/// covers a whole checked set.
struct Ask {
    title: String,
    note: Option<String>,
    verb: Verb,
    acts: Vec<Act>,
    /// More answers beside the verb, each with the acts it runs, in the order drawn
    /// from the left.
    others: Vec<(&'static str, Vec<Act>)>,
    /// What the answer that runs nothing is called.
    cancel: &'static str,
    /// The answer drawn as the one expected.
    strong: Answer,
}

impl Ask {
    fn new(title: String, note: Option<String>, verb: Verb, acts: Vec<Act>) -> Ask {
        Ask {
            title,
            note,
            verb,
            acts,
            others: Vec::new(),
            cancel: "Cancel",
            strong: Answer::Verb,
        }
    }

    /// The label of each answer, as the dialog lays them out.
    fn answers(&self) -> Vec<(Answer, &'static str)> {
        let mut answers = vec![(Answer::Cancel, self.cancel)];
        let others = self.others.iter().enumerate();
        answers.extend(others.map(|(at, (label, _))| (Answer::Other(at), *label)));
        answers.push((Answer::Verb, self.verb.label()));
        answers
    }
}

/// Which answer a question got.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Answer {
    Verb,
    /// The one of [`Ask::others`] at this index.
    Other(usize),
    Cancel,
}

/// What a yes to an [`Ask`] does, which names and marks its button.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Verb {
    Save,
    Keep,
    Replace,
    Delete,
    Overwrite,
    KeepBoth,
    TakeTheirs,
    Discard,
}

impl Verb {
    fn label(self) -> &'static str {
        match self {
            Verb::Save => "Save",
            Verb::Keep => "Keep in library",
            Verb::Replace => "Replace",
            Verb::Delete => "Delete",
            Verb::Overwrite => "Overwrite",
            Verb::KeepBoth => "Keep both",
            Verb::TakeTheirs => "Take theirs",
            Verb::Discard => "Discard",
        }
    }

    /// The button that answers yes, in the loss color where the answer discards something.
    fn button(self, ui: &mut egui::Ui) -> egui::Response {
        let (label, glyph) = (self.label(), self.glyph());
        match self {
            Verb::Save | Verb::Keep | Verb::KeepBoth => sheet::primary(ui, Some(glyph), label),
            Verb::Replace | Verb::Delete | Verb::Overwrite | Verb::TakeTheirs | Verb::Discard => {
                sheet::destructive(ui, glyph, label)
            }
        }
    }

    fn glyph(self) -> Glyph {
        match self {
            Verb::Save => Glyph::Save,
            Verb::Keep => Glyph::LibraryBig,
            Verb::Replace => Glyph::Replace,
            Verb::Delete | Verb::Discard => Glyph::Trash2,
            Verb::Overwrite => Glyph::Replace,
            Verb::KeepBoth => Glyph::Copy,
            Verb::TakeTheirs => Glyph::RotateCcw,
        }
    }
}

/// The widest the confirmation sheet may be.
const ASK_WIDTH: f32 = 440.0;

/// The height the confirmation sheet keeps for its title and foot, so a long note scrolls
/// and the buttons stay on screen.
const ASK_AROUND: f32 = 160.0;

/// The shortest the note gets, however short the window.
const ASK_FEWEST: f32 = 80.0;

/// What a bulk action's control says over a checked set, and whether it can run.
pub struct Offer {
    /// The words on its button: Queue counts what the attached instrument would take.
    pub label: String,
    pub live: bool,
    /// Why it cannot run, shown on hover while it cannot.
    pub dead: String,
}

impl Offer {
    pub fn of(action: Bulk, checked: &[Item], workspace: &Workspace, state: &DeviceState) -> Offer {
        let wanted = match action {
            Bulk::Tag => !checked.iter().all(|item| item.local().is_none()),
            _ => !bulk(action, checked, state).is_empty(),
        };
        // ⚠️ Only Queue checks what the instrument accepts. Everything else happens on
        // this computer, where another instrument's file is still a file.
        let fits = (action == Bulk::Queue).then(|| act::fits(checked, workspace, state));
        let live = wanted && fits.as_ref().is_none_or(|fits| fits.takes > 0);
        Offer {
            label: match &fits {
                Some(fits) => fits.label(),
                None => action.label().to_string(),
            },
            live,
            dead: fits
                .and_then(|fits| fits.why)
                .unwrap_or_else(|| action.nothing().to_string()),
        }
    }
}

/// The new name Enter commits from an in-place rename, if any.
///
/// A blank or unchanged field returns `None`, so no operation is sent that would do
/// nothing.
pub fn renamed(original: &str, typed: &str) -> Option<String> {
    let typed = typed.trim();
    match typed.is_empty() || typed == original.trim() {
        true => None,
        false => Some(typed.to_string()),
    }
}

pub struct Browser {
    selection: Selection,
    rename: Option<Rename>,
    ask: Option<Ask>,
    /// Questions waiting for the one showing to be answered.
    later: VecDeque<Ask>,
    pub(crate) folders: Folders,
    pub(crate) tags: Tags,
    /// Which of the three sections are showing.
    sections: Sections,
    /// The open branches of the tree. Both places start open, so a new panel shows what
    /// is in them.
    open: BTreeSet<Branch>,
    /// A slot to scroll to and select, once the branches holding it have been laid out.
    jump: Option<(ObjectClass, Location)>,
    rows: Rows,
    /// Acts waiting for the assets they read to be read, in the order they were asked.
    held: Vec<Act>,
    /// Where this computer's folders were drawn as places a drop lands, in drawing order:
    /// `None` for the library's own top level.
    targets: Vec<(egui::Rect, Option<u64>)>,
}

impl Default for Browser {
    fn default() -> Browser {
        Browser {
            selection: Selection::default(),
            rename: None,
            ask: None,
            later: VecDeque::new(),
            folders: Folders::default(),
            tags: Tags::default(),
            sections: Sections::default(),
            open: BTreeSet::from([Branch::Computer, Branch::Instrument]),
            jump: None,
            rows: Rows::default(),
            held: Vec::new(),
            targets: Vec::new(),
        }
    }
}

impl Browser {
    /// Draw the tree and collect what the user asked for.
    pub fn ui(
        &mut self,
        ui: &mut egui::Ui,
        workspace: &Workspace,
        device: &Device,
        queue: &Queue,
        filter: &Filter,
    ) -> Vec<Act> {
        let mut acts = Vec::new();
        self.tree(ui, workspace, device, queue, filter, &mut acts);
        ghost(ui.ctx());
        acts
    }

    /// Forget everything about the library open until now: its folders, its tags, and
    /// whatever was selected, renamed or asked about in it. Whether all files are shown
    /// stays, since that is the window's choice.
    pub(crate) fn leave_library(&mut self) {
        self.selection.clear();
        self.rename = None;
        self.ask = None;
        self.later.clear();
        self.held.clear();
        self.folders.leave();
        self.tags = Tags::default();
        self.open
            .retain(|branch| !matches!(branch, Branch::Folder(_)));
    }

    /// The selection, which the library table and the tree share.
    pub fn picked(&self) -> &Selection {
        &self.selection
    }

    /// The tags on this computer's list, for the views that count them.
    pub fn tags(&self) -> &Tags {
        &self.tags
    }

    /// A click on a row drawn outside the tree, with the same gestures on the same
    /// selection.
    pub fn pick(&mut self, ui: &egui::Ui, item: Item, list: &[Item]) {
        self.clicked(ui, Click { item, list });
    }

    /// Escape clears the selection, wherever its rows were drawn.
    ///
    /// ⚠️ Called whether or not the browser is shown: the library's table shows the same
    /// selection, and a selection nothing draws could never be cleared.
    ///
    /// During a rename or a question, Escape cancels that and the selection stays.
    pub fn let_go(&mut self, ctx: &egui::Context) {
        let busy = self.rename.is_some() || self.ask.is_some();
        if !busy && ctx.input(|input| input.key_pressed(egui::Key::Escape)) {
            self.selection.clear();
        }
    }

    /// Clear the selection, because a click landed below the last row.
    pub fn unpick(&mut self) {
        self.selection.clear();
    }

    /// A click on a row's checkbox, which toggles that row.
    ///
    /// ⚠️ A plain click on the checkbox acts as the ⌘ gesture. A click on the row itself
    /// follows `gesture`.
    pub fn check(&mut self, item: Item) {
        self.rename = None;
        self.selection.toggle(item);
    }

    fn select(&mut self, item: Item) {
        self.rename = None;
        self.selection.only(item);
    }

    /// Cancel an open rename of `what` and drop it from the selection, because its row is
    /// about to go away and will not be drawn to close the editor.
    fn forget_rename(&mut self, what: Item) {
        if self.rename.as_ref().is_some_and(|r| r.what == what) {
            self.rename = None;
        }
        self.selection.forget(what);
    }

    fn start_rename(&mut self, what: Item, from: &str) {
        self.selection.only(what);
        self.rename = Some(Rename {
            what,
            text: from.to_string(),
            fresh: true,
        });
    }

    /// Open the section and branches that hold `item`'s row, so a rename asked for
    /// outside the tree has a row to type in.
    fn reveal(&mut self, item: Item, workspace: &Workspace) {
        let branches: Vec<Branch> = match item {
            Item::Tag(_) => {
                self.sections.tags = true;
                return;
            }
            Item::Local(id) => {
                let mut branches = vec![Branch::Computer];
                let file = workspace.get(id).and_then(|entity| entity.path.as_ref());
                let mut dir = file.map_or_else(LibPath::root, LibPath::parent);
                while !dir.is_root() {
                    branches.extend(self.folders.id_of(&dir).map(Branch::Folder));
                    dir = dir.parent();
                }
                branches
            }
            Item::Folder(_) => vec![Branch::Computer],
            Item::Slot { class, at } => vec![
                Branch::Instrument,
                Branch::Class(class.to_raw()),
                tree::bank_branch(class, at.user_bank()),
            ],
        };
        self.sections.places = true;
        self.open.extend(branches);
    }

    /// Apply a click on a row to the selection.
    ///
    /// ⚠️ No click opens the rename editor. An editor opened by a second click on a
    /// selected row would wait with the whole name selected, and the next keystroke, even
    /// one meant for the document, would replace it. Renaming is F2 and the row's menu.
    fn clicked(&mut self, ui: &egui::Ui, click: Click) {
        self.rename = None;
        match gesture(&ui.input(|input| input.modifiers)) {
            Gesture::Plain => self.selection.plain(click.item),
            Gesture::Toggle => self.selection.toggle(click.item),
            Gesture::Extend => self.selection.extend(click.item, click.list),
        }
    }

    /// What one drag carries: the pressed row, and the rest of the selection when the
    /// pressed row is in it.
    pub(crate) fn carrying(
        &self,
        head: Held,
        name: &str,
        workspace: &Workspace,
        device: &DeviceState,
    ) -> Carried {
        let rest: Vec<Held> = match self.selection.holds(head.what) {
            false => Vec::new(),
            true => self
                .selection
                .items()
                .filter(|item| *item != head.what)
                .filter_map(|item| self.held(item, workspace, device))
                .collect(),
        };
        Carried {
            head,
            name: match rest.len() {
                0 => name.to_string(),
                more => format!("{name}  +{more}"),
            },
            rest,
        }
    }

    /// What the drag rules need to know about a row, drawn in the tree or the library's
    /// table. `None` for a row that is never dragged.
    ///
    /// ⚠️ A drag cannot carry a slot of a partition this app cannot name, or a slot the
    /// scan found empty. Neither holds anything this app could ask the instrument for.
    pub(crate) fn held(
        &self,
        item: Item,
        workspace: &Workspace,
        device: &DeviceState,
    ) -> Option<Held> {
        match item {
            Item::Local(id) => {
                let entity = workspace.get(id)?;
                Some(Held {
                    what: item,
                    kind: Kind::of(entity),
                    filed: self.folders.holding(entity),
                    fits: crate::device::fit(device, entity).allowed(),
                })
            }
            Item::Folder(_) | Item::Tag(_) => None,
            // What is already on the instrument fits it.
            Item::Slot { class, at } => {
                (!read_only(class) && device.slot(class, at).flatten().is_some()).then_some(Held {
                    what: item,
                    kind: Kind::from_class(class),
                    filed: None,
                    fits: true,
                })
            }
        }
    }

    /// The in-place editor, prefilled and selected.
    ///
    /// ⚠️ Only Enter renames; clicking away cancels. Committing on blur would turn a
    /// stray keystroke into an unwanted rename, and the name is the only record of what
    /// an object is, because the files do not store their own names.
    fn rename_row(&mut self, ui: &mut egui::Ui, indent: f32, original: &str) -> Option<String> {
        let rename = self.rename.as_mut()?;
        let output = ui
            .horizontal(|ui| {
                ui.set_min_height(row::CHILD);
                ui.add_space(crate::panel::ROW_INSET + indent);
                // ⚠️ Named, not numbered: a tree line's widgets are numbered by how many
                // lines were drawn before it, which scrolling changes, and an editor whose
                // id changes loses its focus.
                egui::TextEdit::singleline(&mut rename.text)
                    .id_salt("rename")
                    .desired_width(ui.available_width() - crate::panel::ROW_INSET)
                    .show(ui)
            })
            .inner;
        if rename.fresh {
            rename.fresh = false;
            output.response.request_focus();
            output.response.scroll_to_me(None);
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

    /// Take a drop, if the dragged row can land here.
    ///
    /// A target that would refuse is not highlighted. Dropping on it anyway reports why
    /// in the status strip.
    pub(crate) fn drop_zone(
        &mut self,
        ui: &egui::Ui,
        response: &egui::Response,
        onto: Onto,
        acts: &mut Vec<Act>,
    ) {
        match onto {
            Onto::Computer => self.targets.push((response.rect, None)),
            Onto::Group(folder) => self.targets.push((response.rect, Some(folder))),
            Onto::Slot { .. } => {}
        }
        if let Some(carried) = response.dnd_hover_payload::<Carried>() {
            if landing(&carried.head, onto).is_ok() {
                ui.painter().rect_stroke(
                    response.rect,
                    crate::panel::ROW_RADIUS,
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

    /// The folder a file dropped from outside at `at` lands in: the one whose row was
    /// drawn there last frame, and otherwise the library's top level.
    pub fn landing_dir(&self, at: Option<egui::Pos2>) -> LibPath {
        let folder = at.and_then(|at| {
            let (_, folder) = self
                .targets
                .iter()
                .rev()
                .find(|(rect, _)| rect.contains(at))?;
            Some(*folder)
        });
        folder
            .flatten()
            .and_then(|folder| self.folders.path_of(folder).cloned())
            .unwrap_or_else(LibPath::root)
    }

    /// Forget where the folders were drawn, before the frame draws them again.
    pub fn forget_targets(&mut self) {
        self.targets.clear();
    }

    /// Run the drop for the pressed row, and for the rest of what it carries when the
    /// verdict is a copy or a filing.
    ///
    /// ⚠️ A send and a rearrange name one destination, and several rows sent to one slot
    /// would overwrite each other, so those take only the pressed row.
    fn land(&mut self, carried: &Arc<Carried>, onto: Onto, acts: &mut Vec<Act>) {
        let verdict = match landing(&carried.head, onto) {
            Ok(act) => act,
            Err(why) => {
                return acts.push(Act::Refused(format!(
                    "“{}” cannot go there: {why}.",
                    carried.name
                )))
            }
        };
        if !matches!(verdict, Act::Copy { .. } | Act::File { .. }) {
            return acts.push(verdict);
        }
        let kind = std::mem::discriminant(&verdict);
        acts.extend(
            carried
                .all()
                .filter_map(|held| landing(&held, onto).ok())
                .filter(|each| std::mem::discriminant(each) == kind),
        );
    }

    /// The open question, if any, and the acts its answer runs. Questions raised while
    /// one is showing wait their turn.
    ///
    /// ⚠️ Called whether or not the browser is shown: the top bar, the library and the
    /// document header ask questions too.
    pub fn dialog(&mut self, ctx: &egui::Context, acts: &mut Vec<Act>) {
        if self.ask.is_none() {
            self.ask = self.later.pop_front();
        }
        let Some(ask) = &self.ask else {
            return;
        };
        let mut decision = None;
        egui::Modal::new(egui::Id::new("browser_ask"))
            .frame(sheet::frame(&ctx.style().visuals))
            .show(ctx, |ui| {
                ui.set_width(sheet::width(ui.ctx(), ASK_WIDTH));
                ui.add_space(sheet::GAP * 4.0);
                sheet::section(ui, |ui| {
                    ui.add(egui::Label::new(egui::RichText::new(&ask.title).strong()).wrap());
                    if let Some(note) = &ask.note {
                        ui.add_space(sheet::GAP * 2.0);
                        egui::ScrollArea::vertical()
                            .id_salt("browser_ask_note")
                            .max_height(sheet::middle(ui.ctx(), ASK_AROUND, ASK_FEWEST))
                            .show(ui, |ui| ui.add(egui::Label::new(note).wrap()));
                    }
                });
                sheet::foot(
                    ui,
                    |_| {},
                    |ui| {
                        // Right to left: the verb at the far right, the cancel at the left.
                        for (answer, label) in ask.answers().into_iter().rev() {
                            let clicked = match answer {
                                Answer::Verb => ask.verb.button(ui),
                                _ if answer == ask.strong => sheet::primary(ui, None, label),
                                _ => sheet::secondary(ui, None, label),
                            }
                            .clicked();
                            if clicked {
                                decision = Some(answer);
                            }
                        }
                    },
                );
                if ui.input(|input| input.key_pressed(egui::Key::Escape)) {
                    decision = Some(Answer::Cancel);
                }
            });
        let Some(decision) = decision else {
            return;
        };
        let Some(ask) = self.ask.take() else {
            return;
        };
        match decision {
            Answer::Verb => acts.extend(ask.acts),
            Answer::Other(at) => {
                if let Some((_, more)) = ask.others.into_iter().nth(at) {
                    acts.extend(more);
                }
            }
            Answer::Cancel => {}
        }
    }

    /// The title of the question showing or next to show, and its answers.
    #[cfg(test)]
    pub(crate) fn asking(&self) -> Option<(String, Vec<&'static str>)> {
        let ask = self.ask.as_ref().or(self.later.front())?;
        let answers = ask.answers().into_iter().map(|(_, label)| label).collect();
        Some((ask.title.clone(), answers))
    }

    /// The answer that question draws as the one expected.
    #[cfg(all(test, not(target_arch = "wasm32")))]
    pub(crate) fn expected(&self) -> Option<&'static str> {
        let ask = self.ask.as_ref().or(self.later.front())?;
        ask.answers()
            .into_iter()
            .find(|(answer, _)| *answer == ask.strong)
            .map(|(_, label)| label)
    }

    /// Answer that question as a click on the answer labeled `label` would, and return
    /// the acts it runs.
    #[cfg(test)]
    pub(crate) fn answer(&mut self, label: &str) -> Vec<Act> {
        let Some(ask) = self.ask.take().or_else(|| self.later.pop_front()) else {
            return Vec::new();
        };
        if ask.verb.label() == label {
            return ask.acts;
        }
        let other = ask.others.into_iter().find(|(other, _)| *other == label);
        other.map(|(_, acts)| acts).unwrap_or_default()
    }

    /// Ask once the question showing now, if any, is answered.
    fn raise(&mut self, ask: Ask) {
        match self.ask {
            None => self.ask = Some(ask),
            Some(_) => self.later.push_back(ask),
        }
    }

    /// Ask before an asset is deleted with its file.
    fn ask_delete(&mut self, id: u64, name: &str) {
        self.raise(Ask::new(
            format!("Delete “{name}”?"),
            Some("It is deleted from this computer, with its file in the library folder.".into()),
            Verb::Delete,
            vec![Act::Remove(id)],
        ));
    }

    /// Ask what to do about a name already taken in a folder. `over` is what overwriting
    /// runs, where overwriting is allowed; `both` keeps both under `free`.
    fn ask_clash(
        &mut self,
        name: &str,
        dir: &crate::store::LibPath,
        over: Option<Vec<Act>>,
        both: Vec<Act>,
        free: &str,
    ) {
        let place = match dir.is_root() {
            true => "the top level of the library".to_string(),
            false => format!("“{dir}”"),
        };
        let title = format!("“{name}” is already in {place}");
        let both_note = format!("Keep both names the new one “{free}”.");
        self.raise(match over {
            Some(over) => Ask {
                title,
                note: Some(format!(
                    "Overwrite puts the new contents in the file that is there, which keeps \
                     its tags. {both_note}"
                )),
                verb: Verb::Overwrite,
                acts: over,
                others: vec![("Keep both", both)],
                cancel: "Cancel",
                strong: Answer::Verb,
            },
            None => Ask::new(
                title,
                Some(format!(
                    "What is there holds changes of its own, or is not a file, so it cannot \
                     be overwritten. {both_note}"
                )),
                Verb::KeepBoth,
                both,
            ),
        });
    }

    /// Ask what to do about a file changed on disk while this app held an unsaved edit of
    /// it.
    pub(crate) fn ask_conflict(&mut self, id: u64, name: &str) {
        self.raise(Ask {
            title: format!("“{name}” changed on disk"),
            note: Some(
                "Something outside drawbar saved over it while you had unsaved edits. Keep \
                 mine saves your edits over it at the next save. Take theirs discards your \
                 edits. Keep both keeps yours as a new file beside it."
                    .into(),
            ),
            verb: Verb::TakeTheirs,
            acts: vec![Act::Revert(id)],
            others: vec![("Keep both", vec![Act::KeepBoth(id)])],
            cancel: "Keep mine",
            strong: Answer::Cancel,
        });
    }

    /// Ask before another library opens in place of one that cannot keep the assets
    /// named in `unkept`, because nothing may be written to it.
    pub(crate) fn ask_leave(&mut self, unkept: &[String], root: crate::store::Root) {
        const SHOWN: usize = 5;
        let mut names: Vec<String> = unkept
            .iter()
            .take(SHOWN)
            .map(|name| format!("“{name}”"))
            .collect();
        if unkept.len() > SHOWN {
            names.push(format!("and {} more", unkept.len() - SHOWN));
        }
        self.raise(Ask::new(
            "Discard what this library cannot keep?".to_string(),
            Some(format!(
                "Nothing can be written to the library open now, so opening another \
                 discards what is unsaved in it: {}.",
                names.join(", ")
            )),
            Verb::Discard,
            vec![Act::OpenLibraryDiscarding(root)],
        ));
    }

    /// Offer what to do with a slot's former occupant an interrupted write to the
    /// instrument left on this computer. Show the file is offered where the system can
    /// show it.
    pub(crate) fn ask_rescue(&mut self, rescue: Rescue) {
        let act = |what| vec![Act::Rescue(rescue.clone(), what)];
        let mut others = Vec::new();
        if rescue.shows() {
            others.push(("Show the file", act(Rescuing::Show)));
        }
        others.push(("Discard…", act(Rescuing::Confirm)));
        self.raise(Ask {
            title: format!("A write to the instrument left “{}”", rescue.name),
            note: Some(
                "It may be the only copy of what that slot held. Keep in library puts it \
                 in the library, where you can send it back."
                    .into(),
            ),
            verb: Verb::Keep,
            acts: act(Rescuing::Keep),
            others,
            cancel: "Later",
            strong: Answer::Verb,
        });
    }

    /// Ask before a slot's former occupant is deleted.
    pub(crate) fn ask_discard_rescue(&mut self, rescue: Rescue) {
        self.raise(Ask::new(
            format!("Discard “{}”?", rescue.name),
            Some("It may be the only copy of what that slot held.".into()),
            Verb::Discard,
            vec![Act::Rescue(rescue, Rescuing::Discard)],
        ));
    }

    /// Ask before a write back to one slot, showing the note that write carries.
    fn ask_write(&mut self, name: &str, at: String, note: String, act: Act) {
        self.ask = Some(Ask::new(
            format!("Save “{name}” to {at}?"),
            Some(note),
            Verb::Save,
            vec![act],
        ));
    }

    /// Ask, as Finder does, before a drop replaces what is in the destination.
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
        self.ask = Some(Ask::new(
            format!("Replace “{occupant}” in {at} with “{incoming}”?"),
            Some(match warning {
                Some(warning) => format!("{warning}\n\n{note}"),
                None => note,
            }),
            Verb::Replace,
            vec![act],
        ));
    }

    /// One bulk action on the checked set, drawn the same in the inspector's Selection card
    /// and in a checked row's menu.
    ///
    /// The control is disabled when it has nothing to act on, and its hover says why:
    /// nothing of the right kind is checked, or the attached instrument refuses all of
    /// it. Delete asks first, once for the whole set.
    pub(crate) fn bulk_item(
        &mut self,
        ui: &mut egui::Ui,
        action: Bulk,
        checked: &[Item],
        workspace: &Workspace,
        state: &DeviceState,
        acts: &mut Vec<Act>,
    ) {
        let offer = Offer::of(action, checked, workspace, state);
        self.bulk_button(ui, action, &offer, checked, state, acts);
    }

    /// The control for one bulk action over the checked set, as `offer` describes it.
    pub(crate) fn bulk_button(
        &mut self,
        ui: &mut egui::Ui,
        action: Bulk,
        offer: &Offer,
        checked: &[Item],
        state: &DeviceState,
        acts: &mut Vec<Act>,
    ) {
        if action == Bulk::Tag {
            // ⚠️ Drawn in `ui` itself, not a scope: a scope is a region of its own, and a
            // button in it cannot move to the next row of a wrapping layout.
            match offer.live {
                false => {
                    ui.add_enabled(false, egui::Button::new(action.label()))
                        .on_disabled_hover_text(action.nothing());
                }
                true => {
                    crate::menu::button(ui, action.label(), |ui| {
                        let locals: Vec<u64> =
                            checked.iter().copied().filter_map(Item::local).collect();
                        self.tag_items(ui, &locals, acts)
                    });
                }
            }
            return;
        }
        let mut button = ui
            .add_enabled(offer.live, egui::Button::new(&offer.label))
            .on_disabled_hover_text(&offer.dead);
        if action == Bulk::Queue {
            button = button
                .on_hover_text("to the slot it is linked to, or the first free one in its folder");
        }
        if !button.clicked() {
            return;
        }
        let wanted = bulk(action, checked, state);
        match action {
            Bulk::Delete => self.ask_discard(checked, wanted),
            _ => acts.extend(wanted),
        }
        ui.close();
    }

    /// Ask once before a checked set is deleted, saying what happens to each part: a slot
    /// is emptied on the instrument, and a local file only leaves the list.
    fn ask_discard(&mut self, checked: &[Item], acts: Vec<Act>) {
        let slots = checked
            .iter()
            .filter(|item| matches!(item, Item::Slot { .. }))
            .count();
        let locals = checked.iter().filter(|item| item.local().is_some()).count();
        let mut note = Vec::new();
        match slots {
            0 => {}
            1 => note.push("1 item is removed from the instrument. There is no undo.".to_string()),
            n => note.push(format!(
                "{n} items are removed from the instrument. There is no undo."
            )),
        }
        match locals {
            0 => {}
            1 => note.push(
                "1 item is deleted from this computer, with its file in the library folder."
                    .to_string(),
            ),
            n => note.push(format!(
                "{n} items are deleted from this computer, with their files in the library \
                 folder."
            )),
        }
        self.ask = Some(Ask::new(
            format!(
                "Delete {}?",
                crate::strings::counted(slots + locals, "checked item", "checked items")
            ),
            Some(note.join("\n\n")),
            Verb::Delete,
            acts,
        ));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::{self, Bench};
    use crate::workspace::Fresh;

    /// Paint the tree headlessly, to catch a layout that panics or an id that collides.
    fn paint(with_device: bool) {
        use crate::workspace::Origin;

        let mut bench = Bench::new();
        let Bench {
            browser,
            workspace,
            device,
            log,
            ..
        } = &mut bench;
        for kind in [Fresh::Program, Fresh::Live, Fresh::Settings] {
            workspace.create(kind, log).unwrap();
        }
        // A folder with something and a folder in it, an empty folder, and a view of a
        // slot: row shapes the list has no other way to reach.
        let root = crate::store::LibPath::root();
        let full = browser.folders.make(&root, workspace);
        browser.folders.make(&root, workspace);
        let filed = workspace.create(Fresh::Program, log).unwrap();
        browser.folders.file(workspace, filed, Some(full));
        let inner = browser.folders.path_of(full).unwrap().clone();
        browser.folders.make(&inner, workspace);
        browser.open.insert(Branch::Folder(full));
        // A tag on something, and one on nothing: the two shapes the section holds.
        let sunday = browser.tags.make("Sunday").unwrap();
        browser.tags.make("Loud").unwrap();
        browser.tags.set(filed, sunday, true);
        let bytes = workspace.get(filed).unwrap().bytes.to_vec();
        workspace.view(
            "Africa-Split.ne5p".into(),
            Origin::Device {
                class: ObjectClass::Program,
                at: Location { bank: 6, slot: 0 },
            },
            bytes,
            log,
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
            device.pretend_partitions(&crate::device::ELECTRO5);
            // Every branch of the instrument open, so every row shape is painted.
            for class in device.state.classes() {
                browser.open.insert(Branch::Class(class.to_raw()));
                for bank in 0..=8 {
                    browser.open.insert(tree::bank_branch(class, bank));
                }
            }
        }

        // Twice: the second pass runs with the widget state the first left behind.
        let ctx = bench.ctx.clone();
        for _ in 0..2 {
            testing::run(&ctx, egui::RawInput::default(), |ctx| {
                egui::SidePanel::left("places")
                    .exact_width(crate::shell::BROWSER)
                    .show(ctx, |ui| {
                        let acts = bench.browser.ui(
                            ui,
                            &bench.workspace,
                            &bench.device,
                            &bench.queue,
                            &Filter::default(),
                        );
                        bench.act(acts);
                    });
            });
        }
    }

    #[test]
    fn the_tree_paints_with_nothing_attached() {
        paint(false);
    }

    #[test]
    fn the_tree_paints_with_an_instrument_to_show() {
        paint(true);
    }

    /// A drag from an unselected row carries only that row.
    #[test]
    fn a_drag_from_a_picked_row_carries_the_whole_selection() {
        let Bench {
            mut browser,
            mut workspace,
            device,
            mut log,
            ..
        } = Bench::new();
        let ids: Vec<u64> = (0..3)
            .map(|_| workspace.create(Fresh::Program, &mut log).unwrap())
            .collect();
        let apart = workspace.create(Fresh::Live, &mut log).unwrap();
        for id in &ids {
            browser.selection.toggle(Item::Local(*id));
        }

        let head = browser
            .held(Item::Local(ids[0]), &workspace, &device.state)
            .expect("a local is dragged");
        let carried = browser.carrying(head, "Africa Split", &workspace, &device.state);
        assert_eq!(carried.rest.len(), 2);
        assert_eq!(carried.all().count(), 3);
        assert!(carried.name.contains("+2"), "{}", carried.name);

        let outside = browser
            .held(Item::Local(apart), &workspace, &device.state)
            .expect("a local is dragged");
        let alone = browser.carrying(outside, "Squabble B", &workspace, &device.state);
        assert!(
            alone.rest.is_empty(),
            "an unselected row carries only itself"
        );
        assert_eq!(alone.name, "Squabble B", "and says only its own name");
    }

    /// ⚠️ A slot the scan found empty holds nothing to carry. Carried with a selection,
    /// it would become a read of nothing from the instrument, a round trip that can only
    /// fail.
    #[test]
    fn an_empty_slot_is_not_something_a_drag_carries() {
        let Bench {
            mut browser,
            workspace,
            mut device,
            ..
        } = Bench::new();
        device.pretend_scanned(ObjectClass::Program, 7, &["Africa Split", ""]);
        let slot = |slot| Item::Slot {
            class: ObjectClass::Program,
            at: Location { bank: 6, slot },
        };

        let held = browser
            .held(slot(0), &workspace, &device.state)
            .expect("7:1 holds something");
        assert!(
            browser.held(slot(1), &workspace, &device.state).is_none(),
            "7:2 was read and found empty"
        );

        browser.selection.toggle(slot(0));
        browser.selection.toggle(slot(1));
        let carried = browser.carrying(held, "Africa Split", &workspace, &device.state);
        assert!(carried.rest.is_empty(), "the empty slot stays where it is");
        assert_eq!(carried.name, "Africa Split", "and the ghost counts nothing");
    }

    /// ⚠️ The rest of the selection follows only when the drop repeats one act. Filing
    /// three assets is three filings, but sending three to one slot would overwrite each
    /// in turn, so a single destination takes only the pressed row.
    #[test]
    fn a_drop_of_many_repeats_only_where_one_destination_does_not() {
        let Bench {
            mut browser,
            mut workspace,
            device,
            mut log,
            ..
        } = Bench::new();
        let ids: Vec<u64> = (0..3)
            .map(|_| workspace.create(Fresh::Program, &mut log).unwrap())
            .collect();
        for id in &ids {
            browser.selection.toggle(Item::Local(*id));
        }
        let folder = browser
            .folders
            .make(&crate::store::LibPath::root(), &workspace);
        let head = browser
            .held(Item::Local(ids[0]), &workspace, &device.state)
            .unwrap();
        let carried = Arc::new(browser.carrying(head, "Africa Split", &workspace, &device.state));

        let mut filed = Vec::new();
        browser.land(&carried, Onto::Group(folder), &mut filed);
        assert_eq!(filed.len(), 3, "every selected asset goes into the folder");
        assert!(filed.iter().all(|act| matches!(
            act,
            Act::File {
                folder: Some(_),
                ..
            }
        )));

        let mut sent = Vec::new();
        browser.land(
            &carried,
            Onto::Slot {
                class: ObjectClass::Program,
                at: Location { bank: 6, slot: 0 },
            },
            &mut sent,
        );
        assert_eq!(sent.len(), 1, "one slot takes one asset");
        assert!(matches!(sent[0], Act::Send { id, .. } if id == ids[0]));
    }

    /// ⚠️ A row inside a folder is a drop target for that folder. Otherwise, releasing a
    /// filed asset where it was pressed, or on a sibling, would take it out of its
    /// folder.
    #[test]
    fn a_drop_onto_a_row_inside_a_folder_never_unfiles_it() {
        let Bench {
            mut browser,
            mut workspace,
            device,
            mut log,
            ..
        } = Bench::new();
        let folder = browser
            .folders
            .make(&crate::store::LibPath::root(), &workspace);
        let id = workspace.create(Fresh::Program, &mut log).unwrap();
        browser.folders.file(&mut workspace, id, Some(folder));
        let head = browser
            .held(Item::Local(id), &workspace, &device.state)
            .expect("a local is dragged");
        let carried = Arc::new(browser.carrying(head, "Africa Split", &workspace, &device.state));

        let mut onto_sibling = Vec::new();
        browser.land(&carried, tree::onto_list(Some(folder)), &mut onto_sibling);
        assert!(
            !onto_sibling
                .iter()
                .any(|act| matches!(act, Act::File { folder: None, .. })),
            "a drop on a row inside the folder keeps the asset in the folder"
        );

        let mut onto_loose = Vec::new();
        browser.land(&carried, tree::onto_list(None), &mut onto_loose);
        assert!(
            matches!(onto_loose.as_slice(), [Act::File { folder: None, .. }]),
            "a drop on a loose row takes the asset out of the folder"
        );
    }

    /// A file dropped from outside lands in the folder whose row it was dropped on, and
    /// anywhere else, or where the drop point is not known, at the library's top level.
    #[test]
    fn a_file_dropped_on_a_folder_row_lands_in_that_folder() {
        let Bench {
            ctx,
            mut browser,
            workspace,
            device,
            queue,
            ..
        } = Bench::new();
        let folder = browser.folders.make(&LibPath::root(), &workspace);
        let path = browser.folders.path_of(folder).cloned().unwrap();
        let input = testing::screen(egui::vec2(400.0, 600.0), Vec::new());
        let output = testing::run(&ctx, input, |ctx| {
            egui::SidePanel::left("places").show(ctx, |ui| {
                browser.ui(ui, &workspace, &device, &queue, &Filter::default());
            });
        });
        let row = testing::where_(&testing::painted(&output), path.leaf());
        assert_eq!(browser.landing_dir(Some(row.center())), path);
        let elsewhere = egui::pos2(390.0, 590.0);
        assert_eq!(browser.landing_dir(Some(elsewhere)), LibPath::root());
        assert_eq!(browser.landing_dir(None), LibPath::root());

        browser.forget_targets();
        assert_eq!(
            browser.landing_dir(Some(row.center())),
            LibPath::root(),
            "only where the rows were drawn last"
        );
    }

    /// ⚠️ F2 renames only a row selected alone. A rename typed while several are selected
    /// would look like it applies to all of them, and only one would change.
    #[test]
    fn f2_renames_only_while_its_row_is_the_only_one_picked() {
        let Bench {
            ctx,
            mut browser,
            mut workspace,
            device,
            queue,
            mut log,
            ..
        } = Bench::new();
        let one = workspace.create(Fresh::Program, &mut log).unwrap();
        let two = workspace.create(Fresh::Program, &mut log).unwrap();
        let press_f2 = |browser: &mut Browser| {
            let input = egui::RawInput {
                events: vec![testing::key(egui::Key::F2)],
                ..Default::default()
            };
            testing::run(&ctx, input, |ctx| {
                egui::SidePanel::left("places").show(ctx, |ui| {
                    browser.ui(ui, &workspace, &device, &queue, &Filter::default());
                });
            });
            browser.rename.as_ref().map(|rename| rename.what)
        };

        browser.selection.only(Item::Local(one));
        browser.selection.toggle(Item::Local(two));
        assert_eq!(press_f2(&mut browser), None, "two rows selected");
        browser.selection.plain(Item::Local(two));
        assert_eq!(
            press_f2(&mut browser),
            Some(Item::Local(one)),
            "a plain click on one of the two leaves the other sole"
        );
    }

    /// ⚠️ Escape during a rename or a question cancels only that, and the selection stays.
    #[test]
    fn escape_lets_go_of_the_selection_unless_a_name_or_a_question_is_open() {
        let ctx = testing::context();
        let mut browser = Browser::default();
        let escape = egui::RawInput {
            events: vec![testing::key(egui::Key::Escape)],
            ..Default::default()
        };

        browser.start_rename(Item::Local(1), "Africa Split");
        testing::run(&ctx, escape.clone(), |ctx| browser.let_go(ctx));
        assert_eq!(
            browser.picked().items().count(),
            1,
            "Escape in the editor cancels only the rename"
        );

        browser.rename = None;
        browser.ask = Some(Ask::new(
            "Delete 1 checked item?".into(),
            None,
            Verb::Delete,
            Vec::new(),
        ));
        testing::run(&ctx, escape.clone(), |ctx| browser.let_go(ctx));
        assert_eq!(
            browser.picked().items().count(),
            1,
            "Escape in a question cancels only the question"
        );

        browser.ask = None;
        testing::run(&ctx, escape, |ctx| browser.let_go(ctx));
        assert_eq!(browser.picked().items().count(), 0);
    }

    /// End to end: open the editor, type, press Enter, and the new name comes back as an
    /// act. The tests of [`renamed`] cannot see the text field, and detecting Enter on it
    /// is the part that is easy to get wrong.
    #[test]
    fn typing_a_name_and_pressing_enter_renames_the_row() {
        let Bench {
            ctx,
            mut browser,
            mut workspace,
            device,
            queue,
            mut log,
            ..
        } = Bench::new();
        let id = workspace.create(Fresh::Program, &mut log).unwrap();

        // Frame one opens the editor, focused with the name selected; frame two types
        // over the name; frame three commits.
        let frames: [Vec<egui::Event>; 3] = [
            Vec::new(),
            vec![egui::Event::Text("LA Grand".into())],
            vec![testing::key(egui::Key::Enter)],
        ];
        browser.start_rename(Item::Local(id), "Africa Split");

        let mut named = None;
        for events in frames {
            let input = egui::RawInput {
                events,
                ..Default::default()
            };
            testing::run(&ctx, input, |ctx| {
                egui::SidePanel::left("places").show(ctx, |ui| {
                    for act in browser.ui(ui, &workspace, &device, &queue, &Filter::default()) {
                        if let Act::RenameLocal { name, .. } = act {
                            named = Some(name);
                        }
                    }
                });
            });
        }
        assert_eq!(named.as_deref(), Some("LA Grand"));
        assert!(browser.rename.is_none(), "and the editor closes");
    }

    #[test]
    fn revealing_a_filed_asset_opens_every_folder_above_it() {
        let mut bench = Bench::new();
        let id = bench
            .workspace
            .create(Fresh::Program, &mut bench.log)
            .unwrap();
        let outer = bench
            .browser
            .folders
            .make(&LibPath::root(), &bench.workspace);
        let within = bench.browser.folders.path_of(outer).unwrap().clone();
        let inner = bench.browser.folders.make(&within, &bench.workspace);
        bench
            .browser
            .folders
            .file(&mut bench.workspace, id, Some(inner));

        bench.act(vec![Act::Reveal(Item::Local(id))]);
        for branch in [
            Branch::Computer,
            Branch::Folder(outer),
            Branch::Folder(inner),
        ] {
            assert!(bench.browser.open.contains(&branch), "{branch:?} is shut");
        }
    }

    /// A slot renamed from outside the tree, with the browser hidden and its folder shut,
    /// still gets a row to type the name in.
    #[test]
    fn renaming_a_slot_from_another_tab_shows_the_row_it_types_in() {
        let mut bench = Bench::new();
        let class = ObjectClass::Program;
        bench.device.pretend_partitions(&crate::device::ELECTRO5);
        bench.device.pretend_scanned(class, 7, &["Africa Split"]);
        bench.shell.browser_open = false;
        let slot = Item::Slot {
            class,
            at: Location::from_user(7, 1),
        };

        bench.browser.start_rename(slot, "Africa Split");
        bench.act(vec![Act::Reveal(slot)]);
        assert!(bench.shell.browser_open, "the browser is shown");

        let frames: [Vec<egui::Event>; 3] = [
            Vec::new(),
            vec![egui::Event::Text("LA Grand".into())],
            vec![testing::key(egui::Key::Enter)],
        ];
        let mut named = None;
        for events in frames {
            let input = egui::RawInput {
                events,
                ..Default::default()
            };
            let Bench {
                ctx,
                browser,
                workspace,
                device,
                queue,
                ..
            } = &mut bench;
            testing::run(ctx, input, |ctx| {
                egui::SidePanel::left("places").show(ctx, |ui| {
                    for act in browser.ui(ui, workspace, device, queue, &Filter::default()) {
                        if let Act::RenameSlot { name, .. } = act {
                            named = Some(name);
                        }
                    }
                });
            });
        }
        assert_eq!(named.as_deref(), Some("LA Grand"));
    }

    #[test]
    fn a_rename_that_changes_nothing_is_not_a_rename() {
        assert_eq!(renamed("Africa Split", "Africa Split"), None);
        assert_eq!(renamed("Africa Split", "  Africa Split "), None);
        assert_eq!(renamed("Africa Split", "   "), None);
        assert_eq!(renamed("Africa Split", ""), None);
    }

    #[test]
    fn a_rename_takes_the_typed_name_trimmed() {
        assert_eq!(renamed("Africa Split", "LA Grand"), Some("LA Grand".into()));
        assert_eq!(
            renamed("Africa Split", "  LA Grand  "),
            Some("LA Grand".into())
        );
    }

    /// Two assets given a new tag have that tag, and the third does not.
    #[test]
    fn a_tag_put_on_a_multi_selection_is_on_exactly_those_assets() {
        let mut bench = Bench::new();
        let ids: Vec<u64> = (0..3)
            .map(|_| {
                bench
                    .workspace
                    .create(Fresh::Program, &mut bench.log)
                    .unwrap()
            })
            .collect();

        bench.act(vec![Act::NewTag(ids[..2].to_vec())]);
        let Some(Item::Tag(tag)) = bench.browser.rename.as_ref().map(|r| r.what) else {
            panic!("a new tag opens its editor");
        };
        assert!(bench.browser.tags.on_all(&ids[..2], tag));
        assert!(!bench.browser.tags.worn(ids[2]).contains(&tag));
    }

    /// The question's verb runs what was asked, Cancel and Escape run nothing, and each
    /// closes the question.
    #[test]
    fn a_confirmation_runs_its_acts_only_on_its_verb() {
        let ctx = testing::context();
        egui_extras::install_image_loaders(&ctx);
        let at = Location { bank: 6, slot: 3 };
        let delete = || Act::DeleteSlot {
            class: ObjectClass::Program,
            at,
        };
        let frame = |browser: &mut Browser, events: Vec<egui::Event>| {
            let mut acts = Vec::new();
            let input = egui::RawInput {
                events,
                ..Default::default()
            };
            let output = testing::run(&ctx, input, |ctx| browser.dialog(ctx, &mut acts));
            (acts, testing::painted(&output))
        };

        for (answer, runs) in [("Delete", true), ("Cancel", false), ("Escape", false)] {
            let mut browser = Browser {
                ask: Some(Ask::new(
                    "Delete “Africa Split” from Programs 7:4?".into(),
                    Some("It is removed from the instrument. There is no undo.".into()),
                    Verb::Delete,
                    vec![delete()],
                )),
                ..Browser::default()
            };
            // A sheet is laid out unseen on its first frame.
            frame(&mut browser, Vec::new());
            let (_, placed) = frame(&mut browser, Vec::new());
            let events = match answer {
                "Escape" => vec![testing::key(egui::Key::Escape)],
                word => {
                    let at = testing::where_(&placed, word).center();
                    frame(&mut browser, vec![testing::button(at, true)]);
                    vec![testing::button(at, false)]
                }
            };
            let (acts, _) = frame(&mut browser, events);
            assert_eq!(acts.contains(&delete()), runs, "{answer}: {acts:?}");
            assert!(browser.ask.is_none(), "{answer} closes the question");
        }
    }

    #[test]
    fn the_delete_question_counts_in_the_number_it_names() {
        let mut browser = Browser::default();
        browser.ask_discard(&[Item::Local(1)], Vec::new());
        let ask = browser.ask.take().expect("a question");
        assert_eq!(ask.title, "Delete 1 checked item?");
        assert_eq!(
            ask.note.as_deref(),
            Some("1 item is deleted from this computer, with its file in the library folder.")
        );

        let slot = |slot| Item::Slot {
            class: ObjectClass::Program,
            at: Location { bank: 0, slot },
        };
        browser.ask_discard(&[slot(0), slot(1), Item::Local(1)], Vec::new());
        let ask = browser.ask.take().expect("a question");
        assert_eq!(ask.title, "Delete 3 checked items?");
        let note = ask.note.unwrap_or_default();
        assert!(
            note.starts_with("2 items are removed from the instrument."),
            "{note}"
        );
    }

    /// A new tag's name is typed in the browser's tags section, so making one shows it.
    #[test]
    fn a_new_tag_opens_the_section_its_name_is_typed_in() {
        let mut bench = Bench::new();
        bench.browser.sections.tags = false;
        bench.shell.browser_open = false;

        bench.act(vec![Act::NewTag(Vec::new())]);
        assert!(bench.browser.sections.tags, "the tags section is open");
        assert!(bench.shell.browser_open, "the browser is open");
    }

    /// ⚠️ A view is the only copy of its bytes and the store skips it, so a tag on a view
    /// would be lost with its tab. Tagging keeps it on this computer first, and the log
    /// says so.
    #[test]
    fn tagging_a_view_keeps_it_on_this_computer_first_and_says_so() {
        use crate::workspace::Origin;

        let mut bench = Bench::new();
        let bytes = Fresh::Program.bytes().unwrap();
        let id = bench.workspace.view(
            "Africa-Split.ne5p".into(),
            Origin::Device {
                class: ObjectClass::Program,
                at: Location { bank: 6, slot: 0 },
            },
            bytes,
            &mut bench.log,
        );
        let tag = bench.browser.tags.make("Sunday").unwrap();
        assert!(bench.workspace.is_view(id));

        bench.act(vec![Act::Tag { ids: vec![id], tag }]);
        assert!(!bench.workspace.is_view(id), "it is kept on this computer");
        assert!(bench.browser.tags.worn(id).contains(&tag));
        let said = bench.log.transcript();
        assert!(said.contains("Kept a copy on this computer"), "{said}");
    }

    /// The review warns once per format, however many items carry it.
    ///
    /// The instrument reports a model the acceptance table does not know, so the only
    /// check left is against the folder's own formats. A known family would refuse these
    /// files outright.
    #[test]
    fn the_review_warns_once_when_a_batch_is_of_another_model() {
        use crate::workspace::Origin;

        let Bench {
            mut workspace,
            mut device,
            mut queue,
            mut log,
            ..
        } = Bench::new();
        let class = ObjectClass::Program;
        device.pretend_scanned(class, 7, &["Africa Split", "Squabble B"]);
        device.pretend_attached_as("unnamed device");

        let mut ids = Vec::new();
        for slot in 0..2 {
            let bytes = Fresh::Stage4Program.bytes().unwrap();
            let at = Location { bank: 6, slot };
            let id = workspace.ingest(
                format!("stage-{slot}.ns4p"),
                Origin::Device { class, at },
                bytes,
                &mut log,
            );
            crate::queue::enqueue(&workspace, &mut device, &mut queue, &mut log, id, class, at);
            ids.push(id);
        }

        let warnings = send_warnings(&queue, &workspace, &device.state);
        let foreign = warnings
            .iter()
            .filter(|warning| warning.contains("This file is ns4p"))
            .count();
        assert_eq!(
            foreign, 1,
            "two files of one model, one warning: {warnings:?}"
        );
    }
}
