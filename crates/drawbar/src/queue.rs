//! What is waiting to go to the instrument, and the slot each asset will be written to.
//!
//! The queue holds one entry per destination and one per asset: queueing a second asset
//! for a slot displaces the first, and queueing an asset that is already waiting moves it.
//! An asset is owed to the instrument while [`Queue::holds`] it.

use std::io::Cursor;
use std::ops::Range;

use eframe::egui;
use nord_usb::wire::ProgramInfo;
use nord_usb::{Location, ObjectClass};

use crate::app::{bad, good, ui as ui_text, warn};
use crate::bring::{Change, Row, Rows, Sheet, Way, DIFFERS};
use crate::browser::{cell_ink, Act, Carried, Held, Item, Kind};
use crate::device::{fit, Device, DeviceState, Fit};
use crate::fields::fields_of;
use crate::icon::{painted, sized, Glyph};
use crate::link::{Keyboard, Spot, Standing};
use crate::log::Log;
use crate::menu::MENU_FOOT;
use crate::panel::{cut, row_ink, Track, GAP, GLYPH, PAD};
use crate::plays;
use crate::strings::{display_name, label, place, shown};
use crate::workspace::{first_difference, wire_body, LocalEntity, Workspace};

/// One asset waiting to be written, and the slot it is waiting for.
pub struct Queued {
    /// The asset on this computer.
    pub id: u64,
    pub class: ObjectClass,
    pub at: Location,
    /// What the slot holds, as the last read of its bank found it.
    pub replaces: Occupancy,
    /// Why the last attempt to write it stopped. Cleared when it is queued again, and
    /// when [`refit`] finds that the attached instrument accepts it.
    pub failure: Option<String>,
    /// The keyboard it was queued for, where one was attached. It is written to no other.
    pub keyboard: Option<Keyboard>,
}

impl Queued {
    /// The name the slot holds and the one a send of `name` gives it, where the two
    /// differ. The name goes with the sound, so a send renames the slot even when the
    /// bytes are the same.
    pub fn renames(&self, name: &str) -> Option<(&str, String)> {
        let Occupancy::Held(occupant) = &self.replaces else {
            return None;
        };
        let given = crate::device::slot_label(name)?;
        let renamed = self.class.names_its_slots() && given != occupant.name;
        renamed.then_some((occupant.name.as_str(), given))
    }
}

/// How what is waiting differs from what the slot holds.
pub enum Diff {
    /// The occupant's bytes are on their way.
    Pending,
    /// The two wire bodies are the same bytes.
    Identical,
    /// Both bodies carry a registry, so the difference is a list of fields.
    Fields(Vec<FieldDiff>),
    /// One of them has no registry, so the difference is an offset into the wire body.
    Bytes { first_at: usize },
    /// There is nothing to replace.
    Empty,
}

/// One field the two bodies do not agree on.
///
/// ⚠️ `path` is the registry path that `nord-format` reads and writes;
/// [`crate::strings::label`] turns it into words for display. `here` and `there` are the
/// rendered values.
pub struct FieldDiff {
    pub path: String,
    pub here: String,
    pub there: String,
    /// The class of piano or sample the field names by id, for a library reference.
    pub refers: Option<ObjectClass>,
}

impl FieldDiff {
    /// The field's label, then what this computer holds and what the keyboard holds, in words:
    /// a piano or sample by the name it goes by here or on the keyboard, and an id of 0 as
    /// the dials.
    pub fn said(&self, workspace: &Workspace, device: &DeviceState) -> [String; 3] {
        let Some(class) = self.refers else {
            return [label(&self.path), self.here.clone(), self.there.clone()];
        };
        let named = |value: &String| match crate::document::library_id(value) {
            Some(0) => "by the dials".to_string(),
            Some(id) => plays::name_of((class, id), workspace, device).unwrap_or(value.clone()),
            None => value.clone(),
        };
        let what = crate::strings::title(Kind::from_class(class).chip());
        [what, named(&self.here), named(&self.there)]
    }
}

/// The class of piano or sample a field names by id, if it is such a reference.
fn refers(field: &nord_format::fields::Field) -> Option<ObjectClass> {
    use nord_format::fields::{ControlKind, Library};
    match field.spec.control {
        ControlKind::Reference(Library::Piano) => Some(ObjectClass::Piano),
        ControlKind::Reference(Library::Sample) => Some(ObjectClass::Sample),
        _ => None,
    }
}

/// What is in the slot an entry is waiting for.
///
/// ⚠️ `Unknown` is not `Vacant`: a bank nobody has read may hold something.
pub enum Occupancy {
    /// The bank has not been scanned, so a compare read is on its way to find out.
    Unknown,
    /// Read, and holding nothing.
    Vacant,
    /// Read, and holding this.
    Held(Occupant),
}

impl Occupancy {
    /// What the scan cache says is in a slot. A slot in a bank no walk has reached is
    /// `Unknown`, never empty; the compare read that [`enqueue`] asks for settles it.
    pub fn of(state: &DeviceState, class: ObjectClass, at: Location) -> Occupancy {
        match state.slot(class, at) {
            Some(Some(info)) => Occupancy::Held(Occupant::of(info)),
            Some(None) => Occupancy::Vacant,
            None => Occupancy::Unknown,
        }
    }

    pub fn occupant(&self) -> Option<&Occupant> {
        match self {
            Occupancy::Held(held) => Some(held),
            Occupancy::Unknown | Occupancy::Vacant => None,
        }
    }
}

/// What a slot holds, as the read that found it reported.
pub struct Occupant {
    pub name: String,
    /// ⚠️ `None` where the class reports no checksum. `None` means the checksums cannot
    /// be compared; it never means they match.
    pub crc: Option<u32>,
    pub body_len: u32,
}

impl Occupant {
    fn of(info: &ProgramInfo) -> Occupant {
        Occupant {
            name: info.name.trim().to_string(),
            crc: info.crc32,
            body_len: info.body_len,
        }
    }
}

/// Everything owed to the instrument, in the order it was asked for.
#[derive(Default)]
pub struct Queue {
    list: Vec<Queued>,
}

/// The outcome of queueing an asset, which decides what the log says.
enum Put {
    /// A new entry, for a slot nothing else was waiting for.
    Made,
    /// This asset was already waiting for this same slot. Nothing moved.
    Standing,
    /// It was waiting for another slot, and moved here.
    Moved,
    /// Something else was waiting for this slot, and was dropped.
    Instead(u64),
}

/// An asset in the queue, and the slot it waits for.
pub type Waiting = (u64, ObjectClass, Location);

/// Place an asset on a slot by hand, for Sync to write, and log what that displaced. One
/// placed on the slot it is linked to opens the sheet on its row instead.
pub fn enqueue(
    workspace: &Workspace,
    device: &mut Device,
    queue: &mut Queue,
    log: &mut Log,
    id: u64,
    class: ObjectClass,
    at: Location,
) {
    let Some(put) = place_in(workspace, device, queue, log, (id, class, at)) else {
        return;
    };
    let name = workspace.get(id).map_or("", |it| it.name.as_str());
    let where_ = place(class, at);
    let other = |other: u64| workspace.get(other).map_or("", |it| it.name.as_str());
    log.say(match put {
        Put::Standing => return,
        Put::Made | Put::Moved => format!("“{name}” goes to {where_} on Sync."),
        Put::Instead(was) => format!(
            "“{name}” goes to {where_} on Sync; “{}” left out of Sync.",
            other(was)
        ),
    });
}

/// Queue an asset as [`enqueue`] does, saying only what it refused.
fn place_in(
    workspace: &Workspace,
    device: &mut Device,
    queue: &mut Queue,
    log: &mut Log,
    (id, class, at): Waiting,
) -> Option<Put> {
    let entity = workspace.get(id)?;
    let name = &entity.name;
    let where_ = place(class, at);
    // A send writes what the queue holds, so an asset the instrument refuses must never
    // enter it.
    if let Fit::Refuses(why) = fit(&device.state, entity) {
        log.trouble(format!("“{name}” cannot go to {where_}. {why}"));
        return None;
    }
    if let Some(why) = name_clash(workspace, &device.state, queue, entity, class, at) {
        log.trouble(format!("“{name}” cannot go to {where_}. {why}"));
        return None;
    }
    let linked = crate::device::standing_of(&device.state, entity);
    if linked.is_some_and(|it| (it.class, it.at) == (class, Some(at))) {
        crate::bring::open(device, Some(id));
        return None;
    }
    let keyboard_side = matches!(
        linked.map(|it| it.standing),
        Some(Standing::Theirs | Standing::Both | Standing::Gone)
    );
    if keyboard_side {
        log.trouble(format!("Sync “{}” first.", display_name(name)));
        return None;
    }
    let holds = Occupancy::of(&device.state, class, at);
    let keyboard = device.state.keyboard().cloned();
    Some(queue.put(entity, (class, at), holds, keyboard))
}

/// Show every waiting entry what the cache of the attached keyboard says its slot holds
/// now. Call whenever a bank is read again and before a send.
pub fn refresh(state: &DeviceState, queue: &mut Queue) {
    for held in &mut queue.list {
        let now = Occupancy::of(state, held.class, held.at);
        if !matches!(now, Occupancy::Unknown) {
            held.replaces = now;
        }
    }
}

/// A write found its slot holding `holds` instead of what it expected: a send placed by
/// hand keeps its place and shows what is there now.
pub fn changed_under(queue: &mut Queue, (id, class, at): Waiting, holds: Option<&ProgramInfo>) {
    let waiting = queue.list.iter_mut();
    let mut waiting = waiting.filter(|held| (held.id, held.class, held.at) == (id, class, at));
    let Some(held) = waiting.next() else {
        return;
    };
    held.replaces = holds.map_or(Occupancy::Vacant, |info| {
        Occupancy::Held(Occupant::of(info))
    });
}

/// Why a write of `entity` into `at` would be refused for its name: a slot of the
/// library already has it, or another entry waiting for the library would write it. An
/// entry waiting for `at` itself is no clash, since queueing this one displaces it.
///
/// ⚠️ The instrument keeps one object of each name in a library. Slot classes take any
/// number, and this returns `None` for them.
fn name_clash(
    workspace: &Workspace,
    state: &DeviceState,
    queue: &Queue,
    entity: &LocalEntity,
    class: ObjectClass,
    at: Location,
) -> Option<String> {
    if !class.is_library() {
        return None;
    }
    if let Some(holder) = crate::device::name_taken(state, class, at, entity) {
        return Some(taken(class, holder));
    }
    let label = crate::device::slot_label(&entity.name)?;
    let other = queue.list.iter().find(|held| {
        held.id != entity.id
            && held.class == class
            && held.at != at
            && label_of(workspace, held.id).as_deref() == Some(label.as_str())
    })?;
    Some(waiting(class, other.at))
}

/// The name a queued asset's write would give its slot.
fn label_of(workspace: &Workspace, id: u64) -> Option<String> {
    crate::device::slot_label(&workspace.get(id)?.name)
}

/// Why the instrument would refuse the second of two queued writes of one name.
fn waiting(class: ObjectClass, other: Location) -> String {
    format!(
        "Something going to {} on Sync has the same name.",
        place(class, other)
    )
}

/// Why the instrument refuses a second object of the name `holder` already has.
fn taken(class: ObjectClass, holder: Location) -> String {
    format!(
        "{} already has its name, and the instrument keeps one of each name. Place it \
         there to replace that one, or rename it.",
        place(class, holder)
    )
}

