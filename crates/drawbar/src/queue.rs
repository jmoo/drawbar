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
use crate::browser::{cell_ink, Act, Carried, Held, Item, Kind};
use crate::device::{fit, Device, DeviceCmd, DeviceState, Fit, Purpose};
use crate::fields::fields_of;
use crate::icon::{painted, sized, Glyph};
use crate::log::Log;
use crate::panel::{cell, cut, row_ink, Track, GAP, GLYPH, PAD};
use crate::strings::{label, place};
use crate::workspace::{first_difference, wire_body, LocalEntity, Workspace};

/// One asset waiting to be written, and the slot it is waiting for.
pub struct Queued {
    /// The asset on this computer.
    pub id: u64,
    pub class: ObjectClass,
    pub at: Location,
    /// What the slot held when this was queued.
    pub replaces: Occupancy,
    /// How what is waiting differs from what the slot holds.
    pub diff: Diff,
    /// How far the compare read of [`Queued::at`] has progressed.
    read: Read,
    /// The stamp of the asset's bytes [`Queued::diff`] was made from, so an edit under a
    /// waiting entry is noticed without comparing anything.
    stamp: u64,
    /// Why the last attempt to write it stopped. Cleared when it is queued again, and
    /// when [`refit`] finds that the attached instrument accepts it.
    pub failure: Option<String>,
}

/// How far the compare read of the slot an entry is waiting for has progressed.
enum Read {
    /// No read has been asked for, or the read found the slot empty.
    Unasked,
    /// Asked for, and not yet answered.
    Asked,
    /// The occupant's bytes, kept so that an edit made after this entry was queued is
    /// diffed against them without reading the slot again.
    Answered(Vec<u8>),
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
}

/// What is in the slot an entry is waiting for.
///
/// ⚠️ `Unknown` is not `Vacant`: a bank nobody has read may hold something, and calling
/// its slot free is the mistake this type exists to prevent.
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

    /// What is known about the slot, in the words used by both the question before a
    /// write and the queue row.
    ///
    /// ⚠️ The three variants must read differently: a slot nothing has read is not an
    /// empty one, and a write is about to happen either way.
    pub fn said(&self, class: ObjectClass, at: Location) -> String {
        let where_ = place(class, at);
        match self {
            Occupancy::Held(held) => format!(
                "{where_} holds “{}”, {} bytes, which this replaces",
                held.name, held.body_len
            ),
            Occupancy::Vacant => format!("{where_} is empty"),
            Occupancy::Unknown => format!("{where_} has not been read yet"),
        }
    }
}

/// What a slot holds, as the read that found it reported.
pub struct Occupant {
    pub name: String,
    /// ⚠️ `None` where the class reports no checksum, or where the occupant arrived as
    /// bytes from a read instead of as a walk's entry. `None` means the checksums cannot
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

    /// The occupant a compare read answered with, for a slot no walk had reached.
    fn read(name: &str, bytes: &[u8]) -> Occupant {
        let body = nord_usb::envelope::unwrap(bytes)
            .map(|read| read.body.0.len())
            .unwrap_or(bytes.len());
        Occupant {
            name: name.trim().to_string(),
            crc: None,
            body_len: u32::try_from(body).unwrap_or(u32::MAX),
        }
    }
}

/// Everything owed to the instrument, in the order it was asked for.
#[derive(Default)]
pub struct Queue {
    list: Vec<Queued>,
    /// The entry the review shows in detail.
    pub(crate) picked: Option<u64>,
}

/// The outcome of queueing an asset, which decides what the log says.
enum Put {
    /// A new entry, for a slot nothing else was waiting for.
    Made,
    /// This asset was already waiting for this same slot. Nothing moved.
    Standing,
    /// It was waiting for another slot, and moved here.
    Moved(ObjectClass, Location),
    /// Something else was waiting for this slot, and was dropped.
    Instead(u64),
}

/// Ask the instrument what is in the slot an entry is waiting for.
fn read_occupant(device: &mut Device, log: &mut Log, class: ObjectClass, at: Location) {
    device.send(
        DeviceCmd::Get {
            class,
            at,
            why: Purpose::Compare,
        },
        log,
    );
}

/// Queue an asset to be written to a slot, and log what that displaced.
///
/// The queue keeps one entry per asset and one per destination, so this moves the
/// asset's entry if it was waiting for another slot and drops whatever else was waiting
/// for this one. The slot is read unless the scan cache shows it empty or holding a body
/// with the same checksum. A slot in a bank the scan never reached is read, not assumed
/// vacant.
///
/// Queueing an asset for the slot it is already waiting for does nothing: it logs
/// nothing and asks the instrument nothing.
pub fn enqueue(
    workspace: &Workspace,
    device: &mut Device,
    queue: &mut Queue,
    log: &mut Log,
    id: u64,
    class: ObjectClass,
    at: Location,
) {
    let Some(entity) = workspace.get(id) else {
        return;
    };
    let name = entity.name.clone();
    let where_ = place(class, at);
    // A send writes what the queue holds, so an asset the instrument refuses must never
    // enter it.
    if let Fit::Refuses(why) = fit(&device.state, entity) {
        return log.trouble(format!("“{name}” cannot go to {where_}. {why}"));
    }
    let holds = Occupancy::of(&device.state, class, at);
    let displaced = match queue.put(entity, class, at, holds) {
        Put::Standing => return,
        Put::Made => None,
        Put::Moved(was, before) => Some(format!(
            "“{name}” is waiting for {where_} rather than {}.",
            place(was, before)
        )),
        Put::Instead(other) => workspace.get(other).map(|other| {
            format!(
                "“{name}” is waiting for {where_}; “{}” is not anymore.",
                other.name
            )
        }),
    };
    if let Some((class, at)) = queue.unread(id) {
        read_occupant(device, log, class, at);
    }
    log.say(displaced.unwrap_or(format!("“{name}” is waiting to be sent to {where_}.")));
}

/// The assets not in the queue whose slot on the attached instrument no longer holds what
/// they were saved as, each with that slot.
///
/// An edit queues nothing; saving an asset that stands for a slot does. This finds saved
/// assets that are not waiting, which a send would skip. It uses the same comparison that
/// [`crate::library::Where::Both`] shows in the table.
///
/// ⚠️ Only an asset's link counts, which is the one slot it stands for. An asset the
/// attached instrument refuses has no link however well its origin matches, and counting
/// it would offer a send that [`enqueue`] refuses on every click.
pub fn changed(
    workspace: &Workspace,
    device: &DeviceState,
    queue: &Queue,
) -> Vec<(u64, ObjectClass, Location)> {
    workspace
        .listed()
        .filter(|entity| !queue.holds(entity.id))
        .filter_map(|entity| {
            let (class, at) = entity.link?;
            let info = device.slot(class, at).flatten()?;
            (crate::library::agrees(entity, class, info, queue) == Some(false))
                .then_some((entity.id, class, at))
        })
        .collect()
}

/// The toolbar's offer to queue what [`changed`] finds: the button's label and its
/// hover text. `None` where nothing has changed.
pub fn offer(
    workspace: &Workspace,
    device: &DeviceState,
    queue: &Queue,
) -> Option<(String, String)> {
    let changed = changed(workspace, device, queue).len();
    (changed > 0).then(|| {
        (
            format!("Queue {changed}"),
            format!("queue {changed} changed to send to the keyboard"),
        )
    })
}

