//! The browser's tree: the places a sound can be, the kinds of sound, and the tags on
//! this computer's list.
//!
//! Every row is drawn by [`super::row::row`]. The tree computes its own indents instead
//! of nesting `Ui`s, so a leaf can skip the triangle's box and line its glyph up under
//! the glyph of the branch beside it.

use std::collections::BTreeMap;

use eframe::egui;
use nord_format::accept::Family;
use nord_usb::{Location, ObjectClass};

use super::act::{spare_slot, will_write, Act, Bulk, LOAD_ON_INSTRUMENT};
use super::drag::{kinds_present, qualifier, Item, Kept, Kind, Onto};
use super::row::{row, Cells, Drawn, STEP};
use super::{Ask, Browser, Click, Verb};
use crate::device::{occupancy, read_only, Device, DeviceState};
use crate::filter::{Filter, Narrow, Place, State};
use crate::folders::{Folder, Folders, SHOW_ALL_FILES};
use crate::icon::Glyph;
use crate::libraries::Picking;
use crate::menu::{self, marked, new_menu, OPEN_LIBRARY};
use crate::panel::panel_header;
use crate::queue::{Queue, Queued};
use crate::store::LibPath;
use crate::strings::{place, shown};
use crate::tabs::Spot;
use crate::workspace::{LocalEntity, Workspace};

/// A menu item that runs `act` and closes the menu.
fn offer(ui: &mut egui::Ui, label: &str, hint: Option<&str>, act: Act, acts: &mut Vec<Act>) {
    let mut button = ui.button(label);
    if let Some(hint) = hint {
        button = button.on_hover_text(hint);
    }
    if button.clicked() {
        acts.push(act);
        ui.close();
    }
}

/// A row of the tree with something under it.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub(super) enum Branch {
    Computer,
    Folder(u64),
    Instrument,
    Class(u32),
    /// ⚠️ The bank as the panel numbers it, which keys the scan cache. Never a
    /// [`Location`]'s zero-based bank.
    Bank(u32, u64),
}

/// Which of the three sections are showing.
#[derive(Clone, Copy)]
pub(super) struct Sections {
    pub(super) places: bool,
    kinds: bool,
    pub(super) tags: bool,
}

impl Default for Sections {
    fn default() -> Sections {
        Sections {
            places: true,
            kinds: true,
            tags: true,
        }
    }
}

impl Sections {
    fn open(&mut self, section: Section) -> &mut bool {
        match section {
            Section::Places => &mut self.places,
            Section::Kinds => &mut self.kinds,
            Section::Tags => &mut self.tags,
        }
    }
}

/// One of the three sections, as the header over it names it.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(super) enum Section {
    Places,
    Kinds,
    Tags,
}

impl Section {
    fn title(self) -> &'static str {
        match self {
            Section::Places => "Places",
            Section::Kinds => "Kinds",
            Section::Tags => "Tags",
        }
    }
}

/// One line of the tree, in the order the tree draws them.
#[derive(Clone, Copy, PartialEq, Debug)]
pub(super) enum Line {
    Header(Section),
    /// The space after a section, open or shut, before the next header.
    End(Section),
    Computer,
    Folder {
        id: u64,
        depth: usize,
    },
    /// An asset, drawn under `folder`, or loose in the root for `None`.
    Local {
        id: u64,
        folder: Option<u64>,
        depth: usize,
    },
    /// A file drawbar does not hold: the `index`th of [`Folders::unread`] when `unread`,
    /// and otherwise of [`Folders::others`].
    Stranger {
        unread: bool,
        index: usize,
        depth: usize,
    },
    Lost(u64),
    /// A faint line where `under` has nothing to show.
    Note {
        under: Branch,
        said: &'static str,
        depth: usize,
    },
    Instrument,
    Class(ObjectClass),
    /// ⚠️ The bank as the panel numbers it, as [`DeviceState::banks_of`] lists it.
    Bank(ObjectClass, u32),
    Slot {
        class: ObjectClass,
        bank: u32,
        at: Location,
        depth: usize,
    },
    State(State),
    Kind(Kind),
    Tag(u64),
    NewTag,
}

impl Line {
    /// The height [`row`] or [`panel_header`] gives the line.
    fn height(self) -> f32 {
        match self {
            Line::Header(_) => crate::panel::HEADER,
            // With the gap after the last row and after this, the section's rows end
            // `AFTER_SECTION` above the next header.
            Line::End(_) => AFTER_SECTION - 2.0 * ROW_GAP,
            Line::Computer
            | Line::Instrument
            | Line::State(_)
            | Line::Kind(_)
            | Line::Tag(_)
            | Line::NewTag => super::row::ROW,
            Line::Folder { .. }
            | Line::Local { .. }
            | Line::Stranger { .. }
            | Line::Lost(_)
            | Line::Note { .. }
            | Line::Class(_)
            | Line::Bank(..)
            | Line::Slot { .. } => super::row::CHILD,
        }
    }

    /// What the line stands for, where it can be selected or renamed.
    fn item(self) -> Option<Item> {
        match self {
            Line::Folder { id, .. } => Some(Item::Folder(id)),
            Line::Local { id, .. } => Some(Item::Local(id)),
            Line::Slot { class, at, .. } => Some(Item::Slot { class, at }),
            Line::Tag(id) => Some(Item::Tag(id)),
            _ => None,
        }
    }

    /// The id the line's widgets are made under. It names what the line stands for, not
    /// where it is, so a press or an editor on it survives lines coming and going above.
    fn id(self) -> egui::Id {
        match self {
            Line::Header(section) => egui::Id::new(("header", section.title())),
            Line::End(section) => egui::Id::new(("end", section.title())),
            Line::Computer => egui::Id::new("computer"),
            Line::Folder { id, .. } => egui::Id::new(("folder", id)),
            Line::Local { id, .. } => egui::Id::new(("local", id)),
            Line::Stranger { unread, index, .. } => egui::Id::new(("stranger", unread, index)),
            Line::Lost(id) => egui::Id::new(("lost", id)),
            Line::Note { under, said, .. } => egui::Id::new(("note", under, said)),
            Line::Instrument => egui::Id::new("instrument"),
            Line::Class(class) => egui::Id::new(("class", class.to_raw())),
            Line::Bank(class, bank) => egui::Id::new(("bank", class.to_raw(), bank)),
            Line::Slot { class, at, .. } => {
                egui::Id::new(("slot", class.to_raw(), at.bank, at.slot))
            }
            Line::State(state) => egui::Id::new(("state", state.title())),
            Line::Kind(kind) => egui::Id::new(("kind", Kind::ALL.iter().position(|k| *k == kind))),
            Line::Tag(id) => egui::Id::new(("tag", id)),
            Line::NewTag => egui::Id::new("new tag"),
        }
    }
}

/// What the lines under This computer are taken from. They are taken again only when
/// one of these changes.
#[derive(PartialEq)]
struct Taken {
    layout: u64,
    folders: crate::folders::Shape,
    /// The open folders.
    open: Vec<u64>,
    /// A folder being renamed shows what is in it, open or not.
    renaming: Option<Item>,
}

impl Taken {
    fn of(browser: &Browser, workspace: &Workspace) -> Taken {
        Taken {
            layout: workspace.layout(),
            folders: browser.folders.shape(),
            open: browser
                .open
                .iter()
                .filter_map(|branch| match branch {
                    Branch::Folder(id) => Some(*id),
                    _ => None,
                })
                .collect(),
            renaming: browser.rename.as_ref().map(|rename| rename.what),
        }
    }
}

/// The tree's lines, and where each one starts.
///
/// The lines under This computer are as many as the library holds, so they are kept
/// between frames. The rest are few, and taken every frame.
#[derive(Default)]
pub(super) struct Rows {
    taken: Option<Taken>,
    library: Vec<Line>,
    /// The lines above and below the library's, and whether the library's were between
    /// them, as `lines` was laid out.
    around: [Vec<Line>; 2],
    between: bool,
    spacing: f32,
    lines: Vec<Line>,
    /// The top of each line, from the top of the first, then the bottom of the last
    /// plus one spacing: one more than `lines`.
    tops: Vec<f32>,
    /// How many times the library's lines have been taken.
    #[cfg(test)]
    pub(super) takes: usize,
}

impl Rows {
    fn update(
        &mut self,
        browser: &Browser,
        workspace: &Workspace,
        around: [Vec<Line>; 2],
        spacing: f32,
    ) {
        let between = browser.sections.places && browser.open.contains(&Branch::Computer);
        let mut moved = false;
        if between {
            let taken = Taken::of(browser, workspace);
            if self.taken.as_ref() != Some(&taken) {
                self.library = browser.library_lines(workspace);
                self.taken = Some(taken);
                moved = true;
                #[cfg(test)]
                {
                    self.takes += 1;
                }
            }
        }
        if !moved
            && self.around == around
            && self.between == between
            && self.spacing == spacing
            && !self.tops.is_empty()
        {
            return;
        }
        let [above, below] = &around;
        let library = match between {
            true => self.library.as_slice(),
            false => &[],
        };
        self.lines.clear();
        self.lines.extend(above.iter().chain(library).chain(below));
        self.tops.clear();
        let mut top = 0.0;
        for line in &self.lines {
            self.tops.push(top);
            top += line.height() + spacing;
        }
        self.tops.push(top);
        self.around = around;
        self.between = between;
        self.spacing = spacing;
    }

    /// The height of every line, laid out one under another.
    fn height(&self) -> f32 {
        match self.tops.last() {
            Some(end) if !self.lines.is_empty() => end - self.spacing,
            _ => 0.0,
        }
    }

    /// The lines any part of which is between `from` and `to`, measured from the top of
    /// the first.
    fn within(&self, from: f32, to: f32) -> std::ops::Range<usize> {
        if self.lines.is_empty() {
            return 0..0;
        }
        let first = self.tops[1..].partition_point(|end| *end <= from);
        let last = self.tops[..self.lines.len()].partition_point(|top| *top < to);
        first..last.max(first)
    }

    /// Where line `at` is drawn in a tree whose first line starts at the top of `area`.
    fn rect(&self, at: usize, area: egui::Rect) -> egui::Rect {
        egui::Rect::from_min_size(
            egui::pos2(area.left(), area.top() + self.tops[at]),
            egui::vec2(area.width(), self.lines[at].height()),
        )
    }

    fn position(&self, item: Item) -> Option<usize> {
        self.lines.iter().position(|line| line.item() == Some(item))
    }
}

/// The folders inside each folder, and the files drawbar does not hold in each, gathered
/// in one pass for a pass over the library's lines.
struct Beside<'a> {
    /// In path order, under the folder they are in, or `None` for the root.
    children: BTreeMap<Option<u64>, Vec<&'a Folder>>,
    /// Under the folder they are in: the files drawbar did not read, as `(true, index)`
    /// into [`Folders::unread`], then, while all files are shown, those it does not open,
    /// as `(false, index)` into [`Folders::others`].
    strangers: BTreeMap<LibPath, Vec<(bool, usize)>>,
}

