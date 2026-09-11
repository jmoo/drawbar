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
use nord_format::accept::Family;
use nord_usb::wire::ProgramInfo;
use nord_usb::{Location, ObjectClass};

use crate::app::{accent, micro, ui as ui_text, warn};
use crate::browser::{cell_ink, families_present, qualified, Act, Browser, Bulk, Item, Kind};
use crate::device::{fit, sendable, Device, DeviceState};
use crate::filter::{Filter, Narrow, Place, State};
use crate::icon::{icon, painted, Glyph};
use crate::panel::Track;
use crate::queue::{Diff, Queue};
use crate::shell::{Page, Shell};
use crate::strings::{folder, place, shown};
use crate::tags::Tags;
use crate::workspace::{LocalEntity, Workspace};

/// Which of the two places a row's contents are in.
///
/// ⚠️ `Both` is a **link**: a slot this asset was matched to — see
/// [`crate::device::link`]. Its sign says whether the two still agree, which saving an
/// edit here turns to `false` while the link stays where it was.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Where {
    /// A link, and whether the two still agree — see [`agrees`]. `None` is a link the
    /// two places cannot be compared across.
    Both(Option<bool>),
    Computer,
    /// On this computer, and the attached instrument is not the one whose files these
    /// are. It is in one place and can only stay there.
    Foreign,
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
            Where::Computer | Where::Foreign => "computer",
            Where::Keyboard => "keyboard",
            Where::Unread => "—",
        }
    }

    /// The whole of it, which is what the tooltip says.
    pub fn sentence(self) -> &'static str {
        match self {
            Where::Both(Some(true)) => "On this computer and in a slot holding these very bytes.",
            Where::Both(Some(false)) => {
                "On this computer and in a slot it was matched to, and the two bodies \
                 no longer agree."
            }
            Where::Both(None) => {
                "On this computer and in a slot matched by name. Nothing here can say \
                 whether the two bodies agree."
            }
            Where::Computer => "On this computer only.",
            Where::Foreign => "On this computer only — not for this keyboard.",
            Where::Keyboard => "On the instrument only.",
            Where::Unread => "It came off a slot this session has not read.",
        }
    }

    /// The places it is in, which is what a place filter asks about.
    fn places(self) -> &'static [Place] {
        match self {
            Where::Both(_) => &[Place::Computer, Place::Keyboard],
            Where::Computer | Where::Foreign | Where::Unread => &[Place::Computer],
            Where::Keyboard => &[Place::Keyboard],
        }
    }

    /// Both first, then this computer, the instrument, and the unsayable.
    fn rank(self) -> u8 {
        match self {
            Where::Both(Some(false)) => 0,
            Where::Both(None) => 1,
            Where::Both(Some(true)) => 2,
            Where::Computer => 3,
            Where::Foreign => 4,
            Where::Keyboard => 5,
            Where::Unread => 6,
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
                "This names a {} the instrument has not listed by id ({id:#010x}). Only the \
                 instrument can put a name to one, and only for a slot it has been asked \
                 about.",
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
    /// The family to put in front of the kind's word, where the word alone would not say
    /// whose files these are. [`crate::browser::qualified`] is the rule.
    pub family: Option<Family>,
    pub name: String,
    pub tags: usize,
    /// It holds something other than what it was last saved as. Only a row on this
    /// computer can: a slot holds what it holds.
    pub unsaved: bool,
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
pub fn rows(
    workspace: &Workspace,
    device: &DeviceState,
    queue: &Queue,
    tags: &Tags,
    filter: &Filter,
) -> Vec<Row> {
    let mut rows = Vec::new();
    let mut claimed: Vec<(ObjectClass, Location)> = Vec::new();
    let kept = families_present(workspace);
    let instrument = device.product().and_then(Family::from_product);
    for entity in workspace.listed() {
        // The slot the row stands for, so the instrument's own list does not repeat it.
        if let Some(slot) = entity.spot() {
            claimed.push(slot);
        }
        let worn = tags.worn(entity.id);
        let row = local(entity, device, queue, worn.len(), &kept, instrument);
        let state = state(row.item, row.where_, queue);
        if admits(filter, &row, worn, state) {
            rows.push(row);
        }
    }
    let untagged = BTreeSet::new();
    for class in device.classes() {
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
                if admits(filter, &row, &untagged, None) {
                    rows.push(row);
                }
            }
        }
    }
    rows
}

/// Whether a row survives the narrowing. A row in both places survives a filter naming
/// either of them.
fn admits(filter: &Filter, row: &Row, tags: &BTreeSet<u64>, state: Option<State>) -> bool {
    row.where_
        .places()
        .iter()
        .any(|place| filter.admits(row.kind, *place, tags, state))
}

/// What a row wants doing about it: a write already waiting, or a slot that no longer
/// holds what this row was last saved as. A slot on the instrument wants nothing —
/// it *is* what the instrument holds.
///
/// ⚠️ A queued row is waiting rather than differing, however the two bodies compare. The
/// write already agreed to is what settles them, so counting it under both would ask
/// twice for one thing.
pub fn state(item: Item, where_: Where, queue: &Queue) -> Option<State> {
    match item {
        Item::Local(id) if queue.holds(id) => Some(State::Waiting),
        Item::Local(_) => (where_ == Where::Both(Some(false))).then_some(State::Differs),
        _ => None,
    }
}

