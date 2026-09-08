//! The library: one table over every kind, in both places.
//!
//! A [`Row`] is what the table knows about one thing — where it is, where it goes, how
//! big it is, what it plays — and it is built from the list on this computer and the
//! scanned slots without a frame in sight. [`arrange`] narrows and orders a set of them.
//! Everything under those two is paint.

use std::cmp::Ordering;
use std::collections::BTreeSet;
use std::ops::Range;

use eframe::egui;
use nord_usb::wire::ProgramInfo;
use nord_usb::{Location, ObjectClass};

use crate::app::{accent, micro, ui as ui_text, warn};
use crate::browser::{cell_ink, Act, Browser, Bulk, Item, Kind};
use crate::device::{sendable, Device, DeviceState, BROWSED};
use crate::filter::{Filter, Place};
use crate::icon::{icon, painted, Glyph};
use crate::panel::Track;
use crate::queue::Queue;
use crate::shell::{Page, Shell};
use crate::strings::{folder, place, shown};
use crate::tags::Tags;
use crate::workspace::{LocalEntity, Workspace};

/// Which of the two places a row's contents are in.
///
/// ⚠️ The sign on `Both` compares the file's own body checksum with the one the
/// instrument reports for the slot. A class the device does not checksum reports none,
/// and then the two are known to be in both places and not known to agree.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Where {
    Both(Option<bool>),
    Computer,
    Keyboard,
    /// It came off a slot nothing has read, so the other copy cannot be spoken about.
    Unread,
}

impl Where {
    /// The short word the column carries.
    pub fn short(self) -> &'static str {
        match self {
            Where::Both(Some(true)) => "both =",
            Where::Both(Some(false)) => "both ≠",
            Where::Both(None) => "both",
            Where::Computer => "computer",
            Where::Keyboard => "keyboard",
            Where::Unread => "—",
        }
    }

    /// The whole of it, which is what the tooltip says.
    pub fn sentence(self) -> &'static str {
        match self {
            Where::Both(Some(true)) => {
                "On this computer and on the instrument, and the two bodies match."
            }
            Where::Both(Some(false)) => {
                "On this computer and on the instrument, and the two bodies differ."
            }
            Where::Both(None) => {
                "On this computer and on the instrument. The instrument reports no \
                 checksum for this folder, so nothing here says whether they agree."
            }
            Where::Computer => "On this computer only.",
            Where::Keyboard => "On the instrument only.",
            Where::Unread => "It came off a slot this session has not read.",
        }
    }

    /// The places it is in, which is what a place filter asks about.
    fn places(self) -> &'static [Place] {
        match self {
            Where::Both(_) => &[Place::Computer, Place::Keyboard],
            Where::Computer | Where::Unread => &[Place::Computer],
            Where::Keyboard => &[Place::Keyboard],
        }
    }

    /// Both first, then this computer, the instrument, and the unsayable.
    fn rank(self) -> u8 {
        match self {
            Where::Both(Some(false)) => 0,
            Where::Both(Some(true)) => 1,
            Where::Both(None) => 2,
            Where::Computer => 3,
            Where::Keyboard => 4,
            Where::Unread => 5,
        }
    }
}

/// The library object a row plays that the row's own bytes do not carry.
///
/// ⚠️ A name can only come from the instrument — a program file stores a bare id and no
/// name at all — so `Wanted` is an id **nothing has resolved**, never one the instrument
/// said it did not hold. No read this app makes reports a missing library.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Needs {
    Nothing,
    Named { class: ObjectClass, name: String },
    Wanted { class: ObjectClass, id: u32 },
}

impl Needs {
    /// The words the column carries.
    pub fn text(&self) -> String {
        match self {
            Needs::Nothing => String::new(),
            Needs::Named { name, .. } => name.clone(),
            Needs::Wanted { class, id } => {
                format!("{} {id:#010x}", Kind::from_class(*class).chip())
            }
        }
    }

    /// The whole of it, which is what the tooltip says.
    pub fn sentence(&self) -> String {
        match self {
            Needs::Nothing => "Nothing here says what it plays.".to_string(),
            Needs::Named { class, name } => format!(
                "It plays the {} the instrument calls “{name}”.",
                Kind::from_class(*class).chip()
            ),
            Needs::Wanted { class, id } => format!(
                "It names {} {id:#010x}, which nothing has resolved to a name. Only the \
                 instrument can, and only for a slot it has been asked about.",
                Kind::from_class(*class).chip()
            ),
        }
    }

    fn short(&self) -> Option<ObjectClass> {
        match self {
            Needs::Wanted { class, .. } => Some(*class),
            _ => None,
        }
    }
}

/// One line of the library.
pub struct Row {
    pub item: Item,
    pub kind: Kind,
    pub name: String,
    pub tags: usize,
    pub where_: Where,
    /// The slot it came off, or the slot it is. Nothing on this computer that never came
    /// off one has an address at all.
    pub at: Option<(ObjectClass, Location)>,
    pub size: u64,
    pub needs: Needs,
}

impl Row {
    /// Where a send would write this row, which is [`crate::device::sendable`]'s rule
    /// over an asset that came off a slot. A row that is already on the instrument goes
    /// nowhere.
    fn destination(&self) -> Option<(ObjectClass, Location)> {
        let (class, at) = self.at?;
        (matches!(self.item, Item::Local(_)) && sendable(class)).then_some((class, at))
    }
}

/// Everything the library holds, narrowed by kind, place and tags.
///
/// The list on this computer comes first, and a slot one of its assets came off is that
/// asset's row rather than a second one: a program read off 7:4 and kept is one thing in
/// two places, which is what [`Where::Both`] says.
pub fn rows(workspace: &Workspace, device: &DeviceState, tags: &Tags, filter: &Filter) -> Vec<Row> {
    let mut rows = Vec::new();
    let mut claimed: Vec<(ObjectClass, Location)> = Vec::new();
    for entity in workspace.listed() {
        if let Some(slot) = entity.origin.slot() {
            claimed.push(slot);
        }
        let worn = tags.worn(entity.id);
        let row = local(entity, device, worn.len());
        if admits(filter, &row, worn) {
            rows.push(row);
        }
    }
    let untagged = BTreeSet::new();
    for class in BROWSED {
        for bank in device.banks_of(class) {
            let Some(slots) = device.bank(class, bank) else {
                continue;
            };
            for (index, held) in slots.iter().enumerate() {
                let Some(info) = held else {
                    continue;
                };
                let at = Location::from_user(bank, index as u32 + 1);
                if claimed.contains(&(class, at)) {
                    continue;
                }
                let row = slot(class, at, info, device);
                if admits(filter, &row, &untagged) {
                    rows.push(row);
                }
            }
        }
    }
    rows
}