impl<'a> Beside<'a> {
    fn of(folders: &'a Folders) -> Beside<'a> {
        let ids: BTreeMap<&LibPath, u64> = folders
            .all()
            .iter()
            .map(|folder| (&folder.path, folder.id))
            .collect();
        let mut children: BTreeMap<Option<u64>, Vec<&Folder>> = BTreeMap::new();
        for folder in folders.all() {
            let parent = folder.path.parent();
            let holder = match parent.is_root() {
                true => None,
                false => match ids.get(&parent) {
                    Some(id) => Some(*id),
                    None => continue,
                },
            };
            children.entry(holder).or_default().push(folder);
        }
        let unread = folders
            .unread
            .iter()
            .enumerate()
            .map(|(index, (path, _))| (path, (true, index)));
        let others = folders
            .others
            .iter()
            .enumerate()
            .filter(|_| folders.all_files)
            .map(|(index, path)| (path, (false, index)));
        let mut strangers: BTreeMap<LibPath, Vec<(bool, usize)>> = BTreeMap::new();
        for (path, at) in unread.chain(others) {
            strangers.entry(path.parent()).or_default().push(at);
        }
        Beside {
            children,
            strangers,
        }
    }

    fn children(&self, folder: Option<u64>) -> &[&'a Folder] {
        self.children.get(&folder).map_or(&[], Vec::as_slice)
    }

    fn strangers(&self, dir: &LibPath) -> &[(bool, usize)] {
        self.strangers.get(dir).map_or(&[], Vec::as_slice)
    }
}

/// What the lines in view read from outside the tree, taken once a frame.
struct Frame {
    naming: Naming,
    /// The slots open as views.
    viewed: Vec<(ObjectClass, Location)>,
    /// How many are waiting to be sent, and how many differ from the instrument.
    counts: [usize; 2],
}

/// Where a row's contents start.
///
/// A top-level branch starts 6 px in, a leaf beside it at 26, and a leaf one level down
/// at 36. A leaf skips the triangle's box, which lines its glyph up under the glyph of a
/// branch at the same depth.
fn indent(depth: usize, branch: bool) -> f32 {
    const FIRST: f32 = 6.0;
    const DOWN: f32 = 10.0;
    let past_the_triangle = match branch {
        true => 0.0,
        false => STEP,
    };
    FIRST + DOWN * depth as f32 + past_the_triangle
}

/// The space above the first section, the gap between rows, and the space after each
/// section.
const TOP: f32 = 8.0;
const ROW_GAP: f32 = 1.0;
const AFTER_SECTION: f32 = 8.0;

/// Whether a click landed on the triangle, which opens the branch instead of selecting
/// the row.
fn on_triangle(drawn: &Drawn) -> bool {
    let (Some(box_), Some(at)) = (drawn.chevron, drawn.response.interact_pointer_pos()) else {
        return false;
    };
    box_.expand(3.0).contains(at)
}

/// Turn one of the library's filters on or off, and bring the library forward to show the
/// result. A filter applied out of sight would surprise whoever finds it later.
fn narrow(acts: &mut Vec<Act>, narrow: Narrow) {
    acts.push(Act::ShowTab(Spot::Library));
    acts.push(Act::Narrow(narrow));
}

/// Whether a menu offers [`library_items`]: where this build can pick a folder, or could
/// once the browser turns folder access on, or where
/// [`crate::folders::Folders::libraries`] lists some.
pub fn offers_libraries(folders: &crate::folders::Folders, picking: Picking) -> bool {
    picking != Picking::Absent || !folders.libraries.is_empty()
}

/// The items that open another folder as the library, switch to a recent one, and show
/// the open one's folder. Open library folder… is grayed out where the browser turns
/// folder access off, and says on hover how to turn it on.
pub fn library_items(
    ui: &mut egui::Ui,
    folders: &crate::folders::Folders,
    picking: Picking,
    acts: &mut Vec<Act>,
) {
    if let Some(library) = &folders.reconnect {
        let label = format!("Reconnect {}", library.name);
        offer(
            ui,
            &label,
            None,
            Act::OpenLibrary(library.root.clone()),
            acts,
        );
    }
    match picking {
        Picking::On => offer(ui, OPEN_LIBRARY, None, Act::PickLibrary, acts),
        Picking::TurnedOff => {
            ui.add_enabled(false, egui::Button::new(OPEN_LIBRARY))
                .on_disabled_hover_text(crate::libraries::TURNED_OFF);
        }
        Picking::Absent => {}
    }
    if !folders.libraries.is_empty() {
        menu::button(ui, "Open recent library", |ui| {
            for library in &folders.libraries {
                if marked(ui, &library.name, library.open) && !library.open {
                    acts.push(Act::OpenLibrary(library.root.clone()));
                }
            }
        });
    }
    let reveal = folders.place.as_ref().and_then(|at| at.reveal.as_ref());
    if let Some(url) = reveal {
        if ui.button("Show the library folder").clicked() {
            ui.ctx().open_url(egui::OpenUrl::new_tab(url));
            ui.close();
        }
    }
}

/// A line where a branch has nothing to show.
fn nothing(ui: &mut egui::Ui, depth: usize, said: &str) {
    row(
        ui,
        false,
        &Cells {
            indent: indent(depth, false),
            name: said,
            faint: true,
            child: true,
            ..Cells::default()
        },
    );
}

/// Whether the kinds section is worth showing. With one kind in both places there is
/// nothing to choose between, and the row would only narrow the library to what it
/// already shows.
fn worth_choosing(kinds: &[Kind]) -> bool {
    kinds.len() > 1
}

/// The open-set key for one bank's rows.
pub(super) fn bank_branch(class: ObjectClass, bank: u64) -> Branch {
    Branch::Bank(class.to_raw(), bank)
}

/// What a local row's kind word needs from outside the row: what this computer's list
/// holds, and the attached instrument's family. Read once a frame, because every row asks
/// the same question of the whole list.
struct Naming {
    kept: Kept,
    instrument: Option<Family>,
}

/// Where a drop onto a row of the local list lands: the folder the row is drawn under, or
/// the loose part of the list.
pub(super) fn onto_list(folder: Option<u64>) -> Onto {
    match folder {
        Some(id) => Onto::Group(id),
        None => Onto::Computer,
    }
}

/// Whether a bank's name says anything the number beside every row does not.
///
/// Program banks come back named "Bank 1", "Bank 2", which only repeats the location
/// column. Piano banks come back named "Grand" and "Upright", which is why a caption is
/// shown at all.
fn worth_captioning(bank: u32, name: &str) -> bool {
    let name = name.trim();
    !name.is_empty()
        && name != bank.to_string()
        && !name.eq_ignore_ascii_case(&format!("bank {bank}"))
}

impl Browser {
    pub(super) fn tree(
        &mut self,
        ui: &mut egui::Ui,
        workspace: &Workspace,
        device: &Device,
        queue: &Queue,
        filter: &Filter,
        acts: &mut Vec<Act>,
    ) {
        self.follow_jump(device);
        let frame = self.frame(workspace, device, queue);
        let around = self.lines_around(workspace, device, filter, &frame);
        let mut rows = std::mem::take(&mut self.rows);
        rows.update(self, workspace, around, ROW_GAP);
        self.rename_on_f2(ui, &rows, workspace, device);
        egui::ScrollArea::vertical()
            .id_salt("browser_tree")
            .auto_shrink([false; 2])
            .show_viewport(ui, |ui, viewport| {
                let area = ui.max_rect().with_min_y(ui.max_rect().top() + TOP);
                ui.set_min_height(TOP + rows.height());
                let shown = rows.within(viewport.min.y - TOP, viewport.max.y - TOP);
                workspace.in_view(
                    rows.lines[shown.clone()]
                        .iter()
                        .filter_map(|line| match line {
                            Line::Local { id, .. } => Some(*id),
                            _ => None,
                        }),
                );
                // ⚠️ The line being renamed is drawn wherever it is. An editor left undrawn
                // for a frame loses its focus, and losing focus ends the rename.
                let renaming = self
                    .rename
                    .as_ref()
                    .and_then(|rename| rows.position(rename.what))
                    .filter(|at| !shown.contains(at));
                for at in shown.chain(renaming) {
                    let line = rows.lines[at];
                    let mut inner = ui.new_child(
                        egui::UiBuilder::new()
                            .id_salt(line.id())
                            .max_rect(rows.rect(at, area)),
                    );
                    self.line(
                        &mut inner, line, &frame, workspace, device, queue, filter, acts,
                    );
                }
                self.land_jump(ui, &rows, area);
                let below = area.top() + rows.height() + rows.spacing;
                self.empty_below(ui, area.with_min_y(below));
            });
        self.rows = rows;
    }

    /// The space below the last row. A click there clears the selection.
    fn empty_below(&mut self, ui: &mut egui::Ui, rest: egui::Rect) {
        if rest.height() <= 0.0 {
            return;
        }
        if ui
            .interact(rest, ui.id().with("tree_empty"), egui::Sense::click())
            .clicked()
        {
            self.selection.clear();
        }
    }

    fn twist(&mut self, branch: Branch) {
        if !self.open.remove(&branch) {
            self.open.insert(branch);
        }
    }

    /// Right-clicking an unselected row selects only it; right-clicking a selected row
    /// keeps the selection, so the menu acts on all of it.
    fn aim(&mut self, item: Item) {
        if !self.selection.holds(item) {
            self.select(item);
        }
    }

    /// A jump opens the branches it needs, since its purpose is to reach a slot inside a
    /// closed one.
    fn follow_jump(&mut self, device: &Device) {
        let Some((class, at)) = self.jump else {
            return;
        };
        self.open.insert(Branch::Class(class.to_raw()));
        self.open.insert(bank_branch(class, at.user_bank()));
        // ⚠️ A jump to a slot no scan has reached would hold the branch open as long as
        // the instrument stays attached, because no line is laid out to land it.
        if device.state.slot(class, at).is_none() {
            self.jump = None;
        }
    }

    /// Select the slot a jump was for and scroll to it, once its line is laid out.
    fn land_jump(&mut self, ui: &egui::Ui, rows: &Rows, area: egui::Rect) {
        let Some((class, at)) = self.jump else {
            return;
        };
        let item = Item::Slot { class, at };
        let Some(line) = rows.position(item) else {
            return;
        };
        self.jump = None;
        self.selection.only(item);
        ui.scroll_to_rect(rows.rect(line, area), Some(egui::Align::Center));
    }

