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
use crate::bring::{Rows, Sign, Synced};
use crate::browser::{qualifier, Act, Browser, Item, Kept, Kind, Qualifier};
use crate::device::{fit, place_of, read_only, Device, DeviceState, Linked};
use crate::filter::{Filter, Narrow, Place, State};
use crate::icon::{painted, Glyph};
use crate::link::{Standing, There};
use crate::panel::{cut, list_width, pill_at, row_ink, view_header, Track, ROW_INSET};
use crate::plays::Catalog;
use crate::queue::Queue;
use crate::shell::Shell;
use crate::strings::{counted, folder, place, shown};
use crate::tags::Tags;
use crate::workspace::{LocalEntity, Workspace};
use std::rc::Rc;

/// Which of the two places a row's contents are in.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Where {
    /// Linked to a slot of the attached keyboard that holds something, or that no walk
    /// has read, and how the two compare.
    Both(Standing),
    Computer,
    /// On this computer, and the attached instrument refuses it, so it can only stay
    /// here.
    Foreign,
    Keyboard,
}

impl Where {
    /// The short word on the Where column's pill.
    pub fn short(self) -> &'static str {
        match self {
            Where::Both(_) => "both",
            Where::Computer | Where::Foreign => "computer",
            Where::Keyboard => "keyboard",
        }
    }

    /// The full sentence, for the tooltip.
    pub fn sentence(self) -> &'static str {
        match self {
            Where::Both(Standing::Same) => "On this computer and in its slot, the same.",
            Where::Both(Standing::Ahead) => {
                "On this computer and in its slot, which holds an earlier version."
            }
            Where::Both(Standing::Theirs) => {
                "On this computer and in its slot, which the keyboard changed."
            }
            Where::Both(Standing::Both) => {
                "On this computer and in its slot, and both have changed."
            }
            Where::Both(Standing::Unread) => "On this computer and in a slot not read yet.",
            Where::Both(Standing::Gone) | Where::Computer => "On this computer only.",
            Where::Foreign => "On this computer only. Not for this keyboard.",
            Where::Keyboard => "On the keyboard only.",
        }
    }

    /// The places it is in, which a place filter checks.
    fn places(self) -> &'static [Place] {
        match self {
            Where::Both(_) => &[Place::Computer, Place::Keyboard],
            Where::Computer | Where::Foreign => &[Place::Computer],
            Where::Keyboard => &[Place::Keyboard],
        }
    }

    /// Sort rank: what needs the user first, then the rest of both places, this
    /// computer, and the instrument.
    fn rank(self) -> u8 {
        match self {
            Where::Both(Standing::Theirs | Standing::Both | Standing::Gone) => 0,
            Where::Both(Standing::Ahead) => 1,
            Where::Both(Standing::Unread) => 2,
            Where::Both(Standing::Same) => 3,
            Where::Computer => 4,
            Where::Foreign => 5,
            Where::Keyboard => 6,
        }
    }
}

/// The library (piano or sample) a row plays, which the row's bytes do not name.
///
/// ⚠️ A program file stores a bare id, and a name is found by hashing the names of the
/// pianos and samples on this computer and on the attached instrument. `Wanted` is an id
/// none of those has; an instrument not attached, or not fully read, may still hold it.
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
            Needs::Named { class, name } => {
                format!("It plays the {} “{name}”.", Kind::from_class(*class).chip())
            }
            Needs::Wanted { class, id } => format!(
                "It plays a {} that is not here ({id:#010x}).",
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
    /// The slot it is: a slot's own, or the one an asset is linked to.
    pub at: Option<(ObjectClass, Location)>,
    /// Whether the address says nothing yet of what the slot holds: its bank is unread,
    /// or the keyboard it is on is not attached.
    pub faint: bool,
    /// What the row's mark says of the asset and its slot, and its words.
    pub mark: Option<Mark>,
    pub said: String,
    pub size: u64,
    /// When its file was last modified, as [`LocalEntity::modified`] counts it. A slot, and
    /// an asset with no file on this computer, has none.
    pub modified: Option<u64>,
    pub needs: Needs,
}

impl Row {
    /// Where a send would write this row: the slot an asset is linked to, unless
    /// [`crate::device::read_only`] rules out its class. A row already on the instrument
    /// goes nowhere.
    fn destination(&self) -> Option<(ObjectClass, Location)> {
        let (class, at) = self.at?;
        (matches!(self.item, Item::Local(_)) && !read_only(class)).then_some((class, at))
    }
}