/// Whether a row survives the narrowing. A row in both places survives a filter naming
/// either of them.
fn admits(filter: &Filter, row: &Row, tags: &BTreeSet<u64>) -> bool {
    row.where_
        .places()
        .iter()
        .any(|place| filter.admits(row.kind, *place, tags))
}

fn local(entity: &LocalEntity, device: &DeviceState, tags: usize) -> Row {
    Row {
        item: Item::Local(entity.id),
        kind: Kind::of(entity.entity.as_ref()),
        name: entity.name.clone(),
        tags,
        where_: whereabouts(entity, device),
        at: entity.origin.slot(),
        size: entity.bytes.len() as u64,
        needs: wanted(entity, device),
    }
}

fn slot(class: ObjectClass, at: Location, info: &ProgramInfo, device: &DeviceState) -> Row {
    Row {
        item: Item::Slot { class, at },
        kind: Kind::from_class(class),
        name: info.name.trim().to_string(),
        tags: 0,
        where_: Where::Keyboard,
        at: Some((class, at)),
        size: u64::from(info.body_len),
        needs: played(class, at, device),
    }
}

fn whereabouts(entity: &LocalEntity, device: &DeviceState) -> Where {
    let Some((class, at)) = entity.origin.slot() else {
        return Where::Computer;
    };
    match device.slot(class, at) {
        None => Where::Unread,
        Some(None) => Where::Computer,
        Some(Some(info)) => Where::Both(agrees(entity, info)),
    }
}

/// Whether a file and the slot it came off carry the same body.
///
/// The container's own CRC-32 **is** the checksum the device reports — see the
/// round-trip in [`crate::workspace`] — so the two compare without either body being
/// hashed again, which a sample library on this computer would not survive per frame.
fn agrees(entity: &LocalEntity, info: &ProgramInfo) -> Option<bool> {
    let here = entity.container.as_ref()?.body_crc32?;
    Some(here == info.crc32?)
}

/// The library a file names, and the name the instrument gave it if it has been asked.
pub(crate) fn wanted(entity: &LocalEntity, device: &DeviceState) -> Needs {
    let Some(fields) = entity.entity.as_ref().and_then(crate::fields::fields_of) else {
        return Needs::Nothing;
    };
    // One cell, so the first of the two a program can name is the one it shows.
    for (path, class) in [
        ("piano_panel.id", ObjectClass::Piano),
        ("sample_panel.id", ObjectClass::Sample),
    ] {
        // Zero is "this program references no library", not an id to go looking for.
        let Some(id) = fields
            .iter()
            .find(|field| field.path == path)
            .and_then(|field| crate::document::library_id(&field.value))
            .filter(|id| *id != 0)
        else {
            continue;
        };
        return match device.dependency_name(entity.origin.slot(), class, id) {
            Some(name) => Needs::Named {
                class,
                name: name.to_string(),
            },
            None => Needs::Wanted { class, id },
        };
    }
    Needs::Nothing
}

/// What the instrument said a slot plays, where that slot is the one it was last asked
/// about.
///
/// ⚠️ Programs only. The cached detail records an address and no class, so a set list at
/// the same address would otherwise wear a program's piano.
fn played(class: ObjectClass, at: Location, device: &DeviceState) -> Needs {
    if class != ObjectClass::Program || device.detail.at != Some(at) {
        return Needs::Nothing;
    }
    let Some(deps) = device.detail.deps.as_ref() else {
        return Needs::Nothing;
    };
    deps.iter()
        // ⚠️ `flag` is whether the section owning it is routed; an unrouted row names a
        // library the program does not play.
        .filter(|dep| dep.flag == 1)
        .find(|dep| matches!(dep.class, ObjectClass::Piano | ObjectClass::Sample))
        .map_or(Needs::Nothing, |dep| Needs::Named {
            class: dep.class,
            name: dep.name.trim().to_string(),
        })
}

// ---- narrowing and ordering ------------------------------------------------------

/// One column of the table.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Column {
    Mark,
    Glyph,
    Name,
    Tags,
    Where,
    At,
    Size,
    Needs,
}

/// Which way a column is sorted.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Order {
    Up,
    Down,
}

impl Order {
    fn flipped(self) -> Order {
        match self {
            Order::Up => Order::Down,
            Order::Down => Order::Up,
        }
    }
}

impl Column {
    pub const ALL: [Column; 8] = [
        Column::Mark,
        Column::Glyph,
        Column::Name,
        Column::Tags,
        Column::Where,
        Column::At,
        Column::Size,
        Column::Needs,
    ];

    /// The word over the column. The two that carry a mark rather than a word have none.
    fn head(self) -> &'static str {
        match self {
            Column::Mark | Column::Glyph => "",
            Column::Name => "name",
            Column::Tags => "tags",
            Column::Where => "where",
            Column::At => "at",
            Column::Size => "size",
            Column::Needs => "needs",
        }
    }

    /// Where this column sits in a row, which is where its track is.
    fn index(self) -> usize {
        match self {
            Column::Mark => 0,
            Column::Glyph => 1,
            Column::Name => 2,
            Column::Tags => 3,
            Column::Where => 4,
            Column::At => 5,
            Column::Size => 6,
            Column::Needs => 7,
        }
    }

    /// What the column asks for: a fixed width, or a share of what the fixed ones leave.
    fn track(self) -> Track {
        match self {
            Column::Mark => Track::Px(18.0),
            Column::Glyph => Track::Px(20.0),
            Column::Name => Track::Share(1.9),
            Column::Tags => Track::Px(38.0),
            Column::Where => Track::Px(64.0),
            Column::At => Track::Px(50.0),
            Column::Size => Track::Px(54.0),
            Column::Needs => Track::Share(1.3),
        }
    }
}

/// The gap between two columns.
const GAP: f32 = 8.0;

/// Where each column sits across `width`, laid out by [`crate::panel::tracks`].
pub fn tracks(width: f32) -> [Range<f32>; 8] {
    let wanted = Column::ALL.map(Column::track);
    let held = crate::panel::tracks(width, &wanted, GAP);
    std::array::from_fn(|index| held[index].clone())
}