    /// F2 renames the row selected alone, wherever in the tree it is.
    fn rename_on_f2(&mut self, ui: &egui::Ui, rows: &Rows, workspace: &Workspace, device: &Device) {
        if self.rename.is_some() || !ui.input(|i| i.key_pressed(egui::Key::F2)) {
            return;
        }
        let Some(item) = self.selection.sole() else {
            return;
        };
        if rows.position(item).is_none() {
            return;
        }
        let name = match item {
            Item::Local(id) => workspace.get(id).map(|entity| entity.name.clone()),
            // ⚠️ A partition this app cannot name is only listed.
            Item::Slot { class, at } if !read_only(class) && class.edits_in_place() => device
                .state
                .slot(class, at)
                .flatten()
                .map(|info| info.name.trim().to_string()),
            Item::Slot { .. } | Item::Folder(_) | Item::Tag(_) => None,
        };
        if let Some(name) = name {
            self.start_rename(item, &name);
        }
    }

    fn frame(&self, workspace: &Workspace, device: &Device, queue: &Queue) -> Frame {
        let library = self.sections.places && self.open.contains(&Branch::Computer);
        let naming = Naming {
            kept: match library {
                true => Kept::of(workspace),
                false => Kept::default(),
            },
            instrument: device.state.product().and_then(Family::from_product),
        };
        // The slots open as views, which the list on this computer does not show.
        let viewed = match device.state.connected() && self.open.contains(&Branch::Instrument) {
            true => workspace
                .entities()
                .iter()
                .filter(|entity| !entity.kept)
                .filter_map(|entity| entity.origin.slot())
                .collect(),
            false => Vec::new(),
        };
        let counts = match self.sections.places {
            true => [
                queue.len(),
                crate::queue::changed(workspace, &device.state, queue).len(),
            ],
            false => [0, 0],
        };
        Frame {
            naming,
            viewed,
            counts,
        }
    }

    /// Every line but those under This computer: the lines above them, and the lines
    /// below.
    fn lines_around(
        &self,
        workspace: &Workspace,
        device: &Device,
        filter: &Filter,
        frame: &Frame,
    ) -> [Vec<Line>; 2] {
        let mut above = vec![Line::Header(Section::Places)];
        let mut below = Vec::new();
        if self.sections.places {
            above.push(Line::Computer);
            if self.open.contains(&Branch::Computer) {
                let opening = self.folders.place.as_ref().is_some_and(|at| at.opening);
                let empty = workspace.listed().next().is_none()
                    && self.folders.all().is_empty()
                    && self.strangers_shown(None) == 0
                    && self.folders.unwalked.is_empty();
                let said = match (opening, empty) {
                    (true, true) => Some("Opening the library…"),
                    (false, true) => Some("Drop Nord files here, or use Open…"),
                    (_, false) => None,
                };
                below.extend(said.map(|said| Line::Note {
                    under: Branch::Computer,
                    said,
                    depth: 1,
                }));
            }
            if device.state.connected() {
                self.instrument_lines(device, &mut below);
            }
            for (state, count) in [State::Waiting, State::Differs]
                .into_iter()
                .zip(frame.counts)
            {
                // A row whose count drops to zero stays while its filter is on, so there
                // is always a row to click to turn the filter off.
                if count > 0 || filter.on(Narrow::State(state)) {
                    below.push(Line::State(state));
                }
            }
        }
        below.push(Line::End(Section::Places));
        let kinds = kinds_present(workspace, &device.state);
        if worth_choosing(&kinds) {
            below.push(Line::Header(Section::Kinds));
            if self.sections.kinds {
                below.extend(kinds.into_iter().map(Line::Kind));
            }
            below.push(Line::End(Section::Kinds));
        }
        below.push(Line::Header(Section::Tags));
        if self.sections.tags {
            below.extend(self.tag_ids().into_iter().map(Line::Tag));
            below.push(Line::NewTag);
        }
        below.push(Line::End(Section::Tags));
        [above, below]
    }

    /// The lines under This computer: what is in the root, every open folder down to
    /// what is in it, then the index rows whose file is gone.
    fn library_lines(&self, workspace: &Workspace) -> Vec<Line> {
        let beside = Beside::of(&self.folders);
        let mut lines = Vec::new();
        self.folder_lines(None, &LibPath::root(), 0, &beside, workspace, &mut lines);
        lines.extend(self.folders.lost().iter().map(|lost| Line::Lost(lost.id)));
        lines
    }

    /// What is directly in `folder` at `dir`, or in the root for `None`: its folders,
    /// each with what is in it when it is open, then its assets, then the files drawbar
    /// does not hold, then a line if the folder was not listed whole.
    fn folder_lines(
        &self,
        folder: Option<u64>,
        dir: &LibPath,
        depth: usize,
        beside: &Beside,
        workspace: &Workspace,
        lines: &mut Vec<Line>,
    ) {
        for inner in beside.children(folder) {
            self.folder_line(inner, depth + 1, beside, workspace, lines);
        }
        lines.extend(
            self.folders
                .members(folder, workspace)
                .iter()
                .map(|entity| Line::Local {
                    id: entity.id,
                    folder,
                    depth: depth + 1,
                }),
        );
        lines.extend(
            beside
                .strangers(dir)
                .iter()
                .map(|&(unread, index)| Line::Stranger {
                    unread,
                    index,
                    depth: depth + 1,
                }),
        );
        if self.folders.unwalked.contains(dir) {
            lines.push(Line::Note {
                under: folder.map_or(Branch::Computer, Branch::Folder),
                said: "not all listed",
                depth: depth + 1,
            });
        }
    }

    fn folder_line(
        &self,
        folder: &Folder,
        depth: usize,
        beside: &Beside,
        workspace: &Workspace,
        lines: &mut Vec<Line>,
    ) {
        let id = folder.id;
        lines.push(Line::Folder { id, depth });
        // A folder being renamed shows its contents, since they are what the name
        // describes.
        let renaming = self
            .rename
            .as_ref()
            .is_some_and(|r| r.what == Item::Folder(id));
        if !renaming && !self.open.contains(&Branch::Folder(id)) {
            return;
        }
        let empty = self.folders.members(Some(id), workspace).is_empty()
            && beside.children(Some(id)).is_empty()
            && beside.strangers(&folder.path).is_empty();
        if empty && !self.folders.unwalked.contains(&folder.path) {
            lines.push(Line::Note {
                under: Branch::Folder(id),
                said: "empty; drag sounds here",
                depth: depth + 1,
            });
        }
        self.folder_lines(Some(id), &folder.path, depth, beside, workspace, lines);
    }

    /// The attached instrument, and under it, where open, its classes, their banks and
    /// their slots.
    fn instrument_lines(&self, device: &Device, lines: &mut Vec<Line>) {
        if device.state.product().is_none() {
            return;
        }
        lines.push(Line::Instrument);
        if !self.open.contains(&Branch::Instrument) {
            return;
        }
        for class in device.state.classes() {
            lines.push(Line::Class(class));
            if !self.open.contains(&Branch::Class(class.to_raw())) {
                continue;
            }
            let banks = device.state.banks_of(class);
            if banks.is_empty() {
                lines.push(Line::Note {
                    under: Branch::Class(class.to_raw()),
                    said: "nothing read yet",
                    depth: 2,
                });
            }
            // The live buffer and the settings have a single bank, drawn without a bank
            // row.
            let cut = banks.len() > 1;
            let depth = match cut {
                true => 3,
                false => 2,
            };
            for bank in banks {
                if cut {
                    lines.push(Line::Bank(class, bank));
                    if !self.open.contains(&bank_branch(class, u64::from(bank))) {
                        continue;
                    }
                }
                lines.extend(
                    device
                        .state
                        .slots_of(class, bank)
                        .map(|(at, _)| Line::Slot {
                            class,
                            bank,
                            at,
                            depth,
                        }),
                );
            }
        }
    }

    /// Draw one line into `ui`, which is the line's rect.
    #[allow(clippy::too_many_arguments)]
    fn line(
        &mut self,
        ui: &mut egui::Ui,
        line: Line,
        frame: &Frame,
        workspace: &Workspace,
        device: &Device,
        queue: &Queue,
        filter: &Filter,
        acts: &mut Vec<Act>,
    ) {
        match line {
            Line::Header(section) => {
                let count = match section {
                    Section::Tags => Some(self.tags.all().len().to_string()),
                    Section::Places | Section::Kinds => None,
                };
                panel_header(
                    ui,
                    section.title(),
                    count.as_deref(),
                    self.sections.open(section),
                );
            }
            Line::End(_) => {}
            Line::Computer => self.computer_row(ui, workspace, device, filter, acts),
            Line::Folder { id, depth } => self.folder_row(ui, id, depth, workspace, device, acts),
            Line::Local { id, folder, depth } => {
                if let Some(entity) = workspace.get(id) {
                    let naming = &frame.naming;
                    self.local_row(
                        ui, entity, folder, depth, workspace, device, queue, naming, acts,
                    );
                }
            }
            Line::Stranger {
                unread,
                index,
                depth,
            } => self.stranger_row(ui, unread, index, depth),
            Line::Lost(id) => self.lost_row(ui, id, acts),
            Line::Note { said, depth, .. } => nothing(ui, depth, said),
            Line::Instrument => self.instrument_row(ui, workspace, device, queue, filter, acts),
            Line::Class(class) => self.class_row(ui, device, class, workspace, acts),
            Line::Bank(class, bank) => self.bank_row(ui, device, class, bank),
            Line::Slot {
                class,
                bank,
                at,
                depth,
            } => {
                let viewed = &frame.viewed;
                self.slot_row(
                    ui, device, class, bank, at, depth, viewed, workspace, queue, acts,
                );
            }
            Line::State(state) => {
                let count = match state {
                    State::Waiting => frame.counts[0],
                    State::Differs => frame.counts[1],
                };
                state_row(ui, state, count, filter, acts);
            }
            Line::Kind(kind) => self.kind_row(ui, kind, workspace, device, filter, acts),
            Line::Tag(id) => self.tag_row(ui, id, workspace, device, filter, acts),
            Line::NewTag => new_tag_row(ui, acts),
        }
    }