/// Check everything waiting against the attached instrument again, and log what it
/// refuses.
///
/// ⚠️ The queue outlives a disconnection, so each entry was checked against whatever was
/// attached when it was queued. An entry this instrument refuses keeps its place and
/// carries the reason. A send leaves it out of the batch, so nothing writes it until it
/// is queued again against an instrument that accepts it.
pub fn refit(workspace: &Workspace, state: &DeviceState, queue: &mut Queue, log: &mut Log) {
    for held in &mut queue.list {
        let Some(entity) = workspace.get(held.id) else {
            continue;
        };
        let refusal = match fit(state, entity) {
            Fit::Refuses(why) => Some(why),
            Fit::Unattached | Fit::Takes | Fit::Warn(_) => None,
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

/// Discard what the previous instrument reported about the slots being waited for, and
/// ask the attached one.
///
/// ⚠️ The queue outlives a disconnection, but reads in flight do not. An entry left
/// waiting for an answer that will never come would wait for the rest of the session,
/// and what the last instrument held in a slot says nothing about this one.
pub fn reattach(workspace: &Workspace, device: &mut Device, queue: &mut Queue, log: &mut Log) {
    for held in &mut queue.list {
        let Some(entity) = workspace.get(held.id) else {
            continue;
        };
        held.replaces = Occupancy::of(&device.state, held.class, held.at);
        held.read = Read::Unasked;
        held.diff = verdict(entity, &held.replaces);
    }
    for id in queue.ids() {
        if let Some((class, at)) = queue.unread(id) {
            read_occupant(device, log, class, at);
        }
    }
}

/// Queue every asset [`changed`] finds, each for its own slot.
pub fn queue_changed(workspace: &Workspace, device: &mut Device, queue: &mut Queue, log: &mut Log) {
    for (id, class, at) in changed(workspace, &device.state, queue) {
        enqueue(workspace, device, queue, log, id, class, at);
    }
}

/// The diff before the occupant's bytes have been read. A vacant slot has nothing to
/// replace, and two bodies whose checksums match are identical, because the instrument
/// reports the body's CRC-32 for a slot.
///
/// ⚠️ The checksum is of the current bytes, which a send writes, and not of the saved
/// baseline that [`crate::device::link`] matches a slot against.
fn verdict(entity: &LocalEntity, replaces: &Occupancy) -> Diff {
    let here = entity.container.as_ref().map(|held| held.body_crc32);
    match replaces {
        Occupancy::Vacant => Diff::Empty,
        Occupancy::Held(held) if held.crc.is_some() && held.crc == here => Diff::Identical,
        Occupancy::Held(_) | Occupancy::Unknown => Diff::Pending,
    }
}

/// Diff again every waiting entry whose asset was edited since its last diff.
///
/// An entry with an answered compare read is diffed against the occupant's bytes it kept,
/// without asking the instrument again. An entry whose read is still out keeps waiting
/// for it. An entry the checksums settled without a read is checked against the
/// checksums again, and asks for the read only when they no longer match.
pub fn follow(workspace: &Workspace, device: &mut Device, queue: &mut Queue, log: &mut Log) {
    let mut moved = Vec::new();
    for held in &mut queue.list {
        let Some(entity) = workspace.get(held.id) else {
            continue;
        };
        if entity.stamp == held.stamp {
            continue;
        }
        held.stamp = entity.stamp;
        held.diff = match &held.read {
            Read::Answered(there) => compare(&entity.bytes, there),
            Read::Unasked | Read::Asked => verdict(entity, &held.replaces),
        };
        moved.push(held.id);
    }
    for id in moved {
        if let Some((class, at)) = queue.unread(id) {
            read_occupant(device, log, class, at);
        }
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

impl Queue {
    /// Queue one asset for one slot, and report what that displaced: where this asset was
    /// waiting before, or what was waiting for this slot.
    ///
    /// Callers use [`enqueue`], which wraps this.
    ///
    /// ⚠️ An asset already waiting for this slot keeps its entry and the read that entry
    /// is waiting on. Rebuilding the entry would discard an occupant already read and ask
    /// the instrument for it again.
    fn put(
        &mut self,
        entity: &LocalEntity,
        class: ObjectClass,
        at: Location,
        replaces: Occupancy,
    ) -> Put {
        if let Some(held) = self
            .list
            .iter_mut()
            .find(|held| (held.id, held.class, held.at) == (entity.id, class, at))
        {
            held.failure = None;
            self.picked = Some(entity.id);
            return Put::Standing;
        }
        let moved = self
            .list
            .iter()
            .find(|held| held.id == entity.id)
            .map(|held| (held.class, held.at))
            .filter(|held| *held != (class, at));
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
            diff: verdict(entity, &replaces),
            replaces,
            read: Read::Unasked,
            stamp: entity.stamp,
            failure: None,
        });
        self.picked = Some(entity.id);
        match (moved, instead_of) {
            (Some((was, before)), _) => Put::Moved(was, before),
            (None, Some(other)) => Put::Instead(other),
            (None, None) => Put::Made,
        }
    }

    /// The slot an entry still needs a compare read of, now marked as asked for.
    ///
    /// Queueing, re-queueing, and editing while the read is out all ask here, so the
    /// instrument is asked about a destination only once.
    fn unread(&mut self, id: u64) -> Option<(ObjectClass, Location)> {
        let held = self.list.iter_mut().find(|held| held.id == id)?;
        if !matches!(held.diff, Diff::Pending) {
            return None;
        }
        match held.read {
            Read::Unasked => {
                held.read = Read::Asked;
                Some((held.class, held.at))
            }
            Read::Asked | Read::Answered(_) => None,
        }
    }

    /// Record the occupant a compare read returned for a slot something is waiting for.
    ///
    /// For a slot no walk had reached, this is also how the entry learns that the slot is
    /// occupied.
    pub fn arrived(
        &mut self,
        class: ObjectClass,
        at: Location,
        name: &str,
        there: &[u8],
        workspace: &Workspace,
    ) {
        let Some(held) = self.waiting_for(class, at) else {
            return;
        };
        if let Occupancy::Unknown = held.replaces {
            held.replaces = Occupancy::Held(Occupant::read(name, there));
        }
        let Some(entity) = workspace.get(held.id) else {
            return;
        };
        held.read = Read::Answered(there.to_vec());
        held.stamp = entity.stamp;
        held.diff = compare(&entity.bytes, there);
    }

    /// Record that the compare read of a slot something is waiting for found it empty.
    pub fn vacant(&mut self, class: ObjectClass, at: Location) {
        let Some(held) = self.waiting_for(class, at) else {
            return;
        };
        held.replaces = Occupancy::Vacant;
        held.read = Read::Unasked;
        held.diff = Diff::Empty;
    }

    fn waiting_for(&mut self, class: ObjectClass, at: Location) -> Option<&mut Queued> {
        self.list
            .iter_mut()
            .find(|held| (held.class, held.at) == (class, at))
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
        if self.picked == Some(id) {
            self.picked = self.list.first().map(|held| held.id);
        }
    }

    /// Empty the queue. The queue is only a plan, so this deletes no asset and sends
    /// nothing to the instrument.
    pub fn clear(&mut self) {
        self.list.clear();
        self.picked = None;
    }

    /// Record why a write into `class` stopped, on the entry it stopped on.
    ///
    /// ⚠️ A batch writes its entries in queue order and each written entry leaves the
    /// queue, so the entry it stopped on is the first of that class still waiting without
    /// a failure. Entries that already carry one were left out of the batch, and no write
    /// reached them.
    pub fn stumbled(&mut self, class: ObjectClass, why: &str) {
        if let Some(held) = self
            .list
            .iter_mut()
            .find(|held| held.class == class && held.failure.is_none())
        {
            held.failure = Some(why.to_string());
        }
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
}

/// How what is waiting differs from what the slot holds.
///
/// ⚠️ The bytes decide whether the two agree. A registry covers only the fields it
/// declares, and a bit no field claims is still a difference. Calling two bodies identical
/// because every declared field matches would call a necessary write unnecessary.
///
/// When both bodies decode into registries, the difference is a list of fields;
/// otherwise it is an offset into the wire body. Bodies are compared as the wire carries
/// them, without their file containers, because two containers of one body can differ
/// in their headers alone.
fn compare(here: &[u8], there: &[u8]) -> Diff {
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
                path: field.path,
                here: field.display,
                there: there.display.clone(),
            })
        })
        .collect();
    (!differing.is_empty()).then_some(differing)
}