/// Check everything waiting against the attached instrument again, and log what it
/// refuses.
///
/// ⚠️ The queue outlives a disconnection, so each entry was checked against whatever was
/// attached when it was queued. An entry this instrument refuses keeps its place and
/// carries the reason, and its row goes nowhere until the instrument takes it.
pub fn refit(workspace: &Workspace, state: &DeviceState, queue: &mut Queue, log: &mut Log) {
    // A rename after queueing can give two entries one name. The first in the queue is
    // written first, so the later one is held back.
    let named: Vec<(ObjectClass, Location, Option<String>)> = queue
        .list
        .iter()
        .map(|held| (held.class, held.at, label_of(workspace, held.id)))
        .collect();
    for (nth, held) in queue.list.iter_mut().enumerate() {
        let Some(entity) = workspace.get(held.id) else {
            continue;
        };
        let (class, _, label) = &named[nth];
        let earlier = || {
            named[..nth]
                .iter()
                .find(|(peer, _, other)| {
                    peer == class && class.is_library() && label.is_some() && other == label
                })
                .map(|(class, at, _)| waiting(*class, *at))
        };
        let attached = state.keyboard();
        let elsewhere = held
            .keyboard
            .as_ref()
            .is_some_and(|keyboard| attached.is_some_and(|attached| attached != keyboard));
        let refusal = match (fit(state, entity), elsewhere) {
            (Fit::Refuses(why), _) => Some(why),
            (_, true) => Some("It was placed for another keyboard.".to_string()),
            (Fit::Unattached | Fit::Takes | Fit::Warn(_), false) => {
                crate::device::name_taken(state, held.class, held.at, entity)
                    .map(|holder| taken(held.class, holder))
                    .or_else(earlier)
            }
        };
        // Logged only when the reason changes, so repeated refits stay quiet.
        if let Some(why) = &refusal {
            if held.failure.as_ref() != Some(why) {
                log.say(format!(
                    "“{}” cannot go to {}. {why}",
                    entity.name,
                    place(held.class, held.at)
                ));
            }
        }
        held.failure = refusal;
    }
}

/// Discard what the previous instrument reported about the slots being waited for.
///
/// ⚠️ The queue outlives a disconnection, and what the last instrument held in a slot
/// says nothing about this one.
pub fn reattach(state: &DeviceState, queue: &mut Queue) {
    for held in &mut queue.list {
        held.replaces = Occupancy::of(state, held.class, held.at);
    }
}

/// Move a waiting asset to another slot.
///
/// This is [`enqueue`] for an asset already in the queue: the new slot is read, and
/// whatever was waiting for it is dropped. An asset not in the queue stays out of it.
pub fn retarget(
    workspace: &Workspace,
    device: &mut Device,
    queue: &mut Queue,
    log: &mut Log,
    id: u64,
    class: ObjectClass,
    at: Location,
) {
    if queue.holds(id) {
        enqueue(workspace, device, queue, log, id, class, at);
    }
}

/// Take out of the queue every entry waiting for a slot a sound moved out of or into,
/// which holds another sound now, and say so in one line.
pub fn unqueue_moved(
    workspace: &Workspace,
    queue: &mut Queue,
    log: &mut Log,
    moved: &[(ObjectClass, Vec<crate::workspace::Moved>)],
) {
    let ends = |class: ObjectClass, at: Location| {
        let at = Spot::at(at);
        let moves = moved.iter().filter(|(of, _)| *of == class);
        moves.flat_map(|(_, moves)| moves).any(|(from, to, _)| {
            matches!(to, Some(to @ Spot::At { .. }) if to != from && (*from == at || *to == at))
        })
    };
    let gone: Vec<u64> = queue
        .list
        .iter()
        .filter(|held| ends(held.class, held.at))
        .map(|held| held.id)
        .collect();
    gone.iter().for_each(|id| queue.forget(*id));
    let name = |id: u64| display_name(workspace.get(id).map_or("", |it| it.name.as_str()));
    log.say(match gone.as_slice() {
        [] => return,
        [id] => format!("“{}” left out of Sync.", name(*id)),
        many => format!("{} sounds left out of Sync.", many.len()),
    });
}

impl Queue {
    /// Queue one asset for one slot, and report what that displaced: where this asset was
    /// waiting before, or what was waiting for this slot. Callers use [`enqueue`].
    fn put(
        &mut self,
        entity: &LocalEntity,
        (class, at): (ObjectClass, Location),
        replaces: Occupancy,
        keyboard: Option<Keyboard>,
    ) -> Put {
        let same = |held: &&mut Queued| (held.id, held.class, held.at) == (entity.id, class, at);
        if let Some(held) = self.list.iter_mut().find(same) {
            (held.replaces, held.keyboard, held.failure) = (replaces, keyboard, None);
            return Put::Standing;
        }
        let moved = self.list.iter().any(|held| held.id == entity.id);
        let instead_of = self
            .list
            .iter()
            .find(|held| (held.class, held.at) == (class, at) && held.id != entity.id)
            .map(|held| held.id);
        self.list
            .retain(|held| held.id != entity.id && (held.class, held.at) != (class, at));
        self.list.push(Queued {
            id: entity.id,
            class,
            at,
            replaces,
            failure: None,
            keyboard,
        });
        match (moved, instead_of) {
            (true, _) => Put::Moved,
            (false, Some(other)) => Put::Instead(other),
            (false, None) => Put::Made,
        }
    }

    /// The slots of one class something is already waiting for.
    pub fn waiting_in(&self, class: ObjectClass) -> Vec<Location> {
        self.list
            .iter()
            .filter(|held| held.class == class)
            .map(|held| held.at)
            .collect()
    }

    /// The entry waiting for a slot, if anything is.
    pub fn waiting(&self, class: ObjectClass, at: Location) -> Option<&Queued> {
        self.list
            .iter()
            .find(|held| (held.class, held.at) == (class, at))
    }

    /// Take an asset out of the queue.
    pub fn forget(&mut self, id: u64) {
        self.list.retain(|held| held.id != id);
    }

    /// Empty the queue. The queue is only a plan, so this deletes no asset and sends
    /// nothing to the instrument.
    pub fn clear(&mut self) {
        self.list.clear();
    }

    pub fn holds(&self, id: u64) -> bool {
        self.list.iter().any(|held| held.id == id)
    }

    /// The assets waiting, in the order they will be written.
    pub fn ids(&self) -> Vec<u64> {
        self.list.iter().map(|held| held.id).collect()
    }

    pub fn entries(&self) -> &[Queued] {
        &self.list
    }

    pub fn entry(&self, id: u64) -> Option<&Queued> {
        self.list.iter().find(|held| held.id == id)
    }

    pub fn len(&self) -> usize {
        self.list.len()
    }

    pub fn is_empty(&self) -> bool {
        self.list.is_empty()
    }

    /// Which assets wait where, in order, and which of them stopped: everything the
    /// library's rows read of the queue, to tell when they are stale.
    pub fn shape(&self) -> impl Iterator<Item = (Waiting, bool)> + '_ {
        let held = self.list.iter();
        held.map(|held| ((held.id, held.class, held.at), held.failure.is_some()))
    }
}

/// How what is waiting differs from what the slot holds.
///
/// ⚠️ The bytes decide whether the two agree. A registry covers only the fields it
/// declares, and a bit no field claims is still a difference.
///
/// When both bodies decode into registries, the difference is a list of fields;
/// otherwise it is an offset into the wire body. Bodies are compared as the wire carries
/// them, without their file containers, because two containers of one body can differ
/// in their headers alone.
pub(crate) fn compare(here: &[u8], there: &[u8]) -> Diff {
    let (mine, held) = match (wire_body(here), wire_body(there)) {
        (Some(mine), Some(held)) => (mine, held),
        // Not a container this app can unwrap, so the whole input is compared.
        _ => (here.to_vec(), there.to_vec()),
    };
    let Some(first_at) = first_difference(&mine, &held) else {
        return Diff::Identical;
    };
    match apart(here, there) {
        Some(fields) => Diff::Fields(fields),
        None => Diff::Bytes { first_at },
    }
}

/// The registered fields two bodies disagree on, if both decode into a registry and any
/// field differs.
fn apart(here: &[u8], there: &[u8]) -> Option<Vec<FieldDiff>> {
    let decode = |bytes: &[u8]| nord_format::from_stream(&mut Cursor::new(bytes)).ok();
    let mine = fields_of(&decode(here)?)?;
    let held = fields_of(&decode(there)?)?;
    let differing: Vec<FieldDiff> = mine
        .into_iter()
        .filter_map(|field| {
            let there = held.iter().find(|other| other.path == field.path)?;
            (there.display != field.display).then(|| FieldDiff {
                refers: refers(&field),
                path: field.path,
                here: field.display,
                there: there.display.clone(),
            })
        })
        .collect();
    (!differing.is_empty()).then_some(differing)
}

/// The height of one waiting item, and of its first line, which its name and destination
/// share; what the write does goes on the second.
const ROW: f32 = 44.0;
const ROW_LINE: f32 = 26.0;

/// The state glyph at the end of a row.
const SMALL: f32 = 13.0;

/// The destination chip's height, and its padding at each end.
const CHIP: f32 = 20.0;
const CHIP_PAD: f32 = 6.0;

/// The font sizes a row uses.
const NAME: f32 = 12.5;
const NOTE: f32 = 11.0;
const MONO: f32 = 10.5;

/// The item list's preferred and least widths, and the geometry of the diff beside it.
const ITEMS: f32 = 250.0;
const ITEMS_LEAST: f32 = 170.0;
const HEAD: f32 = 24.0;
const DIFF_ROW: f32 = 26.0;
const DIFF_MONO: f32 = 11.5;

/// The sheet's largest size, and its margin from the window's edge.
const SHEET: egui::Vec2 = egui::vec2(940.0, 580.0);
const SHEET_MARGIN: f32 = 24.0;

/// Everything the sheet draws from.
struct Drawn<'a> {
    rows: Vec<Row>,
    queue: &'a Queue,
    workspace: &'a Workspace,
    device: &'a DeviceState,
}

/// The sheet: one row per sound that differs between this computer and the keyboard, the
/// way each goes, and how the picked one differs. Nothing is written or brought in until
/// Sync. It shows once the read it asked for has landed.
pub fn sheet(
    ctx: &egui::Context,
    device: &mut Device,
    (queue, sync): (&Queue, &Rows),
    workspace: &Workspace,
    acts: &mut Vec<Act>,
) {
    let Device { state, asks, .. } = device;
    let Some(sheet) = asks.sheet.as_mut() else {
        return;
    };
    let pressed = sheet.pressed();
    if pressed.is_none() && !sheet.read(state) {
        return;
    }
    let read = pressed.is_none();
    let rows = match read {
        true => sync.list.clone(),
        false => Vec::new(),
    };
    if sheet
        .picked
        .is_none_or(|id| !rows.iter().any(|row| row.id == id))
    {
        sheet.picked = rows.first().map(|row| row.id);
    }
    let drawn = Drawn {
        rows,
        queue,
        workspace,
        device: state,
    };
    let screen = ctx.screen_rect();
    let size = egui::vec2(
        SHEET.x.min(screen.width() - 2.0 * SHEET_MARGIN),
        SHEET.y.min(screen.height() - 2.0 * SHEET_MARGIN),
    );
    let visuals = ctx.style().visuals.clone();
    let frame = egui::Frame::new()
        .fill(visuals.panel_fill)
        .stroke(visuals.widgets.noninteractive.bg_stroke)
        .corner_radius(crate::sheet::SHEET_RADIUS)
        .shadow(visuals.window_shadow);
    let before = acts.len();
    let mut open = true;
    let shown = egui::Modal::new(egui::Id::new("sync"))
        .frame(frame)
        .backdrop_color(egui::Color32::from_black_alpha(115))
        .show(ctx, |ui| {
            ui.set_min_size(size);
            ui.set_max_size(size);
            ui.spacing_mut().item_spacing.y = 0.0;
            open &= head(ui, state);
            open &= foot(ui, sheet, &drawn, read, acts);
            body(ui, sheet, &drawn, acts);
        });
    let left = acts[before..].iter().any(|act| matches!(act, Act::Open(_)));
    if !open || shown.should_close() || left {
        asks.sheet = None;
    }
}