    fn computer_row(
        &mut self,
        ui: &mut egui::Ui,
        workspace: &Workspace,
        device: &Device,
        filter: &Filter,
        acts: &mut Vec<Act>,
    ) {
        let here = Narrow::Place(Place::Computer);
        let place = self.folders.place.clone().unwrap_or_default();
        let drawn = row(
            ui,
            filter.on(here),
            &Cells {
                indent: indent(0, true),
                open: Some(self.open.contains(&Branch::Computer)),
                glyph: Some(Glyph::LibraryBig),
                name: place.name.as_deref().unwrap_or("This computer"),
                whole: place.name.is_some(),
                count: Some(self.folders.count(None, workspace).to_string()),
                ..Cells::default()
            },
        );
        // The branch's head row takes a drop, so there is always a target that no drag
        // can have started from.
        self.drop_zone(ui, &drawn.response, Onto::Computer, acts);
        let triangle = on_triangle(&drawn);
        let response = match (&place.note, place.label.is_empty()) {
            (_, true) => drawn.response,
            (None, false) => drawn.response.on_hover_text(&place.label),
            (Some(note), false) => drawn
                .response
                .on_hover_text(format!("{}\n{note}", place.label)),
        };
        if response.clicked() {
            match triangle {
                true => self.twist(Branch::Computer),
                false => narrow(acts, here),
            }
        }
        let listed = Browser::standing_for(workspace, |_| true);
        response.context_menu(|ui| {
            self.set_menu(ui, &listed, workspace, device, acts, |browser, ui, acts| {
                offer(ui, "Open…", None, Act::OpenFiles, acts);
                menu::button(ui, "New", |ui| new_menu(ui, acts));
                let all = browser.folders.all_files;
                if marked(ui, SHOW_ALL_FILES, all) {
                    browser.folders.all_files = !all;
                }
                let picking = crate::libraries::picking();
                if offers_libraries(&browser.folders, picking) {
                    ui.separator();
                    library_items(ui, &browser.folders, picking, acts);
                }
            });
        });
    }

    /// A file drawbar does not hold: one it did not read, or one it does not open.
    fn stranger_row(&self, ui: &mut egui::Ui, unread: bool, index: usize, depth: usize) {
        let found = match unread {
            true => self
                .folders
                .unread
                .get(index)
                .map(|(path, why)| (path, Some(why.as_str()))),
            false => self.folders.others.get(index).map(|path| (path, None)),
        };
        let Some((path, why)) = found else {
            return;
        };
        let drawn = row(
            ui,
            false,
            &Cells {
                indent: indent(depth, false),
                glyph: Some(match why {
                    Some(_) => Glyph::CircleAlert,
                    None => Glyph::CircleDashed,
                }),
                name: path.leaf(),
                note: why.map(|_| "not read"),
                faint: true,
                child: true,
                whole: true,
                ..Cells::default()
            },
        );
        drawn.response.on_hover_text(match why {
            Some(why) => format!("drawbar did not read this file: {why}."),
            None => "drawbar does not open this kind of file.".to_string(),
        });
    }

    /// The assets directly in `folder`, or in the root for `None`, as rows.
    fn members_of(&self, folder: Option<u64>, workspace: &Workspace) -> Vec<Item> {
        self.folders
            .members(folder, workspace)
            .iter()
            .map(|entity| Item::Local(entity.id))
            .collect()
    }

    fn folder_row(
        &mut self,
        ui: &mut egui::Ui,
        id: u64,
        depth: usize,
        workspace: &Workspace,
        device: &Device,
        acts: &mut Vec<Act>,
    ) {
        let item = Item::Folder(id);
        let Some(name) = self.folders.name_of(id).map(str::to_string) else {
            return;
        };
        if self.rename.as_ref().is_some_and(|r| r.what == item) {
            if let Some(name) = self.rename_row(ui, indent(depth, true), &name) {
                acts.push(Act::RenameFolder { id, name });
            }
            return;
        }
        let drawn = row(
            ui,
            self.selection.holds(item),
            &Cells {
                indent: indent(depth, true),
                open: Some(self.open.contains(&Branch::Folder(id))),
                glyph: Some(Glyph::Folder),
                name: &name,
                count: Some(self.folders.count(Some(id), workspace).to_string()),
                child: true,
                ..Cells::default()
            },
        );
        self.drop_zone(ui, &drawn.response, Onto::Group(id), acts);
        if drawn.response.clicked() {
            match on_triangle(&drawn) {
                true => self.twist(Branch::Folder(id)),
                false => {
                    let parent = self
                        .folders
                        .path_of(id)
                        .and_then(|path| self.folders.id_of(&path.parent()));
                    let list: Vec<Item> = self
                        .folders
                        .children(parent)
                        .into_iter()
                        .map(Item::Folder)
                        .collect();
                    self.clicked(ui, Click { item, list: &list });
                }
            }
        }
        drawn.response.context_menu(|ui| {
            self.aim(item);
            let inside = self.members_of(Some(id), workspace);
            self.set_menu(ui, &inside, workspace, device, acts, |browser, ui, acts| {
                offer(ui, "New folder", None, Act::NewFolderIn(id), acts);
                if ui.button("Rename").clicked() {
                    browser.start_rename(item, &name);
                    ui.close();
                }
                offer(
                    ui,
                    "Remove folder",
                    Some("what is in it moves up a level; nothing is deleted"),
                    Act::RemoveFolder(id),
                    acts,
                );
            });
        });
    }

    /// How many lines [`Beside::strangers`] holds for a folder's files.
    fn strangers_shown(&self, folder: Option<u64>) -> usize {
        let Some(dir) = self.folders.dir(folder) else {
            return 0;
        };
        let unread = self.folders.unread.iter().map(|(path, _)| path);
        let others = self
            .folders
            .others
            .iter()
            .filter(|_| self.folders.all_files);
        unread
            .chain(others)
            .filter(|path| path.parent() == dir)
            .count()
    }

    /// An index row whose file is gone, for an asset this app does not hold. It keeps
    /// something the file alone would not give back, until the user lets it go.
    fn lost_row(&mut self, ui: &mut egui::Ui, id: u64, acts: &mut Vec<Act>) {
        let Some(lost) = self.folders.lost().iter().find(|lost| lost.id == id) else {
            return;
        };
        let name = match &lost.row.path {
            Some(path) => path.to_string(),
            None => lost.row.name.clone(),
        };
        let drawn = row(
            ui,
            false,
            &Cells {
                indent: indent(1, false),
                glyph: Some(Glyph::CircleAlert),
                name: &name,
                note: Some("missing"),
                faint: true,
                tags: self.tags.worn(id).len(),
                child: true,
                ..Cells::default()
            },
        );
        drawn
            .response
            .on_hover_text(
                "Its file is gone from the library folder. Its tags and the slot it came \
                 from are kept until you delete it.",
            )
            .context_menu(|ui| {
                offer(ui, "Delete", None, Act::Forget(id), acts);
            });
    }

    #[allow(clippy::too_many_arguments)]
    fn local_row(
        &mut self,
        ui: &mut egui::Ui,
        entity: &LocalEntity,
        folder: Option<u64>,
        depth: usize,
        workspace: &Workspace,
        device: &Device,
        queue: &Queue,
        naming: &Naming,
        acts: &mut Vec<Act>,
    ) {
        let item = Item::Local(entity.id);
        let kind = Kind::of(entity);
        let selected = self.selection.holds(item);

        // While a name is being typed, the row senses nothing: a drag sense over the
        // field would take the clicks that place the cursor.
        if self.rename.as_ref().is_some_and(|r| r.what == item) {
            if let Some(name) = self.rename_row(ui, indent(depth, false), &entity.name) {
                acts.push(Act::RenameLocal {
                    id: entity.id,
                    name,
                });
            }
            return;
        }

        let owed = queue.entry(entity.id).map(destination);
        let wears = self.tags.worn(entity.id).len();
        let word =
            crate::strings::kind_word(kind, qualifier(entity, &naming.kept, naming.instrument));
        let trouble = match (
            self.folders.missing.contains(&entity.id),
            self.folders.duplicates.contains(&entity.id),
        ) {
            (true, _) => Some("missing"),
            (false, true) => Some("same name as another"),
            (false, false) => entity.verify.note(),
        };
        let drawn = row(
            ui,
            selected,
            &Cells {
                indent: indent(depth, false),
                glyph: Some(kind.glyph()),
                name: &entity.name,
                note: trouble.or(owed.as_deref()).or(Some(word.as_str())),
                unsaved: entity.is_unsaved(),
                dot: mark(entity, &device.state, queue, ui.visuals()),
                tags: wears,
                child: true,
                ..Cells::default()
            },
        );
        // ⚠️ The row already shows the full name on hover when it truncates it. A hover
        // here would show it twice.
        let response = drawn.response;

        if response.dragged() {
            if let Some(head) = self.held(item, workspace, &device.state) {
                let carried = self.carrying(head, &entity.name, workspace, &device.state);
                egui::DragAndDrop::set_payload(ui.ctx(), carried);
            }
        }
        // A drop onto a row lands where the row is drawn. It is taken here so the
        // branch's zone does not act on it again.
        self.drop_zone(ui, &response, onto_list(folder), acts);

        if response.double_clicked() {
            acts.push(Act::Open(item));
        } else if response.clicked() {
            let list = self.members_of(folder, workspace);
            self.clicked(ui, Click { item, list: &list });
        }

        response.context_menu(|ui| self.menu(ui, item, workspace, device, queue, acts));
    }

    /// The menu of a row that stands for a set of assets: the bulk actions on the set,
    /// then the row's own items.
    ///
    /// One builder for the folder, tag, kind, place and class rows. They differ in which
    /// assets they stand for and in their own items. Otherwise they offer the same
    /// actions, disabled for the same reasons and labeled the same way.
    fn set_menu(
        &mut self,
        ui: &mut egui::Ui,
        members: &[Item],
        workspace: &Workspace,
        device: &Device,
        acts: &mut Vec<Act>,
        own: impl FnOnce(&mut Browser, &mut egui::Ui, &mut Vec<Act>),
    ) {
        for action in [Bulk::Queue, Bulk::Export] {
            self.bulk_item(ui, action, members, workspace, &device.state, acts);
        }
        ui.separator();
        own(self, ui, acts);
    }

    /// The assets on this computer a row stands for, in the order the list holds them.
    fn standing_for(workspace: &Workspace, keep: impl Fn(&LocalEntity) -> bool) -> Vec<Item> {
        workspace
            .listed()
            .filter(|entity| keep(entity))
            .map(|entity| Item::Local(entity.id))
            .collect()
    }

    /// The menu a row offers, in the tree or in the library table.
    ///
    /// A row inside a checked set of several offers what the Selection card offers,
    /// because the menu acts on the whole set.
    ///
    /// Folders and tags appear only in the tree, so the rows that draw them build their
    /// menus.
    pub fn menu(
        &mut self,
        ui: &mut egui::Ui,
        item: Item,
        workspace: &Workspace,
        device: &Device,
        queue: &Queue,
        acts: &mut Vec<Act>,
    ) {
        self.aim(item);
        let checked: Vec<Item> = self.selection.items().collect();
        if checked.len() > 1 && self.selection.holds(item) {
            for action in Bulk::ALL {
                self.bulk_item(ui, action, &checked, workspace, &device.state, acts);
            }
            return;
        }
        match item {
            Item::Local(id) => self.local_menu(ui, id, workspace, device, acts),
            Item::Slot { class, at } => self.slot_menu(ui, class, at, device, queue, acts),
            Item::Folder(_) | Item::Tag(_) => {}
        }
    }

