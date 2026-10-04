//! The library: one table of every kind of asset, on this computer and on the instrument.
//!
//! A [`Row`] is what the table knows about one asset: where it is, where it goes, how big
//! it is, and what it plays. Rows are built from the list on this computer and the
//! scanned slots without any UI. [`arrange`] filters and orders them. Everything else
//! here paints.

use std::collections::BTreeSet;
use std::ops::Range;

use eframe::egui;
use nord_format::accept::Family;
use nord_usb::wire::ProgramInfo;
use nord_usb::{Location, ObjectClass};

use crate::app::{accent, tint, ui as ui_text, warn};
use crate::browser::{qualifier, Act, Browser, Item, Kept, Kind, Qualifier};
use crate::device::{fit, read_only, Device, DeviceState};
use crate::filter::{Filter, Narrow, Place, State};
use crate::icon::{painted, Glyph};
use crate::panel::{cut, list_width, pill_at, row_ink, view_header, Track, ROW_INSET};
use crate::queue::{Diff, Queue};
use crate::shell::Shell;
use crate::strings::{counted, folder, place, shown};
use crate::tags::Tags;
use crate::workspace::{LocalEntity, Workspace};

/// Which of the two places a row's contents are in.
///
/// ⚠️ `Both` is a **link**: a slot [`crate::device::link`] matched this asset to. Its
/// value says whether the two still agree. Saving an edit here turns it to `false`, and
/// the link stays where it was.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Where {
    /// A link, and whether the two still agree according to [`agrees`]. `None` means
    /// the two copies cannot be compared.
    Both(Option<bool>),
    Computer,
    /// On this computer, and the attached instrument refuses it, so it can only stay
    /// here.
    Foreign,
    Keyboard,
    /// It came off a slot this session has not read, so nothing is known about the other
    /// copy.
    Unread,
}

impl Where {
    /// The short word on the Where column's pill.
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

    /// The full sentence, for the tooltip.
    pub fn sentence(self) -> &'static str {
        match self {
            Where::Both(Some(true)) => "On this computer and in a slot holding the same bytes.",
            Where::Both(Some(false)) => {
                "On this computer and in a slot it was matched to, and the two bodies \
                 no longer agree."
            }
            Where::Both(None) => {
                "On this computer and in a slot matched by name. Nothing here can say \
                 whether the two bodies agree."
            }
            Where::Computer => "On this computer only.",
            Where::Foreign => "On this computer only. Not for this keyboard.",
            Where::Keyboard => "On the instrument only.",
            Where::Unread => "It came off a slot this session has not read.",
        }
    }

    /// The places it is in, which a place filter checks.
    fn places(self) -> &'static [Place] {
        match self {
            Where::Both(_) => &[Place::Computer, Place::Keyboard],
            Where::Computer | Where::Foreign | Where::Unread => &[Place::Computer],
            Where::Keyboard => &[Place::Keyboard],
        }
    }

    /// The mark a row with these whereabouts wears, given whether a write to its slot is
    /// `waiting`. Only a linked asset gets one, and a waiting write means it differs.
    pub fn mark(self, waiting: bool) -> Option<Mark> {
        let Where::Both(agrees) = self else {
            return None;
        };
        Some(match (waiting, agrees) {
            (true, _) | (false, Some(false)) => Mark::Differs,
            (false, Some(true)) => Mark::Agrees,
            // A green dot is a claim, and there is no evidence for one.
            (false, None) => Mark::Unknown,
        })
    }

    /// Sort rank: both places first, then this computer, the instrument, and unread.
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

/// The library (piano or sample) a row plays, which the row's bytes do not name.
///
/// ⚠️ A name can only come from the instrument, because a program file stores a bare id
/// and no name. `Wanted` is an id **nothing has resolved**, never one the instrument said
/// it did not hold. No read this app makes reports a missing library.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Needs {
    Nothing,
    Named { class: ObjectClass, name: String },
    Wanted { class: ObjectClass, id: u32 },
}

impl Needs {
    /// The text in the Needs column.
    pub fn text(&self) -> String {
        match self {
            Needs::Nothing => String::new(),
            Needs::Named { name, .. } => name.clone(),
            Needs::Wanted { class, id } => {
                format!("{} {id:#010x}", Kind::from_class(*class).chip())
            }
        }
    }