/// The sheet's header: what it is, and which keyboard. Returns false when close was
/// clicked.
fn head(ui: &mut egui::Ui, device: &DeviceState) -> bool {
    let visuals = ui.visuals().clone();
    let quiet = crate::app::caption(&visuals);
    let lit = crate::app::accent(&visuals);
    let mut open = true;
    let (rect, _) =
        ui.allocate_exact_size(egui::vec2(ui.available_width(), 62.0), egui::Sense::hover());
    let mut bar = ui.new_child(
        egui::UiBuilder::new()
            .max_rect(rect.shrink2(egui::vec2(16.0, 0.0)))
            .layout(egui::Layout::left_to_right(egui::Align::Center)),
    );
    bar.spacing_mut().item_spacing.x = 10.0;
    let (tile, _) = bar.allocate_exact_size(egui::Vec2::splat(32.0), egui::Sense::hover());
    bar.painter()
        .rect_filled(tile, 9.0, crate::app::tint(lit, 0.16));
    painted(&bar, Glyph::RefreshCw, tile.shrink(8.0), lit);
    let to = device.product().unwrap_or("the keyboard");
    bar.label(
        egui::RichText::new(format!("Sync with {to}"))
            .font(egui::FontId::new(15.0, crate::app::bold())),
    );
    bar.with_layout(egui::Layout::right_to_left(egui::Align::Center), |bar| {
        crate::panel::flat(bar);
        let close = egui::Button::image(sized(Glyph::X, 14.0, quiet))
            .image_tint_follows_text_color(false)
            .corner_radius(7.0)
            .min_size(egui::Vec2::splat(28.0));
        if bar.add(close).on_hover_text("close").clicked() {
            open = false;
        }
    });
    open
}

/// The sheet's foot: what to know before writing, then Not now and Sync. Returns false
/// when the sheet should close.
fn foot(
    ui: &mut egui::Ui,
    sheet: &mut Sheet,
    drawn: &Drawn,
    read: bool,
    acts: &mut Vec<Act>,
) -> bool {
    let visuals = ui.visuals().clone();
    let whole = ui.max_rect();
    let rect = egui::Rect::from_min_max(egui::pos2(whole.left(), whole.bottom() - 58.0), whole.max);
    ui.painter().hline(
        rect.x_range(),
        rect.top() + 0.5,
        egui::Stroke::new(1.0_f32, visuals.widgets.noninteractive.bg_stroke.color),
    );
    let mut open = true;
    let mut bar = ui.new_child(
        egui::UiBuilder::new()
            .max_rect(rect.shrink2(egui::vec2(16.0, 0.0)))
            .layout(egui::Layout::right_to_left(egui::Align::Center)),
    );
    bar.spacing_mut().item_spacing.x = 8.0;
    let plan = crate::bring::plan(sheet, &drawn.rows, drawn.device, drawn.workspace);
    let pressed = sheet.pressed();
    let label = match pressed.unwrap_or(plan.len()) {
        0 => "Sync".to_string(),
        n => format!("Sync {n}"),
    };
    let live = read && !plan.is_empty();
    let sync = crate::panel::accent_button(&mut bar, Glyph::RefreshCw, &label, live);
    let up = plan.iter().filter(|step| step.way == Way::Up);
    let up = up.filter_map(|step| Some((step.row.class, drawn.workspace.get(step.row.id)?)));
    let warnings = crate::browser::send_warnings(up, drawn.device);
    if sync.clicked() {
        sheet.accept(plan);
        acts.push(Act::SyncAll);
    }
    if crate::panel::tonal_button(&mut bar, None, "Not now").clicked() {
        open = false;
    }
    if let Some(first) = warnings.first() {
        let room = bar.available_width();
        bar.with_layout(egui::Layout::left_to_right(egui::Align::Center), |bar| {
            bar.set_max_width(room);
            bar.spacing_mut().item_spacing.x = 8.0;
            bar.add(sized(Glyph::CircleAlert, 15.0, warn(&visuals)));
            let said = first.trim_start_matches("⚠️ ");
            let label = bar.add(egui::Label::new(egui::RichText::new(said).size(12.0)).truncate());
            if warnings.len() > 1 {
                label.on_hover_text(warnings.join("\n\n"));
            }
        });
    }
    open
}

/// The rows beside how the picked one differs.
fn body(ui: &mut egui::Ui, sheet: &mut Sheet, drawn: &Drawn, acts: &mut Vec<Act>) {
    let whole = ui.max_rect();
    let body = egui::Rect::from_min_max(
        egui::pos2(whole.left() + 10.0, ui.cursor().top() + 4.0),
        egui::pos2(whole.right() - 10.0, whole.bottom() - 58.0 - 10.0),
    );
    let mut inside = ui.new_child(egui::UiBuilder::new().max_rect(body));
    let ui = &mut inside;
    if drawn.rows.is_empty() {
        ui.add_space(GAP);
        ui.horizontal(|ui| {
            ui.add_space(PAD);
            let said = match sheet.pressed() {
                Some(_) => SYNCING,
                None => "Nothing to sync.",
            };
            ui.label(
                egui::RichText::new(said)
                    .text_style(ui_text())
                    .weak()
                    .italics(),
            );
        });
        return;
    }
    let list = (body.width() * 0.3).clamp(ITEMS_LEAST, ITEMS);
    let items = egui::Rect::from_min_size(body.min, egui::vec2(list, body.height()));
    let pane = egui::Rect::from_min_max(egui::pos2(items.right() + 8.0, body.top()), body.max);
    let mut column = ui.new_child(egui::UiBuilder::new().max_rect(items));
    egui::ScrollArea::vertical()
        .id_salt("sync_rows")
        .auto_shrink([false; 2])
        .show(&mut column, |ui| {
            ui.spacing_mut().item_spacing.y = 2.0;
            let shown = drawn.rows.iter();
            let shown = shown.filter(|row| sheet.shows(row, &drawn.rows, drawn.workspace));
            for row in shown.collect::<Vec<_>>() {
                if item(ui, row, sheet, drawn, acts).clicked() {
                    sheet.picked = Some(row.id);
                }
            }
        });
    ui.painter()
        .rect_filled(pane, 9.0, ui.visuals().window_fill);
    let picked = drawn.rows.iter().find(|row| Some(row.id) == sheet.picked);
    let Some((row, entity)) = picked.and_then(|row| Some((row, drawn.workspace.get(row.id)?)))
    else {
        return;
    };
    let mut pane = ui.new_child(egui::UiBuilder::new().max_rect(pane.shrink(10.0)));
    let words = row.said(drawn.workspace, drawn.queue);
    title(&mut pane, &entity.name, &words);
    let shown = sheet.shown.as_ref().filter(|it| it.id == row.id);
    let (Some(shown), Some(at)) = (shown, row.at) else {
        return;
    };
    let entry = placed(row, drawn.queue);
    let renamed = entry.and_then(|held| held.renames(&entity.name));
    table(
        &mut pane,
        &shown.diff,
        renamed,
        (row.class, at),
        drawn.workspace,
        drawn.device,
    );
}

/// The picked row's name, and what Sync does with it, which is shortened first.
fn title(ui: &mut egui::Ui, name: &str, words: &str) {
    let (rect, _) =
        ui.allocate_exact_size(egui::vec2(ui.available_width(), 28.0), egui::Sense::hover());
    let mut bar = ui.new_child(
        egui::UiBuilder::new()
            .max_rect(rect)
            .layout(egui::Layout::left_to_right(egui::Align::Center)),
    );
    bar.spacing_mut().item_spacing.x = 8.0;
    bar.label(
        egui::RichText::new(crate::strings::display_name(name))
            .font(egui::FontId::new(12.5, crate::app::bold())),
    );
    bar.add(
        egui::Label::new(
            egui::RichText::new(words)
                .size(12.0)
                .color(crate::app::caption(bar.visuals())),
        )
        .truncate(),
    );
    ui.add_space(4.0);
}

/// The column heads, and under them the slot's new name where a write renames it, then
/// either the fields that differ or one line for any other kind of difference.
pub fn table(
    ui: &mut egui::Ui,
    diff: &Diff,
    renamed: Option<(&str, String)>,
    (class, at): (ObjectClass, Location),
    workspace: &Workspace,
    device: &DeviceState,
) {
    let width = ui.available_width();
    let tracks = crate::panel::tracks(width, &DIFF_TRACKS, GAP);
    diff_head(ui, width, &tracks);
    if let Some((was, given)) = &renamed {
        field_row(ui, width, &tracks, [NAME_ROW, given, was]);
    }
    let Diff::Fields(fields) = diff else {
        let (tint, said) = summary(diff, class, at, ui.visuals());
        one_row(ui, width, &tracks, tint, &said);
        return;
    };
    egui::ScrollArea::vertical()
        .id_salt("sync_diff")
        .auto_shrink([false; 2])
        .show(ui, |ui| {
            ui.spacing_mut().item_spacing.y = 1.0;
            for field in fields {
                let [label, here, there] = field.said(workspace, device);
                field_row(ui, width, &tracks, [&label, &here, &there]);
            }
        });
}

/// The diff row for the slot's name.
const NAME_ROW: &str = "Name";

/// The single line for a diff that is not a field list.
///
/// ⚠️ Only a slot read and found empty is free. While the read is out, this says only
/// that it is out.
fn summary(
    diff: &Diff,
    class: ObjectClass,
    at: Location,
    visuals: &egui::Visuals,
) -> (egui::Color32, String) {
    let quiet = visuals.weak_text_color();
    match diff {
        Diff::Pending => (quiet, format!("reading what is in {}…", place(class, at))),
        Diff::Empty => (good(visuals), "the slot is free".into()),
        Diff::Identical => (quiet, "the keyboard holds the same".into()),
        Diff::Fields(_) | Diff::Bytes { .. } => (warn(visuals), DIFFERS.into()),
    }
}

/// One row: what it is, where, the ways it may go, and what Sync does with it. A row
/// that waits for the user is marked `!` before its name, as the library marks its file.
///
/// ⚠️ Nothing inside is a widget, for the reason [`crate::browser::Cells`] gives: a
/// label allocates a hover rect that wins the hit test over the row, so a click would
/// land on whichever word is under the pointer.
fn item(
    ui: &mut egui::Ui,
    row: &Row,
    sheet: &mut Sheet,
    drawn: &Drawn,
    acts: &mut Vec<Act>,
) -> egui::Response {
    let (rect, response) = ui.allocate_exact_size(
        egui::vec2(ui.available_width(), ROW),
        egui::Sense::click_and_drag(),
    );
    let Some(entity) = drawn.workspace.get(row.id) else {
        return response;
    };
    let selected = sheet.picked == Some(row.id);
    let visuals = ui.visuals().clone();
    let painter = ui.painter().clone();
    let (ink, quiet) = row_ink(&painter, rect, selected, response.hovered(), &visuals);
    let line = egui::Rect::from_min_size(rect.min, egui::vec2(rect.width(), ROW_LINE));
    let box_ = |right: f32, width: f32| {
        egui::Rect::from_center_size(
            egui::pos2(right - width / 2.0, line.center().y),
            egui::vec2(width, SMALL + 4.0),
        )
    };
    let placed = placed(row, drawn.queue);
    let key = (row.id, row.at.map(|at| (at.bank, at.slot)));
    if placed.is_some() {
        let leave = ui.interact(
            box_(rect.right() - PAD, SMALL),
            ui.id().with(("leave", key)),
            egui::Sense::click(),
        );
        let leaving = match leave.hovered() {
            true => visuals.text_color(),
            false => visuals.weak_text_color(),
        };
        painted(
            ui,
            Glyph::X,
            box_(rect.right() - PAD, SMALL).shrink(2.0),
            cell_ink(selected, leaving, &visuals),
        );
        if leave.on_hover_text("leave out of Sync").clicked() {
            acts.push(Act::Unqueue(row.id));
        }
    }
    let waits = sheet.waits(row, &drawn.rows, drawn.workspace);
    let right = ways_control(
        ui,
        row,
        sheet,
        drawn,
        (rect.right() - PAD - SMALL - GAP, line.center().y),
        selected,
    );
    let right = match (&row.change, placed, row.at) {
        (_, Some(held), _) => destination(
            ui,
            held,
            line,
            right,
            quiet,
            drawn.device,
            drawn.queue,
            acts,
        ),
        (_, None, Some(at)) => address(&painter, line, right, &shown(at), quiet),
        (_, None, None) => right,
    };
    let left = rect.left() + PAD;
    painted(
        ui,
        Kind::of(entity).glyph(),
        egui::Rect::from_center_size(
            egui::pos2(left + GLYPH / 2.0, line.center().y),
            egui::Vec2::splat(GLYPH),
        ),
        ink,
    );
    let name_at = left + GLYPH + GAP;
    let marked = match waits {
        true => {
            let mark = painter.text(
                egui::pos2(name_at, line.center().y),
                egui::Align2::LEFT_CENTER,
                "!",
                egui::FontId::proportional(NAME),
                cell_ink(selected, bad(&visuals), &visuals),
            );
            mark.right() + 4.0
        }
        false => name_at,
    };
    cut(
        &painter,
        marked,
        line.center().y,
        right - marked,
        crate::strings::display_name(&entity.name),
        egui::TextFormat::simple(egui::FontId::proportional(NAME), ink),
    );
    // The chip beside a send placed by hand already says where it goes.
    let words = match (placed, &row.change) {
        (Some(_), Change::Placed { waits: None }) => placed_words(row.id, drawn.queue),
        _ => row.said(drawn.workspace, drawn.queue),
    };
    let failed = placed.is_some_and(|held| held.failure.is_some());
    let note_ink = match (failed, selected) {
        (true, false) => bad(&visuals),
        _ => quiet,
    };
    cut(
        &painter,
        name_at,
        (line.bottom() + rect.bottom()) / 2.0 - 2.0,
        rect.right() - PAD - name_at,
        &words,
        egui::TextFormat::simple(egui::FontId::proportional(NOTE), note_ink),
    );
    if response.double_clicked() {
        acts.push(Act::Open(Item::Local(entity.id)));
    }
    if response.dragged() && placed.is_some() {
        egui::DragAndDrop::set_payload(ui.ctx(), carried(entity, drawn.device));
    }
    response.on_hover_text(format!("{}\n{words}", entity.name))
}