/// The rows the table shows: what the omnibox admits, in the order a column asks for.
///
/// ⚠️ Ties break on the name and then the row itself, so the order does not depend on
/// the order the rows happened to be built in.
pub fn arrange(mut rows: Vec<Row>, query: &str, by: Column, order: Order) -> Vec<Row> {
    let query = query.trim().to_lowercase();
    if !query.is_empty() {
        rows.retain(|row| row.name.to_lowercase().contains(&query));
    }
    rows.sort_by(|a, b| {
        let ranked = match order {
            Order::Up => compare(by, a, b),
            Order::Down => compare(by, a, b).reverse(),
        };
        ranked
            .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
            .then_with(|| a.item.cmp(&b.item))
    });
    rows
}

fn compare(by: Column, a: &Row, b: &Row) -> Ordering {
    match by {
        // A mark is a control rather than a fact about the row, so its head sorts
        // nothing and the tie-breakers stand.
        Column::Mark => Ordering::Equal,
        Column::Glyph => a.kind.plural().cmp(b.kind.plural()),
        Column::Name => a.name.to_lowercase().cmp(&b.name.to_lowercase()),
        Column::Tags => a.tags.cmp(&b.tags),
        Column::Where => a.where_.rank().cmp(&b.where_.rank()),
        Column::At => address(a).cmp(&address(b)),
        Column::Size => a.size.cmp(&b.size),
        Column::Needs => a.needs.text().cmp(&b.needs.text()),
    }
}

/// A row with no address sorts after every row that has one, rather than at `0:0`.
fn address(row: &Row) -> (bool, u32, u32, u32) {
    match row.at {
        Some((class, at)) => (false, class.to_raw(), at.bank, at.slot),
        None => (true, 0, 0, 0),
    }
}

// ---- the consequence of a selection -----------------------------------------------

/// What sending the picked rows would do, in one sentence.
pub fn consequence(rows: &[&Row], device: &DeviceState, queue: &Queue) -> String {
    let mut going: Vec<(ObjectClass, Location)> =
        rows.iter().filter_map(|row| row.destination()).collect();
    going.sort_unstable_by_key(|(class, at)| (class.to_raw(), at.bank, at.slot));

    let mut said = Vec::new();
    if !going.is_empty() {
        said.push(format!("→ {}", spans(&going)));
        let occupied = going
            .iter()
            .filter(|(class, at)| device.slot(*class, *at).flatten().is_some())
            .count();
        said.push(match occupied {
            0 => "every slot is empty".to_string(),
            1 => "1 slot occupied".to_string(),
            n => format!("{n} slots occupied"),
        });
    }
    let waiting = rows
        .iter()
        .filter(|row| matches!(row.item, Item::Local(id) if queue.holds(id)))
        .count();
    if waiting > 0 {
        said.push(format!("{waiting} already waiting"));
    }
    for class in [ObjectClass::Piano, ObjectClass::Sample] {
        let short = rows
            .iter()
            .filter(|row| row.needs.short() == Some(class))
            .count();
        let named = Kind::from_class(class).chip();
        match short {
            0 => {}
            1 => said.push(format!("1 needs a {named} the instrument has not named")),
            n => said.push(format!("{n} need a {named} the instrument has not named")),
        }
    }
    match said.is_empty() {
        true => "Nothing picked goes to the instrument.".to_string(),
        false => said.join(" · "),
    }
}

/// The destinations as one run per folder: `Programs 7:1–7:4`.
fn spans(going: &[(ObjectClass, Location)]) -> String {
    let mut runs: Vec<String> = Vec::new();
    for class in BROWSED {
        let mut ats = going
            .iter()
            .filter(|(held, _)| *held == class)
            .map(|(_, at)| *at);
        let Some(first) = ats.next() else {
            continue;
        };
        let last = ats.next_back().unwrap_or(first);
        runs.push(match first == last {
            true => place(class, first),
            false => format!("{} {}–{}", folder(class), shown(first), shown(last)),
        });
    }
    runs.join(", ")
}

// ---- the view ----------------------------------------------------------------------

/// The height of a row, of the head over them, and of the bar over that.
const ROW: f32 = 24.0;
const HEAD: f32 = 20.0;
const BAR: f32 = 28.0;

/// The room the bar, the table and the footer keep at each end.
///
/// ⚠️ The table's is the tree's own row indent. Without it the first track starts at the
/// panel's edge and the mark's left stroke is painted half outside the window.
const PAD: f32 = 8.0;

/// A kind glyph in a row, and the smaller ones beside a count.
const GLYPH: f32 = 13.0;
const SMALL: f32 = 10.0;

/// The selection mark's box.
const MARK: f32 = 11.0;

/// The faces a cell paints in. Painted rather than laid out, so the sizes are here
/// rather than resolved from the named styles in [`crate::app`].
const NAME: f32 = 12.0;
const MONO: f32 = 10.5;

/// The centre's default view.
pub struct Library {
    by: Column,
    order: Order,
}

impl Default for Library {
    fn default() -> Library {
        Library {
            by: Column::Name,
            order: Order::Up,
        }
    }
}

impl Library {
    pub fn ui(
        &mut self,
        ui: &mut egui::Ui,
        browser: &mut Browser,
        workspace: &Workspace,
        device: &Device,
        queue: &Queue,
        shell: &Shell,
    ) -> Vec<Act> {
        // The bar, the head and the rows are flush: the table's own lines are the only
        // horizontal rules in it.
        ui.spacing_mut().item_spacing.y = 0.0;
        let mut acts = Vec::new();
        let held = rows(workspace, &device.state, browser.tags(), &shell.filter);
        let held = arrange(held, &shell.omnibox, self.by, self.order);

        bar(ui, &held, queue, browser.tags(), &shell.filter);
        let picked: Vec<&Row> = held
            .iter()
            .filter(|row| browser.picked().holds(row.item))
            .collect();
        if !picked.is_empty() {
            // Claimed before the table, so the strip keeps its height whatever the table
            // does with what is left.
            egui::TopBottomPanel::bottom("library_footer")
                .resizable(false)
                .frame(egui::Frame::new())
                .show_inside(ui, |ui| {
                    footer(ui, &picked, browser, &device.state, queue, &mut acts)
                });
        }
        self.table(ui, &held, browser, workspace, device, &mut acts);
        acts
    }