/// The height of one waiting item.
const ROW: f32 = 32.0;

/// The state glyph at the end of a row.
const SMALL: f32 = 13.0;

/// The destination chip's height, and its padding at each end.
const CHIP: f32 = 20.0;
const CHIP_PAD: f32 = 6.0;

/// The font sizes a row uses.
const NAME: f32 = 12.5;
const MONO: f32 = 10.5;

/// The item list's preferred and least widths, and the geometry of the diff beside it.
const ITEMS: f32 = 250.0;
const ITEMS_LEAST: f32 = 170.0;
const HEAD: f32 = 24.0;
const DIFF_ROW: f32 = 26.0;
const DIFF_MONO: f32 = 11.5;

/// The review's largest size, and its margin from the window's edge.
const SHEET: egui::Vec2 = egui::vec2(940.0, 580.0);
const SHEET_MARGIN: f32 = 24.0;

/// The review of what is waiting: what each write replaces, how it differs, and what to
/// know before sending. Nothing is written until Send all.
///
/// Returns whether it stays open: Escape, a click on the backdrop, Not now, Send all, or
/// opening an item's document closes it.
pub fn review(
    ctx: &egui::Context,
    queue: &mut Queue,
    workspace: &Workspace,
    device: &DeviceState,
    acts: &mut Vec<Act>,
) -> bool {
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
    let shown = egui::Modal::new(egui::Id::new("review"))
        .frame(frame)
        .backdrop_color(egui::Color32::from_black_alpha(115))
        .show(ctx, |ui| {
            ui.set_min_size(size);
            ui.set_max_size(size);
            ui.spacing_mut().item_spacing.y = 0.0;
            open &= review_head(ui, queue, device);
            let foot = review_foot(ui, queue, workspace, device, acts);
            open &= foot;
            review_body(ui, queue, workspace, device, acts);
        });
    let opened_one = acts[before..]
        .iter()
        .any(|act| matches!(act, Act::Open(_) | Act::SendAll));
    open && !shown.should_close() && !opened_one
}

/// The review's header: what it is, where the writes go, and how many of each kind.
/// Returns false when close was clicked.
fn review_head(ui: &mut egui::Ui, queue: &Queue, device: &DeviceState) -> bool {
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
    painted(&bar, Glyph::Upload, tile.shrink(8.0), lit);
    bar.vertical(|ui| {
        ui.spacing_mut().item_spacing.y = 2.0;
        ui.label(
            egui::RichText::new("Review send queue")
                .font(egui::FontId::new(15.0, crate::app::bold())),
        );
        let to = device.product().unwrap_or("the instrument");
        ui.label(
            egui::RichText::new(format!("To {to} · nothing is written until you send"))
                .size(12.0)
                .color(quiet),
        );
    });
    bar.with_layout(egui::Layout::right_to_left(egui::Align::Center), |bar| {
        bar.spacing_mut().item_spacing.x = 6.0;
        crate::panel::flat(bar);
        let close = egui::Button::image(sized(Glyph::X, 14.0, quiet))
            .image_tint_follows_text_color(false)
            .corner_radius(7.0)
            .min_size(egui::Vec2::splat(28.0));
        if bar.add(close).on_hover_text("close").clicked() {
            open = false;
        }
        bar.add_space(4.0);
        let (writes, replaces, failed) = counts(queue);
        if failed > 0 {
            crate::panel::signal_pill(
                bar,
                &plural(failed, "cannot go", "cannot go"),
                bad(&visuals),
            );
        }
        if replaces > 0 {
            crate::panel::signal_pill(bar, &format!("{replaces} replace"), warn(&visuals));
        }
        crate::panel::quiet_pill(bar, &plural(writes, "write", "writes"));
    });
    open
}

/// How many entries will be written, how many of those replace something, and how many
/// the attached instrument has refused.
fn counts(queue: &Queue) -> (usize, usize, usize) {
    let failed = queue
        .entries()
        .iter()
        .filter(|held| held.failure.is_some())
        .count();
    let replaces = queue
        .entries()
        .iter()
        .filter(|held| held.failure.is_none() && matches!(held.replaces, Occupancy::Held(_)))
        .count();
    (queue.len() - failed, replaces, failed)
}

/// `n` and the noun for it.
fn plural(n: usize, one: &str, many: &str) -> String {
    match n {
        1 => format!("1 {one}"),
        n => format!("{n} {many}"),
    }
}