/// How many rows the two places hold differently with no write waiting to settle it —
/// the count the tree's row and the library's chip both carry.
pub fn differing(workspace: &Workspace, device: &DeviceState, queue: &Queue) -> usize {
    workspace
        .listed()
        .filter(|entity| {
            state(
                Item::Local(entity.id),
                whereabouts(entity, device, queue),
                queue,
            ) == Some(State::Differs)
        })
        .count()
}

/// The row one item makes, for a caller holding an item rather than the whole table.
///
/// The same two builders [`rows`] uses, so what the inspector reads about a picked row
/// is what the table shows on it. A folder or a tag is a grouping rather than a thing,
/// and makes no row.
pub fn row_of(
    item: Item,
    workspace: &Workspace,
    device: &DeviceState,
    queue: &Queue,
    tags: &Tags,
) -> Option<Row> {
    match item {
        Item::Local(id) => {
            let entity = workspace.get(id)?;
            let instrument = device.product().and_then(Family::from_product);
            Some(local(
                entity,
                device,
                queue,
                tags.worn(id).len(),
                &families_present(workspace),
                instrument,
            ))
        }
        Item::Slot { class, at } => {
            Some(slot(class, at, device.slot(class, at).flatten()?, device))
        }
        Item::Folder(_) | Item::Tag(_) => None,
    }
}

fn local(
    entity: &LocalEntity,
    device: &DeviceState,
    queue: &Queue,
    tags: usize,
    kept: &[Family],
    instrument: Option<Family>,
) -> Row {
    let family = Family::of_tag(&entity.tag());
    Row {
        item: Item::Local(entity.id),
        kind: Kind::of(entity.entity.as_ref()),
        family: qualified(kept, family, instrument)
            .then_some(family)
            .flatten(),
        name: entity.name.clone(),
        tags,
        unsaved: entity.is_unsaved(),
        where_: whereabouts(entity, device, queue),
        at: entity.spot(),
        size: entity.bytes.len() as u64,
        needs: wanted(entity, device),
    }
}

fn slot(class: ObjectClass, at: Location, info: &ProgramInfo, device: &DeviceState) -> Row {
    Row {
        item: Item::Slot { class, at },
        kind: Kind::from_class(class),
        // What is on the instrument is the instrument's own; the word never needs it.
        family: None,
        name: info.name.trim().to_string(),
        tags: 0,
        unsaved: false,
        where_: Where::Keyboard,
        at: Some((class, at)),
        size: u64::from(info.body_len),
        needs: played(class, at, device),
    }
}

/// Where a row's contents are, which for anything on this computer is decided by its
/// link: a linked asset is in both places, and the sign is [`agrees`].
fn whereabouts(entity: &LocalEntity, device: &DeviceState, queue: &Queue) -> Where {
    let held = entity
        .link
        .and_then(|(class, at)| Some((class, device.slot(class, at).flatten()?)));
    let Some((class, info)) = held else {
        if !fit(device, entity).allowed() {
            return Where::Foreign;
        }
        return match entity.origin.slot() {
            Some((class, at)) if device.slot(class, at).is_none() => Where::Unread,
            _ => Where::Computer,
        };
    };
    Where::Both(agrees(entity, class, info, queue))
}

/// Whether the slot an asset stands for still holds what that asset was last saved as.
///
/// The one comparison behind the library's sign, the tree's dot and what a "queue
/// changed" walks. Equality is claimed only where something says so:
///
/// - the checksum a walk reported for the slot against the checksum of the saved bytes,
///   which is read once at ingest and nothing hashes again;
/// - a write this app made into that slot, whose bytes are the ones it is saved as —
///   the one thing it knows about a slot without reading it back;
/// - a compare read that fetched the occupant and found the bodies equal.
///
/// ⚠️ `None` where none of them answers, which is *not known* rather than *the same*.
/// An address and a length are not evidence: a class reporting no checksum is linked by
/// name or by being the only slot there is, and neither says anything about the body in
/// it. Only a read settles one of those.
pub fn agrees(
    entity: &LocalEntity,
    class: ObjectClass,
    info: &ProgramInfo,
    queue: &Queue,
) -> Option<bool> {
    if let (Some(here), Some(there)) = (entity.saved.crc32, info.crc32) {
        return Some(here == there);
    }
    if wrote(entity, class, info.location) {
        return Some(true);
    }
    match queue.entry(entity.id).map(|held| &held.diff) {
        Some(Diff::Identical) => Some(true),
        Some(Diff::Fields(_) | Diff::Bytes { .. }) => Some(false),
        _ => None,
    }
}

/// Whether this app wrote what the asset is saved as into this very slot.
///
/// ⚠️ The checksums are the two sets of bytes: a save of anything else moves the
/// baseline off the bytes the write put there, and the write stops answering for it.
fn wrote(entity: &LocalEntity, class: ObjectClass, at: Location) -> bool {
    entity.wrote.is_some_and(|wrote| {
        (wrote.class, wrote.at) == (class, at) && Some(wrote.crc32) == entity.saved.crc32
    })
}