    fn table(
        &mut self,
        ui: &mut egui::Ui,
        rows: &[Row],
        browser: &mut Browser,
        workspace: &Workspace,
        device: &Device,
        acts: &mut Vec<Act>,
    ) {
        // The head and every row start where a row of the tree starts, so the whole grid
        // moves together and the scroll bar stays at the panel's own edge.
        let room = ui.available_rect_before_wrap();
        let mut inset = ui.new_child(
            egui::UiBuilder::new()
                .max_rect(room.with_min_x(room.left() + PAD))
                .layout(*ui.layout()),
        );
        let ui = &mut inset;

        // The head and the rows are laid out to one width, so a scroll bar the rows make
        // room for must come off the head as well.
        let body = ui.available_height() - HEAD;
        let scrolls = rows.len() as f32 * ROW > body;
        let bar = match scrolls {
            true => ui.spacing().scroll.bar_width,
            false => 0.0,
        };
        let width = (ui.available_width() - bar).max(0.0);
        let tracks = tracks(width);
        self.head(ui, width, &tracks);
        if rows.is_empty() {
            return nothing(ui);
        }

        let list: Vec<Item> = rows.iter().map(|row| row.item).collect();
        egui::ScrollArea::vertical()
            .id_salt("library_table")
            .auto_shrink([false; 2])
            .show_rows(ui, ROW, rows.len(), |ui, shown| {
                for row in shown.filter_map(|index| rows.get(index)) {
                    paint(
                        ui, row, width, &tracks, browser, &list, workspace, device, acts,
                    );
                }
            });
    }

    /// 20 px of column heads, each one a click that sorts by it.
    fn head(&mut self, ui: &mut egui::Ui, width: f32, tracks: &[Range<f32>; 8]) {
        let (rect, response) =
            ui.allocate_exact_size(egui::vec2(width, HEAD), egui::Sense::click());
        let visuals = ui.visuals().clone();
        let painter = ui.painter().clone();
        painter.rect_filled(rect, 0.0, visuals.faint_bg_color);
        let quiet = crate::app::caption(&visuals);
        let strong = visuals.widgets.active.fg_stroke.color;

        for (column, track) in Column::ALL.iter().zip(tracks) {
            if column.head().is_empty() {
                continue;
            }
            let sorted = self.by == *column;
            let ink = match sorted {
                true => strong,
                false => quiet,
            };
            let mut room = track.end - track.start;
            if sorted {
                // ⚠️ The vendored Lucide set has no chevron-up, so ascending points the
                // way the tree's shut branch does rather than upwards.
                let glyph = match self.order {
                    Order::Up => Glyph::ChevronRight,
                    Order::Down => Glyph::ChevronDown,
                };
                let box_ = egui::Rect::from_min_size(
                    egui::pos2(
                        rect.left() + track.end - SMALL,
                        rect.center().y - SMALL / 2.0,
                    ),
                    egui::Vec2::splat(SMALL),
                );
                painted(ui, glyph, box_, ink);
                room = (room - SMALL - 2.0).max(0.0);
            }
            let mut job = egui::text::LayoutJob::simple_singleline(
                column.head().to_uppercase(),
                egui::FontId::proportional(9.5),
                ink,
            );
            job.wrap = egui::text::TextWrapping::truncate_at_width(room);
            let galley = painter.layout_job(job);
            painter.galley(
                egui::pos2(
                    rect.left() + track.start,
                    rect.center().y - galley.size().y / 2.0,
                ),
                galley,
                egui::Color32::PLACEHOLDER,
            );
        }

        let Some(column) = response
            .clicked()
            .then(|| under(&response, rect, tracks))
            .flatten()
        else {
            return;
        };
        match self.by == column {
            true => self.order = self.order.flipped(),
            false => {
                self.by = column;
                self.order = Order::Up;
            }
        }
    }
}

/// 28 px: what the library is over, the tags narrowing it, and what wants attention.
fn bar(ui: &mut egui::Ui, rows: &[Row], queue: &Queue, tags: &Tags, filter: &Filter) {
    let differ = rows
        .iter()
        .filter(|row| row.where_ == Where::Both(Some(false)))
        .count();
    let waiting = queue.len();
    let (rect, _) =
        ui.allocate_exact_size(egui::vec2(ui.available_width(), BAR), egui::Sense::hover());
    let mut inner = ui.new_child(
        egui::UiBuilder::new()
            .max_rect(rect.shrink2(egui::vec2(PAD, 0.0)))
            .layout(egui::Layout::left_to_right(egui::Align::Center)),
    );
    let ui = &mut inner;
    ui.spacing_mut().item_spacing.x = 6.0;
    let ink = ui.visuals().widgets.inactive.fg_stroke.color;
    icon(ui, Glyph::LibraryBig, GLYPH, ink);
    ui.label(
        egui::RichText::new(format!("Library · {}", over(filter)))
            .size(12.0)
            .color(ink),
    );
    for tag in filter.tags.iter().filter_map(|id| tags.name_of(*id)) {
        chip(ui, Glyph::Tag, tag, ink);
    }
    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
        let tint = warn(ui.visuals());
        // ⚠️ What is owed is the whole list's, not this view's: it is the number the
        // toolbar's Send carries, and a filtered table would report a different one.
        if waiting > 0 {
            chip(ui, Glyph::Clock, &format!("{waiting} waiting"), tint)
                .on_hover_text("everything owed back to the instrument");
        }
        if differ > 0 {
            chip(ui, Glyph::CircleAlert, &format!("{differ} differ"), tint)
                .on_hover_text("here and on the instrument, and the two bodies differ");
        }
    });
}

/// What the library is over, in the words the filter's own rows use.
fn over(filter: &Filter) -> String {
    let mut narrowed = Vec::new();
    if let Some(kind) = filter.kind {
        narrowed.push(kind.plural().to_string());
    }
    if let Some(place) = filter.place {
        narrowed.push(
            match place {
                Place::Computer => "this computer",
                Place::Keyboard => "the instrument",
            }
            .to_string(),
        );
    }
    match narrowed.is_empty() {
        true => "everything".to_string(),
        false => narrowed.join(" · "),
    }
}

/// A bordered glyph and a word, for the bar's states and its tags.
fn chip(ui: &mut egui::Ui, glyph: Glyph, text: &str, tint: egui::Color32) -> egui::Response {
    let border = ui.visuals().widgets.noninteractive.bg_stroke.color;
    egui::Frame::new()
        .stroke(egui::Stroke::new(1.0_f32, border))
        .corner_radius(2.0)
        .inner_margin(egui::Margin::symmetric(5, 1))
        .show(ui, |ui| {
            ui.spacing_mut().item_spacing.x = 4.0;
            icon(ui, glyph, SMALL, tint);
            ui.label(egui::RichText::new(text).text_style(ui_text()).color(tint));
        })
        .response
}