    fn local_menu(
        &mut self,
        ui: &mut egui::Ui,
        id: u64,
        workspace: &Workspace,
        device: &Device,
        acts: &mut Vec<Act>,
    ) {
        let Some(entity) = workspace.get(id) else {
            return;
        };
        let item = Item::Local(id);
        let picked = self.selection.locals();
        offer(ui, "Open", None, Act::Open(item), acts);
        self.bulk_item(ui, Bulk::Queue, &[item], workspace, &device.state, acts);
        offer(ui, "Export…", None, Act::Export(id), acts);
        if ui.button("Rename").clicked() {
            self.start_rename(item, &entity.name);
            acts.push(Act::Reveal(item));
            ui.close();
        }
        offer(ui, "Duplicate", None, Act::DuplicateLocal(id), acts);
        self.filing_menu(ui, id, self.folders.holding(entity), acts);
        menu::button(ui, "Tag", |ui| self.tag_items(ui, &picked, acts));
        ui.separator();
        if ui.button("Delete…").clicked() {
            self.ask_delete(id, &entity.name);
            ui.close();
        }
    }

    /// The folders an asset can be moved into, for those who prefer a menu to dragging.
    fn filing_menu(&self, ui: &mut egui::Ui, id: u64, filed: Option<u64>, acts: &mut Vec<Act>) {
        if self.folders.all().is_empty() {
            return;
        }
        menu::button(ui, "Move to folder", |ui| {
            for folder in self.folders.all() {
                if marked(ui, folder.path.as_str(), filed == Some(folder.id)) {
                    acts.push(Act::File {
                        id,
                        folder: Some(folder.id),
                    });
                }
            }
            ui.separator();
            if ui
                .add_enabled(filed.is_some(), egui::Button::new("Out to the top level"))
                .clicked()
            {
                acts.push(Act::File { id, folder: None });
                ui.close();
            }
        });
    }

    /// Every tag, checked where it is on everything selected, then New tag.
    ///
    /// Only the items, so the row's menu and the Selection card can each give them their
    /// own label.
    pub fn tag_items(&self, ui: &mut egui::Ui, picked: &[u64], acts: &mut Vec<Act>) {
        for tag in self.tags.all() {
            let on = self.tags.on_all(picked, tag.id);
            if marked(ui, &tag.name, on) {
                let ids = picked.to_vec();
                acts.push(match on {
                    true => Act::Untag { ids, tag: tag.id },
                    false => Act::Tag { ids, tag: tag.id },
                });
            }
        }
        if !self.tags.all().is_empty() {
            ui.separator();
        }
        offer(ui, "New tag…", None, Act::NewTag(picked.to_vec()), acts);
    }

    fn instrument_row(
        &mut self,
        ui: &mut egui::Ui,
        workspace: &Workspace,
        device: &Device,
        queue: &Queue,
        filter: &Filter,
        acts: &mut Vec<Act>,
    ) {
        let Some(product) = device.state.product().map(str::to_string) else {
            return;
        };
        let held = occupancy(
            ObjectClass::Program,
            &device.state.inventory,
            device.state.allocation_unit(ObjectClass::Program),
        );
        let drawn = row(
            ui,
            filter.on(Narrow::Place(Place::Keyboard)),
            &Cells {
                indent: indent(0, true),
                open: Some(self.open.contains(&Branch::Instrument)),
                glyph: Some(Glyph::Keyboard),
                name: &product,
                dot: Some((crate::app::good(ui.visuals()), "attached")),
                count: held,
                ..Cells::default()
            },
        );
        let response = drawn
            .response
            .clone()
            .on_hover_text("attached; the count is its programs");
        if response.clicked() {
            match on_triangle(&drawn) {
                true => self.twist(Branch::Instrument),
                // ⚠️ Not [`narrow`]: the instrument's own tab is what this row opens, so
                // the narrowing it asks for is already in front.
                false => {
                    acts.push(Act::ShowTab(Spot::Keyboard));
                    acts.push(Act::Narrow(Narrow::Place(Place::Keyboard)));
                }
            }
        }
        let reading = device
            .state
            .classes()
            .into_iter()
            .filter_map(|class| device.state.scan.progress(class))
            .any(|progress| progress.running);
        // The label counts what the batch would write, which leaves out entries this
        // instrument has refused.
        let waiting = will_write(queue).count();
        // What this row stands for on this computer: every asset that stands for a slot.
        let off_it = Browser::standing_for(workspace, |entity| entity.spot().is_some());
        drawn.response.context_menu(|ui| {
            self.set_menu(ui, &off_it, workspace, device, acts, |_, ui, acts| {
                if ui
                    .add_enabled(!reading, egui::Button::new("Read everything again"))
                    .on_disabled_hover_text("already reading")
                    .clicked()
                {
                    acts.push(Act::Resync);
                    ui.close();
                }
                if waiting > 0 {
                    let label = format!("Send all ({waiting})");
                    offer(ui, &label, None, Act::AskSendAll, acts);
                }
                ui.separator();
                offer(ui, "Disconnect", None, Act::Disconnect, acts);
            });
        });
    }

    fn class_row(
        &mut self,
        ui: &mut egui::Ui,
        device: &Device,
        class: ObjectClass,
        workspace: &Workspace,
        acts: &mut Vec<Act>,
    ) {
        let open = self.open.contains(&Branch::Class(class.to_raw()));
        let progress = device.state.scan.progress(class);
        let count = match progress {
            Some(p) if p.running => Some(match p.total {
                Some(total) => format!("{} of {total}", p.done + 1),
                None => "reading…".to_string(),
            }),
            _ => occupancy(
                class,
                &device.state.inventory,
                device.state.allocation_unit(class),
            ),
        };
        let drawn = row(
            ui,
            false,
            &Cells {
                indent: indent(1, true),
                open: Some(open),
                glyph: Some(Kind::from_class(class).glyph()),
                name: device.state.folder_name(class),
                note: read_only(class).then_some("read only"),
                count,
                child: true,
                ..Cells::default()
            },
        );
        if drawn.response.clicked() {
            match on_triangle(&drawn) {
                true => self.twist(Branch::Class(class.to_raw())),
                false => acts.push(Act::ShowClass(class)),
            }
        }
        let focus = device.state.focused(class);
        let off_it = Browser::standing_for(workspace, |entity| {
            entity.spot().is_some_and(|(held, _)| held == class)
        });
        drawn.response.context_menu(|ui| {
            self.set_menu(ui, &off_it, workspace, device, acts, |browser, ui, acts| {
                offer(
                    ui,
                    "Read this folder again",
                    Some("Read everything reads the whole instrument; this reads one folder"),
                    Act::ReadAgain(class),
                    acts,
                );
                if let Some(at) = focus {
                    if ui
                        .button("Go to loaded")
                        .on_hover_text(format!("the panel is on {}", shown(at)))
                        .clicked()
                    {
                        browser.jump = Some((class, at));
                        ui.close();
                    }
                }
            });
        });
    }