/// The mark a local row wears at its right end: what the attached instrument holds where
/// this asset stands.
///
/// ⚠️ The one rule, and the only dot a local row wears. `good` is a slot holding what
/// this asset was last saved as; `warn` is one holding something else, or a write
/// already waiting to change it; the caption ink is a slot nothing can say either way
/// about, which is the unsigned `both` of [`Where::Both`]; nothing at all is an asset
/// with no slot to stand on.
pub fn keyboard_mark(
    entity: &LocalEntity,
    device: &DeviceState,
    queue: &Queue,
    visuals: &egui::Visuals,
) -> Option<egui::Color32> {
    let (class, at) = entity.spot()?;
    let info = device.slot(class, at).flatten()?;
    if queue.holds(entity.id) {
        return Some(warn(visuals));
    }
    match agrees(entity, class, info, queue) {
        Some(true) => Some(crate::app::good(visuals)),
        Some(false) => Some(warn(visuals)),
        // A green dot is a claim, and nothing here has the evidence to make one.
        None => Some(crate::app::caption(visuals)),
    }
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
    let mine = rows.iter().filter(|row| matches!(row.item, Item::Local(_)));
    let (fits, of) = mine.fold((0, 0), |(fits, of), row| {
        (fits + usize::from(row.where_ != Where::Foreign), of + 1)
    });
    if let Some(unfit) = device
        .product()
        .and_then(|product| crate::strings::fitting(fits, of, product))
    {
        said.push(unfit);
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

/// The destinations as one run per folder: `Programs 7:1–7:4`, in the order `going` is
/// sorted into.
fn spans(going: &[(ObjectClass, Location)]) -> String {
    let mut folders: Vec<ObjectClass> = going.iter().map(|(class, _)| *class).collect();
    folders.dedup();
    let mut runs: Vec<String> = Vec::new();
    for class in folders {
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
        let held = rows(
            workspace,
            &device.state,
            queue,
            browser.tags(),
            &shell.filter,
        );
        let held = arrange(held, &shell.omnibox, self.by, self.order);

        let counts = [queue.len(), differing(workspace, &device.state, queue)];
        bar(ui, counts, browser.tags(), &shell.filter, &mut acts);
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
                    footer(
                        ui,
                        &picked,
                        browser,
                        workspace,
                        &device.state,
                        queue,
                        &mut acts,
                    )
                });
        }
        self.table(ui, &held, browser, workspace, device, queue, &mut acts);
        acts
    }

    #[allow(clippy::too_many_arguments)]
    fn table(
        &mut self,
        ui: &mut egui::Ui,
        rows: &[Row],
        browser: &mut Browser,
        workspace: &Workspace,
        device: &Device,
        queue: &Queue,
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
        let shown = egui::ScrollArea::vertical()
            .id_salt("library_table")
            .auto_shrink([false; 2])
            .show_rows(ui, ROW, rows.len(), |ui, shown| {
                for row in shown.filter_map(|index| rows.get(index)) {
                    paint(
                        ui, row, width, &tracks, browser, &list, workspace, device, queue, acts,
                    );
                }
            });
        // The room under the last row: a click there is a click on no row, which lets go
        // of everything picked.
        let rest = shown
            .inner_rect
            .with_min_y(shown.inner_rect.top() + shown.content_size.y);
        if rest.height() > 0.0
            && ui
                .interact(rest, ui.id().with("library_empty"), egui::Sense::click())
                .clicked()
        {
            browser.unpick();
        }
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
///
/// ⚠️ Both counts are the whole list's, not this view's. They are the numbers the tree's
/// own rows carry and the toolbar's Send acts on — and a chip counting only what survives
/// its own narrowing would report a different number the moment it was clicked.
fn bar(ui: &mut egui::Ui, counts: [usize; 2], tags: &Tags, filter: &Filter, acts: &mut Vec<Act>) {
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
        let states = [
            (State::Waiting, Glyph::Clock),
            (State::Differs, Glyph::CircleAlert),
        ];
        for ((state, glyph), count) in states.into_iter().zip(counts) {
            // A chip that has gone to nothing stays while it is the one narrowing, so
            // there is always something left to click to widen the table again.
            if count == 0 && !filter.on(Narrow::State(state)) {
                continue;
            }
            let text = format!("{count} {}", state.word());
            let drawn = chip(ui, glyph, &text, warn(ui.visuals()));
            let picked = ui
                .interact(
                    drawn.rect,
                    drawn.id.with(state.word()),
                    egui::Sense::click(),
                )
                .on_hover_text(state.sentence());
            if picked.clicked() {
                acts.push(Act::Narrow(Narrow::State(state)));
            }
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
    if let Some(state) = filter.state {
        narrowed.push(state.word().to_string());
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
    queue: &Queue,
    acts: &mut Vec<Act>,
) {
    let selected = browser.picked().holds(row.item);
    let (rect, response) =
        ui.allocate_exact_size(egui::vec2(width, ROW), egui::Sense::click_and_drag());
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
    let write =
        |box_: egui::Rect, text: &str, font: egui::FontId, tint: egui::Color32, italics: bool| {
            let mut job = egui::text::LayoutJob::default();
            job.append(
                text,
                0.0,
                egui::TextFormat {
                    font_id: font,
                    color: tint,
                    italics,
                    ..egui::TextFormat::default()
                },
            );
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
        &crate::browser::starred(&row.name, row.unsaved),
        egui::FontId::proportional(NAME),
        ink,
        row.unsaved,
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
            false,
        );
    }
    // The same mark the tree's dot paints, in the words this column carries.
    let mark = row
        .item
        .local()
        .and_then(|id| workspace.get(id))
        .and_then(|entity| keyboard_mark(entity, &device.state, queue, &visuals));
    write(
        cell(Column::Where),
        row.where_.short(),
        egui::FontId::proportional(NAME - 1.0),
        match mark {
            Some(tint) => cell_ink(selected, tint, &visuals),
            None => quiet,
        },
        false,
    );
    if let Some((_, at)) = row.at {
        write(
            cell(Column::At),
            &shown(at),
            egui::FontId::monospace(MONO),
            quiet,
            false,
        );
    }
    write(
        cell(Column::Size),
        &crate::room::measure(row.size),
        egui::FontId::monospace(MONO),
        quiet,
        false,
    );
    // ⚠️ Quiet, id and all. An id nothing has resolved is a question nobody has asked
    // the instrument, not a library reported missing.
    write(
        cell(Column::Needs),
        &row.needs.text(),
        egui::FontId::proportional(NAME - 1.0),
        quiet,
        false,
    );

    // A row of the table is dragged like a row of the tree: the same payload, so it
    // lands on the same targets and means the same thing there.
    if response.dragged() {
        if let Some(head) = browser.held(row.item, workspace, &device.state) {
            let carried = browser.carrying(head, &row.name, workspace, &device.state);
            egui::DragAndDrop::set_payload(ui.ctx(), carried);
        }
    }

    let checked = checkbox
        .map(|box_| {
            box_.on_hover_text(tooltip(
                row,
                Column::Mark,
                browser.tags(),
                workspace,
                device,
            ))
        })
        .is_some_and(|box_| box_.clicked());
    let response = match under(&response, rect, tracks) {
        Some(column) => {
            response.on_hover_text(tooltip(row, column, browser.tags(), workspace, device))
        }
        None => response,
    };
    if checked {
        browser.check(row.item);
    } else if response.double_clicked() {
        acts.push(Act::Open(row.item));
    } else if response.clicked() {
        browser.pick(ui, row.item, list);
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
/// Two columns grow a fact the row does not carry: the tags column holds a count and the
/// hover is where the names are, and the address is one of possibly several slots holding
/// these very bytes. Both are worked out for the hovered row alone.
fn tooltip(
    row: &Row,
    column: Column,
    tags: &Tags,
    workspace: &Workspace,
    device: &Device,
) -> String {
    match column {
        Column::Mark => {
            "check it to act on several at once; a click on the row picks it alone".to_string()
        }
        Column::Glyph => crate::strings::kind_word(row.kind, row.family),
        Column::Name => row.name.clone(),
        Column::Tags => match worn(row, tags) {
            names if names.is_empty() => "no tags".to_string(),
            names => names.join(", "),
        },
        Column::Where => row.where_.sentence().to_string(),
        Column::At => match row.at {
            Some((class, at)) => {
                let where_ = place(class, at);
                match (row.where_, also_holding(row, workspace, device)) {
                    // A class reporting no checksum is linked by the name it gave this
                    // asset, so the address is where the name is rather than where the
                    // body is.
                    (Where::Both(None), _) => format!("{where_}, matched by name"),
                    (_, 0) => where_,
                    (_, more) => format!("{where_}, and {more} more hold the same bytes"),
                }
            }
            None => "it never came off a slot".to_string(),
        },
        Column::Size => format!("{} bytes", row.size),
        Column::Needs => row.needs.sentence(),
    }
}

/// How many slots beyond the linked one hold this row's own bytes. A row that is a slot
/// is not linked to anything and answers zero.
fn also_holding(row: &Row, workspace: &Workspace, device: &Device) -> usize {
    row.item
        .local()
        .and_then(|id| workspace.get(id))
        .map_or(0, |entity| {
            crate::device::also_holding(&device.state, entity)
        })
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
#[allow(clippy::too_many_arguments)]
fn footer(
    ui: &mut egui::Ui,
    picked: &[&Row],
    browser: &mut Browser,
    workspace: &Workspace,
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
                ui.scope(|ui| {
                    crate::panel::flat(ui);
                    if ui
                        .add(egui::Button::new(
                            egui::RichText::new("clear").text_style(ui_text()),
                        ))
                        .on_hover_text("let go of everything picked — or press Escape")
                        .clicked()
                    {
                        browser.unpick();
                    }
                });
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
                        browser.bulk_item(ui, *action, &checked, workspace, device, acts);
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
            family: None,
            name: name.to_string(),
            tags: 0,
            unsaved: false,
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
        device.pretend_partitions(&crate::device::ELECTRO5);
        device.pretend_scanned(ObjectClass::SetList, 1, &["Sunday"]);
        // One linked to the slot holding its bytes, so it is in both places, and one
        // that is on this computer alone.
        let both = workspace.ingest(
            "Africa-Split.ne5p".into(),
            Origin::Device {
                class: ObjectClass::Program,
                at: at(6, 0),
            },
            bytes,
            &mut log,
        );
        let crc = workspace
            .get(both)
            .and_then(|entity| entity.saved.crc32)
            .expect("every CBIN container has one");
        device.pretend_bodies(
            ObjectClass::Program,
            7,
            &[Some(("Africa Split", crc)), Some(("Squabble B", crc ^ 1))],
        );
        device.relink(&mut workspace);
        workspace.create(Fresh::Live, &mut log).unwrap();
        let sunday = tags.make("Sunday");
        tags.set(both, sunday, true);

        let names = |filter: &Filter| -> Vec<String> {
            rows(&workspace, &device.state, &Queue::default(), &tags, filter)
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

    /// A linked asset is in both places, and the sign says whether the slot still
    /// reports the asset's own checksum — which an edit here turns over while the link
    /// stays where it was.
    #[test]
    fn a_linked_asset_is_in_both_places_and_says_when_the_two_stop_agreeing() {
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
        let id = workspace.ingest(
            "Africa-Split.ne5p".into(),
            Origin::Device {
                class: ObjectClass::Program,
                at: at(6, 0),
            },
            bytes.clone(),
            &mut log,
        );
        let crc = workspace
            .get(id)
            .and_then(|entity| entity.saved.crc32)
            .expect("every CBIN container has one");
        let filter = Filter::default();
        let where_ = |workspace: &Workspace, device: &Device| {
            rows(workspace, &device.state, &Queue::default(), &tags, &filter)
                .into_iter()
                .find(|row| matches!(row.item, Item::Local(_)))
                .map(|row| row.where_)
        };
        // Nothing read: the other copy cannot be spoken about at all.
        device.relink(&mut workspace);
        assert_eq!(where_(&workspace, &device), Some(Where::Unread));

        device.pretend_bodies(ObjectClass::Program, 7, &[Some(("Africa Split", crc))]);
        device.relink(&mut workspace);
        assert_eq!(where_(&workspace, &device), Some(Where::Both(Some(true))));

        // Edited here and not saved: the slot still holds what this was saved as, so the
        // two places have not parted.
        let (_, edited) =
            crate::fields::apply(&bytes, &[("center_panel.gain".into(), "96".into())])
                .expect("the registry takes the set");
        workspace.replace_bytes(id, edited, &mut log);
        device.relink(&mut workspace);
        assert_eq!(where_(&workspace, &device), Some(Where::Both(Some(true))));

        // Saved: the link stays put and the sign turns over.
        workspace.mark_saved(id);
        device.relink(&mut workspace);
        assert_eq!(
            workspace.get(id).unwrap().link,
            Some((ObjectClass::Program, at(6, 0)))
        );
        assert_eq!(where_(&workspace, &device), Some(Where::Both(Some(false))));

        // A slot the scan found vacant holds nothing to link to.
        device.pretend_bodies(ObjectClass::Program, 7, &[None]);
        device.relink(&mut workspace);
        assert_eq!(where_(&workspace, &device), Some(Where::Computer));
    }

    /// Equality is claimed only where something says so. A settings folder holds one
    /// slot and that slot reports no checksum, so an asset matched to it stands in both
    /// places with nothing said about the two bodies — until a read fetches the occupant,
    /// or this app writes the bytes there itself.
    #[test]
    fn a_slot_reporting_no_checksum_says_nothing_until_it_is_read_or_written() {
        let ctx = context();
        let mut workspace = Workspace::new(ctx.clone());
        let mut device = Device::new(ctx);
        let mut log = Log::default();
        let mut queue = Queue::default();
        let tags = Tags::default();
        let visuals = egui::Visuals::dark();
        let class = ObjectClass::Settings;
        let held_at = at(6, 0);

        let id = workspace.create(Fresh::Settings, &mut log).unwrap();
        let bytes = workspace.get(id).unwrap().bytes.clone();
        let held = workspace.get(id).unwrap();
        let crc = held.saved.crc32.expect("a container");
        let body_len = held.container.as_ref().expect("a container").body_len;

        // The walk reports what such a slot reports: a name and a length, and no
        // checksum at all — the length being this asset's own.
        device.pretend_attached();
        device.pretend(crate::device::DeviceEvent::BankScanned {
            class,
            bank: 7,
            slots: vec![Some(ProgramInfo {
                location: held_at,
                body_len: u32::try_from(body_len).unwrap(),
                format: "ne5s".into(),
                version: 1,
                crc32: None,
                name: "Live Settings".into(),
            })],
        });
        device.poll(&mut log, &mut workspace, &mut Tabs::default(), &mut queue);
        assert_eq!(
            workspace.get(id).unwrap().link,
            Some((class, held_at)),
            "the one slot the folder has"
        );

        let said = |workspace: &Workspace, device: &Device, queue: &Queue| {
            let entity = workspace.get(id).expect("it is on the list");
            let info = device
                .state
                .slot(class, held_at)
                .flatten()
                .expect("the slot was scanned");
            (
                agrees(entity, class, info, queue),
                rows(workspace, &device.state, queue, &tags, &Filter::default())
                    .into_iter()
                    .find(|row| matches!(row.item, Item::Local(_)))
                    .map(|row| row.where_),
                keyboard_mark(entity, &device.state, queue, &visuals),
            )
        };

        assert_eq!(
            said(&workspace, &device, &queue),
            (
                None,
                Some(Where::Both(None)),
                Some(crate::app::caption(&visuals))
            ),
            "an address and a length are not a body"
        );

        // A compare read fetched the occupant, and the two bodies are the same bytes.
        crate::queue::enqueue(
            &workspace,
            &mut device,
            &mut queue,
            &mut log,
            id,
            class,
            held_at,
        );
        queue.arrived(class, held_at, "Live Settings", &bytes, &workspace);
        assert_eq!(said(&workspace, &device, &queue).0, Some(true));

        // A write of this app's own, with nothing waiting to change it.
        queue.clear();
        workspace.landed(id, class, held_at, bytes.clone());
        device.relink(&mut workspace);
        assert_eq!(
            said(&workspace, &device, &queue),
            (
                Some(true),
                Some(Where::Both(Some(true))),
                Some(crate::app::good(&visuals))
            ),
            "this app put those bytes there"
        );

        // A walk that reports a checksum, and it is not this body's.
        device.pretend_bodies(class, 7, &[Some(("Live Settings", crc ^ 1))]);
        device.relink(&mut workspace);
        assert_eq!(
            said(&workspace, &device, &queue),
            (
                Some(false),
                Some(Where::Both(Some(false))),
                Some(warn(&visuals))
            ),
            "what the instrument reports outlives our own write"
        );
    }

    /// ⚠️ An Electro 5 factory program is a type-0 file, and so is every copy Nord Sound
    /// Manager exports from one. Its header carries no body checksum, so nothing links
    /// it to the slot it came off unless that checksum is hashed from the body.
    #[test]
    fn a_type_0_asset_links_to_the_slot_reporting_its_body_checksum() {
        let ctx = context();
        let mut workspace = Workspace::new(ctx.clone());
        let mut device = Device::new(ctx);
        let mut log = Log::default();
        let (queue, tags) = (Queue::default(), Tags::default());
        let visuals = egui::Visuals::dark();
        let held_at = at(6, 0);

        let bytes = {
            let id = workspace.create(Fresh::Program, &mut log).unwrap();
            let bytes = crate::workspace::as_type_0(&workspace.get(id).unwrap().bytes);
            workspace.remove(id, &mut log);
            bytes
        };
        let id = workspace.ingest(
            "Circling Bells.ne5p".into(),
            Origin::File("Circling Bells.ne5p".into()),
            bytes,
            &mut log,
        );
        let crc = workspace
            .get(id)
            .and_then(|entity| entity.saved.crc32)
            .expect("hashed from the body the header does not checksum");

        device.pretend_bodies(
            ObjectClass::Program,
            7,
            &[Some(("Circling Bells", crc)), Some(("Squabble B", crc ^ 1))],
        );
        device.relink(&mut workspace);

        assert_eq!(
            workspace.get(id).unwrap().link,
            Some((ObjectClass::Program, held_at))
        );
        let where_ = rows(&workspace, &device.state, &queue, &tags, &Filter::default())
            .into_iter()
            .find(|row| matches!(row.item, Item::Local(_)))
            .map(|row| row.where_);
        assert_eq!(where_, Some(Where::Both(Some(true))));
        assert_eq!(
            keyboard_mark(workspace.get(id).unwrap(), &device.state, &queue, &visuals),
            Some(crate::app::good(&visuals))
        );
    }

    /// A link is matched on the saved bytes, which is what the sign and the dot are
    /// already read against. An asset holding an edit nothing has saved still finds the
    /// slot holding what it was saved as, however far its bytes have moved since; saving
    /// that edit takes the baseline off the slot and turns both signs over, leaving the
    /// link where it was matched.
    #[test]
    fn an_unsaved_edit_still_links_to_the_slot_holding_the_saved_bytes() {
        let ctx = context();
        let mut workspace = Workspace::new(ctx.clone());
        let mut device = Device::new(ctx);
        let mut log = Log::default();
        let (queue, tags) = (Queue::default(), Tags::default());
        let visuals = egui::Visuals::dark();
        let held_at = at(6, 0);

        let id = workspace.create(Fresh::Program, &mut log).unwrap();
        let bytes = workspace.get(id).unwrap().bytes.clone();
        let saved_as = workspace.get(id).unwrap().saved.crc32.unwrap();

        // Edited before anything is read, so there is no earlier link to fall back on.
        let (_, edited) =
            crate::fields::apply(&bytes, &[("center_panel.gain".into(), "96".into())])
                .expect("the registry takes the set");
        workspace.replace_bytes(id, edited, &mut log);
        assert!(workspace.get(id).unwrap().is_unsaved());
        assert_ne!(
            workspace
                .get(id)
                .unwrap()
                .container
                .as_ref()
                .map(|held| held.body_crc32),
            Some(saved_as),
            "the edit moved the bytes it holds now"
        );

        let where_ = |workspace: &Workspace, device: &Device| {
            rows(workspace, &device.state, &queue, &tags, &Filter::default())
                .into_iter()
                .find(|row| matches!(row.item, Item::Local(_)))
                .map(|row| row.where_)
        };
        let mark = |workspace: &Workspace, device: &Device| {
            keyboard_mark(workspace.get(id).unwrap(), &device.state, &queue, &visuals)
        };

        device.pretend_bodies(ObjectClass::Program, 7, &[Some(("Africa Split", saved_as))]);
        device.relink(&mut workspace);
        assert_eq!(
            workspace.get(id).unwrap().link,
            Some((ObjectClass::Program, held_at)),
            "the slot holds what this was saved as"
        );
        assert_eq!(
            crate::device::also_holding(&device.state, workspace.get(id).unwrap()),
            0,
            "the one slot holding it is the one it is linked to"
        );
        assert_eq!(mark(&workspace, &device), Some(crate::app::good(&visuals)));
        assert_eq!(where_(&workspace, &device), Some(Where::Both(Some(true))));

        workspace.mark_saved(id);
        device.relink(&mut workspace);
        assert_eq!(
            workspace.get(id).unwrap().link,
            Some((ObjectClass::Program, held_at)),
            "no slot holds the new baseline, and where it stands is where it stands"
        );
        assert_eq!(mark(&workspace, &device), Some(warn(&visuals)));
        assert_eq!(where_(&workspace, &device), Some(Where::Both(Some(false))));
    }

    /// The state axis narrows the library to what wants doing about it — and a write
    /// already waiting takes its row out of "differs" and into "waiting", so one thing
    /// to do is asked for once.
    #[test]
    fn a_row_waiting_to_be_sent_is_not_also_one_that_differs() {
        use crate::filter::{Narrow, State};

        let ctx = context();
        let mut workspace = Workspace::new(ctx.clone());
        let mut device = Device::new(ctx);
        let mut log = Log::default();
        let (tags, mut queue) = (Tags::default(), Queue::default());

        let id = workspace.create(Fresh::Program, &mut log).unwrap();
        let bytes = workspace.get(id).unwrap().bytes.clone();
        let crc = workspace
            .get(id)
            .and_then(|entity| entity.saved.crc32)
            .expect("every CBIN container has one");
        device.pretend_bodies(ObjectClass::Program, 7, &[Some(("Africa Split", crc))]);
        device.relink(&mut workspace);
        // Edited and saved: the link stays where it was and the two bodies part.
        let (_, edited) =
            crate::fields::apply(&bytes, &[("center_panel.gain".into(), "96".into())])
                .expect("the registry takes the set");
        workspace.replace_bytes(id, edited, &mut log);
        workspace.mark_saved(id);
        device.relink(&mut workspace);

        fn narrowed(
            workspace: &Workspace,
            device: &Device,
            queue: &Queue,
            tags: &Tags,
            state: crate::filter::State,
        ) -> usize {
            let mut filter = Filter::default();
            filter.narrow(Narrow::State(state));
            rows(workspace, &device.state, queue, tags, &filter)
                .into_iter()
                .filter(|row| matches!(row.item, Item::Local(_)))
                .count()
        }
        assert_eq!(differing(&workspace, &device.state, &queue), 1);
        assert_eq!(
            narrowed(&workspace, &device, &queue, &tags, State::Differs),
            1
        );
        assert_eq!(
            narrowed(&workspace, &device, &queue, &tags, State::Waiting),
            0
        );

        crate::queue::enqueue(
            &workspace,
            &mut device,
            &mut queue,
            &mut log,
            id,
            ObjectClass::Program,
            at(6, 0),
        );
        assert!(queue.holds(id), "it is owed back to the slot it came off");
        assert_eq!(
            differing(&workspace, &device.state, &queue),
            0,
            "the write already agreed to is what settles the two"
        );
        assert_eq!(
            narrowed(&workspace, &device, &queue, &tags, State::Waiting),
            1
        );
        assert_eq!(
            narrowed(&workspace, &device, &queue, &tags, State::Differs),
            0
        );
    }

    /// A class whose slots report no checksum is linked by name, and the table says so:
    /// the two places are named without a sign between them, and the address says what
    /// matched them.
    #[test]
    fn a_class_linked_by_name_reads_both_without_a_sign() {
        let ctx = context();
        let mut workspace = Workspace::new(ctx.clone());
        let mut device = Device::new(ctx);
        let mut log = Log::default();
        let (tags, filter, queue) = (Tags::default(), Filter::default(), Queue::default());

        workspace.create(Fresh::Settings, &mut log).unwrap();
        device.pretend_scanned(ObjectClass::Settings, 1, &["Settings"]);
        device.relink(&mut workspace);

        let row = rows(&workspace, &device.state, &queue, &tags, &filter)
            .into_iter()
            .find(|row| matches!(row.item, Item::Local(_)))
            .expect("the settings row");
        assert_eq!(row.where_, Where::Both(None));
        assert_eq!(row.where_.short(), "both");
        assert!(
            tooltip(&row, Column::At, &tags, &workspace, &device).ends_with("matched by name"),
            "{}",
            tooltip(&row, Column::At, &tags, &workspace, &device)
        );
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

    /// Every word one frame painted, wherever in the tree of shapes it ended up.
    fn painted(output: &egui::FullOutput) -> Vec<String> {
        fn words(shape: &egui::Shape, into: &mut Vec<String>) {
            match shape {
                egui::Shape::Text(text) => into.push(text.galley.text().to_string()),
                egui::Shape::Vec(shapes) => shapes.iter().for_each(|shape| words(shape, into)),
                _ => {}
            }
        }
        let mut said = Vec::new();
        for clipped in &output.shapes {
            words(&clipped.shape, &mut said);
        }
        said
    }

    /// One mark, four states: no slot to stand on, a slot holding what this was saved
    /// as, a slot holding something else, and a write already waiting to change it.
    #[test]
    fn the_keyboard_mark_says_what_the_instrument_holds_where_this_stands() {
        let ctx = context();
        let mut workspace = Workspace::new(ctx.clone());
        let mut device = Device::new(ctx.clone());
        let mut log = Log::default();
        let mut queue = Queue::default();
        let visuals = egui::Visuals::dark();
        let (good, warn) = (crate::app::good(&visuals), warn(&visuals));

        let bytes = {
            let id = workspace.create(Fresh::Program, &mut log).unwrap();
            let bytes = workspace.get(id).unwrap().bytes.clone();
            workspace.remove(id, &mut log);
            bytes
        };
        let off = |workspace: &mut Workspace, slot: u32, log: &mut Log| {
            workspace.ingest(
                format!("off-{slot}.ne5p"),
                Origin::Device {
                    class: ObjectClass::Program,
                    at: at(6, slot),
                },
                bytes.clone(),
                log,
            )
        };
        let same = off(&mut workspace, 0, &mut log);
        let other = off(&mut workspace, 1, &mut log);
        let waiting = off(&mut workspace, 2, &mut log);
        let nowhere = workspace.ingest("typed.ne5p".into(), Origin::Fresh, bytes, &mut log);
        let held = workspace.get(same).unwrap().saved.crc32.unwrap();

        let mark = |workspace: &Workspace, device: &Device, queue: &Queue, id: u64| {
            keyboard_mark(workspace.get(id).unwrap(), &device.state, queue, &visuals)
        };
        // Nothing scanned: no slot holds anything, so no row says anything about one.
        assert_eq!(mark(&workspace, &device, &queue, same), None);

        device.pretend_bodies(
            ObjectClass::Program,
            7,
            &[
                Some(("off-0", held)),
                Some(("off-1", held ^ 1)),
                Some(("off-2", held)),
            ],
        );
        assert_eq!(mark(&workspace, &device, &queue, same), Some(good));
        assert_eq!(mark(&workspace, &device, &queue, other), Some(warn));
        assert_eq!(
            mark(&workspace, &device, &queue, nowhere),
            None,
            "it came off nowhere"
        );

        // Waiting to be written wins: the slot agrees now, and is about to stop.
        assert_eq!(mark(&workspace, &device, &queue, waiting), Some(good));
        crate::queue::enqueue(
            &workspace,
            &mut device,
            &mut queue,
            &mut log,
            waiting,
            ObjectClass::Program,
            at(6, 2),
        );
        assert_eq!(mark(&workspace, &device, &queue, waiting), Some(warn));
    }

    /// An asset holding an edit nothing has saved says so where its name is written.
    #[test]
    fn an_unsaved_name_wears_a_star_in_the_table() {
        const WIDTH: f32 = 900.0;

        let ctx = context();
        let mut workspace = Workspace::new(ctx.clone());
        let device = Device::new(ctx.clone());
        let mut log = Log::default();
        let mut browser = Browser::default();
        let mut library = Library::default();
        let (queue, shell) = (Queue::default(), Shell::default());

        let id = workspace.create(Fresh::Program, &mut log).unwrap();
        workspace.rename(id, "Africa Split".into());
        let draw = |library: &mut Library, browser: &mut Browser, workspace: &Workspace| {
            let input = egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(WIDTH, 540.0),
                )),
                ..Default::default()
            };
            painted(&ctx.run(input, |ctx| {
                egui::CentralPanel::default()
                    .frame(egui::Frame::new())
                    .show(ctx, |ui| {
                        library.ui(ui, browser, workspace, &device, &queue, &shell);
                    });
            }))
        };

        let said = draw(&mut library, &mut browser, &workspace);
        assert!(said.contains(&"Africa Split".to_string()), "{said:?}");

        let bytes = workspace.get(id).unwrap().bytes.clone();
        let (_, edited) =
            crate::fields::apply(&bytes, &[("center_panel.gain".into(), "96".into())]).unwrap();
        workspace.replace_bytes(id, edited, &mut log);

        let said = draw(&mut library, &mut browser, &workspace);
        assert!(said.contains(&"Africa Split*".to_string()), "{said:?}");
    }

    /// ⚠️ A row of the table is a row of the tree: it starts the same drag, and a drop
    /// files the asset exactly as a drag from the tree's own row does. Two drag paths
    /// would be two sets of rules for one gesture.
    #[test]
    fn a_row_dragged_from_the_table_onto_a_folder_files_the_asset() {
        const WIDTH: f32 = 900.0;

        let ctx = context();
        let mut workspace = Workspace::new(ctx.clone());
        let mut device = Device::new(ctx.clone());
        let mut log = Log::default();
        let mut tabs = Tabs::default();
        let mut browser = Browser::default();
        let mut library = Library::default();
        let mut queue = Queue::default();
        let shell = Shell::default();
        let id = workspace.create(Fresh::Program, &mut log).unwrap();
        apply(
            &mut browser,
            &mut Shell::default(),
            vec![Act::NewFolder],
            &mut workspace,
            &mut device,
            &mut tabs,
            &mut queue,
            &mut log,
        );

        // The one row of the table, in its name column; and the folder row of the tree,
        // under the section header and the 22 px row for this computer.
        let from = egui::pos2(crate::shell::BROWSER + PAD + 60.0, BAR + HEAD + ROW / 2.0);
        let onto = egui::pos2(100.0, crate::panel::HEADER + 22.0 + 10.0);
        let button = |pos, pressed| egui::Event::PointerButton {
            pos,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: egui::Modifiers::NONE,
        };
        let escape = egui::Event::Key {
            key: egui::Key::Escape,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: egui::Modifiers::NONE,
        };
        // The new folder opens its rename editor, and Escape closes it; then the pointer
        // presses on the table's row, carries it over the folder, and lets go.
        let frames: [Vec<egui::Event>; 6] = [
            Vec::new(),
            vec![escape],
            vec![egui::Event::PointerMoved(from)],
            vec![button(from, true)],
            vec![egui::Event::PointerMoved(onto)],
            vec![button(onto, false)],
        ];

        let mut asked = Vec::new();
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
                egui::SidePanel::left("places")
                    .exact_width(crate::shell::BROWSER)
                    .frame(egui::Frame::new())
                    .show(ctx, |ui| {
                        asked.extend(browser.ui(
                            ui,
                            &workspace,
                            &device,
                            &queue,
                            &Filter::default(),
                        ));
                    });
                egui::CentralPanel::default()
                    .frame(egui::Frame::new())
                    .show(ctx, |ui| {
                        library.ui(ui, &mut browser, &workspace, &device, &queue, &shell);
                    });
            });
        }

        let filed: Vec<(u64, Option<u64>)> = asked
            .iter()
            .filter_map(|act| match act {
                Act::File { id, folder } => Some((*id, *folder)),
                _ => None,
            })
            .collect();
        assert!(
            matches!(filed.as_slice(), [(dragged, Some(_))] if *dragged == id),
            "the drag filed the row it started on: {filed:?}"
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