/// Which column the pointer is over, for the tooltip and for the head's sort click.
fn under(response: &egui::Response, rect: egui::Rect, tracks: &[Range<f32>; 8]) -> Option<Column> {
    let at = response.interact_pointer_pos().or(response.hover_pos())?;
    let x = at.x - rect.left();
    Column::ALL
        .iter()
        .zip(tracks)
        .find(|(_, track)| (track.start..track.end + GAP).contains(&x))
        .map(|(column, _)| *column)
}

/// One row of the table.
///
/// ⚠️ Nothing inside is a widget, for the reason [`crate::browser::Cells`] gives: a label
/// allocates a hover rect that wins the hit test over the row, and the click lands on
/// whichever word happens to be under it. The row is the only thing that senses, and the
/// tooltip is whichever cell the pointer is in.
#[allow(clippy::too_many_arguments)]
fn paint(
    ui: &mut egui::Ui,
    row: &Row,
    width: f32,
    tracks: &[Range<f32>; 8],
    browser: &mut Browser,
    list: &[Item],
    workspace: &Workspace,
    device: &Device,
    acts: &mut Vec<Act>,
) {
    let selected = browser.picked().holds(row.item);
    let (rect, response) = ui.allocate_exact_size(egui::vec2(width, ROW), egui::Sense::click());
    let visuals = ui.visuals().clone();
    let painter = ui.painter().clone();
    let fill = match (selected, response.hovered()) {
        (true, _) => Some(visuals.selection.bg_fill),
        (false, true) => Some(visuals.faint_bg_color),
        (false, false) => None,
    };
    if let Some(fill) = fill {
        painter.rect_filled(rect, 3.0, fill);
    }
    let ink = match selected {
        true => visuals.selection.stroke.color,
        false => visuals.text_color(),
    };
    let quiet = cell_ink(selected, visuals.weak_text_color(), &visuals);

    let cell = |column: Column| {
        let track = &tracks[column.index()];
        egui::Rect::from_min_max(
            egui::pos2(rect.left() + track.start, rect.top()),
            egui::pos2(rect.left() + track.end, rect.bottom()),
        )
    };
    let write = |box_: egui::Rect, text: &str, font: egui::FontId, tint: egui::Color32| {
        let mut job = egui::text::LayoutJob::simple_singleline(text.to_string(), font, tint);
        job.wrap = egui::text::TextWrapping::truncate_at_width(box_.width());
        let galley = painter.layout_job(job);
        painter.galley(
            egui::pos2(box_.left(), box_.center().y - galley.size().y / 2.0),
            galley,
            egui::Color32::PLACEHOLDER,
        );
    };

    let checkbox = mark(ui, cell(Column::Mark), selected, &response);
    let glyph = cell(Column::Glyph);
    if glyph.width() > 0.0 {
        painted(
            ui,
            row.kind.glyph(),
            egui::Rect::from_center_size(
                egui::pos2(glyph.left() + GLYPH / 2.0, glyph.center().y),
                egui::Vec2::splat(GLYPH),
            ),
            ink,
        );
    }
    write(
        cell(Column::Name),
        &row.name,
        egui::FontId::proportional(NAME),
        ink,
    );
    if row.tags > 0 {
        let tags = cell(Column::Tags);
        painted(
            ui,
            Glyph::Tag,
            egui::Rect::from_center_size(
                egui::pos2(tags.left() + SMALL / 2.0, tags.center().y),
                egui::Vec2::splat(SMALL),
            ),
            quiet,
        );
        let count = tags.with_min_x(tags.left() + SMALL + 3.0);
        write(
            count,
            &row.tags.to_string(),
            egui::FontId::monospace(MONO),
            quiet,
        );
    }
    let differs = row.where_ == Where::Both(Some(false));
    write(
        cell(Column::Where),
        row.where_.short(),
        egui::FontId::proportional(NAME - 1.0),
        match differs {
            true => cell_ink(selected, warn(&visuals), &visuals),
            false => quiet,
        },
    );
    if let Some((_, at)) = row.at {
        write(
            cell(Column::At),
            &shown(at),
            egui::FontId::monospace(MONO),
            quiet,
        );
    }
    write(
        cell(Column::Size),
        &crate::room::measure(row.size),
        egui::FontId::monospace(MONO),
        quiet,
    );
    write(
        cell(Column::Needs),
        &row.needs.text(),
        egui::FontId::proportional(NAME - 1.0),
        match row.needs.short().is_some() {
            true => cell_ink(selected, warn(&visuals), &visuals),
            false => quiet,
        },
    );

    let checked = checkbox
        .map(|box_| box_.on_hover_text(tooltip(row, Column::Mark, browser.tags())))
        .is_some_and(|box_| box_.clicked());
    let response = match under(&response, rect, tracks) {
        Some(column) => response.on_hover_text(tooltip(row, column, browser.tags())),
        None => response,
    };
    if checked {
        browser.check(row.item);
    } else if response.double_clicked() {
        acts.push(Act::Open(row.item));
    } else if response.clicked() {
        browser.pick(ui, row.item, &row.name, &response, list);
    }
    response.context_menu(|ui| browser.menu(ui, row.item, workspace, device, acts));
}

/// The 11 px box that says whether a row is checked, and takes the click that changes
/// it. A column too narrow to draw the box offers none.
fn mark(
    ui: &egui::Ui,
    box_: egui::Rect,
    picked: bool,
    row: &egui::Response,
) -> Option<egui::Response> {
    if box_.width() < MARK {
        return None;
    }
    let visuals = ui.visuals().clone();
    let at = egui::Rect::from_center_size(
        egui::pos2(box_.left() + MARK / 2.0, box_.center().y),
        egui::Vec2::splat(MARK),
    );
    match picked {
        true => {
            ui.painter().rect_filled(at, 2.0, accent(&visuals));
            painted(
                ui,
                Glyph::Check,
                at.shrink(1.5),
                visuals.selection.stroke.color,
            );
        }
        false => {
            ui.painter().rect_stroke(
                at,
                2.0,
                egui::Stroke::new(1.0_f32, visuals.widgets.noninteractive.bg_stroke.color),
                egui::StrokeKind::Inside,
            );
        }
    }
    Some(ui.interact(at, row.id.with("mark"), egui::Sense::click()))
}