    /// One bank, as a branch over its slots.
    ///
    /// ⚠️ A bank is a container only in the browser. The instrument has no folders inside
    /// a class, only a bank and slot number per location, but four hundred rows in one
    /// run cannot be navigated, so the list is split by bank. A bank the device named
    /// shows its name; for pianos, those names are the panel's categories.
    fn bank_row(&mut self, ui: &mut egui::Ui, device: &Device, class: ObjectClass, bank: u32) {
        let Some(slots) = device.state.bank(class, bank) else {
            return;
        };
        let count = slots.len();
        let held = slots.iter().filter(|slot| slot.is_some()).count();
        let name = device
            .state
            .bank_name(class, bank)
            .filter(|name| worth_captioning(bank, name))
            .map(|name| format!("{bank} · {name}"))
            .unwrap_or_else(|| format!("Bank {bank}"));
        let drawn = row(
            ui,
            false,
            &Cells {
                indent: indent(2, true),
                open: Some(self.open.contains(&bank_branch(class, u64::from(bank)))),
                glyph: Some(Glyph::Folder),
                name: &name,
                count: Some(format!("{held}/{count}")),
                child: true,
                ..Cells::default()
            },
        );
        if drawn.response.clicked() {
            self.twist(bank_branch(class, u64::from(bank)));
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn slot_row(
        &mut self,
        ui: &mut egui::Ui,
        device: &Device,
        class: ObjectClass,
        bank: u32,
        at: Location,
        depth: usize,
        viewed: &[(ObjectClass, Location)],
        workspace: &Workspace,
        queue: &Queue,
        acts: &mut Vec<Act>,
    ) {
        let held = device
            .state
            .slot(class, at)
            .flatten()
            .map(|info| info.name.trim().to_string());
        let item = Item::Slot { class, at };
        let selected = self.selection.holds(item);

        // While a name is being typed, the row senses nothing: a drag sense over the
        // field would take the clicks that place the cursor.
        if self.rename.as_ref().is_some_and(|r| r.what == item) {
            let was = held.clone().unwrap_or_default();
            if let Some(name) = self.rename_row(ui, indent(depth, false), &was) {
                acts.push(Act::RenameSlot { class, at, name });
            }
            return;
        }

        let loaded = device.state.focused(class) == Some(at);
        // A slot open as a view says so here, because the tab strip cannot: a tab shows
        // the document's name, which for a view is the slot's name.
        let viewing = viewed.contains(&(class, at));
        let drawn = row(
            ui,
            selected,
            &Cells {
                indent: indent(depth, false),
                glyph: Some(Kind::from_class(class).glyph()),
                at: Some(shown(at)),
                name: held.as_deref().unwrap_or("empty"),
                note: viewing.then_some("open"),
                faint: held.is_none(),
                loaded,
                child: true,
                ..Cells::default()
            },
        );
        let mut response = drawn.response;
        if loaded {
            response = response.on_hover_text("loaded on the instrument's panel");
        }
        if viewing {
            response = response
                .on_hover_text("open in a tab as a view of this slot — it is not on this computer");
        }

        // ⚠️ A partition this app cannot name is only listed.
        let fetchable = !read_only(class);

        if let Some(name) = &held {
            if response.dragged() {
                if let Some(head) = self.held(item, workspace, &device.state) {
                    let carried = self.carrying(head, name, workspace, &device.state);
                    egui::DragAndDrop::set_payload(ui.ctx(), carried);
                }
            }
        }
        self.drop_zone(ui, &response, Onto::Slot { class, at }, acts);

        if response.double_clicked() {
            if held.is_some() && fetchable {
                acts.push(Act::Open(item));
            }
        } else if response.clicked() {
            let list: Vec<Item> = device
                .state
                .slots_of(class, bank)
                .map(|(at, _)| Item::Slot { class, at })
                .collect();
            self.clicked(ui, Click { item, list: &list });
        }

        if held.is_none() {
            return;
        }
        response.context_menu(|ui| self.menu(ui, item, workspace, device, queue, acts));
    }

    /// The menu of a slot. An empty slot has no menu.
    fn slot_menu(
        &mut self,
        ui: &mut egui::Ui,
        class: ObjectClass,
        at: Location,
        device: &Device,
        queue: &Queue,
        acts: &mut Vec<Act>,
    ) {
        let Some(name) = device
            .state
            .slot(class, at)
            .flatten()
            .map(|info| info.name.trim().to_string())
        else {
            return;
        };
        // ⚠️ A partition this app cannot name is only listed.
        if read_only(class) {
            ui.label(egui::RichText::new("drawbar does not know what this folder holds.").weak());
            return;
        }
        let item = Item::Slot { class, at };
        let free = spare_slot(&device.state, class, queue);
        offer(
            ui,
            "Open",
            Some("a view of this slot, without adding it to the list on this computer"),
            Act::Open(item),
            acts,
        );
        offer(
            ui,
            "Copy to this computer",
            None,
            Act::Copy { class, at },
            acts,
        );
        offer(
            ui,
            LOAD_ON_INSTRUMENT,
            None,
            Act::LoadOnInstrument { class, at },
            acts,
        );
        ui.separator();
        let edits = class.edits_in_place();
        let fixed = "the instrument keeps the name a sound was sent with, and copies only \
                     what it is sent";
        if ui
            .add_enabled(edits, egui::Button::new("Rename"))
            .on_disabled_hover_text(fixed)
            .clicked()
        {
            self.start_rename(item, &name);
            acts.push(Act::Reveal(item));
            ui.close();
        }
        let duplicate = ui
            .add_enabled(edits && free.is_some(), egui::Button::new("Duplicate"))
            .on_disabled_hover_text(match edits {
                true => "every slot read so far is taken or already queued",
                false => fixed,
            });
        if duplicate.clicked() {
            if let Some(to) = free {
                acts.push(Act::DuplicateSlot {
                    class,
                    from: at,
                    to,
                });
            }
            ui.close();
        }
        ui.separator();
        if ui.button("Delete…").clicked() {
            self.ask = Some(Ask::new(
                format!("Delete “{name}” from {}?", place(class, at)),
                Some("It is removed from the instrument. There is no undo.".into()),
                Verb::Delete,
                vec![Act::DeleteSlot { class, at }],
            ));
            ui.close();
        }
    }

    fn kind_row(
        &mut self,
        ui: &mut egui::Ui,
        kind: Kind,
        workspace: &Workspace,
        device: &Device,
        filter: &Filter,
        acts: &mut Vec<Act>,
    ) {
        let asked = Narrow::Kind(kind);
        let drawn = row(
            ui,
            filter.on(asked),
            &Cells {
                indent: indent(0, false),
                glyph: Some(kind.glyph()),
                name: kind.plural(),
                ..Cells::default()
            },
        );
        if drawn.response.clicked() {
            narrow(acts, asked);
        }
        let of_it = Browser::standing_for(workspace, |entity| Kind::of(entity) == kind);
        drawn.response.context_menu(|ui| {
            self.set_menu(ui, &of_it, workspace, device, acts, |_, _, _| {});
        });
    }

    fn tag_row(
        &mut self,
        ui: &mut egui::Ui,
        id: u64,
        workspace: &Workspace,
        device: &Device,
        filter: &Filter,
        acts: &mut Vec<Act>,
    ) {
        let item = Item::Tag(id);
        let Some(name) = self.tags.name_of(id).map(str::to_string) else {
            return;
        };
        if self.rename.as_ref().is_some_and(|r| r.what == item) {
            if let Some(name) = self.rename_row(ui, indent(0, false), &name) {
                acts.push(Act::RenameTag { id, name });
            }
            return;
        }
        let asked = Narrow::Tag(id);
        let drawn = row(
            ui,
            filter.on(asked),
            &Cells {
                indent: indent(0, false),
                glyph: Some(Glyph::Tag),
                name: &name,
                count: Some(self.tags.count(id).to_string()),
                ..Cells::default()
            },
        );
        if drawn.response.clicked() {
            narrow(acts, asked);
        }
        let wearing =
            Browser::standing_for(workspace, |entity| self.tags.worn(entity.id).contains(&id));
        drawn.response.context_menu(|ui| {
            self.aim(item);
            self.set_menu(
                ui,
                &wearing,
                workspace,
                device,
                acts,
                |browser, ui, acts| {
                    if ui.button("Rename").clicked() {
                        browser.start_rename(item, &name);
                        ui.close();
                    }
                    offer(
                        ui,
                        "Remove tag",
                        Some("removes the tag from everything; nothing is deleted"),
                        Act::RemoveTag(id),
                        acts,
                    );
                },
            );
        });
    }

    fn tag_ids(&self) -> Vec<u64> {
        self.tags.all().iter().map(|tag| tag.id).collect()
    }
}

/// A row that narrows the library to what is in one state.
fn state_row(ui: &mut egui::Ui, state: State, count: usize, filter: &Filter, acts: &mut Vec<Act>) {
    let (glyph, dot) = match state {
        State::Waiting => (Glyph::Upload, crate::app::warn(ui.visuals())),
        State::Differs => (Glyph::CircleAlert, crate::app::bad(ui.visuals())),
    };
    let asked = Narrow::State(state);
    let drawn = row(
        ui,
        filter.on(asked),
        &Cells {
            indent: indent(0, false),
            glyph: Some(glyph),
            name: state.title(),
            dot: Some((dot, state.sentence())),
            count: Some(count.to_string()),
            ..Cells::default()
        },
    );
    if drawn.response.on_hover_text(state.sentence()).clicked() {
        narrow(acts, asked);
    }
}

fn new_tag_row(ui: &mut egui::Ui, acts: &mut Vec<Act>) {
    let drawn = row(
        ui,
        false,
        &Cells {
            indent: indent(0, false),
            glyph: Some(Glyph::Plus),
            name: "New tag",
            faint: true,
            ..Cells::default()
        },
    );
    if drawn.response.clicked() {
        acts.push(Act::NewTag(Vec::new()));
    }
}

/// The status dot on a local row, with its color and hover text.
fn mark(
    entity: &LocalEntity,
    device: &DeviceState,
    queue: &Queue,
    visuals: &egui::Visuals,
) -> Option<(egui::Color32, &'static str)> {
    let mark = crate::library::keyboard_mark(entity, device, queue)?;
    Some((
        crate::library::mark_ink(mark, visuals),
        crate::library::mark_words(mark),
    ))
}

/// Where a queued asset is going, for the note that says so.
fn destination(held: &Queued) -> String {
    format!("→ {}", place(held.class, held.at))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::{self, context, words, Bench};
    use crate::workspace::Fresh;

    /// Open library folder… is offered where a folder can be picked, grayed out with the
    /// way to turn folder access on where Brave turns it off, and missing elsewhere.
    #[test]
    fn open_library_folder_is_grayed_out_where_brave_turns_folder_access_off() {
        let ctx = context();
        ctx.style_mut(|style| style.interaction.tooltip_delay = 0.0);
        let folders = Folders::default();
        let frame = |picking: Picking, events: Vec<egui::Event>, time: f64| {
            let mut acts = Vec::new();
            let input = egui::RawInput {
                time: Some(time),
                ..testing::screen(egui::vec2(800.0, 600.0), events)
            };
            let output = testing::run(&ctx, input, |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| {
                    library_items(ui, &folders, picking, &mut acts);
                });
            });
            (output, acts)
        };
        // Where the item was painted, and how its text was faded.
        let item = |output: &egui::FullOutput| {
            testing::shapes(output)
                .into_iter()
                .find_map(|(shape, _)| match shape {
                    egui::Shape::Text(text) if text.galley.text() == OPEN_LIBRARY => Some((
                        egui::Rect::from_min_size(text.pos, text.galley.size()).center(),
                        text.fallback_color.gamma_multiply(text.opacity_factor),
                    )),
                    _ => None,
                })
        };

        let (output, _) = frame(Picking::Absent, Vec::new(), 0.0);
        assert_eq!(item(&output), None);
        assert!(!offers_libraries(&folders, Picking::Absent));

        let (output, _) = frame(Picking::On, Vec::new(), 1.0);
        let (at, ink) = item(&output).expect("offered");
        let (_, acts) = frame(Picking::On, testing::click(at), 2.0);
        assert!(matches!(acts.as_slice(), [Act::PickLibrary]));

        assert!(offers_libraries(&folders, Picking::TurnedOff));
        let (_, acts) = frame(Picking::TurnedOff, testing::click(at), 3.0);
        assert!(acts.is_empty(), "a click does nothing");
        frame(Picking::TurnedOff, vec![egui::Event::PointerMoved(at)], 4.0);
        let (output, _) = frame(Picking::TurnedOff, Vec::new(), 5.0);
        let (_, faded) = item(&output).expect("offered, grayed out");
        assert_ne!(faded, ink, "grayed out");
        let said = words(&output);
        let hint = crate::libraries::TURNED_OFF;
        assert!(said.iter().any(|word| word == hint), "{said:?}");
    }

    /// ⚠️ A row shows the sound's name without its format tag. The name the workspace
    /// holds keeps the tag; only the drawn text drops it.
    #[test]
    fn a_row_paints_its_name_without_the_format_tag() {
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
        workspace.rename(id, "Africa Split.ne5p".into());

        let output = testing::run(&ctx, egui::RawInput::default(), |ctx| {
            egui::SidePanel::left("browser")
                .exact_width(crate::shell::BROWSER)
                .show(ctx, |ui| {
                    browser.ui(ui, &workspace, &device, &queue, &Filter::default());
                });
        });

        let said = words(&output);
        assert!(said.iter().any(|word| word == "Africa Split"), "{said:?}");
        assert!(!said.iter().any(|word| word.contains(".ne5p")), "{said:?}");
        assert_eq!(workspace.get(id).unwrap().name, "Africa Split.ne5p");
    }

    #[test]
    fn files_drawbar_does_not_open_show_only_while_all_files_are_shown() {
        let Bench {
            ctx,
            mut browser,
            workspace,
            device,
            queue,
            ..
        } = Bench::new();
        browser.folders.others = vec![LibPath::root().join("cover.jpg")];
        browser.folders.unread = vec![(LibPath::root().join("Huge.nsmp"), "too big".into())];
        let drawn = |browser: &mut Browser| {
            let output = testing::run(&ctx, egui::RawInput::default(), |ctx| {
                egui::SidePanel::left("browser")
                    .exact_width(crate::shell::BROWSER)
                    .show(ctx, |ui| {
                        browser.ui(ui, &workspace, &device, &queue, &Filter::default());
                    });
            });
            words(&output)
        };

        let said = drawn(&mut browser);
        assert!(said.iter().any(|word| word == "Huge.nsmp"), "{said:?}");
        assert!(!said.iter().any(|word| word == "cover.jpg"), "{said:?}");
        browser.folders.all_files = true;
        let said = drawn(&mut browser);
        assert!(said.iter().any(|word| word == "cover.jpg"), "{said:?}");
    }