/// The send placed by hand a row stands for.
fn placed<'a>(row: &Row, queue: &'a Queue) -> Option<&'a Queued> {
    matches!(row.change, Change::Placed { .. })
        .then(|| queue.entry(row.id))
        .flatten()
}

/// What the sheet says while the plan Sync was pressed on waits to run.
const SYNCING: &str = "Syncing…";

/// The least width of one way's button, and the space between two.
const WAY: f32 = 20.0;
const WAY_GAP: f32 = 2.0;

/// How a way's button reads on a row of `change`. Down is the keyboard's side, which for
/// a replaced or removed sound is its link alone.
fn way_sign(way: Way, change: &Change) -> &'static str {
    match (way, change) {
        (Way::Up, _) => "↑",
        (Way::Down, Change::Removed | Change::Replaced { .. }) => "Unlink",
        (Way::Down, _) => "↓",
    }
}

/// What a way's arrow does, on hover.
fn way_hint(way: Way, change: &Change) -> Option<&'static str> {
    match (way, change) {
        (Way::Up, _) => Some("to the keyboard"),
        (Way::Down, Change::Moved { .. }) => Some("the link follows"),
        (Way::Down, Change::Removed | Change::Replaced { .. }) => None,
        (Way::Down, _) => Some("to this computer"),
    }
}

/// A row's ways, right-aligned to `right` on the line centered at `middle`: the
/// keyboard's side, then `↑`. The lit one is the way Sync takes. A click on it leaves the
/// row out, and a click on the other turns the row. A way the row is kept from is faint,
/// and one its kind of row never goes is not drawn. Returns the left edge of the space
/// they took.
fn ways_control(
    ui: &mut egui::Ui,
    row: &Row,
    sheet: &mut Sheet,
    drawn: &Drawn,
    (right, middle): (f32, f32),
    selected: bool,
) -> f32 {
    let lit = sheet.way(row, &drawn.rows, drawn.workspace);
    let allowed = row.ways(drawn.workspace).allowed;
    let kept_from = matches!(row.change, Change::Ahead | Change::Theirs | Change::Both);
    let key = (row.id, row.at.map(|at| (at.bank, at.slot)));
    let font = egui::FontId::proportional(NAME);
    let mut left = right;
    for way in [Way::Up, Way::Down] {
        let offered = allowed.contains(&way) || (way == Way::Down && kept_from);
        let sign = way_sign(way, &row.change);
        let faint = crate::app::unlit(ui.visuals());
        let galley = ui
            .painter()
            .layout_no_wrap(sign.to_string(), font.clone(), faint);
        let width = match offered {
            true => chip_size(&galley).x.max(WAY),
            false => WAY,
        };
        let box_ = egui::Rect::from_center_size(
            egui::pos2(left - width / 2.0, middle),
            egui::vec2(width, CHIP),
        );
        left = box_.left() - WAY_GAP;
        if !offered {
            continue;
        }
        if !sheet.takes(row, way, &drawn.rows, drawn.workspace) {
            let at = box_.center() - galley.size() / 2.0;
            ui.painter().galley(at, galley, faint);
            continue;
        }
        let id = ui.id().with(("way", key, way == Way::Up));
        let chip = way_chip(ui, box_, id, sign, lit == Some(way), selected);
        let chip = match way_hint(way, &row.change) {
            Some(hint) => chip.on_hover_text(hint),
            None => chip,
        };
        if chip.clicked() {
            sheet.turn(row, way, &drawn.rows, drawn.workspace);
        }
    }
    left + WAY_GAP - GAP
}

/// One way's button: `sign` in `box_`, solid while `lit`.
fn way_chip(
    ui: &mut egui::Ui,
    box_: egui::Rect,
    id: egui::Id,
    sign: &str,
    lit: bool,
    selected: bool,
) -> egui::Response {
    let response = ui.interact(box_, id, egui::Sense::click());
    let visuals = ui.visuals();
    let (fill, ink) = match (lit, response.hovered(), selected) {
        (true, _, true) => (
            Some(visuals.selection.stroke.color),
            visuals.selection.bg_fill,
        ),
        (true, _, false) => (Some(warn(visuals)), visuals.panel_fill),
        (false, true, _) => (
            Some(visuals.widgets.hovered.weak_bg_fill),
            cell_ink(selected, visuals.text_color(), visuals),
        ),
        (false, false, _) => (None, cell_ink(selected, visuals.weak_text_color(), visuals)),
    };
    if let Some(fill) = fill {
        ui.painter().rect_filled(box_, 4.0, fill);
    }
    ui.painter().text(
        box_.center(),
        egui::Align2::CENTER_CENTER,
        sign,
        egui::FontId::proportional(NAME),
        ink,
    );
    response
}

/// A row's address, painted right-aligned to `right`. Returns its left edge.
fn address(
    painter: &egui::Painter,
    line: egui::Rect,
    right: f32,
    text: &str,
    ink: egui::Color32,
) -> f32 {
    let galley = painter.layout_no_wrap(text.to_string(), egui::FontId::monospace(MONO), ink);
    let width = galley.size().x;
    painter.galley(
        egui::pos2(right - width, line.center().y - galley.size().y / 2.0),
        galley,
        ink,
    );
    right - width - GAP
}

/// What a send placed by hand does to its slot, or why it stopped: `new`,
/// `replaces “Bright Wurly”` or `checking`.
pub fn placed_words(id: u64, queue: &Queue) -> String {
    match queue.entry(id).map(|held| (&held.failure, &held.replaces)) {
        Some((Some(why), _)) => why.clone(),
        Some((None, Occupancy::Vacant)) => "new".into(),
        Some((None, Occupancy::Held(occupant))) => format!("replaces “{}”", occupant.name),
        Some((None, Occupancy::Unknown)) | None => "checking".into(),
    }
}

/// What dragging a waiting entry carries.
///
/// ⚠️ `fits` asks the instrument attached now. [`refit`] keeps entries that instrument
/// refuses, so an entry being in the queue does not mean it fits.
fn carried(entity: &LocalEntity, device: &DeviceState) -> Carried {
    Carried {
        head: Held {
            what: Item::Local(entity.id),
            kind: Kind::of(entity),
            filed: None,
            fits: fit(device, entity).allowed(),
        },
        name: entity.name.clone(),
        rest: Vec::new(),
    }
}

/// Where an entry is going, as a chip that opens a slot picker.
///
/// Returns the left edge of the space it took, which is where the name before it must
/// stop.
#[allow(clippy::too_many_arguments)]
fn destination(
    ui: &mut egui::Ui,
    held: &Queued,
    row: egui::Rect,
    right: f32,
    ink: egui::Color32,
    device: &DeviceState,
    queue: &Queue,
    acts: &mut Vec<Act>,
) -> f32 {
    let galley = ui
        .painter()
        .layout_no_wrap(shown(held.at), egui::FontId::monospace(MONO), ink);
    let size = chip_size(&galley);
    let box_ = egui::Rect::from_min_size(
        egui::pos2(right - size.x, row.center().y - CHIP / 2.0),
        size,
    );
    let id = ui.id().with(("destination", held.id));
    let chip = flat_chip(ui, box_, id, galley, false).on_hover_text("change where this goes");
    let screen = ui.ctx().screen_rect();
    let room = (screen.bottom() - box_.bottom()).max(box_.top() - screen.top()) - MENU_FOOT;
    // ⚠️ A menu closes on any click, and switching banks is a click. The picker stays
    // open until a cell is picked or a click lands outside it.
    let shown = egui::Popup::menu(&chip)
        .close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside)
        .width(crate::keyboard::grid_width(PICKER_COLUMNS) + ui.spacing().menu_margin.sum().x)
        .show(|ui| {
            if let Some(at) = picker(ui, held, device, queue, room) {
                acts.push(Act::Retarget {
                    id: held.id,
                    class: held.class,
                    at,
                });
                ui.close();
            }
        });
    // ⚠️ egui stacks areas of one order by the last one clicked and remembers it, so a
    // picker opened from a sheet would open behind it after a click in the sheet.
    let parent = ui.layer_id();
    if let Some(shown) = shown.filter(|shown| shown.response.layer_id.order == parent.order) {
        ui.ctx().set_sublayer(parent, shown.response.layer_id);
    }
    box_.left() - GAP
}

/// Cells per row in the picker. Wider than the map's grid, which has a whole tab and
/// room for more rows.
const PICKER_COLUMNS: usize = 6;

/// The base id of the picker's controls.
///
/// ⚠️ Derived from the entry, not from `ui.id()`: the picker is drawn in a popup's `Ui`,
/// not the row's, and two entries' pickers must not share a bank or a cell.
fn salt(held: &Queued) -> egui::Id {
    egui::Id::new(("picker", held.class, held.id))
}

/// The picker behind the chip: a row of bank chips, then the shown bank's slots drawn as
/// the keyboard map's cells, scrolling where the picker would be taller than `room`.
/// Returns the slot a click picked, if any.
///
/// The bank shown is stored per entry, so reopening the picker shows the bank it was
/// left on.
fn picker(
    ui: &mut egui::Ui,
    held: &Queued,
    device: &DeviceState,
    queue: &Queue,
    room: f32,
) -> Option<Location> {
    let banks = device.banks_of(held.class);
    if banks.is_empty() {
        ui.label(
            egui::RichText::new("Nothing in this folder has been read.")
                .text_style(ui_text())
                .weak()
                .italics(),
        );
        return None;
    }
    let salt = salt(held);
    let kept = salt.with("bank");
    let mut bank = ui
        .data(|data| data.get_temp::<u32>(kept))
        .filter(|bank| banks.contains(bank))
        .unwrap_or(held.at.bank + 1);
    ui.horizontal(|ui| {
        for offered in banks.iter().copied() {
            if bank_chip(ui, salt.with(("bank", offered)), offered, offered == bank).clicked() {
                bank = offered;
                ui.data_mut(|data| data.insert_temp(kept, bank));
            }
        }
    });
    let slots = device.bank(held.class, bank).unwrap_or_default();
    let above = ui.cursor().top() - ui.min_rect().top() + ui.spacing().menu_margin.sum().y;
    let mut picked = None;
    let tall = (room - above).max(0.0);
    egui::ScrollArea::vertical()
        .min_scrolled_height(tall)
        .max_height(tall)
        .show(ui, |ui| {
            crate::keyboard::grid(ui, PICKER_COLUMNS, slots.len(), |ui, index, rect| {
                let at = Location::from_user(bank, index as u32 + 1);
                let state = crate::keyboard::State::of(
                    false,
                    queue.waiting(held.class, at).is_some(),
                    slots[index].is_some(),
                );
                let response = ui.interact(
                    rect,
                    salt.with(("slot", at.bank, at.slot)),
                    egui::Sense::click(),
                );
                crate::keyboard::paint_cell(
                    ui,
                    rect,
                    at,
                    slots[index].as_ref(),
                    state,
                    at == held.at,
                    response.hovered(),
                );
                // A cell truncates the name to 42 px, so the hover text shows it in full.
                let occupant = slots[index].as_ref().map(|info| info.name.trim());
                let response = response.on_hover_text(match occupant {
                    Some(name) if !name.is_empty() => format!("{} — {name}", place(held.class, at)),
                    _ => format!("{} — empty", place(held.class, at)),
                });
                if response.clicked() {
                    picked = Some(at);
                }
            });
        });
    picked
}