/// The review's foot, along the bottom: what to know before sending, then Clear, Not now,
/// and Send all. Returns false when the review should close.
fn review_foot(
    ui: &mut egui::Ui,
    queue: &Queue,
    workspace: &Workspace,
    device: &DeviceState,
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
    let sending = queue.len() - counts(queue).2;
    let send = crate::panel::accent_button(
        &mut bar,
        Glyph::Upload,
        &format!("Send all {sending}"),
        sending > 0,
    )
    .on_disabled_hover_text("Nothing waiting can go to the instrument attached now.");
    if send.clicked() {
        acts.push(Act::SendAll);
    }
    if crate::panel::tonal_button(&mut bar, None, "Not now").clicked() {
        open = false;
    }
    if !queue.is_empty()
        && crate::panel::tonal_button(&mut bar, None, "Clear")
            .on_hover_text("stop waiting to send any of it; nothing is deleted")
            .clicked()
    {
        acts.push(Act::ClearQueue);
    }
    let warnings = crate::browser::send_warnings(queue, workspace, device);
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

/// The waiting items beside the diff of the one picked.
fn review_body(
    ui: &mut egui::Ui,
    queue: &mut Queue,
    workspace: &Workspace,
    device: &DeviceState,
    acts: &mut Vec<Act>,
) {
    let whole = ui.max_rect();
    let body = egui::Rect::from_min_max(
        egui::pos2(whole.left() + 10.0, ui.cursor().top() + 4.0),
        egui::pos2(whole.right() - 10.0, whole.bottom() - 58.0 - 10.0),
    );
    let mut inside = ui.new_child(egui::UiBuilder::new().max_rect(body));
    let ui = &mut inside;
    if queue.is_empty() {
        ui.add_space(GAP);
        ui.horizontal(|ui| {
            ui.add_space(PAD);
            ui.label(
                egui::RichText::new("Nothing is waiting to be sent.")
                    .text_style(ui_text())
                    .weak()
                    .italics(),
            );
        });
        return;
    }
    if queue.picked.is_none_or(|id| queue.entry(id).is_none()) {
        queue.picked = queue.entries().first().map(|held| held.id);
    }
    let list = (body.width() * 0.3).clamp(ITEMS_LEAST, ITEMS);
    let items = egui::Rect::from_min_size(body.min, egui::vec2(list, body.height()));
    let pane = egui::Rect::from_min_max(egui::pos2(items.right() + 8.0, body.top()), body.max);
    let picked = queue.picked;
    let mut clicked = None;
    let mut column = ui.new_child(egui::UiBuilder::new().max_rect(items));
    egui::ScrollArea::vertical()
        .id_salt("queue_items")
        .auto_shrink([false; 2])
        .show(&mut column, |ui| {
            ui.spacing_mut().item_spacing.y = 2.0;
            for held in queue.entries() {
                let Some(entity) = workspace.get(held.id) else {
                    continue;
                };
                let drawn = item(
                    ui,
                    held,
                    entity,
                    picked == Some(held.id),
                    device,
                    queue,
                    acts,
                );
                if drawn.clicked() {
                    clicked = Some(held.id);
                }
            }
        });
    ui.painter()
        .rect_filled(pane, 9.0, ui.visuals().window_fill);
    if let Some(held) = picked.and_then(|id| queue.entry(id)) {
        let mut diff = ui.new_child(egui::UiBuilder::new().max_rect(pane.shrink(10.0)));
        let name = workspace
            .get(held.id)
            .map_or("", |entity| entity.name.as_str());
        diff_title(&mut diff, held, name);
        table(&mut diff, held);
    }
    if let Some(id) = clicked {
        queue.picked = Some(id);
    }
}

/// The diff's title: what is going where, and how many fields it changes.
fn diff_title(ui: &mut egui::Ui, held: &Queued, name: &str) {
    let visuals = ui.visuals().clone();
    let (rect, _) =
        ui.allocate_exact_size(egui::vec2(ui.available_width(), 28.0), egui::Sense::hover());
    let mut bar = ui.new_child(
        egui::UiBuilder::new()
            .max_rect(rect)
            .layout(egui::Layout::left_to_right(egui::Align::Center)),
    );
    bar.spacing_mut().item_spacing.x = 8.0;
    if let Diff::Fields(fields) = &held.diff {
        bar.with_layout(egui::Layout::right_to_left(egui::Align::Center), |bar| {
            let differ = plural(fields.len(), "field differs", "fields differ");
            crate::panel::signal_pill(bar, &differ, warn(&visuals));
            bar.with_layout(egui::Layout::left_to_right(egui::Align::Center), |bar| {
                title_words(bar, held, name);
            });
        });
    } else {
        title_words(&mut bar, held, name);
    }
    ui.add_space(4.0);
}

/// The picked item's name, and what is known of the slot it goes to, which is shortened
/// first.
fn title_words(ui: &mut egui::Ui, held: &Queued, name: &str) {
    ui.label(
        egui::RichText::new(crate::strings::display_name(name))
            .font(egui::FontId::new(12.5, crate::app::bold())),
    );
    ui.add(
        egui::Label::new(
            egui::RichText::new(format!("→ {}", held.replaces.said(held.class, held.at)))
                .size(12.0)
                .color(crate::app::caption(ui.visuals())),
        )
        .truncate(),
    );
}

/// The four column heads, and under them either the fields that differ or a single line
/// describing any other kind of difference.
pub fn table(ui: &mut egui::Ui, held: &Queued) {
    let width = ui.available_width();
    let tracks = crate::panel::tracks(width, &DIFF_TRACKS, GAP);
    diff_head(ui, width, &tracks);

    let Diff::Fields(fields) = &held.diff else {
        let (glyph, tint, said) = summarize(held, ui.visuals());
        return one_row(ui, width, &tracks, glyph, tint, &said);
    };
    egui::ScrollArea::vertical()
        .id_salt("queue_diff")
        .auto_shrink([false; 2])
        .show(ui, |ui| {
            ui.spacing_mut().item_spacing.y = 1.0;
            for field in fields {
                field_row(ui, width, &tracks, field);
            }
        });
}

/// The single line for a diff that is not a field list.
///
/// ⚠️ Only a slot read and found empty is free. While the read is out, this says only
/// that it is out.
fn summarize(held: &Queued, visuals: &egui::Visuals) -> (Glyph, egui::Color32, String) {
    let quiet = visuals.weak_text_color();
    match &held.diff {
        Diff::Pending => (
            Glyph::Gauge,
            quiet,
            format!("reading what is in {}…", place(held.class, held.at)),
        ),
        Diff::Empty => (Glyph::CircleCheck, good(visuals), "the slot is free".into()),
        Diff::Identical => (
            Glyph::Equal,
            quiet,
            "the instrument already holds these bytes".into(),
        ),
        Diff::Bytes { first_at } => (
            Glyph::ArrowRight,
            warn(visuals),
            format!("bytes differ from {first_at:#06x}"),
        ),
        // A field list is drawn as rows, not as this line.
        Diff::Fields(_) => (Glyph::ArrowRight, warn(visuals), String::new()),
    }
}

/// One waiting item: what it is, where it goes, and what it would replace.
///
/// ⚠️ Nothing inside is a widget, for the reason [`crate::browser::Cells`] gives: a
/// label allocates a hover rect that wins the hit test over the row, so a click would
/// land on whichever word is under the pointer.
#[allow(clippy::too_many_arguments)]
fn item(
    ui: &mut egui::Ui,
    held: &Queued,
    entity: &LocalEntity,
    selected: bool,
    device: &DeviceState,
    queue: &Queue,
    acts: &mut Vec<Act>,
) -> egui::Response {
    let (rect, response) = ui.allocate_exact_size(
        egui::vec2(ui.available_width(), ROW),
        egui::Sense::click_and_drag(),
    );
    let visuals = ui.visuals().clone();
    let painter = ui.painter().clone();
    let (ink, quiet) = row_ink(&painter, rect, selected, response.hovered(), &visuals);

    // Lay out the right end first, so the name is truncated to the room left.
    let box_ = |right: f32| {
        egui::Rect::from_center_size(
            egui::pos2(right - SMALL / 2.0, rect.center().y),
            egui::Vec2::splat(SMALL),
        )
    };
    let unqueue = ui.interact(
        box_(rect.right() - PAD),
        ui.id().with(("unqueue", held.id)),
        egui::Sense::click(),
    );
    // The × stays weak until the pointer is over it.
    let leaving = match unqueue.hovered() {
        true => visuals.text_color(),
        false => visuals.weak_text_color(),
    };
    painted(
        ui,
        Glyph::X,
        box_(rect.right() - PAD),
        cell_ink(selected, leaving, &visuals),
    );
    if unqueue.on_hover_text("remove from the queue").clicked() {
        acts.push(Act::Unqueue(held.id));
    }
    let (glyph, tint, why) = state(held, &visuals);
    painted(
        ui,
        glyph,
        box_(rect.right() - PAD - SMALL - GAP),
        cell_ink(selected, tint, &visuals),
    );
    let right = destination(
        ui,
        held,
        rect,
        rect.right() - PAD - 2.0 * (SMALL + GAP),
        quiet,
        device,
        queue,
        acts,
    );

    let left = rect.left() + PAD;
    painted(
        ui,
        Kind::of(entity).glyph(),
        egui::Rect::from_center_size(
            egui::pos2(left + GLYPH / 2.0, rect.center().y),
            egui::Vec2::splat(GLYPH),
        ),
        ink,
    );
    let name_at = left + GLYPH + GAP;
    cut(
        &painter,
        name_at,
        rect.center().y,
        right - name_at,
        crate::strings::display_name(&entity.name),
        egui::TextFormat::simple(egui::FontId::proportional(NAME), ink),
    );

    // Double-click opens the asset's document, as it does in the tree.
    if response.double_clicked() {
        acts.push(Act::Open(Item::Local(entity.id)));
    }
    // A drag carries the same payload as the asset's library row, so dropping it on a
    // slot sends it there.
    if response.dragged() {
        egui::DragAndDrop::set_payload(ui.ctx(), carried(entity, device));
    }
    // The row truncates the name, so the hover text shows it in full.
    response.on_hover_text(format!("{}\n{why}", entity.name))
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
    let galley = ui.painter().layout_no_wrap(
        place(held.class, held.at),
        egui::FontId::monospace(MONO),
        ink,
    );
    let size = chip_size(&galley);
    let box_ = egui::Rect::from_min_size(
        egui::pos2(right - size.x, row.center().y - CHIP / 2.0),
        size,
    );
    let id = ui.id().with(("destination", held.id));
    let chip = flat_chip(ui, box_, id, galley, false).on_hover_text("change where this goes");
    // ⚠️ A menu closes on any click, and switching banks is a click. The picker stays
    // open until a cell is picked or a click lands outside it.
    egui::Popup::menu(&chip)
        .close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside)
        .width(crate::keyboard::grid_width(PICKER_COLUMNS) + ui.spacing().menu_margin.sum().x)
        .show(|ui| {
            if let Some(at) = picker(ui, held, device, queue) {
                acts.push(Act::Retarget {
                    id: held.id,
                    class: held.class,
                    at,
                });
                ui.close();
            }
        });
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
    egui::Id::new(("picker", held.class.to_raw(), held.id))
}