    /// Drawing the tree asks every folder row for its count and its contents, and none
    /// of that is taken again until something moves.
    #[test]
    fn frames_of_the_tree_count_the_folders_once() {
        let Bench {
            ctx,
            mut browser,
            mut workspace,
            device,
            queue,
            mut log,
            ..
        } = Bench::new();
        let mut parent = LibPath::root();
        for _ in 0..3 {
            let id = browser.folders.make(&parent, &workspace);
            browser.open.insert(Branch::Folder(id));
            parent = browser.folders.path_of(id).unwrap().clone();
            let asset = workspace.create(Fresh::Program, &mut log).unwrap();
            workspace.place(asset, parent.join("Grand.ne5p"));
        }
        for _ in 0..3 {
            testing::run(&ctx, egui::RawInput::default(), |ctx| {
                egui::SidePanel::left("browser")
                    .exact_width(crate::shell::BROWSER)
                    .show(ctx, |ui| {
                        browser.ui(ui, &workspace, &device, &queue, &Filter::default());
                    });
            });
        }
        assert_eq!(browser.folders.censuses.get(), 1);
    }

    /// A file not read yet is a row under its own name that says it is being read, and
    /// drawing it asks for it.
    #[test]
    fn a_row_not_read_yet_says_so_and_is_asked_for() {
        let Bench {
            ctx,
            mut browser,
            mut workspace,
            device,
            queue,
            mut log,
            ..
        } = Bench::new();
        let saved = crate::workspace::Saved {
            id: 1,
            name: "Grand.ne5p".into(),
            path: Some(LibPath::root().join("Grand.ne5p")),
            origin: crate::workspace::Origin::Fresh,
            saved: Vec::new(),
            file: None,
            unread: Some(1),
            unsaved: None,
        };
        workspace.restore(vec![saved], None, &mut log);
        assert!(!workspace.wanted(1));
        let output = testing::run(&ctx, egui::RawInput::default(), |ctx| {
            egui::SidePanel::left("browser")
                .exact_width(crate::shell::BROWSER)
                .show(ctx, |ui| {
                    browser.ui(ui, &workspace, &device, &queue, &Filter::default());
                });
        });
        let said = words(&output);
        assert!(said.iter().any(|word| word == "Grand"), "{said:?}");
        assert!(said.iter().any(|word| word == "reading…"), "{said:?}");
        assert!(workspace.wanted(1));
    }

    /// `count` programs loose in the library, named `Sound 0000` on, in that order.
    fn sounds(bench: &mut Bench, count: usize) -> Vec<u64> {
        let bytes = Fresh::Program.bytes().unwrap();
        (0..count)
            .map(|i| {
                let name = format!("Sound {i:04}.ne5p");
                let origin = crate::workspace::Origin::Fresh;
                bench
                    .workspace
                    .ingest(name, origin, bytes.clone(), &mut bench.log)
            })
            .collect()
    }

    /// One frame of the browser beside an empty window, scrolled to `scroll` first where
    /// given, and what it asked for.
    fn frame(
        bench: &mut Bench,
        input: egui::RawInput,
        scroll: Option<f32>,
    ) -> (egui::FullOutput, Vec<Act>) {
        let ctx = bench.ctx.clone();
        let mut acts = Vec::new();
        let output = testing::run(&ctx, input, |ctx| {
            egui::SidePanel::left("browser")
                .exact_width(crate::shell::BROWSER)
                .show(ctx, |ui| {
                    if let Some(y) = scroll {
                        let area = ui.make_persistent_id(egui::Id::new("browser_tree"));
                        let mut state =
                            egui::scroll_area::State::load(ctx, area).unwrap_or_default();
                        state.offset.y = y;
                        state.store(ctx, area);
                    }
                    let Bench {
                        browser,
                        workspace,
                        device,
                        queue,
                        ..
                    } = bench;
                    acts = browser.ui(ui, workspace, device, queue, &Filter::default());
                });
        });
        (output, acts)
    }

    const SCREEN: egui::Vec2 = egui::vec2(800.0, 600.0);

    /// Input on the screen the tests draw the browser beside.
    fn on_screen(events: Vec<egui::Event>) -> egui::RawInput {
        testing::screen(SCREEN, events)
    }

    /// Where `name` was painted, if it was.
    fn painted_at(output: &egui::FullOutput, name: &str) -> Option<egui::Rect> {
        testing::painted(output)
            .into_iter()
            .find(|word| word.text == name)
            .map(|word| word.rect)
    }

    #[test]
    fn a_tree_of_thousands_of_rows_paints_only_the_rows_in_view() {
        let mut bench = Bench::new();
        sounds(&mut bench, 2000);
        let (output, _) = frame(&mut bench, on_screen(Vec::new()), None);
        let painted: Vec<String> = testing::words(&output)
            .into_iter()
            .filter(|word| word.starts_with("Sound "))
            .collect();
        let room = (SCREEN.y / super::super::row::CHILD) as usize;
        assert!(painted.contains(&"Sound 0000".to_string()), "{painted:?}");
        assert!(
            painted.len() <= room,
            "{} rows painted on a screen with room for {room}",
            painted.len()
        );

        frame(&mut bench, on_screen(Vec::new()), Some(1.0e7));
        let (output, _) = frame(&mut bench, on_screen(Vec::new()), None);
        let last = painted_at(&output, "Sound 1999").expect("the last row, scrolled to");
        assert!(last.bottom() <= SCREEN.y, "{last:?} is below the screen");
        assert!(painted_at(&output, "Sound 0000").is_none(), "the first row");
    }

    /// The rows in view are asked for, and a row out of view is not.
    #[test]
    fn only_the_rows_in_view_are_asked_for() {
        let mut bench = Bench::new();
        let saved = (1..=500)
            .map(|id| crate::workspace::Saved {
                id,
                name: format!("Sound {id:04}.ne5p"),
                path: Some(LibPath::root().join(&format!("Sound {id:04}.ne5p"))),
                origin: crate::workspace::Origin::Fresh,
                saved: Vec::new(),
                file: None,
                unread: Some(1),
                unsaved: None,
            })
            .collect();
        bench.workspace.restore(saved, None, &mut bench.log);
        frame(&mut bench, on_screen(Vec::new()), None);
        assert!(bench.workspace.wanted(1), "the first row is in view");
        assert!(!bench.workspace.wanted(500), "the last row is not");
    }

    /// A row whose file is read while it is drawn says what the file holds on the next
    /// frame, from the lines it already had.
    #[test]
    fn a_row_read_while_drawn_says_what_it_holds() {
        let mut bench = Bench::new();
        let saved = crate::workspace::Saved {
            id: 1,
            name: "Grand.ne5p".into(),
            path: Some(LibPath::root().join("Grand.ne5p")),
            origin: crate::workspace::Origin::Fresh,
            saved: Vec::new(),
            file: None,
            unread: Some(1),
            unsaved: None,
        };
        bench.workspace.restore(vec![saved], None, &mut bench.log);
        let (output, _) = frame(&mut bench, on_screen(Vec::new()), None);
        assert!(testing::words(&output).contains(&"reading…".to_string()));

        assert_eq!(bench.workspace.take_wanted(), vec![1]);
        let bytes = Fresh::Program.bytes().unwrap();
        bench.workspace.took(1, Some(bytes), None);
        bench.workspace.settle_files(&mut bench.log);
        let takes = bench.browser.rows.takes;
        let (output, _) = frame(&mut bench, on_screen(Vec::new()), None);
        let said = testing::words(&output);
        assert!(!said.contains(&"reading…".to_string()), "{said:?}");
        assert!(said.contains(&"program".to_string()), "{said:?}");
        assert_eq!(
            bench.browser.rows.takes, takes,
            "the lines stand for the same rows"
        );
    }

    /// A rename of a row out of view keeps its editor: what is typed into it renames the
    /// row.
    #[test]
    fn a_row_out_of_view_is_renamed_by_what_is_typed() {
        let mut bench = Bench::new();
        let ids = sounds(&mut bench, 2000);
        bench
            .browser
            .start_rename(Item::Local(ids[1999]), "Sound 1999.ne5p");
        let frames = [
            Vec::new(),
            vec![egui::Event::Text("LA Grand".into())],
            vec![testing::key(egui::Key::Enter)],
        ];
        let mut named = None;
        for events in frames {
            for act in frame(&mut bench, on_screen(events), None).1 {
                if let Act::RenameLocal { id, name } = act {
                    named = Some((id, name));
                }
            }
        }
        assert_eq!(named, Some((ids[1999], "LA Grand".to_string())));
    }

    /// The lines of the library are laid out again when a branch opens, and not on a
    /// frame that changes nothing.
    #[test]
    fn frames_of_the_tree_lay_out_the_library_once_per_change() {
        let mut bench = Bench::new();
        let id = bench
            .browser
            .folders
            .make(&LibPath::root(), &bench.workspace);
        sounds(&mut bench, 3);
        for _ in 0..3 {
            frame(&mut bench, on_screen(Vec::new()), None);
        }
        assert_eq!(bench.browser.rows.takes, 1);
        bench.browser.twist(Branch::Folder(id));
        frame(&mut bench, on_screen(Vec::new()), None);
        assert_eq!(bench.browser.rows.takes, 2);
    }

    #[test]
    fn a_shift_click_selects_every_row_between_even_those_out_of_view() {
        let mut bench = Bench::new();
        let ids = sounds(&mut bench, 2000);
        let (output, _) = frame(&mut bench, on_screen(Vec::new()), None);
        let first = painted_at(&output, "Sound 0000").unwrap().center();
        frame(&mut bench, on_screen(testing::click(first)), None);
        assert_eq!(bench.browser.selection.sole(), Some(Item::Local(ids[0])));

        frame(&mut bench, on_screen(Vec::new()), Some(1.0e7));
        let (output, _) = frame(&mut bench, on_screen(Vec::new()), None);
        let last = painted_at(&output, "Sound 1999").unwrap().center();
        let mut shift = on_screen(
            testing::click(last)
                .into_iter()
                .map(|event| match event {
                    egui::Event::PointerButton {
                        pos,
                        button,
                        pressed,
                        ..
                    } => egui::Event::PointerButton {
                        pos,
                        button,
                        pressed,
                        modifiers: egui::Modifiers::SHIFT,
                    },
                    event => event,
                })
                .collect(),
        );
        shift.modifiers = egui::Modifiers::SHIFT;
        // Long enough after the first click that the two are not a double click.
        shift.time = Some(10.0);
        frame(&mut bench, shift, None);
        assert_eq!(bench.browser.selection.items().count(), ids.len());
        assert!(ids
            .iter()
            .all(|id| bench.browser.selection.holds(Item::Local(*id))));
    }