/// One bank chip in the picker: the bank number, highlighted while that bank is shown.
fn bank_chip(ui: &mut egui::Ui, id: egui::Id, bank: u32, on: bool) -> egui::Response {
    let ink = match on {
        true => ui.visuals().selection.stroke.color,
        false => ui.visuals().text_color(),
    };
    let galley = ui
        .painter()
        .layout_no_wrap(bank.to_string(), egui::FontId::monospace(MONO), ink);
    let (box_, _) = ui.allocate_exact_size(chip_size(&galley), egui::Sense::hover());
    flat_chip(ui, box_, id, galley, on)
}

/// The box a chip needs around `galley`.
fn chip_size(galley: &egui::Galley) -> egui::Vec2 {
    egui::vec2(galley.size().x + 2.0 * CHIP_PAD, CHIP)
}

/// A chip painted by hand, so it senses under its own id: `galley` in `box_`, with no
/// fill until the pointer is over it, or the selection's fill while `on`.
fn flat_chip(
    ui: &mut egui::Ui,
    box_: egui::Rect,
    id: egui::Id,
    galley: std::sync::Arc<egui::Galley>,
    on: bool,
) -> egui::Response {
    let response = ui.interact(box_, id, egui::Sense::click());
    let visuals = ui.visuals();
    let fill = match (on, response.hovered()) {
        (true, _) => Some(visuals.selection.bg_fill),
        (false, true) => Some(visuals.widgets.hovered.weak_bg_fill),
        (false, false) => None,
    };
    if let Some(fill) = fill {
        ui.painter().rect_filled(box_, 2.0, fill);
    }
    ui.painter().galley(
        egui::pos2(
            box_.left() + CHIP_PAD,
            box_.center().y - galley.size().y / 2.0,
        ),
        galley,
        egui::Color32::PLACEHOLDER,
    );
    response
}

/// The diff's three columns: the field, what this computer holds, and what the keyboard
/// holds. Any of them may shrink to nothing.
const DIFF_TRACKS: [Track; 3] = [Track::Share(1.4), Track::Share(1.0), Track::Share(1.0)];

/// The row of column heads over the diff.
fn diff_head(ui: &mut egui::Ui, width: f32, tracks: &[Range<f32>]) {
    let (rect, _) = ui.allocate_exact_size(egui::vec2(width, HEAD), egui::Sense::hover());
    let ink = crate::app::caption(ui.visuals());
    for (head, track) in ["Field", "This computer", "Keyboard"].iter().zip(tracks) {
        cut(
            ui.painter(),
            rect.left() + track.start,
            rect.center().y,
            track.end - track.start,
            head,
            egui::TextFormat::simple(egui::FontId::proportional(11.5), ink),
        );
    }
}

/// One value the two sides disagree on: the field's name, what this computer holds, and
/// what the keyboard holds.
fn field_row(ui: &mut egui::Ui, width: f32, tracks: &[Range<f32>], said: [&str; 3]) {
    let (rect, response) =
        ui.allocate_exact_size(egui::vec2(width, DIFF_ROW), egui::Sense::hover());
    let visuals = ui.visuals().clone();
    let mono = egui::FontId::monospace(DIFF_MONO);
    let fonts = [egui::FontId::proportional(DIFF_MONO), mono.clone(), mono];
    for ((text, font), track) in said.iter().zip(fonts).zip(tracks) {
        cut(
            ui.painter(),
            rect.left() + track.start,
            rect.center().y,
            track.end - track.start,
            text,
            egui::TextFormat::simple(font, visuals.text_color()),
        );
    }
    let [name, here, there] = said;
    let _ = response.on_hover_text(format!("{name}: {here} here, {there} on the keyboard"));
}