/// Everything the library holds, narrowed by kind, place, and tags.
///
/// The list on this computer comes first. A slot one of its assets is linked to is shown
/// in that asset's row: a program sent to 7:4 is one thing in two places, a
/// [`Where::Both`].
pub fn rows(
    workspace: &Workspace,
    device: &DeviceState,
    tags: &Tags,
    filter: &Filter,
    sync: &Rows,
) -> Vec<Row> {
    let mut rows = Vec::new();
    let mut claimed = BTreeSet::new();
    let kept = Kept::of(workspace);
    let instrument = device.product().and_then(Family::from_product);
    let catalog = Catalog::of(workspace, device);
    for entity in workspace.listed() {
        let worn = tags.worn(entity.id);
        let row = local(
            entity,
            device,
            worn.len(),
            (&kept, instrument, &catalog),
            sync,
        );
        // Claim the row's slot so the instrument's list does not repeat it.
        if let Some(slot) = row.at.filter(|_| device.connected()) {
            claimed.insert(Item::Slot {
                class: slot.0,
                at: slot.1,
            });
        }
        let state = state(row.mark);
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
                if claimed.contains(&Item::Slot { class, at }) {
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

/// What a row needs: Sync, or the user. A slot on the instrument needs nothing, since it
/// is what the instrument holds.
pub fn state(mark: Option<Mark>) -> Option<State> {
    match mark {
        Some(Mark::Up | Mark::Down) => Some(State::Sync),
        Some(Mark::Attention) => Some(State::Attention),
        Some(Mark::Unsaved) | None => None,
    }
}

/// The row for one item, built as [`rows`] builds it, so the inspector shows what the
/// table shows. A folder or a tag makes no row.
pub fn row_of(
    item: Item,
    workspace: &Workspace,
    device: &DeviceState,
    tags: &Tags,
    sync: &Rows,
) -> Option<Row> {
    let instrument = device.product().and_then(Family::from_product);
    let kept = Kept::of(workspace);
    let catalog = Catalog::of(workspace, device);
    row_with(
        item,
        workspace,
        device,
        tags,
        (&kept, instrument, &catalog),
        sync,
    )
}

/// [`row_of`], with what it reads of the whole list already taken.
#[allow(clippy::too_many_arguments)]
fn row_with(
    item: Item,
    workspace: &Workspace,
    device: &DeviceState,
    tags: &Tags,
    list: (&Kept, Option<Family>, &Catalog),
    sync: &Rows,
) -> Option<Row> {
    match item {
        Item::Local(id) => {
            let entity = workspace.get(id)?;
            let worn = tags.worn(id).len();
            Some(local(entity, device, worn, list, sync))
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
    tags: usize,
    (kept, instrument, catalog): (&Kept, Option<Family>, &Catalog),
    sync: &Rows,
) -> Row {
    let placed = place_of(device, entity);
    let linked = placed.filter(|_| device.connected());
    let marked = keyboard_mark(sync, entity.id);
    Row {
        item: Item::Local(entity.id),
        kind: Kind::of(entity),
        qualifier: qualifier(entity, kept, instrument),
        name: entity.name.clone(),
        tags,
        unsaved: entity.is_unsaved(),
        where_: whereabouts(entity, device, linked.as_ref()),
        at: placed.and_then(|placed| Some((placed.class, placed.at?))),
        faint: placed.is_none_or(|placed| placed.faint()),
        mark: marked.map(|(mark, _)| mark),
        said: marked.map_or_else(String::new, |(_, said)| said.to_string()),
        size: entity.size(),
        modified: entity.modified(),
        needs: wanted(entity, catalog),
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
        faint: false,
        mark: None,
        said: String::new(),
        size: u64::from(info.body_len),
        modified: None,
        needs: played(class, at, device),
    }
}

/// Where an asset's contents are: in both places while it is linked to a slot of the
/// attached keyboard that is not empty.
fn whereabouts(entity: &LocalEntity, device: &DeviceState, linked: Option<&Linked>) -> Where {
    match linked {
        Some(linked) if linked.there != There::Vacant => Where::Both(linked.standing),
        _ if !fit(device, entity).allowed() => Where::Foreign,
        _ => Where::Computer,
    }
}

/// What a mark on a row claims: the mark the tree paints at a row's right end, and the
/// star after a name. A file's Sync row gives the first three, and its words.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Mark {
    Up,
    Down,
    Attention,
    Unsaved,
}

impl Mark {
    pub fn of(sign: Sign) -> Mark {
        match sign {
            Sign::Up => Mark::Up,
            Sign::Down => Mark::Down,
            Sign::Attention => Mark::Attention,
        }
    }
}

/// The color a mark is painted in, everywhere it is painted.
pub fn mark_ink(mark: Mark, visuals: &egui::Visuals) -> egui::Color32 {
    match mark {
        Mark::Up | Mark::Down => warn(visuals),
        Mark::Attention => crate::app::bad(visuals),
        Mark::Unsaved => visuals.text_color(),
    }
}

/// What the unsaved mark means, in words.
pub const UNSAVED: &str = "Edited since it was last saved";

/// The character a mark is drawn as in front of a name or an address.
pub fn mark_sign(mark: Mark) -> &'static str {
    match mark {
        Mark::Up => "↑",
        Mark::Down => "↓",
        Mark::Attention => "!",
        Mark::Unsaved => "*",
    }
}

/// The mark of the file `id` and its words, from its Sync row.
pub fn keyboard_mark(rows: &Rows, id: u64) -> Option<(Mark, &str)> {
    let (sign, said) = rows.mark(id)?;
    Some((Mark::of(sign), said))
}

/// The first library a program names, by name where a piano or sample here or on the
/// attached instrument goes by its id. A program not read this session names what a
/// read of it found before.
pub(crate) fn wanted(entity: &LocalEntity, catalog: &Catalog) -> Needs {
    let Some(plays) = crate::plays::named(entity).first() else {
        return Needs::Nothing;
    };
    let (class, id) = plays.key();
    match catalog.get((class, id)) {
        Some(found) => Needs::Named {
            class,
            name: found.name.clone(),
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
    /// The row's [`Mark`] against its slot.
    Mark,
    Name,
    Tags,
    Where,
    At,
    Size,
    Modified,
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
    pub const ALL: [Column; COLUMNS] = [
        Column::Glyph,
        Column::Kind,
        Column::Mark,
        Column::Name,
        Column::Tags,
        Column::Where,
        Column::At,
        Column::Size,
        Column::Modified,
        Column::Needs,
    ];

    /// The column's heading. The glyph column has none: its word is the kind beside it.
    fn head(self) -> &'static str {
        match self {
            Column::Glyph | Column::Mark => "",
            Column::Kind => "Kind",
            Column::Name => "Name",
            Column::Tags => "Tags",
            Column::Where => "Where",
            Column::At => "At",
            Column::Size => "Size",
            Column::Modified => "Modified",
            Column::Needs => "Needs",
        }
    }

    /// What the column asks for: a fixed width, or a share of what the fixed ones leave.
    fn track(self) -> Track {
        match self {
            Column::Glyph => Track::Px(16.0),
            Column::Mark => Track::Px(MARK),
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
            Column::Modified => Track::Px(MODIFIED_W),
            Column::Needs => Track::Share(1.3),
        }
    }

    /// Whether the column's right edge can be dragged. Needs, the last, keeps its share so
    /// the columns always fill the width; while it is hidden, the name keeps its share in
    /// its place and cannot be dragged either.
    fn resizable(self) -> bool {
        !matches!(self, Column::Glyph | Column::Mark | Column::Needs)
    }

    /// The column's name where its width is kept.
    fn key(self) -> &'static str {
        match self {
            Column::Glyph => "glyph",
            Column::Kind => "kind",
            Column::Mark => "mark",
            Column::Name => "name",
            Column::Tags => "tags",
            Column::Where => "where",
            Column::At => "at",
            Column::Size => "size",
            Column::Modified => "modified",
            Column::Needs => "needs",
        }
    }
}

const COLUMNS: usize = 10;

/// The widths columns were dragged to, by column; `None` keeps the column's own track.
pub type Widths = [Option<f32>; COLUMNS];

/// The gap between two columns.
const GAP: f32 = 10.0;

/// The narrowest and widest a column can be dragged to.
const COLUMN_LEAST: f32 = 24.0;
const COLUMN_MOST: f32 = 480.0;

/// The width of the grip on a column's right edge in the head.
const GRIP: f32 = 8.0;

/// The columns that give way to the name in a narrow table, first to go first.
const YIELDING: [Column; 8] = [
    Column::Modified,
    Column::Needs,
    Column::Tags,
    Column::Size,
    Column::Kind,
    Column::At,
    Column::Where,
    Column::Mark,
];

/// The width of the Modified column, which holds a day and a time: `28 May 2026, 20:00`.
const MODIFIED_W: f32 = 112.0;

/// The width the name keeps while another column can give way: about 16 characters.
const NAME_LEAST: f32 = 100.0;

/// Where each column sits across `width`, laid out by [`crate::panel::tracks`], with any
/// dragged `widths` in place of the columns' own tracks.
///
/// Where the name would be narrower than about 16 characters, or than a width it was
/// dragged to, the secondary columns are hidden in turn, least needed first, until it is
/// not. A hidden column's track is empty, at the end of the one before it. Needs shows
/// only while an instrument is `attached`, since only an instrument names what a sound
/// plays. While Needs is hidden, the name takes its own share whatever it was dragged to,
/// so the columns still fill the width.
pub fn tracks(width: f32, widths: &Widths, attached: bool) -> [Range<f32>; COLUMNS] {
    let wanted = Column::ALL.map(|column| match widths[column as usize] {
        Some(px) => Track::Px(px),
        None => column.track(),
    });
    let least = widths[Column::Name as usize].map_or(NAME_LEAST, |px| px.min(NAME_LEAST));
    let mut shown = Column::ALL.map(|column| attached || column != Column::Needs);
    let mut laid = lay(width, &wanted, &shown);
    for column in YIELDING {
        let name = &laid[Column::Name as usize];
        if name.end - name.start >= least {
            break;
        }
        shown[column as usize] = false;
        laid = lay(width, &wanted, &shown);
    }
    laid
}

/// The `shown` columns laid out across `width`, and the others empty.
fn lay(width: f32, wanted: &[Track; COLUMNS], shown: &[bool; COLUMNS]) -> [Range<f32>; COLUMNS] {
    let fills = !shown[Column::Needs as usize];
    let kept: Vec<Track> = (0..COLUMNS)
        .filter(|&at| shown[at])
        .map(|at| match fills && at == Column::Name as usize {
            true => Column::Name.track(),
            false => wanted[at],
        })
        .collect();
    let mut held = crate::panel::tracks(width, &kept, GAP).into_iter();
    let mut end = 0.0;
    std::array::from_fn(|at| {
        let track = match shown[at] {
            true => held.next().unwrap_or(end..end),
            false => end..end,
        };
        end = track.end;
        track
    })
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
    Address((bool, Option<ObjectClass>, u32, u32)),
}

impl Key {
    fn of(by: Column, row: &Row, name: String) -> Key {
        let column = match by {
            // The glyph and the word beside it are the same fact, so a click on either
            // orders the table the same way.
            Column::Glyph | Column::Kind => Ranked::Text(word(row)),
            Column::Name => Ranked::Name,
            Column::Tags => Ranked::Count(row.tags as u64),
            Column::Mark | Column::Where => Ranked::Count(u64::from(row.where_.rank())),
            Column::At => Ranked::Address(address(row)),
            Column::Size => Ranked::Count(row.size),
            // A row with no time sorts as the oldest.
            Column::Modified => Ranked::Count(row.modified.unwrap_or(0)),
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
fn address(row: &Row) -> (bool, Option<ObjectClass>, u32, u32) {
    match row.at {
        Some((class, at)) => (false, Some(class), at.bank, at.slot),
        None => (true, None, 0, 0),
    }
}

/// What sending the picked rows would do, in one sentence, or `None` when no row is
/// bound for a slot and nothing about them needs saying.
pub fn consequence(rows: &[&Row], device: &DeviceState, queue: &Queue) -> Option<String> {
    let mut going: Vec<(ObjectClass, Location)> =
        rows.iter().filter_map(|row| row.destination()).collect();
    going.sort_unstable_by_key(|(class, at)| (*class, at.bank, at.slot));

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
        said.push(format!("{waiting} already in Sync"));
    }
    for class in [ObjectClass::Piano, ObjectClass::Sample] {
        let short = rows
            .iter()
            .filter(|row| row.needs.short() == Some(class))
            .count();
        let named = Kind::from_class(class).chip();
        match short {
            0 => {}
            1 => said.push(format!("1 needs a {named} that is not here")),
            n => said.push(format!("{n} need a {named} that is not here")),
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

/// The width of the mark column.
const MARK: f32 = 10.0;

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
    synced: Synced,
    /// What the rows find pianos and samples by, and the list and instrument revisions it
    /// was taken at.
    catalog: Option<((u64, u64), Catalog)>,
    /// How many times the order of the rows has been taken.
    #[cfg(test)]
    pub(crate) built: usize,
}

impl Default for Library {
    fn default() -> Library {
        Library {
            by: Column::Name,
            order: Order::Up,
            widths: [None; COLUMNS],
            table: None,
            synced: Synced::default(),
            catalog: None,
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
    /// holds (its kind, where it is, its size, its file's time, what it needs), which any
    /// change to the list may move. The names, slots, tags and queue they read otherwise
    /// are covered by the rest.
    revision: Option<u64>,
    layout: u64,
    device: u64,
    queue: Vec<(crate::queue::Waiting, bool)>,
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
            queue: queue.shape().collect(),
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
        let counts = self.synced(workspace, device, queue).counts();
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
        let sync = self.synced(workspace, device, queue);
        let held = rows(workspace, &device.state, tags, &shell.filter, &sync);
        let held = arrange(held, &shell.omnibox, self.by, self.order);
        Table {
            sources,
            items: held.iter().map(|row| row.item).collect(),
        }
    }

    /// The Sync rows. See [`Synced::of`].
    pub(crate) fn synced(
        &mut self,
        workspace: &Workspace,
        device: &Device,
        queue: &Queue,
    ) -> Rc<Rows> {
        self.synced.of(workspace, device, queue)
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
        let sync = self.synced(workspace, device, queue);
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
        let tracks = tracks(
            (width - 2.0 * CELL_PAD).max(0.0),
            &self.widths,
            device.state.connected(),
        );
        if tracks[self.by as usize].is_empty() {
            self.by = Column::Name;
            self.order = Order::Up;
            ui.ctx().request_repaint();
        }
        self.head(ui, width, &tracks);
        ui.add_space(ABOVE);
        if items.is_empty() {
            return nothing(ui);
        }

        ui.spacing_mut().item_spacing.y = ROW_GAP;
        let taken = (workspace.revision(), device.revision());
        if self.catalog.as_ref().is_none_or(|(at, _)| *at != taken) {
            self.catalog = Some((taken, Catalog::of(workspace, &device.state)));
        }
        let Some((_, catalog)) = &self.catalog else {
            return;
        };
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
                            tags,
                            (&kept, instrument, catalog),
                            &sync,
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
    fn grips(&mut self, ui: &mut egui::Ui, content: egui::Rect, tracks: &[Range<f32>; COLUMNS]) {
        let fills = tracks[Column::Needs as usize].is_empty();
        for (column, track) in Column::ALL.iter().zip(tracks) {
            if !column.resizable() || track.is_empty() || (fills && *column == Column::Name) {
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
    fn head(&mut self, ui: &mut egui::Ui, width: f32, tracks: &[Range<f32>; COLUMNS]) {
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
            if column.head().is_empty() || track.is_empty() {
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
/// tree's rows. A chip counting only what passes its own filter would change the moment
/// it was clicked.
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
            (State::Attention, Glyph::CircleAlert, counts[1]),
            (State::Sync, Glyph::RefreshCw, counts[0]),
        ];
        for (state, glyph, count) in states {
            let on = filter.on(Narrow::State(state));
            // A chip whose count is zero stays while it is the active filter, so there is
            // always something to click to clear it.
            if count == 0 && !on {
                continue;
            }
            let text = format!("{} {count}", state.title());
            let signal = match state {
                State::Sync => warn(ui.visuals()),
                State::Attention => crate::app::bad(ui.visuals()),
            };
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
        narrowed.push(state.title().to_lowercase());
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
    tracks: &[Range<f32>; COLUMNS],
) -> Option<Column> {
    let at = response.interact_pointer_pos().or(response.hover_pos())?;
    let x = at.x - content.left();
    Column::ALL
        .iter()
        .zip(tracks)
        .find(|(_, track)| (track.start..track.end + GAP).contains(&x))
        .map(|(column, _)| *column)
}

/// The Where pill's ink and fill: the accent while a write to the row's slot is waiting.
/// The mark column says the rest.
///
/// ⚠️ On the selection fill every signal fails contrast, so a selected row's pill takes
/// the selection's text color.
fn where_look(
    state: Option<State>,
    selected: bool,
    visuals: &egui::Visuals,
) -> (egui::Color32, egui::Color32) {
    let signal = match state {
        Some(State::Sync) => Some(accent(visuals)),
        Some(State::Attention) | None => None,
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
/// ⚠️ Nothing inside is a widget: a label allocates a hover rect that wins the hit test
/// over the row, so a click would land on whichever word is under the pointer.
#[allow(clippy::too_many_arguments)]
fn paint(
    ui: &mut egui::Ui,
    row: &Row,
    width: f32,
    tracks: &[Range<f32>; COLUMNS],
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
    if let Some(mark) = row.mark {
        // ⚠️ On the selection fill every signal fails contrast, as in `where_look`.
        let tint = match selected {
            true => ink,
            false => mark_ink(mark, &visuals),
        };
        let font = egui::FontId::new(NAME, crate::app::bold());
        write(
            cell(Column::Mark),
            mark_sign(mark),
            egui::TextFormat::simple(font, tint),
        );
    }
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
    let tags = cell(Column::Tags);
    if row.tags > 0 && tags.width() > 0.0 {
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
    let (pill_ink, pill_fill) = where_look(state(row.mark), selected, &visuals);
    pill_at(
        &painter,
        where_.left(),
        where_.center().y,
        where_.width(),
        row.where_.short(),
        pill_ink,
        pill_fill,
    );
    let at = row.at.map_or_else(|| "—".to_string(), |(_, at)| shown(at));
    let at_ink = match row.faint || row.at.is_none() {
        true => egui::TextFormat::simple(egui::FontId::monospace(MONO), quiet.gamma_multiply(0.5)),
        false => mono(MONO),
    };
    write(cell(Column::At), &at, at_ink);
    write(
        cell(Column::Size),
        &crate::room::measure(row.size),
        mono(MONO),
    );
    if let Some(modified) = row.modified {
        write(cell(Column::Modified), &dated(modified), plain(TEXT, quiet));
    }
    // ⚠️ Weak ink even for a bare id: an instrument not attached or not fully read may
    // still hold it.
    write(cell(Column::Needs), &row.needs.text(), plain(TEXT, quiet));

    // A table row drags like a tree row, with the same payload, so it has the same drop
    // targets and meaning.
    if response.dragged() {
        if let Some(head) = browser.held(row.item, workspace, &device.state) {
            let carried = browser.carrying(head, &row.name, workspace, &device.state);
            egui::DragAndDrop::set_payload(ui.ctx(), carried);
        }
    }

    let said = under(&response, content, tracks)
        .map(|column| tooltip(row, column, browser.tags(), workspace, device))
        .filter(|said| !said.is_empty());
    let response = match said {
        Some(said) => response.on_hover_text(said),
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
/// Two columns add a fact the row does not carry: the tags column shows a count and the
/// hover names the tags, and the address may be one of several slots holding the same
/// bytes. Both are computed for the hovered row only.
fn tooltip(
    row: &Row,
    column: Column,
    tags: &Tags,
    workspace: &Workspace,
    device: &Device,
) -> String {
    match column {
        Column::Glyph | Column::Kind => word(row),
        Column::Mark => crate::bring::said_as_hover(&row.said),
        Column::Name => row.name.clone(),
        Column::Tags => match worn(row, tags) {
            names if names.is_empty() => "no tags".to_string(),
            names => names.join(", "),
        },
        Column::Where => row.where_.sentence().to_string(),
        Column::At => match (row.at, also_at(row, workspace, device)) {
            (Some((class, at)), _) => place(class, at),
            (None, also) if also.is_empty() => "in no slot".to_string(),
            (None, also) => format!("Also at {}", also.join(", ")),
        },
        Column::Size => format!("{} bytes", row.size),
        Column::Modified => match row.modified {
            Some(modified) => dated(modified),
            None => "no file on this computer".to_string(),
        },
        Column::Needs => row.needs.sentence(),
    }
}

/// When a file was last modified, on the user's clock, from the nanoseconds since the
/// Unix epoch that [`Row::modified`] holds.
pub fn dated(modified: u64) -> String {
    crate::when::at(modified / 1_000_000, crate::when::local_offset)
}

/// The addresses of the slots holding what an unlinked row holds. See
/// [`crate::device::also_at`].
fn also_at(row: &Row, workspace: &Workspace, device: &Device) -> Vec<String> {
    let entity = row.item.local().and_then(|id| workspace.get(id));
    let also = entity.map(|entity| crate::device::also_at(&device.state, workspace, entity));
    let also = also.unwrap_or_default().into_iter();
    also.map(|(_, at)| shown(at)).collect()
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
                "Nothing to show. Import Nord files, connect an instrument, or clear the search and filters.",
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
    use crate::bring::Rows;
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
            faint: false,
            mark: None,
            said: String::new(),
            size,
            modified: None,
            needs: Needs::Nothing,
        }
    }

    /// Link an asset to a program slot of the attached keyboard, as what it is saved as.
    fn link(workspace: &mut Workspace, device: &Device, id: u64, at: Location) {
        let keyboard = device.state.keyboard().cloned().expect("attached");
        let print = workspace.get(id).unwrap().saved_print().expect("read");
        let held = crate::link::Held::new(ObjectClass::Program, at, "", print).unwrap();
        workspace.link(id, &keyboard, held);
    }

    #[test]
    fn the_columns_share_the_width_without_overlapping_or_overflowing_it() {
        for width in [430.0_f32, 900.0] {
            let tracks = tracks(width, &[None; COLUMNS], true);
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
        }
        let wide = tracks(900.0, &[None; COLUMNS], true);
        for (column, track) in Column::ALL.iter().zip(&wide) {
            assert!(track.end - track.start > 0.0, "{column:?} vanished at 900");
        }
    }

    /// Programs are told apart by their names.
    #[test]
    fn a_narrow_table_hides_secondary_columns_before_the_name_shrinks() {
        let mut hidden_before = Vec::new();
        for width in (130..=900).rev().step_by(10).map(|width| width as f32) {
            let tracks = tracks(width, &[None; COLUMNS], true);
            let name = &tracks[Column::Name as usize];
            assert!(
                name.end - name.start >= NAME_LEAST,
                "the name at {width}: {name:?}"
            );
            let hidden: Vec<Column> = Column::ALL
                .into_iter()
                .filter(|column| tracks[*column as usize].is_empty())
                .collect();
            let first = &YIELDING[..hidden.len()];
            assert!(
                hidden.iter().all(|column| first.contains(column)),
                "at {width}, {hidden:?} hidden out of turn"
            );
            assert!(
                hidden.len() >= hidden_before.len(),
                "at {width}, only {hidden:?} hidden, though {hidden_before:?} were wider"
            );
            hidden_before = hidden;
        }
    }

    #[test]
    fn the_kind_reaches_120_px_when_wide_and_yields_to_the_name_when_narrow() {
        let width_of = |tracks: &[Range<f32>; COLUMNS], column: Column| {
            let track = &tracks[column as usize];
            track.end - track.start
        };

        let wide = tracks(1000.0, &[None; COLUMNS], true);
        assert!(
            (width_of(&wide, Column::Kind) - 120.0).abs() < 0.01,
            "at 1000 the kind holds the longest word: {}",
            width_of(&wide, Column::Kind)
        );

        let narrow = tracks(430.0, &[None; COLUMNS], true);
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

    #[test]
    fn needs_is_hidden_until_an_instrument_is_attached() {
        let width_of = |tracks: &[Range<f32>; COLUMNS], column: Column| {
            let track = &tracks[column as usize];
            track.end - track.start
        };
        let (attached, alone) = (
            tracks(900.0, &[None; COLUMNS], true),
            tracks(900.0, &[None; COLUMNS], false),
        );
        assert!(width_of(&attached, Column::Needs) > 0.0, "{attached:?}");
        assert_eq!(width_of(&alone, Column::Needs), 0.0, "{alone:?}");
        assert!(
            width_of(&alone, Column::Name) > width_of(&attached, Column::Name),
            "the name takes the room: {alone:?}"
        );
    }

    #[test]
    fn a_dragged_name_fills_the_width_while_needs_is_hidden() {
        let mut widths = [None; COLUMNS];
        widths[Column::Name as usize] = Some(200.0);
        let name = |tracks: &[Range<f32>; COLUMNS]| tracks[Column::Name as usize].clone();
        let end = |tracks: &[Range<f32>; COLUMNS]| {
            tracks.iter().map(|track| track.end).fold(0.0, f32::max)
        };

        let attached = tracks(900.0, &widths, true);
        assert!(
            (name(&attached).end - name(&attached).start - 200.0).abs() < 0.01,
            "{attached:?}"
        );
        assert_eq!(end(&attached), 900.0, "{attached:?}");

        let alone = tracks(900.0, &widths, false);
        assert!(name(&alone).end - name(&alone).start > 200.0, "{alone:?}");
        assert_eq!(end(&alone), 900.0, "{alone:?}");
    }

    #[test]
    fn a_sort_by_a_hidden_column_falls_back_to_the_name() {
        let mut bench = Bench::new();
        let mut library = Library {
            by: Column::Needs,
            ..Library::default()
        };
        library_frame(
            &mut library,
            &mut bench,
            egui::vec2(900.0, 540.0),
            Vec::new(),
        );
        assert_eq!(
            library.by,
            Column::Name,
            "Needs is hidden with no instrument"
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

    #[test]
    fn a_width_below_the_fixed_columns_shrinks_every_track_instead_of_going_negative() {
        for width in [0.0_f32, 40.0, 120.0] {
            let tracks = tracks(width, &[None; COLUMNS], true);
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
                    Where::Both(Standing::Ahead),
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

        let biggest = arrange(held(), "africa", Column::Size, Order::Down);
        assert_eq!(names(&biggest), ["Africa Split", "africa bass"]);
        let smallest = arrange(held(), "africa", Column::Size, Order::Up);
        assert_eq!(names(&smallest), ["africa bass", "Africa Split"]);
    }

    /// ⚠️ A row with no address treated as `0:0` would file everything on this computer at the top of the
    /// instrument's first bank.
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

    /// 23 Oct 2026, 23:30 UTC, as a file's time is counted.
    const LATE: u64 = 1_792_798_200_000_000_000;

    #[test]
    fn the_newest_file_sorts_first_and_a_row_with_no_time_last() {
        let dated = |name: &str, size: u64, modified: Option<u64>| Row {
            modified,
            ..row(name, Kind::Program, Where::Computer, None, size)
        };
        let held = || {
            vec![
                dated("older", 1, Some(LATE - 1)),
                dated("no file", 2, None),
                dated("newest", 3, Some(LATE)),
            ]
        };
        let names = |rows: &[Row]| -> Vec<String> { rows.iter().map(|r| r.name.clone()).collect() };

        let newest = arrange(held(), "", Column::Modified, Order::Down);
        assert_eq!(names(&newest), ["newest", "older", "no file"]);
        let oldest = arrange(held(), "", Column::Modified, Order::Up);
        assert_eq!(names(&oldest), ["no file", "older", "newest"]);
    }

    #[test]
    fn a_file_on_this_computer_says_when_it_was_modified_and_a_slot_does_not() {
        let mut bench = Bench::new();
        let mut create = || {
            bench
                .workspace
                .create(Fresh::Program, &mut bench.log)
                .unwrap()
        };
        let (filed, unfiled) = (create(), create());
        bench.workspace.dated(filed, Some(LATE));
        bench.device.pretend_partitions(&crate::device::ELECTRO5);
        bench
            .device
            .pretend_scanned(ObjectClass::Program, 7, &["Africa Split"]);

        let held = rows(
            &bench.workspace,
            &bench.device.state,
            bench.browser.tags(),
            &Filter::default(),
            &Rows::of(&bench.device.state, &bench.workspace, &bench.queue),
        );
        let times: Vec<(Item, Option<u64>)> =
            held.iter().map(|row| (row.item, row.modified)).collect();
        let slot = Item::Slot {
            class: ObjectClass::Program,
            at: Location::from_user(7, 1),
        };
        assert_eq!(
            times,
            [
                (Item::Local(filed), Some(LATE)),
                (Item::Local(unfiled), None),
                (slot, None),
            ]
        );

        let mut library = Library::default();
        let screen = egui::vec2(1000.0, 540.0);
        let said = library_frame(&mut library, &mut bench, screen, Vec::new());
        assert!(said.iter().any(|word| word.text == "Modified"), "{said:?}");
        let on_the_clock = crate::when::at(LATE / 1_000_000, crate::when::local_offset);
        let cells: Vec<&testing::Word> = said
            .iter()
            .filter(|word| word.text == on_the_clock)
            .collect();
        let [cell] = cells.as_slice() else {
            panic!("one row says {on_the_clock}: {said:?}");
        };
        assert!(
            !cell.galley.elided,
            "the day and the time are written whole"
        );
    }

    #[test]
    fn the_modified_column_holds_any_day_and_time() {
        let ctx = testing::context();
        let mut widest = 0.0_f32;
        testing::run(&ctx, egui::RawInput::default(), |ctx| {
            let font = egui::FontId::proportional(TEXT);
            const MONTHS: [&str; 12] = [
                "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
            ];
            ctx.fonts(|fonts| {
                for month in MONTHS {
                    for digit in 0..=9 {
                        let d = digit.to_string();
                        let text = format!("{d}{d} {month} {d}{d}{d}{d}, {d}{d}:{d}{d}");
                        let galley = fonts.layout_no_wrap(text, font.clone(), egui::Color32::WHITE);
                        widest = widest.max(galley.size().x);
                    }
                }
            });
        });
        assert!(
            widest <= MODIFIED_W,
            "the widest day and time is {widest} wide, and the column {MODIFIED_W}"
        );
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
        link(&mut workspace, &device, both, at(6, 0));
        workspace.create(Fresh::Live, &mut log).unwrap();
        let sunday = tags.make("Sunday").unwrap();
        tags.set(both, sunday, true);

        let names = |filter: &Filter| -> Vec<String> {
            rows(
                &workspace,
                &device.state,
                &tags,
                filter,
                &Rows::of(&device.state, &workspace, &Queue::default()),
            )
            .into_iter()
            .map(|row| row.name)
            .collect()
        };
        let mut filter = Filter::default();
        assert_eq!(
            names(&filter),
            ["Africa-Split.ne5p", "untitled.ne5l", "Squabble B", "Sunday"]
        );

        filter.narrow(Narrow::Kind(Kind::Program));
        assert_eq!(names(&filter), ["Africa-Split.ne5p", "Squabble B"]);

        filter.narrow(Narrow::Place(Place::Computer));
        assert_eq!(names(&filter), ["Africa-Split.ne5p"]);
        filter.narrow(Narrow::Place(Place::Computer));
        filter.narrow(Narrow::Place(Place::Keyboard));
        assert_eq!(names(&filter), ["Africa-Split.ne5p", "Squabble B"]);

        filter.narrow(Narrow::Tag(sunday));
        assert_eq!(names(&filter), ["Africa-Split.ne5p"]);
    }

    /// ⚠️ One rule for the word, the mark, and the count. An asset the attached
    /// instrument refuses is not linked by its origin. Comparing it with that slot would
    /// paint a difference and offer a send that [`crate::queue::enqueue`] refuses on every
    /// click.
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
        let entity = workspace.get(id).expect("it is on the list");
        assert!(!fit(&device.state, entity).allowed(), "a Stage 4 program");
        assert_eq!(whereabouts(entity, &device.state, None), Where::Foreign);
        let sync = Rows::of(&device.state, &workspace, &queue);
        assert!(sync.list.is_empty() && keyboard_mark(&sync, id).is_none());
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
        link(&mut workspace, &device, id, at(6, 0));

        let starred = |workspace: &Workspace| {
            rows(
                workspace,
                &device.state,
                &tags,
                &Filter::default(),
                &Rows::of(&device.state, workspace, &queue),
            )
            .into_iter()
            .find(|row| matches!(row.item, Item::Local(_)))
            .expect("the asset is listed")
            .unsaved
        };
        assert!(!starred(&workspace));

        workspace.mark_pending(id, true);
        assert!(starred(&workspace), "the plan is an edit the row shows");
        let sync = Rows::of(&device.state, &workspace, &queue);
        assert_eq!(
            keyboard_mark(&sync, id),
            None,
            "and the slot still holds what this was saved as",
        );
    }

    /// A write already waiting moves its row from "ready" to "waiting", so one task is
    /// not counted twice.
    #[test]
    fn a_row_waiting_to_be_sent_is_not_also_one_that_differs() {
        use crate::filter::{Narrow, State};

        let Bench {
            mut workspace,
            mut device,
            mut log,
            ..
        } = Bench::new();
        let (tags, queue) = (Tags::default(), Queue::default());

        let id = workspace.create(Fresh::Program, &mut log).unwrap();
        let bytes = workspace.get(id).unwrap().bytes.to_vec();
        let crc = workspace
            .get(id)
            .and_then(|entity| entity.saved.crc32())
            .expect("every CBIN container has one");
        device.pretend_bodies(ObjectClass::Program, 7, &[Some(("Africa Split", crc))]);
        link(&mut workspace, &device, id, at(6, 0));
        // Edited and saved: the link stays where it was and the file is ahead of it.
        let (_, edited) =
            crate::fields::apply(&bytes, &[("center_panel.gain".into(), "96".into())])
                .expect("the registry takes the set");
        workspace.replace_bytes(id, edited, &mut log);
        workspace.mark_saved(id);

        fn narrowed(
            workspace: &Workspace,
            device: &Device,
            queue: &Queue,
            tags: &Tags,
            state: crate::filter::State,
        ) -> usize {
            let mut filter = Filter::default();
            filter.narrow(Narrow::State(state));
            rows(
                workspace,
                &device.state,
                tags,
                &filter,
                &Rows::of(&device.state, workspace, queue),
            )
            .into_iter()
            .filter(|row| matches!(row.item, Item::Local(_)))
            .count()
        }
        assert_eq!(narrowed(&workspace, &device, &queue, &tags, State::Sync), 1);

        assert_eq!(state(Some(Mark::Up)), Some(State::Sync));
        assert_eq!(state(Some(Mark::Down)), Some(State::Sync));
        assert_eq!(state(Some(Mark::Attention)), Some(State::Attention));
        assert_eq!(
            narrowed(&workspace, &device, &queue, &tags, State::Attention),
            0
        );
    }

    /// Each of the three kinds of mark has its own color, and the two ways share one.
    #[test]
    fn every_kind_of_mark_is_painted_apart_from_the_others() {
        let kinds = [Mark::Up, Mark::Attention, Mark::Unsaved];
        for visuals in [egui::Visuals::dark(), egui::Visuals::light()] {
            let inks: BTreeSet<[u8; 4]> = kinds
                .iter()
                .map(|mark| mark_ink(*mark, &visuals).to_array())
                .collect();
            assert_eq!(inks.len(), kinds.len(), "{inks:?}");
            assert_eq!(mark_ink(Mark::Down, &visuals), mark_ink(Mark::Up, &visuals));
        }
    }

    /// What the library marks a file with, and says of it on hover, is what its Sync row
    /// says: ↑ for its own way up, ↓ for its own way down, and `!` for every row that waits
    /// for the player or changes a link.
    #[test]
    fn a_files_mark_and_its_words_come_from_its_sync_row() {
        use crate::bring::{Change, Row as Synced};
        let Bench { workspace, .. } = Bench::new();
        let row = |change| Synced {
            id: 1,
            class: ObjectClass::Program,
            at: Some(at(6, 0)),
            change,
        };
        let marked = |change| {
            let row = row(change);
            (
                mark_sign(Mark::of(row.sign(&workspace))),
                row.said(&workspace, &Queue::default()),
            )
        };
        assert_eq!(marked(Change::Ahead), ("↑", "newer here".into()));
        assert_eq!(
            marked(Change::Theirs),
            ("↓", "newer on the keyboard".into())
        );
        assert_eq!(marked(Change::Both), ("!", "both changed".into()));
        assert_eq!(
            marked(Change::Removed),
            ("!", "removed on the keyboard".into())
        );
        let by = "Clav".to_string();
        assert_eq!(
            marked(Change::Replaced { by }),
            ("!", "replaced by “Clav”".into())
        );
        let moved = Change::Moved {
            to: at(6, 3),
            agrees: true,
            with: None,
        };
        assert_eq!(marked(moved), ("!", "moved to 7:4".into()));
    }

    #[test]
    fn the_mark_cell_says_its_rows_words_on_hover_and_an_unmarked_row_none() {
        let Bench {
            workspace, device, ..
        } = Bench::new();
        let tags = Tags::default();
        let mut held = row(
            "Africa Split",
            Kind::Program,
            Where::Both(Standing::Ahead),
            Some(at(6, 0)),
            121,
        );
        let hover = |held: &Row| tooltip(held, Column::Mark, &tags, &workspace, &device);
        assert_eq!(hover(&held), "");
        held.mark = Some(Mark::Up);
        held.said = "newer here".into();
        assert_eq!(hover(&held), "Newer here");
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
            Some("→ Programs 7:1–7:4 · 2 slots occupied · 1 needs a piano that is not here")
        );

        // One of them is already in the queue.
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
        assert!(said.contains("1 already in Sync"), "{said}");

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
            browser.tags(),
            &Filter::default(),
            &Rows::of(&device.state, &workspace, &queue),
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

        // Escape closes the new folder's rename editor.
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
        let edge = egui::pos2(name.left() - MARK - 1.5 * GAP, name.center().y);
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
        assert_eq!(library.widths, [None; COLUMNS]);
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
        let mut only_at = [None; COLUMNS];
        only_at[Column::At as usize] = Some(60.0);
        assert_eq!(after.widths, only_at);

        eframe::Storage::set_string(
            &mut store,
            Library::KEY,
            "drawbar library 0\nwidth\tat\t60\n".into(),
        );
        let mut after = Library::default();
        after.restore(&store);
        assert_eq!(after.widths, [None; COLUMNS]);
    }

    /// Paints with both docks open and with none. Nothing checks pixels: this catches a layout
    /// that panics or an id that collides.
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

        let on_a_row = on_row(0);
        let ctx = bench.ctx.clone();
        for width in [430.0_f32, 900.0] {
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

    fn poll(bench: &mut Bench) {
        bench.device.poll(
            &mut bench.log,
            &mut bench.workspace,
            &mut bench.tabs,
            &mut bench.queue,
        );
    }

    fn row_of_local(bench: &Bench, id: u64) -> Row {
        let state = &bench.device.state;
        row_of(
            Item::Local(id),
            &bench.workspace,
            state,
            bench.browser.tags(),
            &Rows::of(state, &bench.workspace, &bench.queue),
        )
        .unwrap()
    }

    fn slot_rows(bench: &Bench) -> Vec<Location> {
        let state = &bench.device.state;
        let filter = Filter::default();
        rows(
            &bench.workspace,
            state,
            bench.browser.tags(),
            &filter,
            &Rows::of(state, &bench.workspace, &bench.queue),
        )
        .into_iter()
        .filter_map(|row| match row.item {
            Item::Slot { at, .. } => Some(at),
            _ => None,
        })
        .collect()
    }

    fn save_edited(bench: &mut Bench, id: u64, gain: &str) {
        let bytes = bench.workspace.get(id).unwrap().bytes.to_vec();
        let gain = [("center_panel.gain".into(), gain.into())];
        let edited = crate::fields::apply(&bytes, &gain).unwrap().1;
        bench.workspace.replace_bytes(id, edited, &mut bench.log);
        bench.act(vec![Act::SaveDoc(id)]);
    }

    #[test]
    fn a_new_file_stays_on_this_computer_though_a_slot_holds_its_bytes() {
        let mut bench = Bench::new();
        bench.device.pretend_partitions(&crate::device::ELECTRO5);
        bench
            .device
            .pretend_geometry(ObjectClass::Program, &[("A", 1)]);
        let id = bench
            .workspace
            .create(Fresh::Program, &mut bench.log)
            .unwrap();
        let crc = bench.workspace.get(id).unwrap().saved.crc32().unwrap();
        bench
            .device
            .pretend_bodies(ObjectClass::Program, 1, &[Some(("Init", crc))]);
        poll(&mut bench);

        let row = row_of_local(&bench, id);
        assert_eq!(
            (row.where_, row.at, row.mark),
            (Where::Computer, None, None)
        );
        assert_eq!(slot_rows(&bench), [at(0, 0)], "the slot keeps its own row");
        bench.act(vec![Act::SaveDoc(id)]);
        assert!(bench.queue.is_empty(), "saving it queues nothing");
        let rows = crate::bring::rows(&bench.device.state, &bench.workspace, &bench.queue);
        assert_eq!(rows, [], "Sync has nothing to do with it");
    }

    #[test]
    fn a_linked_file_reads_its_slot_through_its_link_and_saving_queues_nothing() {
        let mut bench = Bench::new();
        bench.device.pretend_partitions(&crate::device::ELECTRO5);
        let class = ObjectClass::Program;
        let bytes = Fresh::Program.bytes().unwrap();
        let origin = Origin::File("Tine.ne5p".into());
        let id = bench
            .workspace
            .ingest("Tine.ne5p".into(), origin, bytes, &mut bench.log);
        let crc = bench.workspace.get(id).unwrap().saved.crc32().unwrap();
        let held = |crc| [Some(("Tine", crc))];
        bench.device.pretend_bodies(class, 7, &held(crc));
        link(&mut bench.workspace, &bench.device, id, at(6, 0));
        let looks = |bench: &Bench| {
            let row = row_of_local(bench, id);
            (row.where_, row.mark)
        };
        assert_eq!(looks(&bench), (Where::Both(Standing::Same), None));
        assert_eq!(slot_rows(&bench), [], "the slot is the file's row");

        bench.device.pretend_bodies(class, 7, &held(crc ^ 1));
        assert_eq!(
            looks(&bench),
            (Where::Both(Standing::Theirs), Some(Mark::Down))
        );
        bench.act(vec![Act::SaveDoc(id)]);
        assert!(
            bench.queue.is_empty(),
            "nothing sends the older copy over the panel's edit"
        );

        bench.device.pretend_bodies(class, 7, &held(crc));
        save_edited(&mut bench, id, "40");
        assert_eq!(
            looks(&bench),
            (Where::Both(Standing::Ahead), Some(Mark::Up))
        );
        assert!(bench.queue.is_empty(), "Save only saves");

        let sent = bench.workspace.get(id).unwrap().bytes.to_vec();
        bench.device.pretend(crate::device::DeviceEvent::Sent {
            id,
            class,
            at: at(6, 0),
            name: "Sent.ne5p".into(),
            sent: crate::device::Payload::Bytes(sent),
        });
        poll(&mut bench);
        let now = bench.workspace.get(id).unwrap().saved.crc32().unwrap();
        bench.device.pretend_bodies(class, 7, &held(now));
        assert_eq!(looks(&bench), (Where::Both(Standing::Same), None));

        bench.device.pretend_bodies(class, 7, &[None]);
        assert_eq!(
            looks(&bench),
            (Where::Computer, Some(Mark::Attention)),
            "emptied on the keyboard"
        );
    }

    #[test]
    fn a_file_linked_into_an_unread_bank_is_dim_there_and_never_hops_to_its_bytes() {
        let mut bench = Bench::new();
        bench.device.pretend_partitions(&crate::device::ELECTRO5);
        let id = bench
            .workspace
            .create(Fresh::Program, &mut bench.log)
            .unwrap();
        let crc = bench.workspace.get(id).unwrap().saved.crc32().unwrap();
        bench
            .device
            .pretend_bodies(ObjectClass::Program, 7, &[Some(("Tine", crc))]);
        link(&mut bench.workspace, &bench.device, id, at(7, 0));
        poll(&mut bench);

        let row = row_of_local(&bench, id);
        assert_eq!(row.where_, Where::Both(Standing::Unread));
        assert_eq!(
            (row.at, row.faint),
            (Some((ObjectClass::Program, at(7, 0))), true)
        );
        assert_eq!(
            slot_rows(&bench),
            [at(6, 0)],
            "the slot holding its bytes is its own"
        );
    }
}