    /// Go to loaded selects the slot the panel is on and scrolls to it, however far down
    /// the tree it is.
    #[test]
    fn going_to_the_loaded_slot_scrolls_it_into_view() {
        let mut bench = Bench::new();
        sounds(&mut bench, 500);
        let class = ObjectClass::Program;
        let names: Vec<String> = (1..=50).map(|slot| format!("Patch {slot}")).collect();
        let names: Vec<&str> = names.iter().map(String::as_str).collect();
        for bank in 1..=8 {
            bench.device.pretend_scanned(class, bank, &names);
        }
        bench.device.pretend_partitions(&crate::device::ELECTRO5);
        let at = Location::from_user(8, 50);
        bench.browser.jump = Some((class, at));

        let mut seen = None;
        for _ in 0..120 {
            let (output, _) = frame(&mut bench, on_screen(Vec::new()), None);
            seen = painted_at(&output, "Patch 50").filter(|rect| rect.bottom() <= SCREEN.y);
            if seen.is_some() && bench.browser.jump.is_none() {
                break;
            }
        }
        assert!(seen.is_some(), "8:50 is never scrolled into view");
        assert_eq!(
            bench.browser.selection.sole(),
            Some(Item::Slot { class, at })
        );
    }

    /// A drag from one row onto a folder row, both reached by scrolling, files the asset
    /// into the folder.
    #[test]
    fn a_drop_onto_a_folder_row_out_of_first_view_lands() {
        let mut bench = Bench::new();
        let dirs: Vec<LibPath> = (0..100)
            .map(|i| LibPath::parse(&format!("F{i:03}")).unwrap())
            .collect();
        bench.browser.folders.sync(&dirs);
        let ids = sounds(&mut bench, 1);
        let target = bench.browser.folders.id_of(&dirs[99]).unwrap();

        frame(&mut bench, on_screen(Vec::new()), Some(1.0e7));
        let (output, _) = frame(&mut bench, on_screen(Vec::new()), None);
        let from = painted_at(&output, "Sound 0000").unwrap().center();
        let onto = painted_at(&output, "F099")
            .expect("the last folder, scrolled to")
            .center();
        let steps = [
            vec![egui::Event::PointerMoved(from), testing::button(from, true)],
            vec![egui::Event::PointerMoved(from + egui::vec2(0.0, -12.0))],
            vec![egui::Event::PointerMoved(onto)],
            vec![testing::button(onto, false)],
        ];
        let mut acts = Vec::new();
        for events in steps {
            acts.extend(frame(&mut bench, on_screen(events), None).1);
        }
        assert!(
            acts.iter().any(|act| matches!(
                act,
                Act::File { id, folder: Some(folder) } if *id == ids[0] && *folder == target
            )),
            "{acts:?}"
        );
    }

    /// ⚠️ A filter applied while a document is in front would narrow a table nobody is
    /// looking at, so every tree row that narrows brings the library forward.
    #[test]
    fn narrowing_the_library_brings_it_forward() {
        for asked in [
            Narrow::Kind(Kind::Program),
            Narrow::Tag(1),
            Narrow::Place(Place::Computer),
            Narrow::State(State::Waiting),
            Narrow::State(State::Differs),
        ] {
            let mut bench = Bench::new();
            let id = bench
                .workspace
                .create(Fresh::Program, &mut bench.log)
                .unwrap();
            bench.tabs.open(id);

            let mut acts = Vec::new();
            narrow(&mut acts, asked);
            bench.act(acts);
            assert!(bench.shell.filter.on(asked), "{asked:?}");
            assert_eq!(bench.tabs.showing(), Spot::Library, "{asked:?}");
        }
    }

    /// ⚠️ A place row narrows the library like a kind or tag row, so it is highlighted
    /// like one. Otherwise nothing in the window shows where the filter came from.
    #[test]
    fn the_place_row_reads_as_on_while_the_library_is_over_that_place() {
        let Bench {
            ctx,
            mut browser,
            workspace,
            device,
            queue,
            ..
        } = Bench::new();
        let lit = ctx.style().visuals.selection.bg_fill;
        let mut on = |filter: &Filter| -> usize {
            let output = testing::run(&ctx, egui::RawInput::default(), |ctx| {
                egui::SidePanel::left("places")
                    .exact_width(crate::shell::BROWSER)
                    .show(ctx, |ui| {
                        browser.ui(ui, &workspace, &device, &queue, filter);
                    });
            });
            testing::rects(&output)
                .iter()
                .filter(|drawn| drawn.fill == lit)
                .count()
        };

        assert_eq!(on(&Filter::default()), 0, "nothing is narrowed to a place");
        let mut filter = Filter::default();
        filter.narrow(Narrow::Place(Place::Computer));
        assert_eq!(on(&filter), 1, "only This computer is highlighted");
    }

    /// The kinds section lists what the two places hold, in one order. A row for a kind
    /// neither place holds would narrow the library to nothing.
    #[test]
    fn the_kinds_section_lists_the_union_of_the_two_places() {
        let Bench {
            mut workspace,
            mut device,
            mut log,
            ..
        } = Bench::new();
        workspace.create(Fresh::Program, &mut log).unwrap();
        let alone = kinds_present(&workspace, &device.state);
        assert_eq!(alone, [Kind::Program]);
        assert!(!worth_choosing(&alone), "nothing to choose between");

        device.pretend_partitions(&crate::device::ELECTRO5);
        workspace.create(Fresh::Stage4Synth, &mut log).unwrap();
        assert_eq!(
            kinds_present(&workspace, &device.state),
            [
                Kind::Program,
                Kind::SetList,
                Kind::Sample,
                Kind::Piano,
                Kind::Live,
                Kind::Settings,
                Kind::Synth,
            ],
            "the instrument's six folders and what is on this computer, in one order"
        );
        assert!(worth_choosing(&kinds_present(&workspace, &device.state)));
    }

    /// Frames of the tree take the library's kinds once, a file read since is in them on
    /// the next frame, and an instrument attached adds its folders without taking them
    /// again.
    #[test]
    fn frames_take_the_kinds_once_per_change() {
        let Bench {
            ctx,
            mut browser,
            mut workspace,
            mut device,
            queue,
            mut log,
            ..
        } = Bench::new();
        let saved = crate::workspace::Saved {
            id: 1,
            name: "Grand.ns4p".into(),
            path: Some(crate::store::LibPath::root().join("Grand.ns4p")),
            origin: crate::workspace::Origin::Fresh,
            saved: Vec::new(),
            file: None,
            unread: Some(1),
            unsaved: None,
        };
        workspace.restore(vec![saved], None, &mut log);
        let draw = |browser: &mut Browser, workspace: &Workspace, device: &Device| {
            testing::run(&ctx, egui::RawInput::default(), |ctx| {
                egui::SidePanel::left("browser")
                    .exact_width(crate::shell::BROWSER)
                    .show(ctx, |ui| {
                        browser.ui(ui, workspace, device, &queue, &Filter::default());
                    });
            });
        };
        for _ in 0..3 {
            draw(&mut browser, &workspace, &device);
        }
        assert_eq!(workspace.kinds_taken.get(), 1);
        assert_eq!(workspace.kinds(), [Kind::Program], "what its name says");

        workspace.take_wanted();
        workspace.took(1, Some(Fresh::Stage4Synth.bytes().unwrap()), None);
        workspace.settle_files(&mut log);
        draw(&mut browser, &workspace, &device);
        assert_eq!(workspace.kinds_taken.get(), 2);
        assert_eq!(workspace.kinds(), [Kind::Synth], "what it holds");

        device.pretend_partitions(&crate::device::ELECTRO5);
        draw(&mut browser, &workspace, &device);
        assert_eq!(
            workspace.kinds_taken.get(),
            2,
            "the instrument is not the list"
        );
        assert!(kinds_present(&workspace, &device.state).contains(&Kind::Live));
    }

    /// ⚠️ The row that turns a filter off goes away with its kind. A filter left on a
    /// kind that is nowhere would show an empty table with nothing to click to clear it.
    #[test]
    fn a_kind_that_leaves_the_union_stops_narrowing() {
        let Bench {
            workspace,
            mut device,
            ..
        } = Bench::new();
        let mut filter = Filter::default();
        device.pretend_partitions(&crate::device::ELECTRO5);
        filter.narrow(Narrow::Kind(Kind::Piano));
        filter.keep_kinds(&kinds_present(&workspace, &device.state));
        assert_eq!(filter.kind, Some(Kind::Piano));

        device.pretend(crate::device::DeviceEvent::Disconnected { lost: false });
        device.poll(
            &mut crate::log::Log::default(),
            &mut Workspace::new(context()),
            &mut crate::tabs::Tabs::default(),
            &mut Queue::default(),
        );
        filter.keep_kinds(&kinds_present(&workspace, &device.state));
        assert_eq!(filter.kind, None, "the instrument took its folders with it");
    }

    /// ⚠️ A duplicate skips slots the queue is already bound for, because two writes to
    /// one address would leave only one.
    #[test]
    fn a_duplicate_lands_past_the_slot_the_queue_is_bound_for() {
        let Bench {
            mut workspace,
            mut device,
            mut queue,
            mut log,
            ..
        } = Bench::new();
        let class = ObjectClass::Program;
        device.pretend_scanned(class, 7, &["Africa Split", "", ""]);
        let id = workspace.create(Fresh::Program, &mut log).unwrap();
        assert_eq!(
            spare_slot(&device.state, class, &queue),
            Some(Location::from_user(7, 2))
        );

        crate::queue::enqueue(
            &workspace,
            &mut device,
            &mut queue,
            &mut log,
            id,
            class,
            Location::from_user(7, 2),
        );
        assert_eq!(
            spare_slot(&device.state, class, &queue),
            Some(Location::from_user(7, 3)),
            "the queue is already bound for 7:2"
        );
    }

    /// The piano categories say something the location column does not; "Bank 1" over
    /// rows already labeled `1:…` does not.
    #[test]
    fn a_bank_caption_only_shows_what_the_number_does_not_say() {
        assert!(worth_captioning(1, "Grand"));
        assert!(worth_captioning(2, "Upright"));
        for furniture in ["Bank 1", "bank 1", "BANK 1", "1", " ", ""] {
            assert!(!worth_captioning(1, furniture), "{furniture:?}");
        }
        // Only a matching number is redundant. "Bank 2" over bank 1 is shown, because one
        // of the two is wrong.
        assert!(worth_captioning(1, "Bank 2"));
    }

    /// ⚠️ A leaf's glyph lines up under the glyph of the branch beside it, because a
    /// leaf's indent includes the triangle's box that a branch draws.
    #[test]
    fn a_leaf_starts_where_the_branch_beside_it_puts_its_glyph() {
        for depth in 0..4 {
            assert_eq!(
                indent(depth, false),
                indent(depth, true) + STEP,
                "depth {depth}"
            );
            assert!(indent(depth + 1, true) > indent(depth, true));
        }
    }
}