/// The single row for a diff that is not a field list.
fn one_row(ui: &mut egui::Ui, width: f32, tracks: &[Range<f32>], tint: egui::Color32, said: &str) {
    let (rect, _) = ui.allocate_exact_size(egui::vec2(width, DIFF_ROW), egui::Sense::hover());
    cut(
        ui.painter(),
        rect.left() + tracks[0].start,
        rect.center().y,
        rect.width(),
        said,
        egui::TextFormat::simple(egui::FontId::proportional(DIFF_MONO), tint),
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::log::Log;
    use crate::tabs::Tabs;
    use crate::testing::{self, Bench};
    use crate::workspace::{Fresh, Origin};

    /// A workspace, and one program's bytes to make assets out of.
    fn bench() -> (Workspace, Log, Vec<u8>) {
        let Bench { workspace, log, .. } = Bench::new();
        (workspace, log, Fresh::Program.bytes().unwrap())
    }

    fn at(slot: u32) -> Location {
        Location { bank: 6, slot }
    }

    /// How many of the entries would write over something already in their slot.
    /// An attached instrument nothing has read yet, and the tabs `poll` wants.
    fn attached(workspace: &Workspace) -> (Device, Tabs) {
        let mut device = Device::new(workspace.ctx().clone());
        device.pretend_attached();
        (device, Tabs::default())
    }

    /// Change a document's bytes the way an edit does.
    fn edit(workspace: &mut Workspace, id: u64, log: &mut Log) {
        let bytes = workspace.get(id).expect("it is in memory").bytes.to_vec();
        let (_, edited) =
            crate::fields::apply(&bytes, &[("center_panel.gain".into(), "96".into())]).unwrap();
        workspace.replace_bytes(id, edited, log);
    }

    /// Identical means the instrument holds the same bytes. A body whose declared fields
    /// all match can still differ in a bit no field claims, and a write that would fix it
    /// is not unnecessary.
    #[test]
    fn a_body_byte_no_field_claims_still_makes_the_bodies_differ() {
        let (_, _, bytes) = bench();
        let read = nord_usb::envelope::unwrap(&bytes).expect("a container");
        let (tag, at, version) = (
            nord_usb::envelope::tag(&read.header),
            nord_usb::envelope::location(&read.header),
            read.header.version,
        );
        let body = read.body.0;
        // The occupant as a slot read delivers it: the wire body in a container this app
        // wraps.
        let flipped = |offset: usize| {
            let mut other = body.clone();
            other[offset] ^= 1;
            nord_usb::envelope::wrap(&tag, at, version, &other).expect("it wraps")
        };

        for offset in 0..body.len() {
            assert!(
                !matches!(compare(&bytes, &flipped(offset)), Diff::Identical),
                "body byte {offset:#06x} differs"
            );
        }

        // A body that still decodes with every registered field unchanged, but differs
        // from this asset's body.
        let unclaimed = (0..body.len()).find(|offset| {
            let other = flipped(*offset);
            apart(&bytes, &other).is_none()
                && nord_format::from_stream(&mut Cursor::new(&other)).is_ok()
        });
        let offset = unclaimed.expect("a program body carries bits no field declares");
        assert!(matches!(
            compare(&bytes, &flipped(offset)),
            Diff::Bytes { first_at } if first_at == offset
        ));
    }

    /// Link an asset to a slot of the attached keyboard, as what it is saved as now.
    fn link_to(workspace: &mut Workspace, device: &Device, id: u64, at: Location) {
        let keyboard = device.state.keyboard().cloned().expect("attached");
        let print = workspace.get(id).unwrap().saved_print().unwrap();
        let held = crate::link::Held::new(ObjectClass::Program, at, "", print).unwrap();
        workspace.link(id, &keyboard, held);
    }

    /// The body checksum of one asset's saved bytes, which is what a slot holding them
    /// reports.
    fn crc(workspace: &Workspace, id: u64) -> u32 {
        workspace
            .get(id)
            .and_then(|entity| entity.saved.crc32())
            .expect("every CBIN container has one")
    }

    /// A linked asset is a Sync row once it is saved over what its slot holds, and stays
    /// one row once queued for that slot. An edit nothing has saved is not a row.
    #[test]
    fn an_asset_is_a_sync_row_once_saved_over_its_slot_and_one_row_while_queued() {
        let (mut workspace, mut log, bytes) = bench();
        let class = ObjectClass::Program;
        let (mut device, _) = attached(&workspace);
        let mut queue = Queue::default();

        let off = |slot: u32, workspace: &mut Workspace, log: &mut Log| {
            workspace.ingest(
                format!("off-{slot}.ne5p"),
                Origin::Device {
                    class,
                    at: at(slot),
                },
                bytes.clone(),
                log,
            )
        };
        let saved = off(0, &mut workspace, &mut log);
        let queued = off(1, &mut workspace, &mut log);
        let unsaved = off(2, &mut workspace, &mut log);
        let held = crc(&workspace, saved);
        device.pretend_bodies(
            class,
            7,
            &[
                Some(("off-0", held)),
                Some(("off-1", held)),
                Some(("off-2", held)),
            ],
        );
        for (slot, id) in [saved, queued, unsaved].into_iter().enumerate() {
            link_to(&mut workspace, &device, id, at(slot as u32));
        }

        let ids = |workspace: &Workspace, device: &Device, queue: &Queue| {
            crate::bring::rows(&device.state, workspace, queue)
                .iter()
                .map(|row| row.id)
                .collect::<Vec<_>>()
        };
        assert_eq!(ids(&workspace, &device, &queue), Vec::<u64>::new());

        // Edited and saved: the slot no longer holds these bytes.
        for id in [saved, queued] {
            edit(&mut workspace, id, &mut log);
            workspace.mark_saved(id);
        }
        // Edited and not saved: the slot still holds what this was saved as.
        edit(&mut workspace, unsaved, &mut log);

        assert_eq!(ids(&workspace, &device, &queue), vec![saved, queued]);

        enqueue(
            &workspace,
            &mut device,
            &mut queue,
            &mut log,
            queued,
            class,
            at(1),
        );
        assert_eq!(ids(&workspace, &device, &queue), vec![saved, queued]);
    }

    /// Two assets cannot wait for one slot, and one asset cannot wait for two: the queue
    /// is a set of destinations and a set of assets at once.
    #[test]
    fn one_entry_per_slot_and_one_per_asset() {
        let (mut workspace, mut log, bytes) = bench();
        let class = ObjectClass::Program;
        let mut asset =
            |name: &str| workspace.ingest(name.into(), Origin::Fresh, bytes.clone(), &mut log);
        let first = asset("first.ne5p");
        let second = asset("second.ne5p");
        let mut queue = Queue::default();

        let landed = queue.put(
            workspace.get(first).unwrap(),
            (class, at(0)),
            Occupancy::Vacant,
            None,
        );
        assert!(matches!(landed, Put::Made));

        let landed = queue.put(
            workspace.get(second).unwrap(),
            (class, at(0)),
            Occupancy::Vacant,
            None,
        );
        assert!(
            matches!(landed, Put::Instead(displaced) if displaced == first),
            "one asset per slot"
        );
        assert_eq!(queue.ids(), vec![second]);

        queue.put(
            workspace.get(first).unwrap(),
            (class, at(1)),
            Occupancy::Vacant,
            None,
        );
        let landed = queue.put(
            workspace.get(first).unwrap(),
            (class, at(2)),
            Occupancy::Vacant,
            None,
        );
        assert!(matches!(landed, Put::Moved), "one slot per asset");
        assert_eq!(queue.ids(), vec![second, first]);

        // Queueing it for the slot it already waits for leaves its entry alone.
        let landed = queue.put(
            workspace.get(first).unwrap(),
            (class, at(2)),
            Occupancy::Vacant,
            None,
        );
        assert!(matches!(landed, Put::Standing));
        assert_eq!(queue.ids(), vec![second, first]);
    }

    /// An entry the instrument attached now refuses stays queued, and dragging it to a
    /// slot is refused as a drag of the same asset from the library would be.
    #[test]
    fn a_waiting_entry_the_instrument_now_refuses_drags_as_refused() {
        let (mut workspace, mut log, bytes) = bench();
        let (mut device, _) = attached(&workspace);
        let class = ObjectClass::Program;
        let id = workspace.ingest("electro".into(), Origin::Fresh, bytes, &mut log);
        let mut queue = Queue::default();
        enqueue(
            &workspace,
            &mut device,
            &mut queue,
            &mut log,
            id,
            class,
            at(0),
        );
        let slot = crate::browser::Onto::Slot { class, at: at(1) };
        let entity = workspace.get(id).unwrap();
        assert!(crate::browser::landing(&carried(entity, &device.state).head, slot).is_ok());

        device.pretend_attached_as("Nord Stage 4");
        refit(&workspace, &device.state, &mut queue, &mut log);
        assert!(queue.entry(id).unwrap().failure.is_some(), "still waiting");
        let dragged = carried(entity, &device.state).head;
        assert!(!dragged.fits);
        match crate::browser::landing(&dragged, slot) {
            Err(why) => assert!(why.contains("format"), "{why}"),
            other => panic!("{other:?} should have been refused"),
        }
    }

    /// A write that stops leaves the rest of the queue where it was, and the entry it
    /// stopped on carries why.
    #[test]
    fn a_failed_write_stays_queued_and_says_why() {
        let (mut workspace, mut log, bytes) = bench();
        let class = ObjectClass::Program;
        let ids: Vec<u64> = (0..3)
            .map(|slot| {
                workspace.ingest(
                    format!("sound {slot}"),
                    Origin::Fresh,
                    bytes.clone(),
                    &mut log,
                )
            })
            .collect();
        let mut queue = Queue::default();
        for (slot, id) in ids.iter().enumerate() {
            queue.put(
                workspace.get(*id).unwrap(),
                (class, at(slot as u32)),
                Occupancy::Vacant,
                None,
            );
        }

        queue.forget(ids[0]);
        assert_eq!(queue.ids(), ids[1..], "the rest are still waiting");
    }

    /// The diff between two bodies with registries lists only the fields that differ.
    /// The pair here is a program and the same program with one field set through the
    /// registry.
    #[test]
    fn a_registry_diff_lists_only_the_fields_that_differ() {
        let (_workspace, _log, here) = bench();
        let (_, there) = crate::fields::apply(&here, &[("center_panel.gain".into(), "96".into())])
            .expect("the registry takes the set");
        assert_ne!(here, there);

        let Diff::Fields(fields) = compare(&here, &there) else {
            panic!("both programs decode into registries");
        };
        let paths: Vec<&str> = fields.iter().map(|field| field.path.as_str()).collect();
        assert_eq!(paths, vec!["center_panel.gain"]);
        let field = &fields[0];
        assert_ne!(field.here, field.there);
        assert_eq!(field.there, "96");

        assert!(matches!(compare(&here, &here), Diff::Identical));
    }

    /// A body with no registry is compared byte for byte, and the reader is pointed at
    /// the first byte the two do not agree on.
    #[test]
    fn a_body_with_no_registry_is_compared_byte_by_byte() {
        let here = b"not a Nord file at all".to_vec();
        let mut there = here.clone();
        there[8] = b'!';

        let Diff::Bytes { first_at } = compare(&here, &there) else {
            panic!("neither of them decodes");
        };
        assert_eq!(first_at, 8);
        assert!(matches!(compare(&here, &here), Diff::Identical));

        // One of them running out is a difference where the shorter one ended.
        let Diff::Bytes { first_at } = compare(&here, &here[..4]) else {
            panic!("one is a prefix of the other");
        };
        assert_eq!(first_at, 4);
    }

    /// An instrument whose piano library has been read up to `banks` of its two banks,
    /// holding a piano called `held` at 1:1.
    fn pianos_read(workspace: &Workspace, banks: u32, held: &str) -> Device {
        let (mut device, _) = attached(workspace);
        device.pretend_partitions(&crate::device::ELECTRO5);
        device.pretend_geometry(ObjectClass::Piano, &[("Grand", 2), ("Upright", 2)]);
        for bank in 1..=banks {
            let name = (bank == 1).then_some((held, 1000));
            device.pretend_lengths(ObjectClass::Piano, bank, &[name, None]);
        }
        device
    }

    /// What a queue holds after queueing `program` alone, in order.
    fn after_queueing(workspace: &Workspace, device: &mut Device, program: u64) -> Queue {
        let mut queue = Queue::default();
        let mut log = Log::default();
        enqueue(
            workspace,
            device,
            &mut queue,
            &mut log,
            program,
            ObjectClass::Program,
            at(0),
        );
        queue
    }

    /// A program placed by hand lists the piano it plays that only this computer has as a
    /// row of its own, going to a free slot, until the keyboard is seen to hold it or a
    /// bank is unread. Nothing is queued for it before Sync.
    #[test]
    fn a_program_placed_by_hand_lists_the_piano_it_plays_as_its_own_row() {
        use crate::bring::{Change, Row, Rows};
        let (mut workspace, mut log, fresh) = bench();
        let piano = workspace.ingest(
            "Tine.npno".into(),
            Origin::Fresh,
            testing::piano(1),
            &mut log,
        );
        let tine = plays::slot_key(ObjectClass::Piano, "Tine").1;
        let (_, bytes) =
            crate::fields::apply(&fresh, &[("piano_panel.id".into(), tine.to_string())]).unwrap();
        let program = workspace.ingest("Keys.ne5p".into(), Origin::Fresh, bytes, &mut log);
        let rows = |device: &mut Device| {
            let queue = after_queueing(&workspace, device, program);
            assert_eq!(queue.ids(), [program], "only what was placed is queued");
            Rows::of(&device.state, &workspace, &queue).list
        };
        let placed = Row {
            id: program,
            class: ObjectClass::Program,
            at: Some(at(0)),
            change: Change::Placed { waits: None },
            fits: true,
        };
        let played = Row {
            id: piano,
            class: ObjectClass::Piano,
            at: Some(Location::from_user(1, 2)),
            change: Change::Plays { by: vec![program] },
            fits: true,
        };
        let mut device = pianos_read(&workspace, 2, "Grand");
        assert_eq!(rows(&mut device), [placed.clone(), played.clone()]);
        let said = played.said(&workspace, &Queue::default());
        assert_eq!(said, "new · “Keys” plays it");
        assert_eq!(
            rows(&mut pianos_read(&workspace, 1, "Grand")),
            std::slice::from_ref(&placed),
            "a bank unread"
        );
        assert_eq!(
            rows(&mut pianos_read(&workspace, 2, "Tine")),
            [placed],
            "already there"
        );
    }

    /// The piano a program plays goes when its program goes: leave the program out and
    /// the piano's row is gone from the sheet and from Sync. With no room for the piano it
    /// goes nowhere and is not counted, and the program still goes.
    #[test]
    fn a_played_pianos_row_follows_its_program_and_one_with_no_room_is_not_counted() {
        use crate::bring::{plan, Change, Rows};
        let (mut workspace, mut log, fresh) = bench();
        workspace.ingest(
            "Tine.npno".into(),
            Origin::Fresh,
            testing::piano(1),
            &mut log,
        );
        let tine = plays::slot_key(ObjectClass::Piano, "Tine").1;
        let (_, bytes) =
            crate::fields::apply(&fresh, &[("piano_panel.id".into(), tine.to_string())]).unwrap();
        let program = workspace.ingest("Keys.ne5p".into(), Origin::Fresh, bytes, &mut log);
        let counted = |device: &mut Device, skip: bool| {
            let queue = after_queueing(&workspace, device, program);
            crate::bring::open(device, None);
            while let Some(class) = device.state.scan.take() {
                device.state.scan.finished(class);
            }
            let sync = Rows::of(&device.state, &workspace, &queue);
            let sheet = device.asks.sheet.as_mut().expect("open");
            let placed = sync.list.iter().find(|row| row.id == program).unwrap();
            if skip {
                sheet.turn(placed, Way::Up, &sync.list, &workspace);
            }
            let steps = plan(sheet, &sync.list, &device.state, &workspace);
            let shown = sync
                .list
                .iter()
                .filter(|row| sheet.shows(row, &sync.list, &workspace));
            let shown: Vec<bool> = shown
                .map(|row| matches!(row.change, Change::Plays { .. }))
                .collect();
            (steps.len(), shown)
        };
        assert_eq!(
            counted(&mut pianos_read(&workspace, 2, "Grand"), false),
            (2, vec![false, true])
        );
        assert_eq!(
            counted(&mut pianos_read(&workspace, 2, "Grand"), true),
            (0, vec![false]),
            "left out, it takes its piano with it"
        );
        let mut full = pianos_read(&workspace, 2, "Grand");
        full.pretend_lengths(
            ObjectClass::Piano,
            1,
            &[Some(("Grand", 1000)), Some(("Upright", 9))],
        );
        full.pretend_lengths(
            ObjectClass::Piano,
            2,
            &[Some(("Rhodes", 9)), Some(("Wurly", 9))],
        );
        assert_eq!(counted(&mut full, false), (1, vec![false, true]), "no room");
    }

    #[test]
    fn a_piano_two_programs_play_goes_up_while_either_does() {
        use crate::bring::{plan, Change, Rows};
        let (mut workspace, mut log, fresh) = bench();
        let piano = workspace.ingest(
            "Tine.npno".into(),
            Origin::Fresh,
            testing::piano(1),
            &mut log,
        );
        let tine = plays::slot_key(ObjectClass::Piano, "Tine").1;
        let (_, bytes) =
            crate::fields::apply(&fresh, &[("piano_panel.id".into(), tine.to_string())]).unwrap();
        let programs = ["Keys.ne5p", "Pad.ne5p"]
            .map(|name| workspace.ingest(name.into(), Origin::Fresh, bytes.clone(), &mut log));
        let mut device = pianos_read(&workspace, 2, "Grand");
        let mut queue = Queue::default();
        for (slot, program) in (0..).zip(programs) {
            let class = ObjectClass::Program;
            enqueue(
                &workspace,
                &mut device,
                &mut queue,
                &mut log,
                program,
                class,
                at(slot),
            );
        }
        crate::bring::open(&mut device, None);
        while let Some(class) = device.state.scan.take() {
            device.state.scan.finished(class);
        }
        let sync = Rows::of(&device.state, &workspace, &queue);
        let played = sync.list.iter().find(|row| row.id == piano).expect("a row");
        let by = programs.to_vec();
        assert_eq!(played.change, Change::Plays { by });
        assert_eq!(played.said(&workspace, &queue), "new · 2 programs play it");

        let sheet = device.asks.sheet.as_mut().expect("open");
        let mut left_out = |program: u64| {
            let row = sync.list.iter().find(|row| row.id == program).unwrap();
            sheet.turn(row, Way::Up, &sync.list, &workspace);
            let steps = plan(sheet, &sync.list, &device.state, &workspace);
            steps.iter().any(|step| step.row.id == piano)
        };
        assert!(left_out(programs[0]), "the other program still plays it");
        assert!(!left_out(programs[1]), "nothing going up plays it");
    }

    #[test]
    fn the_sheet_says_each_placed_write_is_new_or_replaces_and_a_file_placed_on_its_own_slot_is_its_row(
    ) {
        let mut bench = crate::testing::Bench::new();
        let class = ObjectClass::Program;
        let make = |bench: &mut crate::testing::Bench, name: &str| {
            let bytes = Fresh::Program.bytes().unwrap();
            bench
                .workspace
                .ingest(name.into(), Origin::Fresh, bytes, &mut bench.log)
        };
        let (soul, jazzy, tine) = (
            make(&mut bench, "Soul Keys.ne5p"),
            make(&mut bench, "Jazzy.ne5p"),
            make(&mut bench, "Tine.ne5p"),
        );
        bench.device.pretend_bodies(
            class,
            1,
            &[None, Some(("Jazzy", 1)), Some(("Bright Wurly", 2))],
        );
        let base = crate::link::Print { crc: 1, len: 121 };
        let at = Location::from_user(1, 2);
        let keyboard = bench.device.state.keyboard().cloned().unwrap();
        let held = crate::link::Held::new(class, at, "", base).unwrap();
        bench.workspace.link(jazzy, &keyboard, held);
        for (id, slot) in [(soul, 1), (jazzy, 2), (tine, 3)] {
            let at = Location::from_user(1, slot);
            bench.act(vec![Act::Replace { id, class, at }]);
        }
        assert_eq!(placed_words(soul, &bench.queue), "new");
        assert_eq!(placed_words(tine, &bench.queue), "replaces “Bright Wurly”");
        assert!(!bench.queue.holds(jazzy), "its own slot is not queued");
        let opened = bench.device.asks.sheet.as_ref().expect("the sheet opens");
        assert_eq!(opened.picked, Some(jazzy));

        bench.land();
        let sync = crate::bring::Rows::of(&bench.device.state, &bench.workspace, &bench.queue);
        let mut painted = |picked: u64| {
            bench.device.asks.sheet.as_mut().expect("open").picked = Some(picked);
            let mut frame = || {
                let input = testing::screen(egui::vec2(940.0, 600.0), Vec::new());
                testing::run(&bench.ctx, input, |ctx| {
                    let (device, workspace) = (&mut bench.device, &bench.workspace);
                    sheet(
                        ctx,
                        device,
                        (&bench.queue, &sync),
                        workspace,
                        &mut Vec::new(),
                    );
                })
            };
            frame();
            testing::painted(&frame())
        };
        let shown = painted(tine);
        let row = testing::where_(&shown, "replaces “Bright Wurly”");
        assert!(
            row.top() > testing::where_(&shown, "Tine").bottom() - 1.0,
            "the word sits under the name: {row:?}"
        );
        assert!(shown.iter().any(|word| word.text == "newer here"));
        assert!(shown.iter().any(|word| word.text == "new"));
    }

    /// Retargeting a waiting asset moves its entry without adding a second. The entry
    /// then replaces what the new slot holds, and whatever was waiting for that slot is
    /// dropped.
    #[test]
    fn retargeting_moves_the_entry_and_displaces_what_was_waiting_there() {
        let (mut workspace, mut log, bytes) = bench();
        let mut device = Device::new(workspace.ctx().clone());
        let class = ObjectClass::Program;
        device.pretend_scanned(class, 7, &["", "Africa Split", ""]);
        let mut queue = Queue::default();
        let mut asset =
            |name: &str| workspace.ingest(name.into(), Origin::Fresh, bytes.clone(), &mut log);
        let first = asset("first.ne5p");
        let second = asset("second.ne5p");
        for (id, slot) in [(first, 0), (second, 1)] {
            enqueue(
                &workspace,
                &mut device,
                &mut queue,
                &mut log,
                id,
                class,
                at(slot),
            );
        }
        assert!(matches!(
            queue.entry(first).unwrap().replaces,
            Occupancy::Vacant
        ));

        retarget(
            &workspace,
            &mut device,
            &mut queue,
            &mut log,
            first,
            class,
            at(1),
        );

        assert_eq!(queue.ids(), vec![first], "one entry per slot and per asset");
        let held = queue.entry(first).unwrap();
        assert_eq!(held.at, at(1));
        assert_eq!(
            held.replaces.occupant().map(|held| held.name.as_str()),
            Some("Africa Split"),
            "the entry replaces what the new slot holds"
        );

        // Retargeting an asset that is not waiting does not queue it.
        retarget(
            &workspace,
            &mut device,
            &mut queue,
            &mut log,
            second,
            class,
            at(2),
        );
        assert_eq!(queue.ids(), vec![first]);
    }

    /// ⚠️ A menu closes on any click, and switching banks is a click. The picker must stay
    /// open after a bank chip is clicked.
    #[test]
    fn a_click_on_a_bank_chip_leaves_the_picker_open_on_that_bank() {
        let Bench {
            ctx,
            mut workspace,
            mut device,
            mut log,
            ..
        } = Bench::new();
        let class = ObjectClass::Program;
        device.pretend_scanned(class, 7, &["Africa Split", "", ""]);
        device.pretend_scanned(class, 8, &["", "", ""]);
        let bytes = Fresh::Program.bytes().unwrap();
        let id = workspace.ingest("Jazzy Click B".into(), Origin::Fresh, bytes, &mut log);
        let mut queue = Queue::default();
        enqueue(
            &workspace,
            &mut device,
            &mut queue,
            &mut log,
            id,
            class,
            at(1),
        );
        let held = queue.entry(id).expect("it is waiting");

        let chip_id = std::cell::Cell::new(egui::Id::NULL);
        let draw = |events: Vec<egui::Event>| {
            let input = testing::screen(egui::vec2(600.0, 600.0), events);
            testing::run(&ctx, input, |ctx| {
                egui::CentralPanel::default()
                    .frame(egui::Frame::new())
                    .show(ctx, |ui| {
                        chip_id.set(ui.id().with(("destination", held.id)));
                        let row = ui.max_rect();
                        destination(
                            ui,
                            held,
                            row,
                            row.right(),
                            ui.visuals().text_color(),
                            &device.state,
                            &queue,
                            &mut Vec::new(),
                        );
                    });
            });
        };
        // The popup settles into place over a frame or two, so nothing is clicked until
        // it stops moving.
        let settle = || {
            for _ in 0..3 {
                draw(Vec::new());
            }
        };
        let click_at = |on: egui::Pos2| {
            draw(vec![egui::Event::PointerMoved(on)]);
            draw(vec![testing::button(on, true), testing::button(on, false)]);
        };
        let rect_of = |id: egui::Id| ctx.read_response(id).map(|drawn| drawn.rect);

        settle();
        let chip = rect_of(chip_id.get()).expect("the row drew its destination chip");
        click_at(chip.center());

        settle();
        let bank = rect_of(salt(held).with(("bank", 8_u32)))
            .expect("the picker offers every bank that has been read");
        click_at(bank.center());

        settle();
        assert!(
            egui::Popup::is_id_open(&ctx, chip_id.get().with("popup")),
            "the picker stays open after a bank chip is clicked"
        );
        assert!(
            rect_of(salt(held).with(("slot", 7_u32, 0_u32))).is_some(),
            "and it is showing bank 8"
        );
    }

    /// egui raises a clicked area above its peers and remembers that order, so a click in
    /// a sheet could leave a picker it opens underneath it.
    #[test]
    fn the_picker_opens_above_its_sheet_after_a_click_in_the_sheet() {
        let Bench {
            ctx,
            mut workspace,
            mut device,
            mut log,
            ..
        } = Bench::new();
        let class = ObjectClass::Program;
        device.pretend_scanned(class, 7, &["Africa Split", "", ""]);
        let bytes = Fresh::Program.bytes().unwrap();
        let id = workspace.ingest("Jazzy Click B".into(), Origin::Fresh, bytes, &mut log);
        let mut queue = Queue::default();
        enqueue(
            &workspace,
            &mut device,
            &mut queue,
            &mut log,
            id,
            class,
            at(1),
        );
        let held = queue.entry(id).expect("it is waiting");

        let chip_id = std::cell::Cell::new(egui::Id::NULL);
        let draw = |events: Vec<egui::Event>| {
            let input = testing::screen(egui::vec2(800.0, 600.0), events);
            testing::run(&ctx, input, |ctx| {
                egui::Modal::new(egui::Id::new("sheet")).show(ctx, |ui| {
                    ui.set_min_size(egui::vec2(500.0, 400.0));
                    chip_id.set(ui.id().with(("destination", held.id)));
                    let row = egui::Rect::from_min_size(
                        ui.max_rect().min + egui::vec2(0.0, 300.0),
                        egui::vec2(500.0, 28.0),
                    );
                    destination(
                        ui,
                        held,
                        row,
                        row.right(),
                        ui.visuals().text_color(),
                        &device.state,
                        &queue,
                        &mut Vec::new(),
                    );
                });
            });
        };
        let settle = || {
            for _ in 0..3 {
                draw(Vec::new());
            }
        };
        let popup = || chip_id.get().with("popup");
        let open_picker = || {
            settle();
            let chip = ctx
                .read_response(chip_id.get())
                .expect("the sheet drew its chip");
            draw(testing::click(chip.rect.center()));
            settle();
            ctx.memory(|memory| memory.area_rect(popup()))
                .expect("the picker opened")
        };

        open_picker();
        draw(vec![egui::Event::Key {
            key: egui::Key::Escape,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: egui::Modifiers::default(),
        }]);
        settle();
        let sheet = ctx
            .memory(|memory| memory.area_rect(egui::Id::new("sheet")))
            .expect("the sheet is open");
        draw(testing::click(sheet.left_top() + egui::vec2(20.0, 20.0)));
        let area = open_picker();
        assert_eq!(
            ctx.layer_id_at(area.center()),
            Some(egui::LayerId::new(egui::Order::Foreground, popup())),
            "the picker at {area:?} is not the top layer over the sheet at {sheet:?}"
        );
    }

    /// A click on a picker cell returns that slot. The cell's rect comes from the frame
    /// before the click, so the test does not depend on where the bank chips above it
    /// land.
    #[test]
    fn a_click_on_a_picker_cell_returns_that_slot() {
        let Bench {
            ctx,
            mut workspace,
            mut device,
            mut log,
            ..
        } = Bench::new();
        let class = ObjectClass::Program;
        device.pretend_scanned(class, 7, &["Africa Split", "", ""]);
        let bytes = Fresh::Program.bytes().unwrap();
        let id = workspace.ingest("Jazzy Click B".into(), Origin::Fresh, bytes, &mut log);
        let mut queue = Queue::default();
        enqueue(
            &workspace,
            &mut device,
            &mut queue,
            &mut log,
            id,
            class,
            at(1),
        );
        let held = queue.entry(id).expect("it is waiting");

        let wanted = at(2);
        let draw = |events: Vec<egui::Event>| -> (Option<Location>, Option<egui::Pos2>) {
            let input = testing::screen(
                egui::vec2(crate::keyboard::grid_width(PICKER_COLUMNS), 300.0),
                events,
            );
            let mut drawn = (None, None);
            testing::run(&ctx, input, |ctx| {
                egui::CentralPanel::default()
                    .frame(egui::Frame::new())
                    .show(ctx, |ui| {
                        drawn = (
                            picker(ui, held, &device.state, &queue, f32::INFINITY),
                            ctx.read_response(salt(held).with(("slot", wanted.bank, wanted.slot)))
                                .map(|cell| cell.rect.center()),
                        );
                    });
            });
            drawn
        };

        let on_cell = draw(Vec::new())
            .1
            .expect("the picker drew a cell for every slot of the bank");
        draw(vec![egui::Event::PointerMoved(on_cell)]);
        let press = vec![
            testing::button(on_cell, true),
            testing::button(on_cell, false),
        ];
        assert_eq!(draw(press).0, Some(wanted));
    }

    /// A bank of 159 samples is taller than the window. Wherever its chip is, the picker
    /// takes the room up to the window's edge, keeps clear of the chip, and scrolls, so
    /// every slot can be brought into view and picked.
    #[test]
    fn every_slot_of_a_bank_taller_than_the_window_can_be_reached_and_picked() {
        let Bench {
            mut workspace,
            mut device,
            mut log,
            ..
        } = Bench::new();
        let class = ObjectClass::Program;
        let bank = 1;
        let slots = 159;
        device.pretend_scanned(class, bank, &vec![""; slots as usize]);
        let bytes = Fresh::Program.bytes().unwrap();
        let id = workspace.ingest("Jazzy Click B".into(), Origin::Fresh, bytes, &mut log);
        let mut queue = Queue::default();
        let first = Location::from_user(bank, 1);
        enqueue(
            &workspace,
            &mut device,
            &mut queue,
            &mut log,
            id,
            class,
            first,
        );
        let held = queue.entry(id).expect("it is waiting");

        for (size, top) in [
            (egui::vec2(1280.0, 800.0), 120.0),
            (egui::vec2(1280.0, 800.0), 700.0),
            (egui::vec2(900.0, 560.0), 266.0),
        ] {
            let screen = egui::Rect::from_min_size(egui::Pos2::ZERO, size);
            let ctx = testing::context();
            let chip_id = std::cell::Cell::new(egui::Id::NULL);
            let acts = std::cell::RefCell::new(Vec::new());
            let draw = |events: Vec<egui::Event>| {
                testing::run(&ctx, testing::screen(screen.size(), events), |ctx| {
                    egui::CentralPanel::default()
                        .frame(egui::Frame::new())
                        .show(ctx, |ui| {
                            chip_id.set(ui.id().with(("destination", held.id)));
                            let row = egui::Rect::from_min_size(
                                egui::pos2(100.0, top),
                                egui::vec2(680.0, 28.0),
                            );
                            destination(
                                ui,
                                held,
                                row,
                                row.right(),
                                ui.visuals().text_color(),
                                &device.state,
                                &queue,
                                &mut acts.borrow_mut(),
                            );
                        });
                });
            };
            let settle = || {
                for _ in 0..3 {
                    draw(Vec::new());
                }
            };
            let cell = |slot: u32| {
                let at = Location::from_user(bank, slot);
                ctx.read_response(salt(held).with(("slot", at.bank, at.slot)))
                    .unwrap_or_else(|| panic!("{top}: {} has no cell", place(class, at)))
            };
            let shown = |slot: u32| {
                let cell = cell(slot);
                screen.contains_rect(cell.rect) && cell.interact_rect == cell.rect
            };

            settle();
            let chip = ctx.read_response(chip_id.get()).unwrap().rect;
            draw(testing::click(chip.center()));
            settle();
            let area = ctx
                .memory(|memory| memory.area_rect(chip_id.get().with("popup")))
                .expect("the picker opened");
            assert!(
                screen.contains_rect(area),
                "{top}: the picker at {area:?} leaves the window"
            );
            assert!(
                !area.intersect(chip).is_positive(),
                "{top}: the picker at {area:?} covers its chip at {chip:?}"
            );
            let edge = 2.0 * MENU_FOOT;
            assert!(
                area.top() <= screen.top() + edge || area.bottom() >= screen.bottom() - edge,
                "{top}: the picker at {area:?} stops short of the window's edge"
            );

            let over = cell(1).rect.center();
            let mut reached = std::collections::BTreeSet::new();
            for _ in 0..100 {
                reached.extend((1..=slots).filter(|&slot| shown(slot)));
                if shown(slots) {
                    break;
                }
                draw(vec![
                    egui::Event::PointerMoved(over),
                    egui::Event::MouseWheel {
                        unit: egui::MouseWheelUnit::Point,
                        delta: egui::vec2(0.0, -120.0),
                        modifiers: egui::Modifiers::default(),
                    },
                ]);
                settle();
            }
            let missed: Vec<u32> = (1..=slots).filter(|slot| !reached.contains(slot)).collect();
            assert!(missed.is_empty(), "{top}: never in view: {missed:?}");

            let last = cell(slots).rect.center();
            draw(vec![egui::Event::PointerMoved(last)]);
            draw(vec![
                testing::button(last, true),
                testing::button(last, false),
            ]);
            let picked = acts.borrow().iter().find_map(|act| match act {
                Act::Retarget { at, .. } => Some(*at),
                _ => None,
            });
            assert_eq!(picked, Some(Location::from_user(bank, slots)), "{top}");
        }
    }

    /// The sheet's diff table for `held`, sent under `name`, as painted.
    fn table_words(held: &Queued, name: &str, diff: &Diff) -> Vec<testing::Word> {
        let ctx = testing::context();
        let input = testing::screen(egui::vec2(620.0, 300.0), Vec::new());
        let output = testing::run(&ctx, input, |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                table(
                    ui,
                    diff,
                    held.renames(name),
                    (held.class, held.at),
                    &Workspace::new(ctx.clone()),
                    &DeviceState::default(),
                )
            });
        });
        testing::painted(&output)
    }

    /// The name goes with the sound, so a send of the same bytes under another name
    /// renames the slot. The sheet shows the name it gives beside the one the keyboard
    /// holds, and says nothing of a name that stays.
    #[test]
    fn a_send_that_renames_its_slot_shows_the_name_it_gives() {
        let (mut workspace, mut log, bytes) = bench();
        let mut device = Device::new(workspace.ctx().clone());
        let class = ObjectClass::Program;
        device.pretend_scanned(class, 7, &["Incense"]);

        for (name, renamed) in [("qa-c.ne5p", true), ("Incense.ne5p", false)] {
            let mut queue = Queue::default();
            let id = workspace.ingest(
                name.into(),
                Origin::Device { class, at: at(0) },
                bytes.clone(),
                &mut log,
            );
            enqueue(
                &workspace,
                &mut device,
                &mut queue,
                &mut log,
                id,
                class,
                at(0),
            );
            let held = queue.entry(id).unwrap();
            let said = table_words(held, name, &compare(&bytes, &bytes));
            assert_eq!(
                said.iter().any(|word| word.text == NAME_ROW),
                renamed,
                "{name}"
            );
            if renamed {
                let was = testing::where_(&said, "Incense");
                let given = testing::where_(&said, "qa-c");
                assert_eq!(was.left(), testing::where_(&said, "Keyboard").left());
                assert_eq!(given.left(), testing::where_(&said, "This computer").left());
            }
        }
    }

    /// Each row of the diff shows what this computer holds beside what the keyboard holds,
    /// under fixed headings.
    #[test]
    fn the_diff_shows_this_computer_beside_the_keyboard() {
        let (mut workspace, mut log, bytes) = bench();
        let mut device = Device::new(workspace.ctx().clone());
        let class = ObjectClass::Program;
        device.pretend_scanned(class, 7, &["Africa Split"]);
        let mut queue = Queue::default();
        let id = workspace.ingest(
            "Africa Split.ne5p".into(),
            Origin::Device { class, at: at(0) },
            bytes.clone(),
            &mut log,
        );
        edit(&mut workspace, id, &mut log);
        enqueue(
            &workspace,
            &mut device,
            &mut queue,
            &mut log,
            id,
            class,
            at(0),
        );
        let held = queue.entry(id).unwrap();
        let diff = compare(&workspace.get(id).unwrap().bytes, &bytes);
        let Diff::Fields(fields) = &diff else {
            panic!("a program against a program is a field list");
        };
        let field = &fields[0];
        assert_ne!(field.there, field.here);

        let said = table_words(held, "Africa Split.ne5p", &diff);
        let here = testing::where_(&said, "This computer");
        let keyboard = testing::where_(&said, "Keyboard");
        assert!(here.left() < keyboard.left(), "{here:?} {keyboard:?}");
        assert_eq!(testing::where_(&said, &field.here).left(), here.left());
        assert_eq!(testing::where_(&said, &field.there).left(), keyboard.left());
        assert!(
            !said.iter().any(|word| word.text == NAME_ROW),
            "the name stays"
        );
    }

    #[test]
    fn a_piano_or_sample_id_in_a_diff_reads_as_the_name_it_goes_by() {
        let Bench {
            mut workspace,
            device,
            mut log,
            ..
        } = Bench::new();
        workspace.ingest(
            "Tine.npno".into(),
            Origin::Fresh,
            testing::piano(1),
            &mut log,
        );
        let id = |class, name| plays::slot_key(class, name).1.to_string();
        let sets = [
            ("piano_panel.id".into(), id(ObjectClass::Piano, "Tine")),
            ("sample_panel.id".into(), id(ObjectClass::Sample, "Marimba")),
        ];
        let bytes = Fresh::Program.bytes().unwrap();
        let (_, tine) = crate::fields::apply(&bytes, &sets).unwrap();
        let Diff::Fields(fields) = compare(&tine, &bytes) else {
            panic!("two programs differ by field");
        };
        let said: Vec<[String; 3]> = fields
            .iter()
            .map(|field| field.said(&workspace, &device.state))
            .collect();

        assert!(
            said.contains(&["Piano".into(), "Tine".into(), "by the dials".into()]),
            "{said:?}"
        );
        let sample = said.iter().find(|[what, ..]| what == "Sample");
        let unnamed = format!(
            "{:#010x}",
            plays::slot_key(ObjectClass::Sample, "Marimba").1
        );
        assert_eq!(
            sample.map(|[_, here, _]| here.as_str()),
            Some(unnamed.as_str()),
            "an id nothing names stays as it is"
        );
    }

    /// Paints the sheet headlessly with each kind of diff, to catch a layout that panics
    /// or an id that collides.
    #[test]
    fn the_sheet_paints_every_kind_of_diff_at_any_window_width() {
        let Bench {
            ctx,
            mut workspace,
            mut device,
            mut log,
            ..
        } = Bench::new();
        let class = ObjectClass::Program;
        device.pretend_scanned(class, 7, &["Africa Split", "Squabble B", ""]);

        let bytes = Fresh::Program.bytes().unwrap();
        let (_, edited) =
            crate::fields::apply(&bytes, &[("center_panel.gain".into(), "96".into())])
                .expect("the registry takes the set");
        let mut queue = Queue::default();
        for slot in 0..3 {
            let id = workspace.ingest(
                format!("a rather long name for sound {slot}"),
                Origin::Device {
                    class,
                    at: at(slot),
                },
                edited.clone(),
                &mut log,
            );
            enqueue(
                &workspace,
                &mut device,
                &mut queue,
                &mut log,
                id,
                class,
                at(slot),
            );
        }
        crate::bring::open(&mut device, None);
        while let Some(class) = device.state.scan.take() {
            device.state.scan.finished(class);
        }
        let sync = crate::bring::Rows::of(&device.state, &workspace, &queue);
        // One waiting on its read, one with a field list, one for a free slot.
        for width in [700.0_f32, 1280.0] {
            for (slot, id) in queue.ids().into_iter().enumerate() {
                let sheet_ = device.asks.sheet.as_mut().expect("open");
                sheet_.picked = Some(id);
                let diff = compare(&edited, &bytes);
                let diff = match slot {
                    0 => Diff::Pending,
                    1 => diff,
                    _ => Diff::Empty,
                };
                let at = at(slot as u32);
                sheet_.shown = Some(crate::bring::Shown {
                    id,
                    class,
                    at,
                    diff,
                });
                let input = testing::screen(egui::vec2(width, 600.0), Vec::new());
                testing::run(&ctx, input, |ctx| {
                    sheet(
                        ctx,
                        &mut device,
                        (&queue, &sync),
                        &workspace,
                        &mut Vec::new(),
                    );
                });
            }
        }
    }

    /// Every track may shrink to nothing, and none of them ever reaches past the width
    /// it was given or turns negative.
    #[test]
    fn the_diff_grid_gives_every_column_room_until_there_is_none() {
        let laid = |width: f32| crate::panel::tracks(width, &DIFF_TRACKS, GAP);
        for width in [90.0_f32, 240.0, 620.0] {
            let tracks = laid(width);
            for track in &tracks {
                assert!(track.start >= 0.0, "{width}: {track:?}");
                assert!(track.end >= track.start, "{width}: {track:?}");
                assert!(track.end <= width + 0.001, "{width}: {track:?}");
            }
            let overlap = tracks.windows(2).any(|two| two[1].start < two[0].end);
            assert!(!overlap, "{width}");
        }
        // At the design width, every column has room.
        assert!(laid(620.0)
            .iter()
            .all(|track| track.end - track.start > 1.0));
        // Narrower than the fixed track alone, no track is negative.
        assert!(laid(4.0).iter().all(|track| track.end >= track.start));
    }
}