/// The picker behind the chip: a row of bank chips, then the shown bank's slots drawn as
/// the keyboard map's cells. Returns the slot a click picked, if any.
///
/// The bank shown is stored per entry, so reopening the picker shows the bank it was
/// left on.
fn picker(
    ui: &mut egui::Ui,
    held: &Queued,
    device: &DeviceState,
    queue: &Queue,
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
    let mut picked = None;
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

/// The diff's four columns: the field, what is here, the sign between them, and what
/// the instrument holds. Any of them may shrink to nothing.
const DIFF_TRACKS: [Track; 4] = [
    Track::Share(1.4),
    Track::Share(1.0),
    Track::Px(20.0),
    Track::Share(1.0),
];

/// The row of column heads over the diff.
fn diff_head(ui: &mut egui::Ui, width: f32, tracks: &[Range<f32>]) {
    let (rect, _) = ui.allocate_exact_size(egui::vec2(width, HEAD), egui::Sense::hover());
    let visuals = ui.visuals().clone();
    ui.painter().rect_filled(rect, 0.0, visuals.faint_bg_color);
    let ink = crate::app::caption(&visuals);
    for (head, track) in ["field", "on this computer", "", "on the keyboard"]
        .iter()
        .zip(tracks)
    {
        cut(
            ui.painter(),
            rect.left() + track.start,
            rect.center().y,
            track.end - track.start,
            &head.to_uppercase(),
            egui::TextFormat::simple(egui::FontId::proportional(9.5), ink),
        );
    }
}

/// One field the two bodies do not agree on.
fn field_row(ui: &mut egui::Ui, width: f32, tracks: &[Range<f32>], field: &FieldDiff) {
    let (rect, response) =
        ui.allocate_exact_size(egui::vec2(width, DIFF_ROW), egui::Sense::hover());
    let visuals = ui.visuals().clone();
    let mono = egui::FontId::monospace(DIFF_MONO);
    let name = label(&field.path);
    let cells = [
        (
            &name,
            egui::FontId::proportional(DIFF_MONO),
            visuals.text_color(),
            &tracks[0],
        ),
        (
            &field.here,
            mono.clone(),
            visuals.widgets.active.fg_stroke.color,
            &tracks[1],
        ),
        (&field.there, mono, visuals.weak_text_color(), &tracks[3]),
    ];
    for (text, font, ink, track) in cells {
        cut(
            ui.painter(),
            rect.left() + track.start,
            rect.center().y,
            track.end - track.start,
            text,
            egui::TextFormat::simple(font, ink),
        );
    }
    sign(
        ui,
        cell(rect, &tracks[2]),
        Glyph::ArrowRight,
        warn(&visuals),
    );
    let _ = response.on_hover_text(format!("{name}: {} → {}", field.there, field.here));
}

/// The single row for a diff that is not a field list.
fn one_row(
    ui: &mut egui::Ui,
    width: f32,
    tracks: &[Range<f32>],
    glyph: Glyph,
    tint: egui::Color32,
    said: &str,
) {
    let (rect, _) = ui.allocate_exact_size(egui::vec2(width, DIFF_ROW), egui::Sense::hover());
    sign(ui, cell(rect, &tracks[2]), glyph, tint);
    cut(
        ui.painter(),
        rect.left() + tracks[0].start,
        rect.center().y,
        tracks[0].end - tracks[0].start,
        said,
        egui::TextFormat::simple(egui::FontId::proportional(DIFF_MONO), tint),
    );
}

/// The mark between the two values.
fn sign(ui: &egui::Ui, box_: egui::Rect, glyph: Glyph, tint: egui::Color32) {
    painted(
        ui,
        glyph,
        egui::Rect::from_center_size(
            egui::pos2(box_.left() + SMALL / 2.0, box_.center().y),
            egui::Vec2::splat(SMALL),
        ),
        tint,
    );
}

/// The state glyph of an item, its tint, and the sentence explaining it.
fn state(held: &Queued, visuals: &egui::Visuals) -> (Glyph, egui::Color32, String) {
    if let Some(why) = &held.failure {
        return (Glyph::CircleAlert, bad(visuals), why.clone());
    }
    let where_ = place(held.class, held.at);
    let said = || held.replaces.said(held.class, held.at);
    match (&held.diff, &held.replaces) {
        (Diff::Pending, _) => (
            Glyph::Gauge,
            visuals.weak_text_color(),
            format!("reading what is in {where_}"),
        ),
        (Diff::Identical, Occupancy::Held(occupant)) => (
            Glyph::CircleCheck,
            good(visuals),
            format!(
                "{where_} already holds these bytes, under the name “{}”",
                occupant.name
            ),
        ),
        (_, Occupancy::Held(_)) => (Glyph::Replace, warn(visuals), said()),
        (_, Occupancy::Vacant) => (Glyph::CircleCheck, good(visuals), said()),
        // Unread, and no read is out: the write goes ahead without knowing what it
        // replaces.
        (_, Occupancy::Unknown) => (Glyph::CircleDot, visuals.weak_text_color(), said()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::device::DeviceEvent;
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
    fn replacing(queue: &Queue) -> usize {
        queue
            .entries()
            .iter()
            .filter(|held| held.replaces.occupant().is_some())
            .count()
    }

    fn occupant(name: &str, crc: Option<u32>) -> ProgramInfo {
        ProgramInfo {
            location: at(0),
            body_len: 121,
            format: "ne5p".into(),
            version: 4,
            crc32: crc,
            name: name.to_string(),
        }
    }

    /// An attached instrument nothing has read yet, and the tabs `poll` wants.
    fn attached(workspace: &Workspace) -> (Device, Tabs) {
        let mut device = Device::new(workspace.ctx().clone());
        device.pretend_attached();
        (device, Tabs::default())
    }

    /// The first command queued for the instrument, which must be a read.
    fn asked(device: &Device) -> (ObjectClass, Location, Purpose) {
        match device.queued().front().expect("a read was queued") {
            DeviceCmd::Get { class, at, why, .. } => (*class, *at, *why),
            other => panic!("{}", other.label()),
        }
    }

    /// Change a document's bytes the way an edit does.
    fn edit(workspace: &mut Workspace, id: u64, log: &mut Log) {
        let bytes = workspace.get(id).expect("it is in memory").bytes.clone();
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

    /// The body checksum of one asset's saved bytes, which is what a slot holding them
    /// reports.
    fn crc(workspace: &Workspace, id: u64) -> u32 {
        workspace
            .get(id)
            .and_then(|entity| entity.saved.crc32)
            .expect("every CBIN container has one")
    }

    /// An asset has changed once it is saved over what its slot holds, and stops
    /// counting once it is queued. An edit nothing has saved has not changed.
    #[test]
    fn an_asset_has_changed_once_saved_over_its_slot_and_until_queued() {
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

        let ids = |workspace: &Workspace, device: &Device, queue: &Queue| {
            changed(workspace, &device.state, queue)
                .iter()
                .map(|(id, ..)| *id)
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
        device.relink(&mut workspace);

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
        assert_eq!(ids(&workspace, &device, &queue), vec![saved]);
    }

    /// Each changed asset is queued for the slot it stands on, and one that stands on
    /// no slot is not.
    #[test]
    fn queueing_what_changed_makes_one_entry_each() {
        let (mut workspace, mut log, bytes) = bench();
        let class = ObjectClass::Program;
        let (mut device, _) = attached(&workspace);
        let mut queue = Queue::default();

        let ids: Vec<u64> = (0..2)
            .map(|slot| {
                workspace.ingest(
                    format!("off-{slot}.ne5p"),
                    Origin::Device {
                        class,
                        at: at(slot),
                    },
                    bytes.clone(),
                    &mut log,
                )
            })
            .collect();
        let homeless = workspace.ingest("typed-here.ne5p".into(), Origin::Fresh, bytes, &mut log);
        let held = crc(&workspace, ids[0]);
        device.pretend_bodies(class, 7, &[Some(("off-0", held)), Some(("off-1", held))]);
        for id in ids.iter().copied().chain([homeless]) {
            edit(&mut workspace, id, &mut log);
            workspace.mark_saved(id);
        }
        device.relink(&mut workspace);

        queue_changed(&workspace, &mut device, &mut queue, &mut log);

        assert_eq!(queue.ids(), ids);
        assert_eq!(queue.entry(ids[0]).map(|held| held.at), Some(at(0)));
        assert_eq!(queue.entry(ids[1]).map(|held| held.at), Some(at(1)));
        assert!(!queue.holds(homeless), "it stands for no slot");
        assert!(changed(&workspace, &device.state, &queue).is_empty());
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
            class,
            at(0),
            Occupancy::Vacant,
        );
        assert!(matches!(landed, Put::Made));

        let landed = queue.put(
            workspace.get(second).unwrap(),
            class,
            at(0),
            Occupancy::Vacant,
        );
        assert!(
            matches!(landed, Put::Instead(displaced) if displaced == first),
            "one asset per slot"
        );
        assert_eq!(queue.ids(), vec![second]);

        queue.put(
            workspace.get(first).unwrap(),
            class,
            at(1),
            Occupancy::Vacant,
        );
        let landed = queue.put(
            workspace.get(first).unwrap(),
            class,
            at(2),
            Occupancy::Vacant,
        );
        assert!(
            matches!(landed, Put::Moved(was, before) if (was, before) == (class, at(1))),
            "one slot per asset"
        );
        assert_eq!(queue.ids(), vec![second, first]);

        // Queueing it for the slot it already waits for leaves its entry alone.
        let landed = queue.put(
            workspace.get(first).unwrap(),
            class,
            at(2),
            Occupancy::Vacant,
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
                class,
                at(slot as u32),
                Occupancy::Vacant,
            );
        }

        queue.forget(ids[0]);
        queue.stumbled(class, "Programs 7:2 is occupied");
        assert_eq!(queue.ids(), ids[1..], "the rest are still waiting");
        assert_eq!(
            queue.entry(ids[1]).and_then(|held| held.failure.as_deref()),
            Some("Programs 7:2 is occupied"),
        );
        assert!(queue.entry(ids[2]).unwrap().failure.is_none());
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

    /// Four entries, two for occupied slots. A slot whose occupant turns out to be the
    /// same bytes still counts as a replacement.
    #[test]
    fn what_is_waiting_counts_its_replacements_whatever_the_bytes_turn_out_to_be() {
        let (mut workspace, mut log, bytes) = bench();
        let mut device = Device::new(workspace.ctx().clone());
        let class = ObjectClass::Program;
        // Two slots hold something, two are vacant.
        device.pretend_scanned(class, 7, &["Africa Split", "Squabble B", "", ""]);
        let (_, edited) =
            crate::fields::apply(&bytes, &[("center_panel.gain".into(), "96".into())])
                .expect("the registry takes the set");

        let mut queue = Queue::default();
        let mut ids = Vec::new();
        for slot in 0..4 {
            let held = match slot {
                // Slot 1 turns out to hold the same bytes as its entry.
                1 => bytes.clone(),
                _ => edited.clone(),
            };
            let id = workspace.ingest(
                format!("sound {slot}"),
                Origin::Device {
                    class,
                    at: at(slot),
                },
                held,
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
            ids.push(id);
        }

        assert_eq!((queue.len(), replacing(&queue)), (4, 2));
        // A vacant slot needs no read; an occupied one is waiting on the bytes it holds.
        assert!(matches!(queue.entry(ids[0]).unwrap().diff, Diff::Pending));
        assert!(matches!(queue.entry(ids[1]).unwrap().diff, Diff::Pending));
        assert!(matches!(queue.entry(ids[2]).unwrap().diff, Diff::Empty));
        assert_eq!(device.queued().len(), 2, "one read per occupied slot");

        queue.arrived(class, at(0), "Africa Split", &bytes, &workspace);
        queue.arrived(class, at(1), "Squabble B", &bytes, &workspace);
        let Diff::Fields(fields) = &queue.entry(ids[0]).unwrap().diff else {
            panic!("a program against a program is a field list");
        };
        assert_eq!(
            fields.iter().map(|f| f.path.as_str()).collect::<Vec<_>>(),
            vec!["center_panel.gain"]
        );
        assert!(matches!(queue.entry(ids[1]).unwrap().diff, Diff::Identical));
        // Still counted as a replacement: the write goes ahead over identical bytes.
        assert_eq!((queue.len(), replacing(&queue)), (4, 2));
    }

    /// An edit made while an entry waits is diffed against the occupant already read,
    /// without asking the instrument about that slot again.
    #[test]
    fn an_edit_under_a_waiting_entry_is_diffed_again_without_a_second_read() {
        let (mut workspace, mut log, bytes) = bench();
        let (mut device, _tabs) = attached(&workspace);
        let mut queue = Queue::default();
        let class = ObjectClass::Program;
        device.pretend_bodies(class, 7, &[Some(("Africa Split", 7))]);

        let id = workspace.ingest(
            "Africa Split".into(),
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
        queue.arrived(class, at(0), "Africa Split", &bytes, &workspace);
        assert!(matches!(queue.entry(id).unwrap().diff, Diff::Identical));
        let reads = device.queued().len();

        edit(&mut workspace, id, &mut log);
        follow(&workspace, &mut device, &mut queue, &mut log);

        let Diff::Fields(fields) = &queue.entry(id).unwrap().diff else {
            panic!("a program against a program is a field list");
        };
        assert_eq!(
            fields.iter().map(|f| f.path.as_str()).collect::<Vec<_>>(),
            vec!["center_panel.gain"]
        );
        assert_eq!(device.queued().len(), reads, "the slot was not read again");

        // With no further edit, a second pass changes nothing and asks nothing.
        follow(&workspace, &mut device, &mut queue, &mut log);
        assert!(matches!(queue.entry(id).unwrap().diff, Diff::Fields(_)));
        assert_eq!(device.queued().len(), reads);
    }

    /// An entry the checksums settled without a read has no occupant bytes to diff a
    /// later edit against, so that edit asks for the read, once.
    #[test]
    fn an_edit_under_an_entry_settled_by_checksum_asks_for_the_read_once() {
        let (mut workspace, mut log, bytes) = bench();
        let (mut device, _tabs) = attached(&workspace);
        let mut queue = Queue::default();
        let class = ObjectClass::Program;

        let id = workspace.ingest(
            "Africa Split".into(),
            Origin::Device { class, at: at(0) },
            bytes,
            &mut log,
        );
        let held = workspace.get(id).unwrap().saved.crc32.unwrap();
        device.pretend_bodies(class, 7, &[Some(("Africa Split", held))]);
        enqueue(
            &workspace,
            &mut device,
            &mut queue,
            &mut log,
            id,
            class,
            at(0),
        );
        assert!(matches!(queue.entry(id).unwrap().diff, Diff::Identical));
        assert!(device.queued().is_empty(), "the checksums settled it");

        edit(&mut workspace, id, &mut log);
        follow(&workspace, &mut device, &mut queue, &mut log);
        assert!(matches!(queue.entry(id).unwrap().diff, Diff::Pending));
        assert_eq!(asked(&device), (class, at(0), Purpose::Compare));

        follow(&workspace, &mut device, &mut queue, &mut log);
        assert_eq!(device.queued().len(), 1, "asked for once, not once a frame");
    }

    /// ⚠️ A type-0 file stores no body checksum. Every Electro 5 factory program is one,
    /// as is everything Nord Sound Manager exports from one. An entry for such a file
    /// settles against a slot it matches only because the checksum is computed from the
    /// body, not read from the header.
    #[test]
    fn a_type_0_entry_settles_against_the_slot_reporting_its_checksum_without_a_read() {
        let (mut workspace, mut log, bytes) = bench();
        let (mut device, _tabs) = attached(&workspace);
        let mut queue = Queue::default();
        let class = ObjectClass::Program;

        let id = workspace.ingest(
            "Circling Bells.ne5p".into(),
            Origin::File("Circling Bells.ne5p".into()),
            crate::workspace::as_type_0(&bytes),
            &mut log,
        );
        let held = crc(&workspace, id);
        device.pretend_bodies(class, 7, &[Some(("Circling Bells", held))]);
        enqueue(
            &workspace,
            &mut device,
            &mut queue,
            &mut log,
            id,
            class,
            at(0),
        );

        assert!(matches!(queue.entry(id).unwrap().diff, Diff::Identical));
        assert!(device.queued().is_empty(), "the checksums settled it");
    }

    /// Queueing an asset for the slot it is already waiting for changes nothing, so the
    /// instrument is not asked about the slot again and the log does not repeat itself.
    /// Sending a checked set that includes an asset already waiting does this.
    #[test]
    fn queueing_an_asset_where_it_is_already_going_asks_and_says_nothing_further() {
        let (mut workspace, mut log, bytes) = bench();
        let (mut device, _tabs) = attached(&workspace);
        let class = ObjectClass::Program;
        let mut queue = Queue::default();
        device.pretend_bodies(class, 7, &[Some(("Africa Split", 7))]);
        let id = workspace.ingest("Jazzy Click B".into(), Origin::Fresh, bytes, &mut log);

        for _ in 0..3 {
            enqueue(
                &workspace,
                &mut device,
                &mut queue,
                &mut log,
                id,
                class,
                at(0),
            );
        }

        assert_eq!(queue.len(), 1);
        assert_eq!(asked(&device), (class, at(0), Purpose::Compare));
        assert_eq!(device.queued().len(), 1, "one read for one destination");
        assert_eq!(
            log.transcript()
                .matches("is waiting to be sent to Programs 7:1")
                .count(),
            1,
            "{}",
            log.transcript()
        );
    }

    /// Edits made while a compare read is out all need the same answer, so none of them
    /// asks for the read again.
    #[test]
    fn edits_made_while_a_compare_read_is_out_do_not_ask_for_it_again() {
        let (mut workspace, mut log, bytes) = bench();
        let (mut device, _tabs) = attached(&workspace);
        let class = ObjectClass::Program;
        let mut queue = Queue::default();
        device.pretend_bodies(class, 7, &[Some(("Africa Split", 7))]);
        let id = workspace.ingest("Jazzy Click B".into(), Origin::Fresh, bytes, &mut log);
        enqueue(
            &workspace,
            &mut device,
            &mut queue,
            &mut log,
            id,
            class,
            at(0),
        );
        assert_eq!(asked(&device), (class, at(0), Purpose::Compare));

        for gain in ["96", "97", "98"] {
            let held = workspace.get(id).unwrap().bytes.clone();
            let (_, edited) =
                crate::fields::apply(&held, &[("center_panel.gain".into(), gain.into())]).unwrap();
            workspace.replace_bytes(id, edited, &mut log);
            follow(&workspace, &mut device, &mut queue, &mut log);
        }

        assert!(matches!(queue.entry(id).unwrap().diff, Diff::Pending));
        assert_eq!(device.queued().len(), 1, "the answer was already coming");
    }

    /// ⚠️ The queue outlives a connection, but reads in flight do not. An entry still
    /// marked as waiting would wait for the rest of the session, and one keeping the last
    /// instrument's occupant would claim this instrument holds it.
    #[test]
    fn an_entry_waiting_on_a_read_when_the_instrument_went_away_is_asked_again() {
        let (mut workspace, mut log, bytes) = bench();
        let (mut device, mut tabs) = attached(&workspace);
        let class = ObjectClass::Program;
        let mut queue = Queue::default();
        device.pretend_bodies(class, 7, &[Some(("Africa Split", 7))]);
        let id = workspace.ingest("Jazzy Click B".into(), Origin::Fresh, bytes, &mut log);
        enqueue(
            &workspace,
            &mut device,
            &mut queue,
            &mut log,
            id,
            class,
            at(0),
        );
        assert_eq!(asked(&device), (class, at(0), Purpose::Compare));

        device.pretend(DeviceEvent::Disconnected { lost: true });
        device.poll(&mut log, &mut workspace, &mut tabs, &mut queue);
        assert!(
            device.queued().is_empty(),
            "the read went with the connection"
        );

        device.pretend_attached();
        device.pretend(DeviceEvent::Partitions(Vec::new()));
        device.poll(&mut log, &mut workspace, &mut tabs, &mut queue);

        let held = queue.entry(id).expect("it is still waiting");
        assert!(matches!(held.diff, Diff::Pending));
        assert!(
            held.replaces.occupant().is_none(),
            "the last instrument's occupant says nothing about this one"
        );
        assert_eq!(
            asked(&device),
            (class, at(0), Purpose::Compare),
            "the answer that never came is asked for again"
        );
    }

    /// A bank no walk has reached says nothing about its slots, so an entry for one waits
    /// on a read and does not call the slot free.
    #[test]
    fn a_slot_in_an_unscanned_bank_is_read_before_it_is_called_free() {
        let (mut workspace, mut log, bytes) = bench();
        let (mut device, _tabs) = attached(&workspace);
        let class = ObjectClass::Program;
        let mut queue = Queue::default();
        let id = workspace.ingest("Jazzy Click B".into(), Origin::Fresh, bytes, &mut log);

        enqueue(
            &workspace,
            &mut device,
            &mut queue,
            &mut log,
            id,
            class,
            at(6),
        );

        let held = queue.entry(id).unwrap();
        assert!(matches!(held.diff, Diff::Pending));
        assert!(held.replaces.occupant().is_none());
        assert_eq!(asked(&device), (class, at(6), Purpose::Compare));
    }

    /// For a slot no walk had reached, the read settles it: an empty read makes the slot
    /// free, and bytes make the entry a replacement of what the read named.
    #[test]
    fn the_read_of_an_unscanned_slot_settles_what_the_entry_replaces() {
        let (mut workspace, mut log, bytes) = bench();
        let (mut device, mut tabs) = attached(&workspace);
        let class = ObjectClass::Program;
        let (_, edited) =
            crate::fields::apply(&bytes, &[("center_panel.gain".into(), "96".into())])
                .expect("the registry takes the set");
        let mut queue = Queue::default();
        let mut ids = Vec::new();
        for slot in [3, 4] {
            let id = workspace.ingest(
                format!("sound {slot}"),
                Origin::Fresh,
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
            ids.push(id);
        }

        device.pretend(DeviceEvent::Vacant {
            class,
            at: at(3),
            why: Purpose::Compare,
        });
        device.pretend(DeviceEvent::Got {
            name: "Jazzy Click B".into(),
            origin: Origin::Device { class, at: at(4) },
            bytes,
            why: Purpose::Compare,
        });
        device.poll(&mut log, &mut workspace, &mut tabs, &mut queue);

        let empty = queue.entry(ids[0]).unwrap();
        assert!(matches!(empty.diff, Diff::Empty));
        assert!(empty.replaces.occupant().is_none());

        let taken = queue.entry(ids[1]).unwrap();
        let Diff::Fields(fields) = &taken.diff else {
            panic!("a program against a program is a field list");
        };
        assert_eq!(
            fields.iter().map(|f| f.path.as_str()).collect::<Vec<_>>(),
            vec!["center_panel.gain"]
        );
        assert_eq!(
            taken.replaces.occupant().map(|held| held.name.as_str()),
            Some("Jazzy Click B"),
            "the read named what it found"
        );
        assert_eq!((queue.len(), replacing(&queue)), (2, 1));
    }

    /// The scan cache is keyed by the panel's bank number, which is one more than the
    /// wire's and is the number [`DeviceEvent::BankScanned`] carries. An entry for a slot a
    /// walk has read carries the name the walk found.
    #[test]
    fn a_slot_a_walk_has_read_carries_the_name_it_found() {
        let (mut workspace, mut log, bytes) = bench();
        let (mut device, mut tabs) = attached(&workspace);
        let class = ObjectClass::Program;
        let mut queue = Queue::default();
        // Programs 1:7 on the panel is bank 0, slot 6 on the wire.
        let occupied = Location { bank: 0, slot: 6 };
        device.pretend(DeviceEvent::BankScanned {
            class,
            bank: 1,
            slots: (0..7)
                .map(|slot| {
                    (slot == occupied.slot).then(|| ProgramInfo {
                        location: Location { bank: 0, slot },
                        name: "Jazzy Click B".into(),
                        ..occupant("", None)
                    })
                })
                .collect(),
        });
        device.poll(&mut log, &mut workspace, &mut tabs, &mut queue);

        let id = workspace.ingest("Jazzy Click B".into(), Origin::Fresh, bytes, &mut log);
        enqueue(
            &workspace,
            &mut device,
            &mut queue,
            &mut log,
            id,
            class,
            occupied,
        );

        let held = queue.entry(id).unwrap();
        assert_eq!(
            held.replaces.occupant().map(|held| held.name.as_str()),
            Some("Jazzy Click B"),
        );
        assert!(matches!(held.diff, Diff::Pending));
        assert_eq!(asked(&device), (class, occupied, Purpose::Compare));
    }

    /// A slot a walk found empty is free and needs no compare read.
    #[test]
    fn a_scanned_empty_slot_is_free_and_asks_the_instrument_nothing() {
        let (mut workspace, mut log, bytes) = bench();
        let mut device = Device::new(workspace.ctx().clone());
        let class = ObjectClass::Program;
        device.pretend_scanned(class, 7, &["Africa Split", ""]);
        let mut queue = Queue::default();
        let id = workspace.ingest("Jazzy Click B".into(), Origin::Fresh, bytes, &mut log);

        enqueue(
            &workspace,
            &mut device,
            &mut queue,
            &mut log,
            id,
            class,
            at(1),
        );

        let held = queue.entry(id).unwrap();
        assert!(matches!(held.diff, Diff::Empty));
        assert!(held.replaces.occupant().is_none());
        assert!(device.queued().is_empty(), "nothing to ask about");
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
        assert!(matches!(queue.entry(first).unwrap().diff, Diff::Empty));

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
                            picker(ui, held, &device.state, &queue),
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

    /// Paints the review headlessly with each kind of diff, to catch a layout that panics
    /// or an id that collides.
    #[test]
    fn the_review_paints_every_kind_of_diff_at_any_window_width() {
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
        // One waiting on its read, one with a field list, one for a free slot.
        queue.arrived(class, at(1), "Squabble B", &bytes, &workspace);

        for width in [700.0_f32, 1280.0] {
            for picked in queue.ids() {
                queue.picked = Some(picked);
                let input = testing::screen(egui::vec2(width, 600.0), Vec::new());
                testing::run(&ctx, input, |ctx| {
                    review(ctx, &mut queue, &workspace, &device.state, &mut Vec::new());
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