    /// The full sentence, for the tooltip.
    pub fn sentence(&self) -> String {
        match self {
            Needs::Nothing => "Nothing here says what it plays.".to_string(),
            Needs::Named { class, name } => format!(
                "It plays the {} the instrument calls “{name}”.",
                Kind::from_class(*class).chip()
            ),
            Needs::Wanted { class, id } => format!(
                "This names a {} the instrument has not listed by id ({id:#010x}). Only the \
                 instrument can put a name to one, and nothing it has been asked about \
                 names this id.",
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
    /// What to put in front of the kind's word, where the word alone would not say which
    /// of its kind this is. [`crate::browser::qualifier`] decides it.
    pub qualifier: Option<Qualifier>,
    pub name: String,
    pub tags: usize,
    /// It holds something other than what it was last saved as. Only a row on this
    /// computer can be unsaved.
    pub unsaved: bool,
    pub where_: Where,
    /// The slot it came off, or the slot it is. An asset that never came off a slot has
    /// none.
    pub at: Option<(ObjectClass, Location)>,
    pub size: u64,
    pub needs: Needs,
}

impl Row {
    /// Where a send would write this row: the slot an asset came off, unless
    /// [`crate::device::read_only`] rules out its class. A row already on the instrument
    /// goes nowhere.
    fn destination(&self) -> Option<(ObjectClass, Location)> {
        let (class, at) = self.at?;
        (matches!(self.item, Item::Local(_)) && !read_only(class)).then_some((class, at))
    }
}

/// Everything the library holds, narrowed by kind, place, and tags.
///
/// The list on this computer comes first. A slot one of its assets came off is shown in
/// that asset's row, not as a second row: a program read off 7:4 and kept is one thing in
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
    let kept = Kept::of(workspace);
    let instrument = device.product().and_then(Family::from_product);
    for entity in workspace.listed() {
        // Claim the row's slot so the instrument's list does not repeat it.
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

/// What a row needs: a write already waiting, or a slot that no longer holds what this
/// row was last saved as. A slot on the instrument needs nothing, since it is what the
/// instrument holds.
///
/// ⚠️ A queued row counts as waiting, never as differing, however the two bodies compare.
/// The queued write settles them, so counting the row under both would ask twice for one
/// thing.
pub fn state(item: Item, where_: Where, queue: &Queue) -> Option<State> {
    match item {
        Item::Local(id) if queue.holds(id) => Some(State::Waiting),
        Item::Local(_) => (where_ == Where::Both(Some(false))).then_some(State::Differs),
        _ => None,
    }
}

/// The row for one item, for a caller that needs one item and not the whole table.
///
/// Uses the same two builders as [`rows`], so the inspector shows what the table shows.
/// A folder or a tag is a grouping and makes no row.
pub fn row_of(
    item: Item,
    workspace: &Workspace,
    device: &DeviceState,
    queue: &Queue,
    tags: &Tags,
) -> Option<Row> {
    let instrument = device.product().and_then(Family::from_product);
    let kept = Kept::of(workspace);
    row_with(item, workspace, device, queue, tags, &kept, instrument)
}

/// [`row_of`], with what it reads of the whole list already taken.
fn row_with(
    item: Item,
    workspace: &Workspace,
    device: &DeviceState,
    queue: &Queue,
    tags: &Tags,
    kept: &Kept,
    instrument: Option<Family>,
) -> Option<Row> {
    match item {
        Item::Local(id) => {
            let entity = workspace.get(id)?;
            let worn = tags.worn(id).len();
            Some(local(entity, device, queue, worn, kept, instrument))
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
    kept: &Kept,
    instrument: Option<Family>,
) -> Row {
    Row {
        item: Item::Local(entity.id),
        kind: Kind::of(entity),
        qualifier: qualifier(entity, kept, instrument),
        name: entity.name.clone(),
        tags,
        unsaved: entity.is_unsaved(),
        where_: whereabouts(entity, device, queue),
        at: entity.spot(),
        size: entity.size(),
        needs: wanted(entity, device),
    }
}

fn slot(class: ObjectClass, at: Location, info: &ProgramInfo, device: &DeviceState) -> Row {
    Row {
        item: Item::Slot { class, at },
        kind: Kind::from_class(class),
        // A slot is the instrument's own, so its kind word never needs a qualifier.
        qualifier: None,
        name: info.name.trim().to_string(),
        tags: 0,
        unsaved: false,
        where_: Where::Keyboard,
        at: Some((class, at)),
        size: u64::from(info.body_len),
        needs: played(class, at, device),
    }
}

/// Where an asset's contents are. A linked asset is in both places, and whether the two
/// agree comes from [`agrees`].
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
/// This comparison drives the library's sign, the tree's dot, and what "queue changed"
/// finds. Equality is claimed only on evidence:
///
/// - the checksum a walk reported for the slot matches the checksum of the saved bytes,
///   which is taken once at ingest;
/// - this app wrote the saved bytes into that slot, which is the only thing it knows
///   about a slot without reading it back;
/// - a compare read fetched the occupant and found the bodies equal.
///
/// ⚠️ `None` where none of these answers. It means unknown, never the same. An address
/// and a length are not evidence: a class reporting no checksum is linked by name or
/// because it has only one slot, and neither says anything about the body in it. Only a
/// read settles one of those.
pub fn agrees(
    entity: &LocalEntity,
    class: ObjectClass,
    info: &ProgramInfo,
    queue: &Queue,
) -> Option<bool> {
    if let (Some(here), Some(there)) = (entity.saved.crc32(), info.crc32) {
        return Some(here == there);
    }
    if wrote(entity, class, info.location) {
        return Some(true);
    }
    match queue.entry(entity.id).map(|held| &held.diff) {
        Some(Diff::Identical) => Some(true),
        Some(Diff::Fields(_) | Diff::Bytes { .. } | Diff::Checksum) => Some(false),
        _ => None,
    }
}

/// Whether this app wrote the asset's saved bytes into this slot.
///
/// ⚠️ Compared by checksum: saving anything else moves the baseline off the bytes the
/// write put there, and the write no longer counts as evidence.
fn wrote(entity: &LocalEntity, class: ObjectClass, at: Location) -> bool {
    entity.wrote.is_some_and(|wrote| {
        (wrote.class, wrote.at) == (class, at) && Some(wrote.crc32) == entity.saved.crc32()
    })
}

/// What a mark on a row claims: the dot the tree paints at a row's right end, and the
/// star after a name.
///
/// [`mark_words`] explains each one wherever it is drawn.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Mark {
    Agrees,
    Differs,
    /// A slot whose agreement is unknown: the unsigned `both` of [`Where::Both`].
    Unknown,
    Unsaved,
}

/// The color a mark is painted in, everywhere it is painted.
pub fn mark_ink(mark: Mark, visuals: &egui::Visuals) -> egui::Color32 {
    match mark {
        Mark::Agrees => crate::app::good(visuals),
        Mark::Differs => warn(visuals),
        Mark::Unknown => crate::app::caption(visuals),
        Mark::Unsaved => visuals.text_color(),
    }
}

/// What a mark means, in words.
///
/// The tree dot's hover, the table's Where cell's hover, and the document header's hint
/// all use these words, so every mark in the window is explained the same way.
pub fn mark_words(mark: Mark) -> &'static str {
    match mark {
        Mark::Agrees => "on the keyboard, the same as saved here",
        Mark::Differs => {
            "on the keyboard, but different from what is saved here, or waiting to be sent"
        }
        Mark::Unknown => "on the keyboard; whether it matches is not known yet",
        Mark::Unsaved => "edited since it was last saved",
    }
}

/// The mark at a local row's right end: what the attached instrument holds in the slot
/// this asset stands for.
///
/// ⚠️ The only rule for a local row's dot. An asset with no link gets no mark. The link
/// is the slot `whereabouts` reads, and no other slot counts.
pub fn keyboard_mark(entity: &LocalEntity, device: &DeviceState, queue: &Queue) -> Option<Mark> {
    whereabouts(entity, device, queue).mark(queue.holds(entity.id))
}

/// The library a program names, and its name if the instrument has reported one. A
/// program not read this session names what a read of it found before.
pub(crate) fn wanted(entity: &LocalEntity, device: &DeviceState) -> Needs {
    let plays = match (&entity.entity, entity.remembered()) {
        (Some(_), _) => entity.plays,
        (None, Some(known)) => known.plays,
        (None, None) => None,
    };
    let Some(plays) = plays else {
        return Needs::Nothing;
    };
    let (class, id) = (plays.class(), plays.id());
    match device.dependency_name(class, id) {
        Some(name) => Needs::Named {
            class,
            name: name.to_string(),
        },
        None => Needs::Wanted { class, id },
    }
}

/// What the instrument said a slot plays, if that slot is the one it was last asked
/// about.
///
/// ⚠️ Programs only: this column shows the piano or sample a program plays.
fn played(class: ObjectClass, at: Location, device: &DeviceState) -> Needs {
    if class != ObjectClass::Program || device.detail.at != Some((class, at)) {
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

/// One column of the table.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Column {
    Glyph,
    Kind,
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
        Column::Glyph,
        Column::Kind,
        Column::Name,
        Column::Tags,
        Column::Where,
        Column::At,
        Column::Size,
        Column::Needs,
    ];

    /// The column's heading. The glyph column has none: its word is the kind beside it.
    fn head(self) -> &'static str {
        match self {
            Column::Glyph => "",
            Column::Kind => "Kind",
            Column::Name => "Name",
            Column::Tags => "Tags",
            Column::Where => "Where",
            Column::At => "At",
            Column::Size => "Size",
            Column::Needs => "Needs",
        }
    }

    /// What the column asks for: a fixed width, or a share of what the fixed ones leave.
    fn track(self) -> Track {
        match self {
            Column::Glyph => Track::Px(16.0),
            // 120 px holds a kind with its family, as in `Stage 4 program`. It is a capped
            // share so a narrow center gives the room to the name.
            Column::Kind => Track::Capped {
                share: 1.0,
                max: 120.0,
            },
            Column::Name => Track::Share(1.9),
            Column::Tags => Track::Px(34.0),
            // Holds the widest word, "computer", on its pill.
            Column::Where => Track::Px(72.0),
            Column::At => Track::Px(44.0),
            Column::Size => Track::Px(56.0),
            Column::Needs => Track::Share(1.3),
        }
    }

    /// Whether the column's right edge can be dragged. Needs, the last, keeps its share so
    /// the columns always fill the width.
    fn resizable(self) -> bool {
        !matches!(self, Column::Glyph | Column::Needs)
    }

    /// The column's name where its width is kept.
    fn key(self) -> &'static str {
        match self {
            Column::Glyph => "glyph",
            Column::Kind => "kind",
            Column::Name => "name",
            Column::Tags => "tags",
            Column::Where => "where",
            Column::At => "at",
            Column::Size => "size",
            Column::Needs => "needs",
        }
    }
}

/// The widths columns were dragged to, by column; `None` keeps the column's own track.
pub type Widths = [Option<f32>; 8];

/// The gap between two columns.
const GAP: f32 = 10.0;

/// The narrowest and widest a column can be dragged to.
const COLUMN_LEAST: f32 = 24.0;
const COLUMN_MOST: f32 = 480.0;

/// The width of the grip on a column's right edge in the head.
const GRIP: f32 = 8.0;

/// Where each column sits across `width`, laid out by [`crate::panel::tracks`], with any
/// dragged `widths` in place of the columns' own tracks.
pub fn tracks(width: f32, widths: &Widths) -> [Range<f32>; 8] {
    let wanted = Column::ALL.map(|column| match widths[column as usize] {
        Some(px) => Track::Px(px),
        None => column.track(),
    });
    let held = crate::panel::tracks(width, &wanted, GAP);
    std::array::from_fn(|index| held[index].clone())
}

/// The rows the table shows: those matching the omnibox, sorted by a column.
///
/// ⚠️ Ties break on the name and then the item, so the order does not depend on the
/// order the rows were built in.
pub fn arrange(rows: Vec<Row>, query: &str, by: Column, order: Order) -> Vec<Row> {
    let query = query.trim().to_lowercase();
    let (keys, mut rows): (Vec<Key>, Vec<Option<Row>>) = rows
        .into_iter()
        .filter_map(|row| {
            let name = row.name.to_lowercase();
            let shown = query.is_empty() || name.contains(&query);
            shown.then(|| (Key::of(by, &row, name), Some(row)))
        })
        .unzip();
    // Indices are sorted rather than rows, which are large to move.
    let mut order_of: Vec<usize> = (0..keys.len()).collect();
    order_of.sort_unstable_by(|&a, &b| {
        let (a, b) = (&keys[a], &keys[b]);
        let ranked = match order {
            Order::Up => a.column.cmp(&b.column),
            Order::Down => a.column.cmp(&b.column).reverse(),
        };
        ranked
            .then_with(|| a.name.cmp(&b.name))
            .then_with(|| a.item.cmp(&b.item))
    });
    order_of
        .into_iter()
        .filter_map(|at| rows[at].take())
        .collect()
}

/// What a row sorts by, taken once per row so that no comparison allocates.
struct Key {
    column: Ranked,
    /// The name in lowercase, which breaks ties, and then the item.
    name: String,
    item: Item,
}

/// A row's value in the column the table is sorted by.
#[derive(PartialEq, Eq, PartialOrd, Ord)]
enum Ranked {
    /// Sorted by the name, which [`Key::name`] already holds.
    Name,
    Text(String),
    Count(u64),
    Address((bool, u32, u32, u32)),
}

impl Key {
    fn of(by: Column, row: &Row, name: String) -> Key {
        let column = match by {
            // The glyph and the word beside it are the same fact, so a click on either
            // orders the table the same way.
            Column::Glyph | Column::Kind => Ranked::Text(word(row)),
            Column::Name => Ranked::Name,
            Column::Tags => Ranked::Count(row.tags as u64),
            Column::Where => Ranked::Count(u64::from(row.where_.rank())),
            Column::At => Ranked::Address(address(row)),
            Column::Size => Ranked::Count(row.size),
            Column::Needs => Ranked::Text(row.needs.text()),
        };
        Key {
            column,
            name,
            item: row.item,
        }
    }
}

/// The word in the Kind column, which the glyph beside it also represents.
fn word(row: &Row) -> String {
    crate::strings::kind_word(row.kind, row.qualifier)
}

/// A row with no address sorts after every row that has one.
fn address(row: &Row) -> (bool, u32, u32, u32) {
    match row.at {
        Some((class, at)) => (false, class.to_raw(), at.bank, at.slot),
        None => (true, 0, 0, 0),
    }
}

/// What sending the picked rows would do, in one sentence, or `None` when no row is
/// bound for a slot and nothing about them needs saying.
pub fn consequence(rows: &[&Row], device: &DeviceState, queue: &Queue) -> Option<String> {
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
            n => counted(n, "slot occupied", "slots occupied"),
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
    (!said.is_empty()).then(|| said.join(" · "))
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

/// The height of a row, the gap between two rows, and the height of the column heads.
const ROW: f32 = 32.0;
const ROW_GAP: f32 = 2.0;
const HEAD: f32 = 28.0;

/// The padding at each end of a row and of the column heads.
const CELL_PAD: f32 = 10.0;

/// The space between the column heads and the first row.
const ABOVE: f32 = 6.0;

/// The glyphs beside a count, and the glyph of a row's kind.
const SMALL: f32 = 11.0;
const KIND: f32 = 15.0;

/// A filter chip's height, its padding at each end, and the size of its glyph.
const CHIP: f32 = 26.0;
const CHIP_PAD: f32 = 10.0;
const CHIP_GLYPH: f32 = 12.0;

/// The font sizes a cell uses. Cells are painted directly, so the sizes are set here and
/// not taken from the named styles in [`crate::app`].
const NAME: f32 = 13.0;
const TEXT: f32 = 12.0;
const MONO: f32 = 11.5;

/// The center panel's default view.
pub struct Library {
    by: Column,
    order: Order,
    widths: Widths,
    /// The order of the rows as last taken, kept until something it comes from changes.
    table: Option<Table>,
    /// The header's counts, and the revisions they were counted at.
    counts: Option<(Counted, [usize; 2])>,
    /// How many times the order of the rows has been taken.
    #[cfg(test)]
    pub(crate) built: usize,
}

impl Default for Library {
    fn default() -> Library {
        Library {
            by: Column::Name,
            order: Order::Up,
            widths: [None; 8],
            table: None,
            counts: None,
            #[cfg(test)]
            built: 0,
        }
    }
}

/// Everything the order of the table's rows comes from. The order is taken again only
/// when this changes; the rows in view are built every frame.
#[derive(PartialEq)]
struct Sources {
    /// The list's revision, but only where the order or the narrowing reads what an asset
    /// holds (its kind, where it is, its size, what it needs), which any change to the
    /// list may move. The names, slots, tags and queue they read otherwise are covered by
    /// the rest.
    revision: Option<u64>,
    layout: u64,
    device: u64,
    queue: Vec<(u64, std::mem::Discriminant<Diff>)>,
    tags: u64,
    filter: Filter,
    query: String,
    by: Column,
    order: Order,
}

impl Sources {
    fn of(
        library: &Library,
        workspace: &Workspace,
        device: &Device,
        queue: &Queue,
        tags: &Tags,
        shell: &Shell,
    ) -> Sources {
        let filter = &shell.filter;
        let narrows = filter.kind.is_some() || filter.place.is_some() || filter.state.is_some();
        let names = matches!(library.by, Column::Name | Column::Tags | Column::At);
        Sources {
            revision: (narrows || !names).then(|| workspace.revision()),
            layout: workspace.layout(),
            device: device.revision(),
            queue: queue.shape(),
            tags: tags.revision(),
            filter: filter.clone(),
            query: shell.omnibox.clone(),
            by: library.by,
            order: library.order,
        }
    }
}

/// The items the table shows, in order, and what that order came from.
struct Table {
    sources: Sources,
    items: Vec<Item>,
}

/// What the header's counts read: the list, the instrument and the queue.
#[derive(PartialEq)]
struct Counted {
    revision: u64,
    device: u64,
    queue: Vec<(u64, std::mem::Discriminant<Diff>)>,
}

impl Library {
    /// Where the column widths are kept between sessions.
    pub const KEY: &'static str = "drawbar.library";

    const VERSION: &'static str = "drawbar library 1";

    /// Restore the column widths the last session left. An unknown version, column, or a
    /// width out of range is ignored, and that column keeps its own track.
    pub fn restore(&mut self, storage: &dyn eframe::Storage) {
        let Some(text) = storage.get_string(Library::KEY) else {
            return;
        };
        let mut lines = text.lines();
        if lines.next() != Some(Library::VERSION) {
            return;
        }
        for line in lines {
            let mut parts = line.split('\t');
            let (Some("width"), Some(key), Some(px)) = (parts.next(), parts.next(), parts.next())
            else {
                continue;
            };
            let Some(column) = Column::ALL
                .into_iter()
                .find(|column| column.resizable() && column.key() == key)
            else {
                continue;
            };
            if let Ok(px) = px.parse::<f32>() {
                if (COLUMN_LEAST..=COLUMN_MOST).contains(&px) {
                    self.widths[column as usize] = Some(px);
                }
            }
        }
    }

    pub fn keep(&self, storage: &mut dyn eframe::Storage) {
        let mut text = format!("{}\n", Library::VERSION);
        for column in Column::ALL {
            if let Some(px) = self.widths[column as usize] {
                text.push_str(&format!("width\t{}\t{px}\n", column.key()));
            }
        }
        storage.set_string(Library::KEY, text);
    }

    pub fn ui(
        &mut self,
        ui: &mut egui::Ui,
        browser: &mut Browser,
        workspace: &Workspace,
        device: &Device,
        queue: &Queue,
        shell: &Shell,
    ) -> Vec<Act> {
        ui.spacing_mut().item_spacing.y = 0.0;
        let mut acts = Vec::new();
        let sources = Sources::of(self, workspace, device, queue, browser.tags(), shell);
        let table = match self.table.take() {
            Some(table) if table.sources == sources => table,
            _ => self.build(sources, workspace, device, queue, browser.tags(), shell),
        };
        let counts = self.counts(workspace, device, queue);
        header(ui, counts, browser.tags(), &shell.filter, &mut acts);
        self.table(ui, &table, browser, workspace, device, queue, &mut acts);
        self.table = Some(table);
        acts
    }

    /// Take the order of the rows again.
    fn build(
        &mut self,
        sources: Sources,
        workspace: &Workspace,
        device: &Device,
        queue: &Queue,
        tags: &Tags,
        shell: &Shell,
    ) -> Table {
        #[cfg(test)]
        {
            self.built += 1;
        }
        let held = rows(workspace, &device.state, queue, tags, &shell.filter);
        let held = arrange(held, &shell.omnibox, self.by, self.order);
        Table {
            sources,
            items: held.iter().map(|row| row.item).collect(),
        }
    }

    /// What is waiting, and what differs from its slot, counted again only when the list,
    /// the instrument or the queue has changed.
    fn counts(&mut self, workspace: &Workspace, device: &Device, queue: &Queue) -> [usize; 2] {
        let now = Counted {
            revision: workspace.revision(),
            device: device.revision(),
            queue: queue.shape(),
        };
        if let Some((_, counts)) = self.counts.as_ref().filter(|(at, _)| *at == now) {
            return *counts;
        }
        let counts = [
            queue.len(),
            crate::queue::changed(workspace, &device.state, queue).len(),
        ];
        self.counts = Some((now, counts));
        counts
    }

    #[allow(clippy::too_many_arguments)]
    fn table(
        &mut self,
        ui: &mut egui::Ui,
        table: &Table,
        browser: &mut Browser,
        workspace: &Workspace,
        device: &Device,
        queue: &Queue,
        acts: &mut Vec<Act>,
    ) {
        let room = ui.available_rect_before_wrap();
        let ui = &mut ui.new_child(
            egui::UiBuilder::new()
                .max_rect(egui::Rect::from_min_max(
                    room.min + egui::vec2(ROW_INSET, 0.0),
                    room.max - egui::Vec2::splat(ROW_INSET),
                ))
                .layout(*ui.layout()),
        );
        let items = &table.items;
        let width = list_width(ui, items.len(), ROW + ROW_GAP, HEAD + ABOVE);
        let tracks = tracks((width - 2.0 * CELL_PAD).max(0.0), &self.widths);
        self.head(ui, width, &tracks);
        ui.add_space(ABOVE);
        if items.is_empty() {
            return nothing(ui);
        }

        ui.spacing_mut().item_spacing.y = ROW_GAP;
        let shown = egui::ScrollArea::vertical()
            .id_salt("library_table")
            .auto_shrink([false; 2])
            .show_rows(ui, ROW, items.len(), |ui, shown| {
                let shown = items.get(shown).unwrap_or_default();
                let kept = Kept::of(workspace);
                let instrument = device.state.product().and_then(Family::from_product);
                let tags = browser.tags();
                let rows: Vec<Row> = shown
                    .iter()
                    .filter_map(|item| {
                        row_with(
                            *item,
                            workspace,
                            &device.state,
                            queue,
                            tags,
                            &kept,
                            instrument,
                        )
                    })
                    .collect();
                workspace.in_view(rows.iter().filter_map(|row| row.item.local()));
                for row in &rows {
                    paint(
                        ui, row, width, &tracks, browser, items, workspace, device, queue, acts,
                    );
                }
            });
        // A click in the space under the last row clears the selection.
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

    /// A grip on each resizable column's right edge in the head: dragging it sets the
    /// column's width, and a double click gives the column back its own track.
    ///
    /// ⚠️ Added after the head, so a press on a grip drags it instead of sorting.
    fn grips(&mut self, ui: &mut egui::Ui, content: egui::Rect, tracks: &[Range<f32>; 8]) {
        for (column, track) in Column::ALL.iter().zip(tracks) {
            if !column.resizable() {
                continue;
            }
            let edge = content.left() + track.end + GAP / 2.0;
            let rect = egui::Rect::from_center_size(
                egui::pos2(edge, content.center().y),
                egui::vec2(GRIP, content.height()),
            );
            let id = ui.id().with(("column edge", *column as usize));
            let grip = ui.interact(rect, id, egui::Sense::click_and_drag());
            if grip.hovered() || grip.dragged() {
                ui.ctx().set_cursor_icon(egui::CursorIcon::ResizeColumn);
                let ink = ui.visuals().widgets.hovered.bg_stroke.color;
                ui.painter().vline(
                    edge,
                    rect.y_range().shrink(6.0),
                    egui::Stroke::new(1.0_f32, ink),
                );
            }
            let slot = &mut self.widths[*column as usize];
            if grip.double_clicked() {
                *slot = None;
            } else if let Some(at) = grip.interact_pointer_pos().filter(|_| grip.dragged()) {
                let start = content.left() + track.start;
                *slot = Some((at.x - GAP / 2.0 - start).clamp(COLUMN_LEAST, COLUMN_MOST));
            }
        }
    }

    /// The column heads. Clicking one sorts by that column, and clicking it again reverses
    /// the order.
    fn head(&mut self, ui: &mut egui::Ui, width: f32, tracks: &[Range<f32>; 8]) {
        let (rect, response) =
            ui.allocate_exact_size(egui::vec2(width, HEAD), egui::Sense::click());
        let visuals = ui.visuals().clone();
        let painter = ui.painter().clone();
        painter.hline(
            rect.x_range(),
            rect.bottom() - 0.5,
            egui::Stroke::new(1.0_f32, visuals.widgets.noninteractive.bg_stroke.color),
        );
        let quiet = crate::app::caption(&visuals);
        let strong = visuals.widgets.active.fg_stroke.color;
        let font = crate::app::section().resolve(ui.style());
        let content = rect.shrink2(egui::vec2(CELL_PAD, 0.0));

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
                // ⚠️ The vendored Lucide set has no chevron-up, so ascending points right,
                // like a collapsed tree branch.
                let glyph = match self.order {
                    Order::Up => Glyph::ChevronRight,
                    Order::Down => Glyph::ChevronDown,
                };
                let box_ = egui::Rect::from_min_size(
                    egui::pos2(
                        content.left() + track.end - SMALL,
                        content.center().y - SMALL / 2.0,
                    ),
                    egui::Vec2::splat(SMALL),
                );
                painted(ui, glyph, box_, ink);
                room = (room - SMALL - 2.0).max(0.0);
            }
            cut(
                &painter,
                content.left() + track.start,
                content.center().y,
                room,
                column.head(),
                egui::TextFormat::simple(font.clone(), ink),
            );
        }

        self.grips(ui, content, tracks);
        let Some(column) = response
            .clicked()
            .then(|| under(&response, content, tracks))
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

/// The header over the table: what the library shows, and the filters narrowing it or
/// asking for attention.
///
/// ⚠️ Both counts cover the whole list, not this view. They match the numbers on the
/// tree's rows and what the toolbar's Send acts on. A chip counting only what passes its
/// own filter would change the moment it was clicked.
fn header(
    ui: &mut egui::Ui,
    counts: [usize; 2],
    tags: &Tags,
    filter: &Filter,
    acts: &mut Vec<Act>,
) {
    let accent = accent(ui.visuals());
    let shows = over(filter);
    view_header(ui, Glyph::LibraryBig, accent, "Library", &shows, |ui| {
        let states = [
            (State::Differs, Glyph::CircleAlert, counts[1]),
            (State::Waiting, Glyph::Clock, counts[0]),
        ];
        for (state, glyph, count) in states {
            let on = filter.on(Narrow::State(state));
            // A chip whose count is zero stays while it is the active filter, so there is
            // always something to click to clear it.
            if count == 0 && !on {
                continue;
            }
            let text = format!("{count} {}", state.word());
            let signal = warn(ui.visuals());
            if filter_chip(ui, glyph, Some(signal), &text, on)
                .on_hover_text(state.sentence())
                .clicked()
            {
                acts.push(Act::Narrow(Narrow::State(state)));
            }
        }
        let worn: Vec<(u64, &str)> = filter
            .tags
            .iter()
            .filter_map(|id| Some((*id, tags.name_of(*id)?)))
            .collect();
        for (id, name) in worn.into_iter().rev() {
            if filter_chip(ui, Glyph::Tag, None, name, true)
                .on_hover_text(format!("only what is tagged {name}"))
                .clicked()
            {
                acts.push(Act::Narrow(Narrow::Tag(id)));
            }
        }
    });
}

/// A filter as a pill: a glyph and a word, ringed and washed in the accent while it
/// narrows the library. A click toggles it.
///
/// `signal` colors the glyph whether or not the filter is on, for a count that asks for
/// attention.
fn filter_chip(
    ui: &mut egui::Ui,
    glyph: Glyph,
    signal: Option<egui::Color32>,
    text: &str,
    on: bool,
) -> egui::Response {
    let visuals = ui.visuals().clone();
    let ink = match on {
        true => visuals.widgets.active.fg_stroke.color,
        false => crate::app::caption(&visuals),
    };
    let word = ui
        .painter()
        .layout_no_wrap(text.to_owned(), egui::FontId::proportional(TEXT), ink);
    let width = CHIP_PAD + CHIP_GLYPH + 5.0 + word.size().x + CHIP_PAD;
    let (rect, response) = ui.allocate_exact_size(egui::vec2(width, CHIP), egui::Sense::click());
    let accent = accent(&visuals);
    let (edge, fill) = match (on, response.hovered()) {
        (true, _) => (tint(accent, 0.55), tint(accent, 0.14)),
        (false, true) => (
            visuals.widgets.hovered.bg_stroke.color,
            egui::Color32::TRANSPARENT,
        ),
        (false, false) => (
            visuals.widgets.noninteractive.bg_stroke.color,
            egui::Color32::TRANSPARENT,
        ),
    };
    let painter = ui.painter();
    painter.rect(
        rect,
        CHIP / 2.0,
        fill,
        egui::Stroke::new(1.0_f32, edge),
        egui::StrokeKind::Inside,
    );
    let mark = egui::Rect::from_center_size(
        egui::pos2(rect.left() + CHIP_PAD + CHIP_GLYPH / 2.0, rect.center().y),
        egui::Vec2::splat(CHIP_GLYPH),
    );
    painted(ui, glyph, mark, signal.unwrap_or(ink));
    ui.painter().galley(
        egui::pos2(mark.right() + 5.0, rect.center().y - word.size().y / 2.0),
        word,
        egui::Color32::PLACEHOLDER,
    );
    response
}

/// What the library shows, in the words the filter's rows use.
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
    let said = match narrowed.is_empty() {
        true => "everything".to_string(),
        false => narrowed.join(" · "),
    };
    let mut letters = said.chars();
    letters
        .next()
        .map(|first| first.to_uppercase().chain(letters).collect())
        .unwrap_or_default()
}

/// Which column the pointer is over, for the tooltip and for the head's sort click.
/// `content` is the row or head less its padding, where the tracks start.
fn under(
    response: &egui::Response,
    content: egui::Rect,
    tracks: &[Range<f32>; 8],
) -> Option<Column> {
    let at = response.interact_pointer_pos().or(response.hover_pos())?;
    let x = at.x - content.left();
    Column::ALL
        .iter()
        .zip(tracks)
        .find(|(_, track)| (track.start..track.end + GAP).contains(&x))
        .map(|(column, _)| *column)
}

/// The Where pill's ink and fill: the signal of what the row needs, if anything.
///
/// ⚠️ On the selection fill every signal fails contrast, so a selected row's pill takes
/// the selection's text color and the word carries the state.
fn where_look(
    state: Option<State>,
    selected: bool,
    visuals: &egui::Visuals,
) -> (egui::Color32, egui::Color32) {
    let signal = match state {
        Some(State::Waiting) => Some(accent(visuals)),
        Some(State::Differs) => Some(warn(visuals)),
        None => None,
    };
    match (selected, signal) {
        (true, _) => {
            let ink = visuals.selection.stroke.color;
            (ink, tint(ink, 0.12))
        }
        (false, Some(signal)) => (signal, tint(signal, 0.15)),
        (false, None) => (visuals.text_color(), visuals.widgets.inactive.weak_bg_fill),
    }
}

/// One row of the table.
///
/// ⚠️ Nothing inside is a widget, for the reason [`crate::browser::Cells`] gives: a label
/// allocates a hover rect that wins the hit test over the row, so a click would land on
/// whichever word is under the pointer. Only the row senses input, and the tooltip comes
/// from the cell under the pointer.
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
    let (ink, quiet) = row_ink(&painter, rect, selected, response.hovered(), &visuals);
    let content = rect.shrink2(egui::vec2(CELL_PAD, 0.0));

    let cell = |column: Column| crate::panel::cell(content, &tracks[column as usize]);
    let write = |box_: egui::Rect, text: &str, format: egui::TextFormat| {
        cut(
            &painter,
            box_.left(),
            box_.center().y,
            box_.width(),
            text,
            format,
        );
    };
    let plain = |size: f32, tint: egui::Color32| {
        egui::TextFormat::simple(egui::FontId::proportional(size), tint)
    };
    let mono = |size: f32| egui::TextFormat::simple(egui::FontId::monospace(size), quiet);

    let glyph = cell(Column::Glyph);
    if glyph.width() > 0.0 {
        painted(
            ui,
            row.kind.glyph(),
            egui::Rect::from_center_size(
                egui::pos2(glyph.left() + KIND / 2.0, glyph.center().y),
                egui::Vec2::splat(KIND),
            ),
            quiet,
        );
    }
    write(cell(Column::Kind), &word(row), plain(TEXT, quiet));
    let family = match selected {
        true => crate::app::bold(),
        false => egui::FontFamily::Proportional,
    };
    write(
        cell(Column::Name),
        &crate::browser::starred(&row.name, row.unsaved),
        egui::TextFormat {
            italics: row.unsaved,
            ..egui::TextFormat::simple(egui::FontId::new(NAME, family), ink)
        },
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
        let count = tags.with_min_x(tags.left() + SMALL + 4.0);
        write(count, &row.tags.to_string(), mono(MONO - 0.5));
    }
    let where_ = cell(Column::Where);
    let (pill_ink, pill_fill) = where_look(state(row.item, row.where_, queue), selected, &visuals);
    pill_at(
        &painter,
        where_.left(),
        where_.center().y,
        where_.width(),
        row.where_.short(),
        pill_ink,
        pill_fill,
    );
    if let Some((_, at)) = row.at {
        write(cell(Column::At), &shown(at), mono(MONO));
    }
    write(
        cell(Column::Size),
        &crate::room::measure(row.size),
        mono(MONO),
    );
    // ⚠️ Weak ink even for a bare id. An unresolved id is a question nobody has asked
    // the instrument, not a library reported missing.
    write(cell(Column::Needs), &row.needs.text(), plain(TEXT, quiet));

    // A table row drags like a tree row, with the same payload, so it has the same drop
    // targets and meaning.
    if response.dragged() {
        if let Some(head) = browser.held(row.item, workspace, &device.state) {
            let carried = browser.carrying(head, &row.name, workspace, &device.state);
            egui::DragAndDrop::set_payload(ui.ctx(), carried);
        }
    }

    let mark = row
        .item
        .local()
        .and_then(|id| row.where_.mark(queue.holds(id)));
    let response = match under(&response, content, tracks) {
        Some(column) => response.on_hover_text(tooltip(
            row,
            column,
            mark,
            browser.tags(),
            workspace,
            device,
        )),
        None => response,
    };
    if response.double_clicked() {
        acts.push(Act::Open(row.item));
    } else if response.clicked() {
        browser.pick(ui, row.item, list);
    }
    response.context_menu(|ui| browser.menu(ui, row.item, workspace, device, queue, acts));
}

/// The hover text for a cell.
///
/// Three columns add a fact the row does not carry: the tags column shows a count and
/// the hover names the tags, the address may be one of several slots holding the same
/// bytes, and the Where cell adds the [`Mark`] the tree paints for the row. All three
/// are computed for the hovered row only.
fn tooltip(
    row: &Row,
    column: Column,
    mark: Option<Mark>,
    tags: &Tags,
    workspace: &Workspace,
    device: &Device,
) -> String {
    match column {
        Column::Glyph | Column::Kind => word(row),
        Column::Name => row.name.clone(),
        Column::Tags => match worn(row, tags) {
            names if names.is_empty() => "no tags".to_string(),
            names => names.join(", "),
        },
        // Under the sentence saying where the row is, what the tree's dot for it claims.
        Column::Where => match mark {
            Some(mark) => format!("{}\n{}", row.where_.sentence(), mark_words(mark)),
            None => row.where_.sentence().to_string(),
        },
        Column::At => match row.at {
            Some((class, at)) => {
                let where_ = place(class, at);
                match (row.where_, also_holding(row, workspace, device)) {
                    // A class reporting no checksum is linked by name, so the address is
                    // where the name matched and says nothing about the body.
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

/// How many slots beyond the linked one hold this row's saved bytes. A slot row is not
/// linked and returns zero.
fn also_holding(row: &Row, workspace: &Workspace, device: &Device) -> usize {
    row.item
        .local()
        .and_then(|id| workspace.get(id))
        .map_or(0, |entity| {
            crate::device::also_holding(&device.state, entity)
        })
}

/// The names of a row's tags. Only an asset on this computer has tags: a tag belongs to a
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

/// The line the table shows when no row passes the filters.
fn nothing(ui: &mut egui::Ui) {
    ui.horizontal(|ui| {
        ui.add_space(CELL_PAD);
        ui.label(
            egui::RichText::new(
                "Nothing to show. Open Nord files, connect an instrument, or clear the search and filters.",
            )
            .text_style(ui_text())
            .weak()
            .italics(),
        );
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::log::Log;
    use crate::testing::{self, Bench};
    use crate::workspace::{Fresh, Origin};

    fn at(bank: u32, slot: u32) -> Location {
        Location { bank, slot }
    }

    fn row(name: &str, kind: Kind, where_: Where, at: Option<Location>, size: u64) -> Row {
        Row {
            item: Item::Local(size),
            kind,
            qualifier: None,
            name: name.to_string(),
            tags: 0,
            unsaved: false,
            where_,
            at: at.map(|at| (ObjectClass::Program, at)),
            size,
            needs: Needs::Nothing,
        }
    }

    /// ⚠️ Every track shrinks and none goes negative. At the center's width with both
    /// docks open, the address, size, and dependency columns still have room.
    #[test]
    fn the_columns_share_the_width_without_overlapping_or_overflowing_it() {
        for width in [430.0_f32, 900.0] {
            let tracks = tracks(width, &[None; 8]);
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
                let track = &tracks[column as usize];
                assert!(
                    track.end - track.start > 0.0,
                    "{column:?} vanished at {width}"
                );
            }
        }
    }

    /// ⚠️ The kind column gets at most 120 px, and the name must stay readable when the
    /// center is narrow: programs are told apart by their names.
    #[test]
    fn the_kind_reaches_120_px_when_wide_and_yields_to_the_name_when_narrow() {
        let width_of = |tracks: &[Range<f32>; 8], column: Column| {
            let track = &tracks[column as usize];
            track.end - track.start
        };

        let wide = tracks(900.0, &[None; 8]);
        assert!(
            (width_of(&wide, Column::Kind) - 120.0).abs() < 0.01,
            "at 900 the kind holds the longest word: {}",
            width_of(&wide, Column::Kind)
        );

        let narrow = tracks(430.0, &[None; 8]);
        assert!(
            width_of(&narrow, Column::Name) >= 40.0,
            "at 430 the name is unreadable: {}",
            width_of(&narrow, Column::Name)
        );
        assert!(
            width_of(&narrow, Column::Kind) > 0.0,
            "at 430 the kind vanished"
        );
    }

    /// A track is found by its column's discriminant, and the tracks are laid out in
    /// [`Column::ALL`] order. Otherwise every cell after the first mismatch is painted
    /// into the next column.
    #[test]
    fn every_column_indexes_its_own_track() {
        for (index, column) in Column::ALL.iter().enumerate() {
            assert_eq!(*column as usize, index, "{column:?}");
        }
    }

    /// A width too small for the fixed columns still lays out: every track shrinks, and
    /// none runs past the edge or turns negative.
    #[test]
    fn a_width_below_the_fixed_columns_shrinks_every_track_instead_of_going_negative() {
        for width in [0.0_f32, 40.0, 120.0] {
            let tracks = tracks(width, &[None; 8]);
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

        // The search is a case-insensitive substring match on the name only.
        let found = arrange(held(), "AFRICA", Column::Name, Order::Up);
        assert_eq!(names(&found), ["africa bass", "Africa Split"]);
        assert!(arrange(held(), "nothing at all", Column::Name, Order::Up).is_empty());

        // The sort orders what the search left, in either direction.
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

    /// A kind, a place, and a tag narrow together, and an asset in both places passes a
    /// filter naming either place.
    #[test]
    fn the_kind_place_and_tag_filters_compose_over_the_row_model() {
        use crate::filter::Narrow;

        let Bench {
            mut workspace,
            mut device,
            mut log,
            ..
        } = Bench::new();
        let mut tags = Tags::default();

        let bytes = Fresh::Program.bytes().unwrap();
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
            .and_then(|entity| entity.saved.crc32())
            .expect("every CBIN container has one");
        device.pretend_bodies(
            ObjectClass::Program,
            7,
            &[Some(("Africa Split", crc)), Some(("Squabble B", crc ^ 1))],
        );
        device.relink(&mut workspace);
        workspace.create(Fresh::Live, &mut log).unwrap();
        let sunday = tags.make("Sunday").unwrap();
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

        // A tag narrows what the kind and the place left.
        filter.narrow(Narrow::Tag(sunday));
        assert_eq!(names(&filter), ["Africa-Split.ne5p"]);
    }

    /// A linked asset is in both places, and the sign says whether the slot still
    /// reports the asset's saved checksum. Saving an edit turns the sign over, and the
    /// link stays where it was.
    #[test]
    fn a_linked_asset_is_in_both_places_and_says_when_the_two_stop_agreeing() {
        let Bench {
            mut workspace,
            mut device,
            mut log,
            ..
        } = Bench::new();
        let tags = Tags::default();

        let bytes = Fresh::Program.bytes().unwrap();
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
            .and_then(|entity| entity.saved.crc32())
            .expect("every CBIN container has one");
        let filter = Filter::default();
        let where_ = |workspace: &Workspace, device: &Device| {
            rows(workspace, &device.state, &Queue::default(), &tags, &filter)
                .into_iter()
                .find(|row| matches!(row.item, Item::Local(_)))
                .map(|row| row.where_)
        };
        // Nothing read: nothing is known about the other copy.
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

    /// ⚠️ One rule for the word, the dot, and the count. An asset the attached instrument
    /// refuses has no link, however well its origin names a slot. Comparing it with that
    /// slot would paint a difference and offer a send that [`crate::queue::enqueue`]
    /// refuses on every click.
    #[test]
    fn a_foreign_asset_is_neither_marked_against_a_slot_nor_counted_as_changed() {
        let Bench {
            mut workspace,
            mut device,
            queue,
            mut log,
            ..
        } = Bench::new();
        let class = ObjectClass::Program;
        let held_at = at(6, 0);

        let bytes = Fresh::Stage4Program.bytes().unwrap();
        let id = workspace.ingest(
            "Africa-Split.ns4p".into(),
            Origin::Device { class, at: held_at },
            bytes,
            &mut log,
        );
        let crc = workspace
            .get(id)
            .and_then(|entity| entity.saved.crc32())
            .expect("every CBIN container has one");
        // An Electro 5 whose Programs 7:1 holds something other than this.
        device.pretend_bodies(class, 7, &[Some(("Squabble B", crc ^ 1))]);
        device.relink(&mut workspace);

        let entity = workspace.get(id).expect("it is on the list");
        assert!(!fit(&device.state, entity).allowed(), "a Stage 4 program");
        assert_eq!(whereabouts(entity, &device.state, &queue), Where::Foreign);
        assert_eq!(keyboard_mark(entity, &device.state, &queue), None);
        assert!(crate::queue::changed(&workspace, &device.state, &queue).is_empty());
    }

    /// Equality is claimed only on evidence. A settings folder holds one slot that
    /// reports no checksum, so an asset matched to it is in both places with nothing known
    /// about the two bodies, until a read fetches the occupant or this app writes the
    /// bytes there.
    #[test]
    fn a_slot_reporting_no_checksum_says_nothing_until_it_is_read_or_written() {
        let Bench {
            mut workspace,
            mut device,
            mut tabs,
            mut queue,
            mut log,
            ..
        } = Bench::new();
        let tags = Tags::default();
        let class = ObjectClass::Settings;
        let held_at = at(6, 0);

        let id = workspace.create(Fresh::Settings, &mut log).unwrap();
        let bytes = workspace.get(id).unwrap().bytes.to_vec();
        let held = workspace.get(id).unwrap();
        let crc = held.saved.crc32().expect("a container");
        let body_len = held.container.as_ref().expect("a container").body_len();

        // The walk reports a name and this asset's length, and no checksum.
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
        device.poll(&mut log, &mut workspace, &mut tabs, &mut queue);
        assert_eq!(
            workspace.get(id).unwrap().link,
            Some((class, held_at)),
            "the folder's only slot"
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
                keyboard_mark(entity, &device.state, queue),
            )
        };

        assert_eq!(
            said(&workspace, &device, &queue),
            (None, Some(Where::Both(None)), Some(Mark::Unknown)),
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

        // This app wrote the bytes, and nothing is waiting to change them.
        queue.clear();
        workspace.landed(id, class, held_at, bytes.clone());
        device.relink(&mut workspace);
        assert_eq!(
            said(&workspace, &device, &queue),
            (
                Some(true),
                Some(Where::Both(Some(true))),
                Some(Mark::Agrees)
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
                Some(Mark::Differs)
            ),
            "a checksum the instrument reports wins over this app's own write"
        );
    }

    /// ⚠️ An Electro 5 factory program is a type-0 file, and so is every copy Nord Sound
    /// Manager exports from one. Its header carries no body checksum, so nothing links
    /// it to the slot it came off unless the checksum is computed from the body.
    #[test]
    fn a_type_0_asset_links_to_the_slot_reporting_its_body_checksum() {
        let Bench {
            mut workspace,
            mut device,
            mut log,
            ..
        } = Bench::new();
        let (queue, tags) = (Queue::default(), Tags::default());
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
            .and_then(|entity| entity.saved.crc32())
            .expect("computed from the body, since the header carries no checksum");

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
            keyboard_mark(workspace.get(id).unwrap(), &device.state, &queue),
            Some(Mark::Agrees)
        );
    }

    /// A link is matched on the saved bytes, as the sign and the dot are. An asset with an
    /// unsaved edit still links to the slot holding what it was saved as. Saving the edit
    /// moves the baseline off the slot and turns both signs over, and the link stays
    /// where it was.
    #[test]
    fn an_unsaved_edit_still_links_to_the_slot_holding_the_saved_bytes() {
        let Bench {
            mut workspace,
            mut device,
            mut log,
            ..
        } = Bench::new();
        let (queue, tags) = (Queue::default(), Tags::default());
        let held_at = at(6, 0);

        let id = workspace.create(Fresh::Program, &mut log).unwrap();
        let bytes = workspace.get(id).unwrap().bytes.to_vec();
        let saved_as = workspace.get(id).unwrap().saved.crc32().unwrap();

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
            "the edit changed the current bytes"
        );

        let where_ = |workspace: &Workspace, device: &Device| {
            rows(workspace, &device.state, &queue, &tags, &Filter::default())
                .into_iter()
                .find(|row| matches!(row.item, Item::Local(_)))
                .map(|row| row.where_)
        };
        let mark = |workspace: &Workspace, device: &Device| {
            keyboard_mark(workspace.get(id).unwrap(), &device.state, &queue)
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
            "the only slot holding it is the linked one"
        );
        assert_eq!(mark(&workspace, &device), Some(Mark::Agrees));
        assert_eq!(where_(&workspace, &device), Some(Where::Both(Some(true))));

        workspace.mark_saved(id);
        device.relink(&mut workspace);
        assert_eq!(
            workspace.get(id).unwrap().link,
            Some((ObjectClass::Program, held_at)),
            "no slot holds the new baseline, so the link stays put"
        );
        assert_eq!(mark(&workspace, &device), Some(Mark::Differs));
        assert_eq!(where_(&workspace, &device), Some(Where::Both(Some(false))));
    }

    /// An edit an editor has not yet applied to the bytes gets the same star as any other.
    /// A piano library's plan changes no byte, and the star is the only sign on the row
    /// that the file differs from what the user is editing. The dot is about the slot and
    /// still reflects the saved bytes.
    #[test]
    fn a_row_wears_the_star_for_an_edit_that_is_still_an_editors_plan() {
        let Bench {
            mut workspace,
            mut device,
            mut log,
            ..
        } = Bench::new();
        let (queue, tags) = (Queue::default(), Tags::default());

        let id = workspace.create(Fresh::Program, &mut log).unwrap();
        let saved_as = workspace
            .get(id)
            .and_then(|entity| entity.saved.crc32())
            .expect("every CBIN container has one");
        device.pretend_bodies(ObjectClass::Program, 7, &[Some(("Africa Split", saved_as))]);
        device.relink(&mut workspace);

        let starred = |workspace: &Workspace| {
            rows(workspace, &device.state, &queue, &tags, &Filter::default())
                .into_iter()
                .find(|row| matches!(row.item, Item::Local(_)))
                .expect("the asset is listed")
                .unsaved
        };
        assert!(!starred(&workspace));

        workspace.mark_pending(id, true);
        assert!(starred(&workspace), "the plan is an edit the row shows");
        assert_eq!(
            keyboard_mark(workspace.get(id).unwrap(), &device.state, &queue),
            Some(Mark::Agrees),
            "and the slot still holds what this was saved as",
        );
    }

    /// The state filter narrows the library to rows that need something. A write already
    /// waiting moves its row from "differs" to "waiting", so one task is not counted
    /// twice.
    #[test]
    fn a_row_waiting_to_be_sent_is_not_also_one_that_differs() {
        use crate::filter::{Narrow, State};

        let Bench {
            mut workspace,
            mut device,
            mut log,
            ..
        } = Bench::new();
        let (tags, mut queue) = (Tags::default(), Queue::default());

        let id = workspace.create(Fresh::Program, &mut log).unwrap();
        let bytes = workspace.get(id).unwrap().bytes.to_vec();
        let crc = workspace
            .get(id)
            .and_then(|entity| entity.saved.crc32())
            .expect("every CBIN container has one");
        device.pretend_bodies(ObjectClass::Program, 7, &[Some(("Africa Split", crc))]);
        device.relink(&mut workspace);
        // Edited and saved: the link stays where it was and the two bodies differ.
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
        let Bench {
            mut workspace,
            mut device,
            mut log,
            ..
        } = Bench::new();
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
            tooltip(&row, Column::At, None, &tags, &workspace, &device)
                .ends_with("matched by name"),
            "{}",
            tooltip(&row, Column::At, None, &tags, &workspace, &device)
        );
    }

    /// Four marks, four sentences, four colors. Two marks a reader cannot tell apart
    /// explain nothing.
    #[test]
    fn every_mark_is_painted_and_said_apart_from_the_others() {
        let all = [Mark::Agrees, Mark::Differs, Mark::Unknown, Mark::Unsaved];
        let said: BTreeSet<&str> = all.iter().map(|mark| mark_words(*mark)).collect();
        assert_eq!(said.len(), all.len(), "{said:?}");

        for visuals in [egui::Visuals::dark(), egui::Visuals::light()] {
            let inks: BTreeSet<[u8; 4]> = all
                .iter()
                .map(|mark| mark_ink(*mark, &visuals).to_array())
                .collect();
            assert_eq!(inks.len(), all.len(), "{inks:?}");
        }
    }

    /// A Where cell's hover says what the tree's dot for the row claims, as well as where
    /// the row is.
    #[test]
    fn a_where_cell_says_what_the_trees_mark_claims() {
        let Bench {
            workspace, device, ..
        } = Bench::new();
        let tags = Tags::default();
        let held = row(
            "Africa Split",
            Kind::Program,
            Where::Both(Some(false)),
            Some(at(6, 0)),
            121,
        );

        let plain = tooltip(&held, Column::Where, None, &tags, &workspace, &device);
        assert_eq!(plain, Where::Both(Some(false)).sentence());
        let marked = tooltip(
            &held,
            Column::Where,
            Some(Mark::Differs),
            &tags,
            &workspace,
            &device,
        );
        assert!(marked.starts_with(&plain), "{marked}");
        assert!(marked.ends_with(mark_words(Mark::Differs)), "{marked}");
    }

    /// The sentence about a selection: where the picked rows go, how many of those slots
    /// are taken, what is already waiting, and what nothing has named.
    #[test]
    fn the_selection_says_where_it_goes_and_what_it_would_replace() {
        let Bench {
            mut workspace,
            mut device,
            mut queue,
            mut log,
            ..
        } = Bench::new();
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
            consequence(&picked, &device.state, &queue).as_deref(),
            Some(
                "→ Programs 7:1–7:4 · 2 slots occupied · 1 needs a piano the instrument has not \
                 named"
            )
        );

        // One of them is already in the queue, and the sentence says so.
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
        let said = consequence(&picked, &device.state, &queue).unwrap_or_default();
        assert!(said.contains("1 already waiting"), "{said}");

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
            None
        );
        assert_eq!(consequence(&[], &device.state, &queue), None);
    }

    /// The middle of the table's row `index`, far enough in to land on its name.
    fn on_row(index: usize) -> egui::Pos2 {
        let top = crate::panel::VIEW_HEADER + HEAD + ABOVE + (ROW + ROW_GAP) * index as f32;
        egui::pos2(160.0, top + ROW / 2.0)
    }

    /// A row joins the selection by a ⌘-click on it, beside what is already selected,
    /// where a plain click selects it alone.
    #[test]
    fn a_command_click_on_a_row_adds_it_beside_what_is_selected() {
        let Bench {
            ctx,
            mut browser,
            mut workspace,
            device,
            queue,
            shell,
            mut log,
            ..
        } = Bench::new();
        let mut library = Library::default();
        for kind in [Fresh::Program, Fresh::Live, Fresh::Settings] {
            workspace.create(kind, &mut log).unwrap();
        }

        // A second, so no click lands soon enough after the last to count as a double.
        let mut time = 0.0;
        let mut click = |index: usize, modifiers: egui::Modifiers| {
            let on = on_row(index);
            let frames = [
                vec![egui::Event::PointerMoved(on)],
                vec![testing::button(on, true), testing::button(on, false)],
                Vec::new(),
            ];
            for events in frames {
                time += 1.0;
                let input = egui::RawInput {
                    modifiers,
                    time: Some(time),
                    ..testing::screen(egui::vec2(900.0, 540.0), events)
                };
                testing::run(&ctx, input, |ctx| {
                    egui::CentralPanel::default()
                        .frame(egui::Frame::new())
                        .show(ctx, |ui| {
                            library.ui(ui, &mut browser, &workspace, &device, &queue, &shell);
                        });
                });
            }
            browser.picked().items().count()
        };
        click(0, egui::Modifiers::NONE);
        assert_eq!(
            click(1, egui::Modifiers::COMMAND),
            2,
            "the ⌘-click added its row and kept the one already selected"
        );
        assert_eq!(
            click(2, egui::Modifiers::NONE),
            1,
            "a plain click selected its row alone"
        );
    }

    /// A tag narrowing the library shows as a chip in the header, and a click on the chip
    /// stops narrowing by it.
    #[test]
    fn a_click_on_a_tag_chip_turns_its_filter_off() {
        let mut bench = Bench::new();
        let mut library = Library::default();
        bench.act(vec![Act::NewTag(Vec::new())]);
        let tag = bench.browser.tags().all()[0].id;
        bench.act(vec![Act::RenameTag {
            id: tag,
            name: "Friday".into(),
        }]);
        bench.shell.filter.narrow(Narrow::Tag(tag));

        let ctx = bench.ctx.clone();
        let mut frame = |events: Vec<egui::Event>, bench: &mut Bench| {
            let input = testing::screen(egui::vec2(900.0, 540.0), events);
            testing::painted(&testing::run(&ctx, input, |ctx| {
                egui::CentralPanel::default()
                    .frame(egui::Frame::new())
                    .show(ctx, |ui| {
                        let acts = library.ui(
                            ui,
                            &mut bench.browser,
                            &bench.workspace,
                            &bench.device,
                            &bench.queue,
                            &bench.shell,
                        );
                        bench.act(acts);
                    });
            }))
        };
        let chip = testing::where_(&frame(Vec::new(), &mut bench), "Friday").center();
        frame(testing::click(chip), &mut bench);
        assert!(!bench.shell.filter.on(Narrow::Tag(tag)));
    }

    /// One mark, four states: no slot to stand on, a slot holding what this was saved
    /// as, a slot holding something else, and a write already waiting to change it.
    #[test]
    fn the_keyboard_mark_says_what_the_instrument_holds_where_this_stands() {
        let Bench {
            mut workspace,
            mut device,
            mut queue,
            mut log,
            ..
        } = Bench::new();
        let (good, warn) = (Mark::Agrees, Mark::Differs);

        let bytes = Fresh::Program.bytes().unwrap();
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
        // Made here, and no slot holds its body, so it has no link.
        let (_, typed) = crate::fields::apply(&bytes, &[("center_panel.gain".into(), "96".into())])
            .expect("the registry takes the set");
        let nowhere = workspace.ingest("typed.ne5p".into(), Origin::Fresh, typed, &mut log);
        let held = workspace.get(same).unwrap().saved.crc32().unwrap();

        let mark = |workspace: &Workspace, device: &Device, queue: &Queue, id: u64| {
            keyboard_mark(workspace.get(id).unwrap(), &device.state, queue)
        };
        // Nothing scanned: no row has a mark.
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
        device.relink(&mut workspace);
        assert_eq!(mark(&workspace, &device, &queue, same), Some(good));
        assert_eq!(mark(&workspace, &device, &queue, other), Some(warn));
        assert_eq!(
            mark(&workspace, &device, &queue, nowhere),
            None,
            "it has no link"
        );

        // A waiting write takes precedence: the slot agrees now, and is about to stop.
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
        let Bench {
            ctx,
            mut browser,
            mut workspace,
            device,
            queue,
            shell,
            mut log,
            ..
        } = Bench::new();
        let mut library = Library::default();

        let id = workspace.create(Fresh::Program, &mut log).unwrap();
        workspace.rename(id, "Africa Split".into());
        let draw = |library: &mut Library, browser: &mut Browser, workspace: &Workspace| {
            testing::words(&testing::run(&ctx, screen(), |ctx| {
                egui::CentralPanel::default()
                    .frame(egui::Frame::new())
                    .show(ctx, |ui| {
                        library.ui(ui, browser, workspace, &device, &queue, &shell);
                    });
            }))
        };

        let said = draw(&mut library, &mut browser, &workspace);
        assert!(said.contains(&"Africa Split".to_string()), "{said:?}");

        let bytes = workspace.get(id).unwrap().bytes.to_vec();
        let (_, edited) =
            crate::fields::apply(&bytes, &[("center_panel.gain".into(), "96".into())]).unwrap();
        workspace.replace_bytes(id, edited, &mut log);

        let said = draw(&mut library, &mut browser, &workspace);
        assert!(said.contains(&"Africa Split*".to_string()), "{said:?}");
    }

    /// The table asks for the files of the rows in view, and not of a row out of view.
    #[test]
    fn only_the_table_rows_in_view_are_asked_for() {
        let Bench {
            ctx,
            mut browser,
            mut workspace,
            device,
            queue,
            shell,
            mut log,
            ..
        } = Bench::new();
        let mut library = Library::default();
        let saved = (1..=500)
            .map(|id| crate::workspace::Saved {
                id,
                name: format!("Sound {id:04}.ne5p"),
                path: Some(crate::store::LibPath::root().join(&format!("Sound {id:04}.ne5p"))),
                origin: Origin::Fresh,
                content: crate::workspace::Content::unread(1),
                unsaved: None,
            })
            .collect();
        workspace.restore(saved, None, &mut log);
        testing::run(&ctx, screen(), |ctx| {
            egui::CentralPanel::default()
                .frame(egui::Frame::new())
                .show(ctx, |ui| {
                    library.ui(ui, &mut browser, &workspace, &device, &queue, &shell);
                });
        });
        assert!(workspace.wanted(1), "the first row is in view");
        assert!(!workspace.wanted(500), "the last row is not");
    }

    /// Frames of the tree beside the table take the library's families once, and a file
    /// read since is in them on the next frame.
    #[test]
    fn frames_take_the_families_once_per_change() {
        let Bench {
            ctx,
            mut browser,
            mut workspace,
            device,
            queue,
            shell,
            mut log,
            ..
        } = Bench::new();
        let mut library = Library::default();
        let saved = crate::workspace::Saved {
            id: 1,
            name: "Grand.ns4p".into(),
            path: Some(crate::store::LibPath::root().join("Grand.ns4p")),
            origin: Origin::Fresh,
            content: crate::workspace::Content::unread(1),
            unsaved: None,
        };
        workspace.restore(vec![saved], None, &mut log);
        let mut draw = |browser: &mut Browser, workspace: &Workspace| {
            testing::run(
                &ctx,
                testing::screen(egui::vec2(1200.0, 600.0), Vec::new()),
                |ctx| {
                    egui::SidePanel::left("browser")
                        .exact_width(crate::shell::BROWSER)
                        .show(ctx, |ui| {
                            browser.ui(ui, workspace, &device, &queue, &shell.filter);
                        });
                    egui::CentralPanel::default()
                        .frame(egui::Frame::new())
                        .show(ctx, |ui| {
                            library.ui(ui, browser, workspace, &device, &queue, &shell);
                        });
                },
            );
        };
        for _ in 0..3 {
            draw(&mut browser, &workspace);
        }
        assert_eq!(workspace.families_taken.get(), 1);
        assert_eq!(
            workspace.families_present(),
            [Family::Stage4],
            "what its name says"
        );

        workspace.take_wanted();
        workspace.took(1, Some(Fresh::Program.bytes().unwrap()), None);
        workspace.settle_files(&mut log);
        draw(&mut browser, &workspace);
        assert_eq!(workspace.families_taken.get(), 2);
        assert_eq!(
            workspace.families_present(),
            [Family::Electro5],
            "what it holds"
        );
    }

    /// Unread assets in the root of the library, each one byte long.
    fn unread(names: &[&str]) -> Vec<crate::workspace::Saved> {
        let ids = 1..;
        ids.zip(names)
            .map(|(id, name)| crate::workspace::Saved {
                id,
                name: name.to_string(),
                path: Some(crate::store::LibPath::root().join(name)),
                origin: Origin::Fresh,
                content: crate::workspace::Content::unread(1),
                unsaved: None,
            })
            .collect()
    }

    /// The names the table painted, top to bottom, as it shows them.
    fn names_painted(words: &[testing::Word], names: &[&str]) -> Vec<String> {
        let shown: Vec<&str> = names
            .iter()
            .map(|name| crate::strings::display_name(name))
            .collect();
        let mut found: Vec<&testing::Word> = words
            .iter()
            .filter(|word| shown.contains(&word.text.as_str()))
            .collect();
        found.sort_by(|a, b| a.rect.top().total_cmp(&b.rect.top()));
        found.iter().map(|word| word.text.clone()).collect()
    }

    #[test]
    fn the_order_is_taken_again_only_when_something_it_reads_changes() {
        let mut bench = Bench::new();
        let names = ["Cello.ne5p", "Alto.ne5p", "Bass.ne5p", "Zither.ne5p"];
        let saved = unread(&names[..3]);
        bench.workspace.restore(saved, None, &mut bench.log);
        let mut library = Library::default();
        let size = egui::vec2(900.0, 540.0);
        for _ in 0..3 {
            library_frame(&mut library, &mut bench, size, Vec::new());
        }
        assert_eq!(library.built, 1, "frames with nothing new keep the order");

        let bytes = Fresh::Program.bytes().unwrap();
        let read = crate::room::measure(bytes.len() as u64);
        bench.workspace.take_wanted();
        bench.workspace.took(2, Some(bytes), None);
        bench.workspace.settle_files(&mut bench.log);
        let words = library_frame(&mut library, &mut bench, size, Vec::new());
        assert_eq!(
            library.built, 1,
            "a read does not move a row sorted by name"
        );
        assert!(
            words.iter().any(|word| word.text == read),
            "the row in view shows the {read} read"
        );

        bench.workspace.rename(2, names[3].to_string());
        let words = library_frame(&mut library, &mut bench, size, Vec::new());
        assert_eq!(library.built, 2, "a rename moves the order");
        assert_eq!(names_painted(&words, &names), ["Bass", "Cello", "Zither"]);

        library.by = Column::Size;
        library_frame(&mut library, &mut bench, size, Vec::new());
        assert_eq!(library.built, 3, "a new column orders the table again");
        bench.workspace.take_wanted();
        bench
            .workspace
            .took(1, Some(Fresh::Program.bytes().unwrap()), None);
        bench.workspace.settle_files(&mut bench.log);
        let words = library_frame(&mut library, &mut bench, size, Vec::new());
        assert_eq!(library.built, 4, "sorted by size, a read may move a row");
        assert_eq!(
            names_painted(&words, &names),
            ["Bass", "Cello", "Zither"],
            "the one-byte row first"
        );
    }

    #[test]
    fn a_bank_dropped_for_a_write_leaves_the_table_at_once() {
        let mut bench = Bench::new();
        bench.device.pretend_partitions(&crate::device::ELECTRO5);
        bench
            .device
            .pretend_scanned(ObjectClass::Program, 7, &["Alto", "Bass"]);
        let mut library = Library::default();
        let size = egui::vec2(900.0, 540.0);
        let slots = ["Alto", "Bass"];
        let words = library_frame(&mut library, &mut bench, size, Vec::new());
        assert_eq!(names_painted(&words, &slots), slots);

        let at = Location::from_user(7, 1);
        let delete = crate::device::DeviceCmd::Delete {
            class: ObjectClass::Program,
            at,
        };
        bench.device.send(delete, &mut bench.log);
        bench.device.pump();
        library_frame(&mut library, &mut bench, size, Vec::new());
        assert_eq!(
            library.table.as_ref().map(|table| table.items.len()),
            Some(0),
            "the bank being written is read again, not drawn from what it held"
        );
    }

    #[test]
    fn frames_look_up_no_name_in_the_format_table() {
        let mut bench = Bench::new();
        let names: Vec<String> = (1..=300).map(|n| format!("Sound {n:03}.ne5p")).collect();
        let names: Vec<&str> = names.iter().map(String::as_str).collect();
        let saved = unread(&names);
        let looked = || crate::browser::TAGGED.with(std::cell::Cell::get);
        let before = looked();
        bench.workspace.restore(saved, None, &mut bench.log);
        let listed = looked() - before;
        assert!(
            listed <= names.len(),
            "{listed} lookups to list {}",
            names.len()
        );

        let mut library = Library::default();
        for _ in 0..3 {
            library_frame(
                &mut library,
                &mut bench,
                egui::vec2(900.0, 540.0),
                Vec::new(),
            );
        }
        bench.workspace.rename(7, "Renamed.ne5p".to_string());
        library_frame(
            &mut library,
            &mut bench,
            egui::vec2(900.0, 540.0),
            Vec::new(),
        );
        assert_eq!(
            looked() - before,
            listed + 1,
            "only the rename looked a name up again"
        );
    }

    /// A frame of the table at the center's width with no dock open.
    fn screen() -> egui::RawInput {
        testing::screen(egui::vec2(900.0, 540.0), Vec::new())
    }

    /// The Kind column shows the same word as the browser, so the table and the tree
    /// cannot name one thing two ways, and its heading sorts by it.
    #[test]
    fn the_kind_column_writes_the_browsers_own_word_and_sorts_by_it() {
        let Bench {
            ctx,
            mut browser,
            mut workspace,
            device,
            queue,
            shell,
            mut log,
            ..
        } = Bench::new();
        let mut library = Library::default();
        for kind in [Fresh::Settings, Fresh::Program] {
            workspace.create(kind, &mut log).unwrap();
        }

        let said = testing::words(&testing::run(&ctx, screen(), |ctx| {
            egui::CentralPanel::default()
                .frame(egui::Frame::new())
                .show(ctx, |ui| {
                    library.ui(ui, &mut browser, &workspace, &device, &queue, &shell);
                });
        }));
        assert!(said.contains(&"Kind".to_string()), "{said:?}");
        for word in ["program", "settings"] {
            assert!(said.contains(&word.to_string()), "{word}: {said:?}");
        }

        let held = rows(
            &workspace,
            &device.state,
            &queue,
            browser.tags(),
            &Filter::default(),
        );
        let ordered: Vec<String> = arrange(held, "", Column::Kind, Order::Up)
            .iter()
            .map(word)
            .collect();
        assert_eq!(ordered, ["program", "settings"]);
    }

    /// ⚠️ A table row starts the same drag as a tree row, and a drop files the asset the
    /// same way. Two drag paths would be two sets of rules for one gesture.
    #[test]
    fn a_row_dragged_from_the_table_onto_a_folder_files_the_asset() {
        let mut bench = Bench::new();
        let mut library = Library::default();
        let id = bench
            .workspace
            .create(Fresh::Program, &mut bench.log)
            .unwrap();
        bench.workspace.rename(id, "Africa Split".into());
        bench.act(vec![Act::NewFolder]);

        let ctx = bench.ctx.clone();
        let mut asked = Vec::new();
        let mut frame = |events: Vec<egui::Event>| {
            let input = testing::screen(egui::vec2(900.0, 540.0), events);
            testing::painted(&testing::run(&ctx, input, |ctx| {
                egui::SidePanel::left("places")
                    .exact_width(crate::shell::BROWSER)
                    .frame(egui::Frame::new())
                    .show(ctx, |ui| {
                        asked.extend(bench.browser.ui(
                            ui,
                            &bench.workspace,
                            &bench.device,
                            &bench.queue,
                            &Filter::default(),
                        ));
                    });
                egui::CentralPanel::default()
                    .frame(egui::Frame::new())
                    .show(ctx, |ui| {
                        library.ui(
                            ui,
                            &mut bench.browser,
                            &bench.workspace,
                            &bench.device,
                            &bench.queue,
                            &bench.shell,
                        );
                    });
            }))
        };

        // The new folder opens its rename editor, and Escape closes it. The row is found
        // in the table, right of the tree, and the folder in the tree.
        frame(Vec::new());
        let said = frame(vec![testing::key(egui::Key::Escape)]);
        let found = |text: &str, in_tree: bool| {
            said.iter()
                .find(|word| {
                    word.text == text && (word.rect.left() < crate::shell::BROWSER) == in_tree
                })
                .unwrap_or_else(|| panic!("{text} was not painted: {said:?}"))
                .rect
                .center()
        };
        let (from, onto) = (found("Africa Split", false), found("New folder", true));
        // The pointer presses on the table's row, carries it over the folder, and lets go.
        for events in [
            vec![egui::Event::PointerMoved(from)],
            vec![testing::button(from, true)],
            vec![egui::Event::PointerMoved(onto)],
            vec![testing::button(onto, false)],
        ] {
            frame(events);
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

    /// One frame of the library alone across `screen`, and the words it painted.
    fn library_frame(
        library: &mut Library,
        bench: &mut Bench,
        screen: egui::Vec2,
        events: Vec<egui::Event>,
    ) -> Vec<testing::Word> {
        let ctx = bench.ctx.clone();
        let output = testing::run(&ctx, testing::screen(screen, events), |ctx| {
            egui::CentralPanel::default()
                .frame(egui::Frame::new())
                .show(ctx, |ui| {
                    let acts = library.ui(
                        ui,
                        &mut bench.browser,
                        &bench.workspace,
                        &bench.device,
                        &bench.queue,
                        &bench.shell,
                    );
                    bench.act(acts);
                });
        });
        testing::painted(&output)
    }

    /// Dragging a column's right edge in the head sets the column's width without sorting
    /// by it, and a double click on the edge gives the column its own width back.
    #[test]
    fn a_column_edge_drags_to_a_width_and_a_double_click_resets_it() {
        let mut bench = Bench::new();
        bench
            .workspace
            .create(Fresh::Program, &mut bench.log)
            .unwrap();
        let mut library = Library::default();
        let screen = egui::vec2(900.0, 540.0);
        let mut frame =
            |library: &mut Library, events| library_frame(library, &mut bench, screen, events);
        let said = frame(&mut library, Vec::new());
        let name = testing::where_(&said, "Name");
        let edge = egui::pos2(name.left() - GAP / 2.0, name.center().y);
        let dragged = edge + egui::vec2(40.0, 0.0);

        frame(&mut library, vec![egui::Event::PointerMoved(edge)]);
        frame(&mut library, vec![testing::button(edge, true)]);
        frame(&mut library, vec![egui::Event::PointerMoved(dragged)]);
        frame(&mut library, vec![testing::button(dragged, false)]);
        let said = frame(&mut library, Vec::new());
        let moved = testing::where_(&said, "Name").left() - name.left();
        assert!((moved - 40.0).abs() < 0.5, "the name moved {moved}");
        assert_eq!(library.by, Column::Name, "and nothing was sorted");

        let at = dragged;
        frame(
            &mut library,
            vec![
                testing::button(at, true),
                testing::button(at, false),
                testing::button(at, true),
                testing::button(at, false),
            ],
        );
        let said = frame(&mut library, Vec::new());
        assert_eq!(library.widths, [None; 8]);
        assert_eq!(testing::where_(&said, "Name").left(), name.left());
    }

    /// Dragged widths come back in the next session; a width this build would not lay
    /// out, a column without an edge, or another version is left out.
    #[test]
    fn the_column_widths_come_back_and_nonsense_is_left_out() {
        let mut store = crate::testing::Fake::default();
        let mut before = Library::default();
        before.widths[Column::Kind as usize] = Some(150.0);
        before.widths[Column::Size as usize] = Some(70.0);
        before.keep(&mut store);
        let mut after = Library::default();
        after.restore(&store);
        assert_eq!(after.widths, before.widths);

        eframe::Storage::set_string(
            &mut store,
            Library::KEY,
            format!(
                "{}\nwidth\tkind\tabc\nwidth\tglyph\t50\nwidth\tname\t9000\n\
                 width\tneeds\t80\nwidth\tat\t60\n",
                Library::VERSION
            ),
        );
        let mut after = Library::default();
        after.restore(&store);
        let mut only_at = [None; 8];
        only_at[Column::At as usize] = Some(60.0);
        assert_eq!(after.widths, only_at);

        eframe::Storage::set_string(
            &mut store,
            Library::KEY,
            "drawbar library 0\nwidth\tat\t60\n".into(),
        );
        let mut after = Library::default();
        after.restore(&store);
        assert_eq!(after.widths, [None; 8]);
    }

    /// Paints the table headlessly at the center's width with both docks open and with
    /// none, and picks a row at each. Nothing checks pixels:
    /// this catches a layout that panics or an id that collides.
    #[test]
    fn the_table_paints_at_every_width_the_center_has() {
        let mut bench = Bench::new();
        let mut library = Library::default();

        for kind in [Fresh::Program, Fresh::Live, Fresh::Settings] {
            bench.workspace.create(kind, &mut bench.log).unwrap();
        }
        let device = &mut bench.device;
        device.pretend_scanned(ObjectClass::Program, 7, &["Africa Split", "", "Squabble B"]);
        device.pretend_scanned(ObjectClass::Piano, 1, &["Royal Grand 3D"]);
        device.pretend_scanned(ObjectClass::SetList, 1, &["Sunday"]);

        // Far enough in to land in the name column at either width.
        let on_a_row = on_row(0);
        let ctx = bench.ctx.clone();
        for width in [430.0_f32, 900.0] {
            // The pointer moves, then presses, then the next frame has a row picked.
            let frames: [Vec<egui::Event>; 4] = [
                Vec::new(),
                vec![egui::Event::PointerMoved(on_a_row)],
                vec![
                    testing::button(on_a_row, true),
                    testing::button(on_a_row, false),
                ],
                Vec::new(),
            ];
            for events in frames {
                let input = testing::screen(egui::vec2(width, 540.0), events);
                testing::run(&ctx, input, |ctx| {
                    // The frame the center uses: panels handle their own padding.
                    egui::CentralPanel::default()
                        .frame(egui::Frame::new())
                        .show(ctx, |ui| {
                            let acts = library.ui(
                                ui,
                                &mut bench.browser,
                                &bench.workspace,
                                &bench.device,
                                &bench.queue,
                                &bench.shell,
                            );
                            bench.act(acts);
                        });
                });
            }
            assert!(
                bench.browser.picked().sole().is_some(),
                "a click on a row selected it at {width}"
            );
        }
    }
}