/// The whole of a cell, which is what a hover asks for.
///
/// The tags column is the one that grows a fact the row does not carry: it holds a count,
/// and the hover is where the names are.
fn tooltip(row: &Row, column: Column, tags: &Tags) -> String {
    match column {
        Column::Mark => {
            "check it to act on several at once; a click on the row picks it alone".to_string()
        }
        Column::Glyph => row.kind.chip().to_string(),
        Column::Name => row.name.clone(),
        Column::Tags => match worn(row, tags) {
            names if names.is_empty() => "no tags".to_string(),
            names => names.join(", "),
        },
        Column::Where => row.where_.sentence().to_string(),
        Column::At => match row.at {
            Some((class, at)) => place(class, at),
            None => "it never came off a slot".to_string(),
        },
        Column::Size => format!("{} bytes", row.size),
        Column::Needs => row.needs.sentence(),
    }
}

/// What a row is labelled with. Only a kept asset wears anything: a tag hangs on a
/// workspace id, and a slot has none.
fn worn(row: &Row, tags: &Tags) -> Vec<String> {
    let Item::Local(id) = row.item else {
        return Vec::new();
    };
    tags.worn(id)
        .iter()
        .filter_map(|tag| tags.name_of(*tag))
        .map(str::to_string)
        .collect()
}

/// The strip under the table: what is checked, what sending it would do, and everything
/// that can be asked of the whole set.
fn footer(
    ui: &mut egui::Ui,
    picked: &[&Row],
    browser: &mut Browser,
    device: &DeviceState,
    queue: &Queue,
    acts: &mut Vec<Act>,
) {
    let checked: Vec<Item> = picked.iter().map(|row| row.item).collect();
    egui::Frame::new()
        .stroke(egui::Stroke::new(1.0_f32, accent(ui.visuals())))
        .inner_margin(egui::Margin::symmetric(8, 4))
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 6.0;
                ui.label(
                    egui::RichText::new(format!("{} selected", picked.len()))
                        .text_style(ui_text())
                        .strong(),
                );
                ui.label(
                    egui::RichText::new(consequence(picked, device, queue))
                        .text_style(ui_text())
                        .weak(),
                );
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.spacing_mut().button_padding.y = 0.0;
                    if ui.small_button("Review send queue").clicked() {
                        acts.push(Act::ShowPage(Page::Queue));
                    }
                    // Backwards: the strip runs right to left, so [`Bulk::ALL`]'s first
                    // action has to be drawn last to sit furthest left.
                    for action in Bulk::ALL.iter().rev() {
                        browser.bulk_item(ui, *action, &checked, acts);
                    }
                });
            });
        });
}

/// The one line the table shows when nothing survives the narrowing.
fn nothing(ui: &mut egui::Ui) {
    ui.add_space(6.0);
    ui.label(
        egui::RichText::new(
            "Nothing here — drop Nord files in, attach an instrument, or ask for less.",
        )
        .text_style(micro())
        .weak()
        .italics(),
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::browser::apply;
    use crate::log::Log;
    use crate::tabs::Tabs;
    use crate::workspace::{Fresh, Origin};

    fn at(bank: u32, slot: u32) -> Location {
        Location { bank, slot }
    }

    /// A context dressed the way `DrawbarApp::new` dresses one: the named text styles a
    /// panel resolves are installed there, on both faces.
    fn context() -> egui::Context {
        let ctx = egui::Context::default();
        ctx.all_styles_mut(crate::app::metrics);
        ctx
    }

    fn row(name: &str, kind: Kind, where_: Where, at: Option<Location>, size: u64) -> Row {
        Row {
            item: Item::Local(size),
            kind,
            name: name.to_string(),
            tags: 0,
            where_,
            at: at.map(|at| (ObjectClass::Program, at)),
            size,
            needs: Needs::Nothing,
        }
    }

    /// ⚠️ Every track shrinks and none goes negative. At the width the centre has with
    /// both docks open, the three that carry an address, a size and a dependency are
    /// still wide enough to say something.
    #[test]
    fn the_columns_share_the_width_without_overlapping_or_overflowing_it() {
        for width in [430.0_f32, 900.0] {
            let tracks = tracks(width);
            for (column, track) in Column::ALL.iter().zip(&tracks) {
                assert!(
                    track.start >= 0.0 && track.end >= track.start,
                    "{column:?} at {width}: {track:?}"
                );
                assert!(
                    track.end <= width + 0.01,
                    "{column:?} at {width}: {track:?}"
                );
            }
            for pair in tracks.windows(2) {
                assert!(
                    pair[1].start >= pair[0].end,
                    "columns overlap at {width}: {pair:?}"
                );
            }
            for column in [Column::At, Column::Size, Column::Needs] {
                let track = &tracks[column.index()];
                assert!(
                    track.end - track.start > 0.0,
                    "{column:?} vanished at {width}"
                );
            }
        }
    }

    /// A column's track is the one at its own index, or every cell after the first
    /// mismatch is painted into the column beside it.
    #[test]
    fn every_column_indexes_its_own_track() {
        for (index, column) in Column::ALL.iter().enumerate() {
            assert_eq!(column.index(), index, "{column:?}");
        }
    }

    /// A width nothing fits in still lays out: the tracks shrink together rather than
    /// running past the edge or turning negative.
    #[test]
    fn a_width_below_the_fixed_columns_shrinks_every_track_instead_of_going_negative() {
        for width in [0.0_f32, 40.0, 120.0] {
            let tracks = tracks(width);
            for (column, track) in Column::ALL.iter().zip(&tracks) {
                assert!(track.start >= 0.0, "{column:?} at {width}: {track:?}");
                assert!(track.end >= track.start, "{column:?} at {width}: {track:?}");
                assert!(
                    track.end <= width + 0.01,
                    "{column:?} at {width}: {track:?}"
                );
            }
        }
    }

    /// The omnibox and the sort compose, and neither depends on the order the rows were
    /// built in.
    #[test]
    fn the_search_narrows_and_the_column_orders_what_is_left() {
        let held = || {
            vec![
                row("Africa Split", Kind::Program, Where::Computer, None, 300),
                row(
                    "africa bass",
                    Kind::Program,
                    Where::Both(Some(false)),
                    Some(at(6, 0)),
                    100,
                ),
                row(
                    "Squabble B",
                    Kind::Live,
                    Where::Keyboard,
                    Some(at(0, 0)),
                    200,
                ),
            ]
        };
        let names = |rows: &[Row]| -> Vec<String> { rows.iter().map(|r| r.name.clone()).collect() };

        // The search is a case-insensitive substring on the name and nothing else.
        let found = arrange(held(), "AFRICA", Column::Name, Order::Up);
        assert_eq!(names(&found), ["africa bass", "Africa Split"]);
        assert!(arrange(held(), "nothing at all", Column::Name, Order::Up).is_empty());

        // And the sort is over what the search left, either way round.
        let biggest = arrange(held(), "africa", Column::Size, Order::Down);
        assert_eq!(names(&biggest), ["Africa Split", "africa bass"]);
        let smallest = arrange(held(), "africa", Column::Size, Order::Up);
        assert_eq!(names(&smallest), ["africa bass", "Africa Split"]);
    }

    /// ⚠️ A row with no address sorts after every row that has one. Treating it as `0:0`
    /// would file everything on this computer at the top of the instrument's first bank.
    #[test]
    fn a_row_with_no_address_sorts_after_every_row_that_has_one() {
        let rows = arrange(
            vec![
                row("no slot", Kind::Program, Where::Computer, None, 1),
                row("later", Kind::Program, Where::Keyboard, Some(at(6, 3)), 2),
                row("first", Kind::Program, Where::Keyboard, Some(at(0, 0)), 3),
            ],
            "",
            Column::At,
            Order::Up,
        );
        let names: Vec<&str> = rows.iter().map(|row| row.name.as_str()).collect();
        assert_eq!(names, ["first", "later", "no slot"]);
    }

    /// A kind, a place and a tag narrow together, and a thing in both places survives a
    /// filter naming either of them.
    #[test]
    fn the_kind_place_and_tag_filters_compose_over_the_row_model() {
        use crate::filter::Narrow;

        let ctx = context();
        let mut workspace = Workspace::new(ctx.clone());
        let mut device = Device::new(ctx);
        let mut log = Log::default();
        let mut tags = Tags::default();

        let bytes = {
            let id = workspace.create(Fresh::Program, &mut log).unwrap();
            let bytes = workspace.get(id).unwrap().bytes.clone();
            workspace.remove(id, &mut log);
            bytes
        };
        device.pretend_scanned(ObjectClass::Program, 7, &["Africa Split", "Squabble B"]);
        device.pretend_scanned(ObjectClass::SetList, 1, &["Sunday"]);
        // One off a scanned slot, so it is in both places, and one that never was.
        let both = workspace.ingest(
            "Africa-Split.ne5p".into(),
            Origin::Device {
                class: ObjectClass::Program,
                at: at(6, 0),
            },
            bytes,
            &mut log,
        );
        workspace.create(Fresh::Live, &mut log).unwrap();
        let sunday = tags.make("Sunday");
        tags.set(both, sunday, true);

        let names = |filter: &Filter| -> Vec<String> {
            rows(&workspace, &device.state, &tags, filter)
                .into_iter()
                .map(|row| row.name)
                .collect()
        };
        let mut filter = Filter::default();
        // The slot the kept asset came off is that asset's row, not a second one.
        assert_eq!(
            names(&filter),
            ["Africa-Split.ne5p", "untitled.ne5l", "Squabble B", "Sunday"]
        );

        filter.narrow(Narrow::Kind(Kind::Program));
        assert_eq!(names(&filter), ["Africa-Split.ne5p", "Squabble B"]);

        // In both places, so either place admits it.
        filter.narrow(Narrow::Place(Place::Computer));
        assert_eq!(names(&filter), ["Africa-Split.ne5p"]);
        filter.narrow(Narrow::Place(Place::Computer));
        filter.narrow(Narrow::Place(Place::Keyboard));
        assert_eq!(names(&filter), ["Africa-Split.ne5p", "Squabble B"]);

        // And a tag narrows what the kind and the place left.
        filter.narrow(Narrow::Tag(sunday));
        assert_eq!(names(&filter), ["Africa-Split.ne5p"]);
    }

    /// An asset off a slot the instrument still holds is in both places, and the sign is
    /// the two bodies' own checksum.
    #[test]
    fn a_kept_asset_reads_as_being_in_both_places_when_its_slot_is_still_held() {
        let ctx = egui::Context::default();
        let mut workspace = Workspace::new(ctx.clone());
        let mut device = Device::new(ctx);
        let mut log = Log::default();
        let tags = Tags::default();

        let bytes = {
            let id = workspace.create(Fresh::Program, &mut log).unwrap();
            let bytes = workspace.get(id).unwrap().bytes.clone();
            workspace.remove(id, &mut log);
            bytes
        };
        workspace.ingest(
            "Africa-Split.ne5p".into(),
            Origin::Device {
                class: ObjectClass::Program,
                at: at(6, 0),
            },
            bytes,
            &mut log,
        );
        let filter = Filter::default();
        let where_ = |device: &Device| {
            rows(&workspace, &device.state, &tags, &filter)
                .into_iter()
                .find(|row| matches!(row.item, Item::Local(_)))
                .map(|row| row.where_)
        };
        // Nothing read: the other copy cannot be spoken about at all.
        assert_eq!(where_(&device), Some(Where::Unread));

        device.pretend_scanned(ObjectClass::Program, 7, &["Africa Split"]);
        // ⚠️ `pretend_scanned` reports no checksum, so the two are known to be in both
        // places and not known to agree.
        assert_eq!(where_(&device), Some(Where::Both(None)));

        // A slot the scan found vacant leaves the asset on this computer alone.
        device.pretend_scanned(ObjectClass::Program, 7, &[""]);
        assert_eq!(where_(&device), Some(Where::Computer));
    }

    /// The sentence the footer says: where the picked rows go, how many of those slots
    /// are taken, what is already waiting, and what nothing has named.
    #[test]
    fn the_footer_says_where_a_selection_goes_and_what_it_would_replace() {
        let ctx = egui::Context::default();
        let mut device = Device::new(ctx.clone());
        let mut workspace = Workspace::new(ctx);
        let mut log = crate::log::Log::default();
        let mut queue = Queue::default();
        // Four destinations, two of them already holding something.
        device.pretend_scanned(ObjectClass::Program, 7, &["Africa Split", "Squabble B"]);

        let mut going: Vec<Row> = (0..4)
            .map(|slot| {
                row(
                    "owed",
                    Kind::Program,
                    Where::Computer,
                    Some(at(6, slot)),
                    121,
                )
            })
            .collect();
        going[0].needs = Needs::Wanted {
            class: ObjectClass::Piano,
            id: 0x0102_0304,
        };
        let picked: Vec<&Row> = going.iter().collect();
        assert_eq!(
            consequence(&picked, &device.state, &queue),
            "→ Programs 7:1–7:4 · 2 slots occupied · 1 needs a piano the instrument has not named"
        );

        // One of them is already in the queue, which the sentence says rather than
        // counting it twice over.
        let id = workspace.create(Fresh::Program, &mut log).unwrap();
        going[3].item = Item::Local(id);
        crate::queue::enqueue(
            &workspace,
            &mut device,
            &mut queue,
            &mut log,
            id,
            ObjectClass::Program,
            at(6, 3),
        );
        let picked: Vec<&Row> = going.iter().collect();
        assert!(
            consequence(&picked, &device.state, &queue).contains("1 already waiting"),
            "{}",
            consequence(&picked, &device.state, &queue)
        );

        // A row already on the instrument goes nowhere, so nothing is claimed for it.
        let there = vec![row(
            "held",
            Kind::Program,
            Where::Keyboard,
            Some(at(6, 0)),
            121,
        )];
        let mut only = there;
        only[0].item = Item::Slot {
            class: ObjectClass::Program,
            at: at(6, 0),
        };
        assert_eq!(
            consequence(&only.iter().collect::<Vec<_>>(), &device.state, &queue),
            "Nothing picked goes to the instrument."
        );
        assert_eq!(
            consequence(&[], &device.state, &queue),
            "Nothing picked goes to the instrument."
        );
    }

    /// A box is a checkbox: a plain click on one puts its row in the checked set and
    /// leaves whatever was checked before it alone, so several rows are checked with no
    /// modifier held.
    #[test]
    fn a_click_on_a_row_box_checks_it_beside_what_is_already_checked() {
        const WIDTH: f32 = 900.0;

        let ctx = context();
        let mut workspace = Workspace::new(ctx.clone());
        let device = Device::new(ctx.clone());
        let mut log = Log::default();
        let mut browser = Browser::default();
        let mut library = Library::default();
        let queue = Queue::default();
        let shell = Shell::default();
        for kind in [Fresh::Program, Fresh::Live, Fresh::Settings] {
            workspace.create(kind, &mut log).unwrap();
        }

        // The table starts PAD in, and the mark is the first track inside it.
        let box_x = PAD + tracks(WIDTH - PAD)[Column::Mark.index()].start + MARK / 2.0;
        let on_box = |index: f32| egui::pos2(box_x, BAR + HEAD + ROW * (index + 0.5));
        let mut frames = Vec::new();
        for index in [0.0_f32, 1.0] {
            let press = move |pressed| egui::Event::PointerButton {
                pos: on_box(index),
                button: egui::PointerButton::Primary,
                pressed,
                modifiers: egui::Modifiers::NONE,
            };
            frames.push(vec![egui::Event::PointerMoved(on_box(index))]);
            frames.push(vec![press(true), press(false)]);
            frames.push(Vec::new());
        }
        for events in frames {
            let input = egui::RawInput {
                events,
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(WIDTH, 540.0),
                )),
                ..Default::default()
            };
            let _ = ctx.run(input, |ctx| {
                egui::CentralPanel::default()
                    .frame(egui::Frame::new())
                    .show(ctx, |ui| {
                        library.ui(ui, &mut browser, &workspace, &device, &queue, &shell);
                    });
            });
        }

        assert_eq!(
            browser.picked().items().count(),
            2,
            "both boxes were ticked, and neither click dropped the other row"
        );
    }

    /// Paint the table headlessly at the width the centre has with both docks open and
    /// at the width it has with none, and pick a row in each so the footer is drawn too.
    ///
    /// Nothing checks pixels. What this catches is a layout that panics, an id that
    /// collides, or a track that a row paints past.
    #[test]
    fn the_table_paints_at_every_width_the_centre_has() {
        let ctx = context();
        let mut workspace = Workspace::new(ctx.clone());
        let mut device = Device::new(ctx.clone());
        let mut log = Log::default();
        let mut tabs = Tabs::default();
        let mut browser = Browser::default();
        let mut library = Library::default();
        let mut queue = Queue::default();
        let shell = Shell::default();

        for kind in [Fresh::Program, Fresh::Live, Fresh::Settings] {
            workspace.create(kind, &mut log).unwrap();
        }
        device.pretend_scanned(ObjectClass::Program, 7, &["Africa Split", "", "Squabble B"]);
        device.pretend_scanned(ObjectClass::Piano, 1, &["Royal Grand 3D"]);
        device.pretend_scanned(ObjectClass::SetList, 1, &["Sunday"]);

        // The first row's middle: the bar, the head, and half a row down; and far enough
        // in to land in the name column at either width.
        let on_a_row = egui::pos2(60.0, BAR + HEAD + ROW / 2.0);
        let press = |pressed| egui::Event::PointerButton {
            pos: on_a_row,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: egui::Modifiers::NONE,
        };
        for width in [430.0_f32, 900.0] {
            // The pointer moves, then presses, then the next frame has a row picked and
            // draws the footer under the table.
            let frames: [Vec<egui::Event>; 4] = [
                Vec::new(),
                vec![egui::Event::PointerMoved(on_a_row)],
                vec![press(true), press(false)],
                Vec::new(),
            ];
            for events in frames {
                let input = egui::RawInput {
                    events,
                    screen_rect: Some(egui::Rect::from_min_size(
                        egui::Pos2::ZERO,
                        egui::vec2(width, 540.0),
                    )),
                    ..Default::default()
                };
                let _ = ctx.run(input, |ctx| {
                    // The frame the centre actually uses: panels own their own padding.
                    egui::CentralPanel::default()
                        .frame(egui::Frame::new())
                        .show(ctx, |ui| {
                            let acts =
                                library.ui(ui, &mut browser, &workspace, &device, &queue, &shell);
                            apply(
                                &mut browser,
                                &mut Shell::default(),
                                acts,
                                &mut workspace,
                                &mut device,
                                &mut tabs,
                                &mut queue,
                                &mut log,
                            );
                        });
                });
            }
            assert!(
                browser.picked().sole().is_some(),
                "a click on a row picked it at {width}"
            );
        }
    }
}
