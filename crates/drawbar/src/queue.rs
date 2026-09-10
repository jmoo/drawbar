//! What is waiting to go to the instrument, and where each of it lands.
//!
//! One entry per destination and one per asset: queueing a second asset for a slot
//! displaces the first, and queueing an asset that is already waiting moves it. Being in
//! here is what being owed to the instrument means — [`Queue::holds`] is the flag
//! `LocalEntity` used to carry.

use std::io::Cursor;
use std::ops::Range;

use eframe::egui;
use nord_usb::wire::ProgramInfo;
use nord_usb::{Location, ObjectClass};

use crate::app::{bad, good, ui as ui_text, warn};
use crate::browser::{cell_ink, Act, Carried, Held, Item, Kind};
use crate::device::{fit, Device, DeviceCmd, DeviceState, Fit, Purpose};
use crate::fields::fields_of;
use crate::icon::{painted, Glyph};
use crate::log::Log;
use crate::panel::Track;
use crate::strings::{label, place};
use crate::workspace::{LocalEntity, Workspace};

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
    /// The occupant's own bytes, once a compare read has delivered them.
    ///
    /// Kept so that an edit made after this entry was queued is diffed against them
    /// again rather than by reading the slot a second time.
    there: Option<Vec<u8>>,
    /// The stamp of the asset's bytes [`Queued::diff`] was made from, so an edit under a
    /// waiting entry is noticed without comparing anything.
    stamp: u64,
    /// Why the last attempt to write it stopped. Cleared when it is queued again.
    pub failure: Option<String>,
}

/// How what is waiting differs from what the slot holds.
pub enum Diff {
    /// The occupant's bytes are on their way.
    Pending,
    /// Nothing about the two bodies differs.
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
/// ⚠️ `path` is the registry's own spelling — what `nord-format` reads and writes — and
/// is turned into words by [`crate::strings::label`] where it is shown. `here` and
/// `there` are the rendered values, which is what a reader compares.
pub struct FieldDiff {
    pub path: String,
    pub here: String,
    pub there: String,
}

/// What is in the slot an entry is waiting for.
///
/// ⚠️ The three answers are distinct: a bank nobody has read holds no less than a bank
/// read and found empty, and calling the first free is the whole of what this exists to
/// stop.
pub enum Occupancy {
    /// The bank has not been scanned, so a compare read is on its way to find out.
    Unknown,
    /// Read, and holding nothing.
    Vacant,
    /// Read, and holding this.
    Held(Occupant),
}

impl Occupancy {
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
    /// ⚠️ `None` where the class reports no checksum or the occupant arrived as bytes
    /// rather than as a walk's entry, which is *not comparable* rather than *the same*.
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
    /// The entry the dock shows in detail.
    picked: Option<u64>,
}

/// Wait for an asset to be written to a slot, and say in the log what that displaced.
///
/// One entry per asset and one per destination, so this both moves what was waiting
/// somewhere else and drops what was waiting for this slot. What the slot holds is read
/// again unless the scan cache already answers for it and the two bodies are known to
/// agree; a bank the scan has never reached is read rather than assumed vacant.
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
    // Refused before the entry exists: the queue is what a send walks, so an asset the
    // instrument would not take must never get into it.
    if let Fit::Refuses(why) = fit(&device.state, entity) {
        return log.trouble(format!("“{name}” cannot go to {where_}. {why}"));
    }
    let holds = match device.state.slot(class, at) {
        Some(Some(info)) => Occupancy::Held(Occupant::of(info)),
        Some(None) => Occupancy::Vacant,
        None => Occupancy::Unknown,
    };
    let (moved, instead_of) = queue.put(entity, class, at, holds);

    if matches!(queue.entry(id).map(|held| &held.diff), Some(Diff::Pending)) {
        device.send(
            DeviceCmd::Get {
                class,
                at,
                body: false,
                why: Purpose::Compare,
            },
            log,
        );
    }
    if let Some((was, before)) = moved {
        return log.say(format!(
            "“{name}” is waiting for {where_} rather than {}.",
            place(was, before)
        ));
    }
    if let Some(other) = instead_of.and_then(|id| workspace.get(id)) {
        return log.say(format!(
            "“{name}” is waiting for {where_}; “{}” is not any more.",
            other.name
        ));
    }
    log.say(format!("“{name}” is waiting to be sent to {where_}."));
}

/// The assets whose slot on the attached instrument no longer holds what they were
/// saved as, each with that slot.
///
/// An edit queues nothing; saving one that stands for a slot does. This is the gap
/// between the two — what a send would walk straight past — and it is the same
/// comparison [`crate::library::Where::Both`] shows in the table.
pub fn changed(
    workspace: &Workspace,
    device: &DeviceState,
    queue: &Queue,
) -> Vec<(u64, ObjectClass, Location)> {
    workspace
        .listed()
        .filter(|entity| !queue.holds(entity.id))
        .filter_map(|entity| {
            let (class, at) = entity.spot()?;
            let info = device.slot(class, at).flatten()?;
            (crate::library::agrees(entity, info, queue) == Some(false))
                .then_some((entity.id, class, at))
        })
        .collect()
}

/// What a send would walk past: assets the instrument no longer agrees with, and
/// documents holding edits nothing has saved.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub struct Behind {
    pub changed: usize,
    pub unsaved: usize,
}

impl Behind {
    pub fn of(workspace: &Workspace, device: &DeviceState, queue: &Queue) -> Behind {
        Behind {
            changed: changed(workspace, device, queue).len(),
            unsaved: workspace
                .documents()
                .filter(|entity| entity.is_unsaved())
                .count(),
        }
    }

    pub fn any(self) -> bool {
        self.changed > 0 || self.unsaved > 0
    }

    /// The line that says so, where there is one to say. A part that counts nothing is
    /// left out.
    pub fn said(self) -> Option<String> {
        let mut parts = Vec::new();
        if self.changed > 0 {
            parts.push(format!("{} changed", self.changed));
        }
        if self.unsaved > 0 {
            parts.push(format!("{} unsaved", self.unsaved));
        }
        (!parts.is_empty()).then(|| parts.join(" · "))
    }
}

/// Queue every asset the instrument no longer agrees with, each for its own slot.
pub fn queue_changed(workspace: &Workspace, device: &mut Device, queue: &mut Queue, log: &mut Log) {
    for (id, class, at) in changed(workspace, &device.state, queue) {
        enqueue(workspace, device, queue, log, id, class, at);
    }
}

/// What a waiting entry says about a slot before either body has been read: vacant is
/// nothing to replace, and two bodies whose checksums agree are known to agree, because
/// the container's own CRC-32 is the number the instrument reports for a slot.
fn verdict(entity: &LocalEntity, replaces: &Occupancy) -> Diff {
    let here = entity.container.as_ref().and_then(|held| held.body_crc32);
    match replaces {
        Occupancy::Vacant => Diff::Empty,
        Occupancy::Held(held) if held.crc.is_some() && held.crc == here => Diff::Identical,
        Occupancy::Held(_) | Occupancy::Unknown => Diff::Pending,
    }
}

/// Diff every waiting entry whose asset has moved under it since it was queued.
///
/// The occupant's bytes are kept from the compare read, so an edit made after the entry
/// was made is measured against them here rather than by asking the instrument for the
/// same slot twice. An entry whose read has not come back waits for it; one the
/// checksums settled without a read is settled the same way again, and asks for the read
/// only where they no longer settle it.
pub fn follow(workspace: &Workspace, device: &mut Device, queue: &mut Queue, log: &mut Log) {
    let mut owed = Vec::new();
    for held in &mut queue.list {
        let Some(entity) = workspace.get(held.id) else {
            continue;
        };
        if entity.stamp == held.stamp {
            continue;
        }
        held.stamp = entity.stamp;
        held.diff = match &held.there {
            Some(there) => compare(&entity.bytes, there),
            None => verdict(entity, &held.replaces),
        };
        if matches!(held.diff, Diff::Pending) && held.there.is_none() {
            owed.push((held.class, held.at));
        }
    }
    for (class, at) in owed {
        device.send(
            DeviceCmd::Get {
                class,
                at,
                body: false,
                why: Purpose::Compare,
            },
            log,
        );
    }
}

/// Send a waiting asset somewhere else instead.
///
/// The same bookkeeping as [`enqueue`] — the new slot is read again, and whatever was
/// waiting for it stops — over an asset the queue is already holding. An asset it is not
/// holding is not queued by asking where it goes.
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
    /// Put one asset in the queue for one slot, and say what that moved out of the way:
    /// where this asset was waiting before, and what was waiting for this slot.
    ///
    /// [`enqueue`] is what callers use; this is the bookkeeping under it.
    fn put(
        &mut self,
        entity: &LocalEntity,
        class: ObjectClass,
        at: Location,
        replaces: Occupancy,
    ) -> (Option<(ObjectClass, Location)>, Option<u64>) {
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
            there: None,
            stamp: entity.stamp,
            failure: None,
        });
        self.picked = Some(entity.id);
        (moved, instead_of)
    }

    /// The occupant of a slot something is waiting for, read at last.
    ///
    /// The read is also the answer for a slot no walk had reached, so an entry that was
    /// waiting to find out learns here that the slot is occupied.
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
        held.there = Some(there.to_vec());
        held.stamp = entity.stamp;
        held.diff = compare(&entity.bytes, there);
    }

    /// The read of a slot something is waiting for came back empty: nothing is there.
    pub fn vacant(&mut self, class: ObjectClass, at: Location) {
        let Some(held) = self.waiting_for(class, at) else {
            return;
        };
        held.replaces = Occupancy::Vacant;
        held.there = None;
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

    /// It landed on the instrument, or it is not here to send any more.
    pub fn forget(&mut self, id: u64) {
        self.list.retain(|held| held.id != id);
        if self.picked == Some(id) {
            self.picked = self.list.first().map(|held| held.id);
        }
    }

    /// Nothing is waiting any more. A queue is a plan rather than data, so this deletes
    /// nothing and asks nothing.
    pub fn clear(&mut self) {
        self.list.clear();
        self.picked = None;
    }

    /// A write into `class` stopped, so the entry it stopped on says why.
    ///
    /// ⚠️ A batch writes its entries in queue order and each one that lands leaves the
    /// queue, so the first of that class still waiting is the one it stopped on.
    pub fn stumbled(&mut self, class: ObjectClass, why: &str) {
        if let Some(held) = self.list.iter_mut().find(|held| held.class == class) {
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

    /// What the queue amounts to, in the width a dock header has for it.
    ///
    /// A part that counts nothing is left out, so an empty queue says nothing at all.
    pub fn summary(&self) -> String {
        let replacing = self
            .list
            .iter()
            .filter(|held| held.replaces.occupant().is_some())
            .count();
        let mut said = Vec::new();
        if !self.list.is_empty() {
            said.push(match self.list.len() {
                1 => "1 write".to_string(),
                n => format!("{n} writes"),
            });
        }
        if replacing > 0 {
            said.push(format!("{replacing} replace"));
        }
        said.join(" · ")
    }
}

/// How what is waiting differs from what the slot holds.
///
/// Both bodies decoding into registries is what makes a field list possible; anything
/// else is bytes, compared as the wire carries them rather than as they sit in a file —
/// two containers of one body differ in their headers alone.
fn compare(here: &[u8], there: &[u8]) -> Diff {
    let decode = |bytes: &[u8]| nord_format::from_stream(&mut Cursor::new(bytes)).ok();
    if let (Some(mine), Some(held)) = (decode(here), decode(there)) {
        if let (Some(mine), Some(held)) = (fields_of(&mine), fields_of(&held)) {
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
            return match differing.is_empty() {
                true => Diff::Identical,
                false => Diff::Fields(differing),
            };
        }
    }
    let body = |bytes: &[u8]| {
        nord_usb::envelope::unwrap(bytes)
            .ok()
            .map(|read| read.body.0)
    };
    match (body(here), body(there)) {
        (Some(mine), Some(held)) => bytewise(&mine, &held),
        // Not a container this app can strip, so the whole of what it holds is compared.
        _ => bytewise(here, there),
    }
}

fn bytewise(here: &[u8], there: &[u8]) -> Diff {
    match here.iter().zip(there).position(|(mine, held)| mine != held) {
        Some(first_at) => Diff::Bytes { first_at },
        None if here.len() == there.len() => Diff::Identical,
        // One runs out; the first difference is where the shorter one ended.
        None => Diff::Bytes {
            first_at: here.len().min(there.len()),
        },
    }
}

/// The height of one waiting item, and the room the list keeps at each end.
const ROW: f32 = 22.0;
const PAD: f32 = 8.0;

/// The gap between a row's parts.
const GAP: f32 = 6.0;

/// A kind glyph in a row, and the state glyph at the end of it.
const GLYPH: f32 = 13.0;
const SMALL: f32 = 11.0;

/// The destination chip's own height, and the room it keeps at each end.
const CHIP: f32 = 17.0;
const CHIP_PAD: f32 = 5.0;

/// The faces a row paints in.
const NAME: f32 = 12.0;
const MONO: f32 = 10.5;

/// The item list's width, and the geometry of the diff beside it.
const ITEMS: f32 = 250.0;
const HEAD: f32 = 20.0;
const DIFF_ROW: f32 = 22.0;
const DIFF_MONO: f32 = 11.0;

/// Everything waiting, and what each of it runs into.
pub fn page(
    ui: &mut egui::Ui,
    queue: &mut Queue,
    workspace: &Workspace,
    device: &DeviceState,
    acts: &mut Vec<Act>,
) {
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
    ui.spacing_mut().item_spacing.y = 0.0;
    let picked = queue.picked;
    let mut clicked = None;
    egui::SidePanel::left("queue_items")
        .resizable(false)
        .exact_width(ITEMS)
        .frame(egui::Frame::new())
        .show_inside(ui, |ui| {
            egui::ScrollArea::vertical()
                .id_salt("queue_items")
                .auto_shrink([false; 2])
                .show(ui, |ui| {
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
        });
    if let Some(held) = picked.and_then(|id| queue.entry(id)) {
        diff(ui, held);
    }
    if let Some(id) = clicked {
        queue.picked = Some(id);
    }
}

/// What the picked item would change in the slot it is waiting for.
fn diff(ui: &mut egui::Ui, held: &Queued) {
    let border = ui.visuals().widgets.noninteractive.bg_stroke.color;
    let edge = ui.max_rect();
    ui.painter().vline(
        edge.left(),
        edge.top()..=edge.bottom(),
        egui::Stroke::new(1.0_f32, border),
    );
    table(ui, held);
}

/// The four column heads, and under them either the fields two bodies do not agree on or
/// the one line every other shape of difference comes to.
pub fn table(ui: &mut egui::Ui, held: &Queued) {
    let ui = &mut inset(ui);
    let width = ui.available_width() - PAD;
    let tracks = crate::panel::tracks(width, &DIFF_TRACKS, GAP);
    diff_head(ui, width, &tracks);

    let Diff::Fields(fields) = &held.diff else {
        let (glyph, tint, said) = summarise(held, ui.visuals());
        return one_row(ui, width, &tracks, glyph, tint, &said);
    };
    egui::ScrollArea::vertical()
        .id_salt("queue_diff")
        .auto_shrink([false; 2])
        .show(ui, |ui| {
            for field in fields {
                field_row(ui, width, &tracks, field);
            }
        });
}

/// The one line a diff that is not a field list comes to.
///
/// ⚠️ Only a slot read and found empty is free. While the read is out, all this can say
/// is that it is out.
fn summarise(held: &Queued, visuals: &egui::Visuals) -> (Glyph, egui::Color32, String) {
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
        // A field list is rows rather than a sentence.
        Diff::Fields(_) => (Glyph::ArrowRight, warn(visuals), String::new()),
    }
}

/// One waiting item: what it is, where it goes, and what is in the way.
///
/// ⚠️ Nothing inside is a widget, for the reason [`crate::browser::Cells`] gives: a
/// label allocates a hover rect that wins the hit test over the row, and the click lands
/// on whichever word happens to be under it.
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

    // The right end is claimed first, so the name is cut to whatever is left of the row.
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
    // Flat: nothing under the × until the pointer is on it.
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
        Kind::of(entity.entity.as_ref()).glyph(),
        egui::Rect::from_center_size(
            egui::pos2(left + GLYPH / 2.0, rect.center().y),
            egui::Vec2::splat(GLYPH),
        ),
        ink,
    );
    let name_at = left + GLYPH + GAP;
    let mut job = egui::text::LayoutJob::simple_singleline(
        entity.name.clone(),
        egui::FontId::proportional(NAME),
        ink,
    );
    job.wrap = egui::text::TextWrapping::truncate_at_width((right - name_at).max(0.0));
    let galley = painter.layout_job(job);
    painter.galley(
        egui::pos2(name_at, rect.center().y - galley.size().y / 2.0),
        galley,
        egui::Color32::PLACEHOLDER,
    );

    // Opened the way the tree opens it: a waiting entry stands for an asset, and a
    // reader who wants to see what is going out wants the document.
    if response.double_clicked() {
        acts.push(Act::Open(Item::Local(entity.id)));
    }
    // Dragged like the row this asset has in the library, so a drop on a slot means
    // there what it means anywhere else: this asset goes to that slot.
    if response.dragged() {
        egui::DragAndDrop::set_payload(
            ui.ctx(),
            Carried {
                head: Held {
                    what: Item::Local(entity.id),
                    kind: Kind::of(entity.entity.as_ref()),
                    filed: None,
                    // Nothing the instrument refuses ever reaches the queue.
                    fits: true,
                },
                name: entity.name.clone(),
                rest: Vec::new(),
            },
        );
    }
    response.on_hover_text(why)
}

/// Where an entry is going, as a chip to click: the address, and the picker behind it.
///
/// Answers with the left edge it claimed, which is where the name before it must stop.
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
    let box_ = egui::Rect::from_min_size(
        egui::pos2(
            right - galley.size().x - 2.0 * CHIP_PAD,
            row.center().y - CHIP / 2.0,
        ),
        egui::vec2(galley.size().x + 2.0 * CHIP_PAD, CHIP),
    );
    // Flat: nothing under the address until the pointer is on it.
    let chip = ui.interact(
        box_,
        ui.id().with(("destination", held.id)),
        egui::Sense::click(),
    );
    if chip.hovered() {
        ui.painter()
            .rect_filled(box_, 2.0, ui.visuals().widgets.hovered.weak_bg_fill);
    }
    ui.painter().galley(
        egui::pos2(
            box_.left() + CHIP_PAD,
            box_.center().y - galley.size().y / 2.0,
        ),
        galley,
        egui::Color32::PLACEHOLDER,
    );
    let chip = chip.on_hover_text("change where this goes");
    // ⚠️ A menu shuts on any click, and switching banks is a click. The picker stays up
    // until a cell is taken or the pointer lands outside it.
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

/// How many cells across the picker lays a bank out. Wider than the map's own grid,
/// which has a whole tab to fill and can afford the rows.
const PICKER_COLUMNS: usize = 6;

/// The ids the picker's own controls sense under.
///
/// ⚠️ Salted off the entry rather than off `ui.id()`: the picker is drawn inside a
/// popup's `Ui`, which is not the one the row was drawn in, and two entries' pickers
/// must not share a bank or a cell.
fn salt(held: &Queued) -> egui::Id {
    egui::Id::new(("picker", held.class.to_raw(), held.id))
}

/// The picker behind the chip: one row of bank chips, then that bank's slots as the
/// cells the keyboard's map paints. The slot a click asked for, if it asked for one.
///
/// The bank on show is this popup's own state, so opening the picker again lands where
/// it was left and every entry keeps its own.
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
        // A cell shows a name cut to 42 px, so the hover is the whole of it.
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

/// One bank of the picker: its number, lit while it is the bank on show.
///
/// Painted rather than laid out as a button, so it senses under an id of its own and
/// wears the flat chip the row's destination wears.
fn bank_chip(ui: &mut egui::Ui, id: egui::Id, bank: u32, on: bool) -> egui::Response {
    let visuals = ui.visuals().clone();
    let ink = match on {
        true => visuals.selection.stroke.color,
        false => visuals.text_color(),
    };
    let galley = ui
        .painter()
        .layout_no_wrap(bank.to_string(), egui::FontId::monospace(MONO), ink);
    let (box_, _) = ui.allocate_exact_size(
        egui::vec2(galley.size().x + 2.0 * CHIP_PAD, CHIP),
        egui::Sense::hover(),
    );
    let response = ui.interact(box_, id, egui::Sense::click());
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

/// A grid inset from the left edge by the room its rows keep at the right, so the head
/// and every row under it start where a row of the tree does. Taken here rather than in
/// each cell, so the whole grid moves together.
fn inset(ui: &mut egui::Ui) -> egui::Ui {
    let room = ui.available_rect_before_wrap();
    ui.new_child(
        egui::UiBuilder::new()
            .max_rect(room.with_min_x(room.left() + PAD))
            .layout(*ui.layout()),
    )
}

/// The diff's four columns: the field, what is here, the sign between them, and what
/// the instrument holds. Every one of them may shrink to nothing.
const DIFF_TRACKS: [Track; 4] = [
    Track::Share(1.4),
    Track::Share(1.0),
    Track::Px(20.0),
    Track::Share(1.0),
];

/// 20 px of column heads over the diff.
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
            ui,
            box_of(rect, track),
            &head.to_uppercase(),
            egui::FontId::proportional(9.5),
            ink,
        );
    }
}

/// One field the two bodies do not agree on.
fn field_row(ui: &mut egui::Ui, width: f32, tracks: &[Range<f32>], field: &FieldDiff) {
    let (rect, response) =
        ui.allocate_exact_size(egui::vec2(width, DIFF_ROW), egui::Sense::hover());
    let visuals = ui.visuals().clone();
    let mono = egui::FontId::monospace(DIFF_MONO);
    let strong = visuals.widgets.active.fg_stroke.color;
    let quiet = visuals.weak_text_color();
    let name = label(&field.path);
    cut(
        ui,
        box_of(rect, &tracks[0]),
        &name,
        egui::FontId::proportional(DIFF_MONO),
        visuals.text_color(),
    );
    cut(
        ui,
        box_of(rect, &tracks[1]),
        &field.here,
        mono.clone(),
        strong,
    );
    sign(
        ui,
        box_of(rect, &tracks[2]),
        Glyph::ArrowRight,
        warn(&visuals),
    );
    cut(ui, box_of(rect, &tracks[3]), &field.there, mono, quiet);
    let _ = response.on_hover_text(format!("{name}: {} → {}", field.there, field.here));
}

/// The one row a diff that is not a field list comes to.
fn one_row(
    ui: &mut egui::Ui,
    width: f32,
    tracks: &[Range<f32>],
    glyph: Glyph,
    tint: egui::Color32,
    said: &str,
) {
    let (rect, _) = ui.allocate_exact_size(egui::vec2(width, DIFF_ROW), egui::Sense::hover());
    sign(ui, box_of(rect, &tracks[2]), glyph, tint);
    cut(
        ui,
        box_of(rect, &tracks[0]),
        said,
        egui::FontId::proportional(DIFF_MONO),
        tint,
    );
}

/// A cell of a row, from the track it sits in.
fn box_of(rect: egui::Rect, track: &Range<f32>) -> egui::Rect {
    egui::Rect::from_min_max(
        egui::pos2(rect.left() + track.start, rect.top()),
        egui::pos2(rect.left() + track.end, rect.bottom()),
    )
}

/// One cell, cut to its track with an ellipsis.
fn cut(ui: &egui::Ui, box_: egui::Rect, text: &str, font: egui::FontId, tint: egui::Color32) {
    let mut job = egui::text::LayoutJob::simple_singleline(text.to_string(), font, tint);
    job.wrap = egui::text::TextWrapping::truncate_at_width(box_.width());
    let galley = ui.painter().layout_job(job);
    ui.painter().galley(
        egui::pos2(box_.left(), box_.center().y - galley.size().y / 2.0),
        galley,
        egui::Color32::PLACEHOLDER,
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

/// The glyph that says what this write runs into, and the sentence behind it.
fn state(held: &Queued, visuals: &egui::Visuals) -> (Glyph, egui::Color32, String) {
    if let Some(why) = &held.failure {
        return (Glyph::CircleAlert, bad(visuals), why.clone());
    }
    let where_ = place(held.class, held.at);
    match (&held.diff, held.replaces.occupant()) {
        (Diff::Pending, _) => (
            Glyph::Gauge,
            visuals.weak_text_color(),
            format!("reading what is in {where_}"),
        ),
        (Diff::Identical, Some(occupant)) => (
            Glyph::CircleCheck,
            good(visuals),
            format!(
                "{where_} already holds these bytes, under the name “{}”",
                occupant.name
            ),
        ),
        (_, Some(occupant)) => (
            Glyph::Replace,
            warn(visuals),
            format!(
                "{where_} holds “{}”, {} bytes, which this replaces",
                occupant.name, occupant.body_len
            ),
        ),
        (_, None) => (
            Glyph::CircleCheck,
            good(visuals),
            format!("{where_} is free"),
        ),
    }
}

/// The header's own line: what the queue amounts to, where the dock has room for it.
pub fn heading(queue: &Queue, visuals: &egui::Visuals) -> egui::RichText {
    aside(&queue.summary(), visuals)
}

/// Anything else the header sets beside the title, in the heading's own face.
pub fn aside(said: &str, visuals: &egui::Visuals) -> egui::RichText {
    egui::RichText::new(said)
        .monospace()
        .size(9.5)
        .color(warn(visuals))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::device::DeviceEvent;
    use crate::log::Log;
    use crate::tabs::Tabs;
    use crate::workspace::{Fresh, Origin};

    /// A workspace, and one program's bytes to make assets out of.
    fn bench() -> (Workspace, Log, Vec<u8>) {
        let ctx = egui::Context::default();
        let mut workspace = Workspace::new(ctx);
        let mut log = Log::default();
        let id = workspace.create(Fresh::Program, &mut log).unwrap();
        let bytes = workspace.get(id).unwrap().bytes.clone();
        workspace.remove(id, &mut log);
        (workspace, log, bytes)
    }

    fn at(slot: u32) -> Location {
        Location { bank: 6, slot }
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

    /// The one command the enqueue asked the instrument for.
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

    /// The body checksum of one asset's saved bytes, which is what a slot holding them
    /// reports.
    fn crc(workspace: &Workspace, id: u64) -> u32 {
        workspace
            .get(id)
            .and_then(|entity| entity.saved.crc32)
            .expect("a type-1 container carries one")
    }

    /// An edit queues nothing and neither does saving one the instrument already agrees
    /// with. The two counts a send walks past are separate facts: a slot holding
    /// something other than what was saved, and an edit nothing has saved at all.
    #[test]
    fn the_counts_separate_what_the_instrument_lacks_from_what_nothing_saved() {
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

        let counts =
            |workspace: &Workspace, queue: &Queue| Behind::of(workspace, &device.state, queue);
        assert_eq!(counts(&workspace, &queue), Behind::default());

        // Edited and saved: the slot no longer holds what this is.
        for id in [saved, queued] {
            edit(&mut workspace, id, &mut log);
            workspace.mark_saved(id);
        }
        // Edited and not saved: the slot still holds what this was saved as.
        edit(&mut workspace, unsaved, &mut log);

        assert_eq!(
            counts(&workspace, &queue),
            Behind {
                changed: 2,
                unsaved: 1
            }
        );
        assert_eq!(
            counts(&workspace, &queue).said().as_deref(),
            Some("2 changed · 1 unsaved")
        );

        // What is already waiting is not what a send would walk past.
        enqueue(
            &workspace,
            &mut device,
            &mut queue,
            &mut log,
            queued,
            class,
            at(1),
        );
        assert_eq!(
            changed(&workspace, &device.state, &queue)
                .iter()
                .map(|(id, ..)| *id)
                .collect::<Vec<_>>(),
            vec![saved],
        );
    }

    /// The action closes the gap the line describes: one entry per changed asset, each
    /// for the slot it stands for.
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

        queue_changed(&workspace, &mut device, &mut queue, &mut log);

        assert_eq!(queue.ids(), ids);
        assert_eq!(queue.entry(ids[0]).map(|held| held.at), Some(at(0)));
        assert_eq!(queue.entry(ids[1]).map(|held| held.at), Some(at(1)));
        assert!(!queue.holds(homeless), "it stands for no slot");
        assert_eq!(
            Behind::of(&workspace, &device.state, &queue),
            Behind::default(),
            "the gap is closed"
        );
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
        assert_eq!(landed, (None, None));

        let landed = queue.put(
            workspace.get(second).unwrap(),
            class,
            at(0),
            Occupancy::Vacant,
        );
        assert_eq!(landed.1, Some(first), "one asset per slot");
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
        assert_eq!(landed.0, Some((class, at(1))), "one slot per asset");
        assert_eq!(queue.ids(), vec![second, first]);
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

    /// The summary counts what is waiting and what is in the way, and says nothing about
    /// a part it would count zero of.
    #[test]
    fn the_summary_leaves_out_what_it_would_count_zero_of() {
        let (mut workspace, mut log, bytes) = bench();
        let class = ObjectClass::Program;
        let mut queue = Queue::default();
        assert_eq!(queue.summary(), "");

        let held = occupant("Africa Split", Some(7));
        for slot in 0..3 {
            let id = workspace.ingest(
                format!("sound {slot}"),
                Origin::Fresh,
                bytes.clone(),
                &mut log,
            );
            let entity = workspace.get(id).unwrap();
            match slot {
                0 => queue.put(
                    entity,
                    class,
                    at(slot),
                    Occupancy::Held(Occupant::of(&held)),
                ),
                _ => queue.put(entity, class, at(slot), Occupancy::Vacant),
            };
        }
        assert_eq!(queue.summary(), "3 writes · 1 replace");

        let alone = workspace.ingest("alone".into(), Origin::Fresh, bytes, &mut log);
        let mut queue = Queue::default();
        queue.put(
            workspace.get(alone).unwrap(),
            class,
            at(0),
            Occupancy::Vacant,
        );
        assert_eq!(queue.summary(), "1 write");
    }

    /// An asset the queue holds is owed to the instrument; that is the whole of what
    /// `pending` meant, and it stops being owed when the write lands.
    #[test]
    fn what_is_owed_is_what_the_queue_holds() {
        let (mut workspace, mut log, bytes) = bench();
        let id = workspace.ingest(
            "Africa-Split.ne5p".into(),
            Origin::Device {
                class: ObjectClass::Program,
                at: at(3),
            },
            bytes,
            &mut log,
        );
        let mut queue = Queue::default();
        assert!(!queue.holds(id));

        queue.put(
            workspace.get(id).unwrap(),
            ObjectClass::Program,
            at(3),
            Occupancy::Vacant,
        );
        assert!(queue.holds(id));

        queue.forget(id);
        assert!(!queue.holds(id) && queue.is_empty());
    }

    /// The diff between two bodies with a registry is the fields they do not agree on,
    /// and nothing else — the pair here is one program and the same program with one
    /// field set through the registry.
    #[test]
    fn a_registry_diff_lists_exactly_the_fields_that_differ() {
        let (_workspace, _log, here) = bench();
        let (_, there) = crate::fields::apply(&here, &[("center_panel.gain".into(), "96".into())])
            .expect("the registry takes the set");
        assert_ne!(here, there);

        let Diff::Fields(fields) = compare(&here, &there) else {
            panic!("two programs are two registries");
        };
        let paths: Vec<&str> = fields.iter().map(|field| field.path.as_str()).collect();
        assert_eq!(paths, vec!["center_panel.gain"]);
        let field = &fields[0];
        assert_ne!(field.here, field.there);
        assert_eq!(field.there, "96");

        // The same bytes on both sides differ in nothing at all.
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

    /// Four things waiting, two of them onto occupied slots: the header counts what it
    /// holds, and a slot whose occupant turns out to be these very bytes stops being a
    /// difference at all.
    #[test]
    fn what_is_waiting_adds_up_to_the_summary_the_header_shows() {
        let (mut workspace, mut log, bytes) = bench();
        let ctx = workspace.ctx().clone();
        let mut device = Device::new(ctx);
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
                // The one that turns out to hold exactly what is waiting for it.
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

        assert_eq!(queue.summary(), "4 writes · 2 replace");
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
        // Still a replacement, and still counted as one: identical bytes are written
        // over identical bytes.
        assert_eq!(queue.summary(), "4 writes · 2 replace");
    }

    /// The diff belongs to the asset, not to the moment it was queued. An edit made
    /// while an entry waits is measured against the occupant already read, so the fields
    /// change under it and the instrument is not asked about that slot again.
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

        // Nothing has moved since, so a second pass changes nothing and asks nothing.
        follow(&workspace, &mut device, &mut queue, &mut log);
        assert!(matches!(queue.entry(id).unwrap().diff, Diff::Fields(_)));
        assert_eq!(device.queued().len(), reads);
    }

    /// An entry the checksums settled without a read has no occupant to measure a later
    /// edit against, so that edit is what makes the read worth asking for.
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

    /// A bank no walk has reached says nothing about its slots, so an entry onto one is
    /// waiting on a read rather than claiming the slot is free.
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

    /// The read is the answer for a slot no walk had reached: empty makes it a free
    /// slot, and bytes make it a replacement of what the read named.
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
        assert_eq!(queue.summary(), "2 writes · 1 replace");
    }

    /// The scan cache is keyed by the panel's bank number — one more than the wire's,
    /// and the number [`DeviceEvent::BankScanned`] carries — so an entry onto a slot a
    /// walk has read carries the name that walk found in it.
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

    /// A slot a walk read and found empty is free, and free needs no reading.
    #[test]
    fn a_scanned_empty_slot_is_free_and_asks_the_instrument_nothing() {
        let (mut workspace, mut log, bytes) = bench();
        let ctx = workspace.ctx().clone();
        let mut device = Device::new(ctx);
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

    /// Sending a waiting asset somewhere else moves its one entry rather than adding a
    /// second: the new slot is read again, what that slot holds is what the entry now
    /// replaces, and whatever was waiting for it stops.
    #[test]
    fn retargeting_moves_the_entry_and_displaces_what_was_waiting_there() {
        let (mut workspace, mut log, bytes) = bench();
        let ctx = workspace.ctx().clone();
        let mut device = Device::new(ctx);
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
            "what the new slot holds is what it now replaces"
        );

        // Asking where something goes does not put it in the queue.
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

    /// ⚠️ A menu shuts on any click, and switching banks is a click. Reaching the second
    /// bank's slots means the picker survives the chip that got there.
    #[test]
    fn a_click_on_a_bank_chip_leaves_the_picker_open_on_that_bank() {
        let ctx = egui::Context::default();
        ctx.all_styles_mut(crate::app::metrics);
        let mut workspace = Workspace::new(ctx.clone());
        let mut log = Log::default();
        let mut device = Device::new(ctx.clone());
        let class = ObjectClass::Program;
        device.pretend_scanned(class, 7, &["Africa Split", "", ""]);
        device.pretend_scanned(class, 8, &["", "", ""]);
        let bytes = {
            let id = workspace.create(Fresh::Program, &mut log).unwrap();
            let held = workspace.get(id).unwrap().bytes.clone();
            workspace.remove(id, &mut log);
            held
        };
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
            let input = egui::RawInput {
                events,
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(600.0, 600.0),
                )),
                ..Default::default()
            };
            let _ = ctx.run(input, |ctx| {
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
        // The popup lands where the chip is and settles over a frame or two, so
        // nothing is clicked until its cells stop moving.
        let settle = || {
            for _ in 0..3 {
                draw(Vec::new());
            }
        };
        let click_at = |on: egui::Pos2| {
            let press = |pressed| egui::Event::PointerButton {
                pos: on,
                button: egui::PointerButton::Primary,
                pressed,
                modifiers: egui::Modifiers::NONE,
            };
            draw(vec![egui::Event::PointerMoved(on)]);
            draw(vec![press(true), press(false)]);
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
            "the picker stays up across a bank"
        );
        assert!(
            rect_of(salt(held).with(("slot", 7_u32, 0_u32))).is_some(),
            "and it is showing bank 8"
        );
    }

    /// A click on one of the picker's cells is what re-targets an entry. The cell's own
    /// rect comes from the frame before the click, so nothing here depends on where the
    /// bank chips over it happened to land.
    #[test]
    fn a_click_on_the_pickers_cell_answers_with_that_slot() {
        let ctx = egui::Context::default();
        ctx.all_styles_mut(crate::app::metrics);
        let mut workspace = Workspace::new(ctx.clone());
        let mut log = Log::default();
        let mut device = Device::new(ctx.clone());
        let class = ObjectClass::Program;
        device.pretend_scanned(class, 7, &["Africa Split", "", ""]);
        let bytes = {
            let id = workspace.create(Fresh::Program, &mut log).unwrap();
            let held = workspace.get(id).unwrap().bytes.clone();
            workspace.remove(id, &mut log);
            held
        };
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
            let input = egui::RawInput {
                events,
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(crate::keyboard::grid_width(PICKER_COLUMNS), 300.0),
                )),
                ..Default::default()
            };
            let mut drawn = (None, None);
            let _ = ctx.run(input, |ctx| {
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
        let press = |pressed| egui::Event::PointerButton {
            pos: on_cell,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: egui::Modifiers::NONE,
        };

        assert_eq!(draw(vec![press(true), press(false)]).0, Some(wanted));
    }

    /// Paint the page headlessly, with each shape a diff can be in it. What this catches
    /// is a layout that panics or an id that collides, neither of which a test on the
    /// rules would see.
    #[test]
    fn the_dock_page_paints_every_shape_a_diff_comes_in() {
        let ctx = egui::Context::default();
        ctx.all_styles_mut(crate::app::metrics);
        let mut workspace = Workspace::new(ctx.clone());
        let mut log = Log::default();
        let mut device = Device::new(ctx.clone());
        let class = ObjectClass::Program;
        device.pretend_scanned(class, 7, &["Africa Split", "Squabble B", ""]);

        let bytes = {
            let id = workspace.create(Fresh::Program, &mut log).unwrap();
            let held = workspace.get(id).unwrap().bytes.clone();
            workspace.remove(id, &mut log);
            held
        };
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
        // One waiting on its read, one with a field list, one onto a free slot.
        queue.arrived(class, at(1), "Squabble B", &bytes, &workspace);

        for width in [430.0_f32, 900.0] {
            for picked in queue.ids() {
                queue.picked = Some(picked);
                let _ = ctx.run(egui::RawInput::default(), |ctx| {
                    egui::TopBottomPanel::bottom("dock")
                        .exact_height(crate::shell::DOCK_BODY)
                        .frame(egui::Frame::new())
                        .show(ctx, |ui| {
                            ui.set_width(width);
                            page(ui, &mut queue, &workspace, &device.state, &mut Vec::new());
                        });
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
        // Wide enough for the design's own geometry, every column says something.
        assert!(laid(620.0)
            .iter()
            .all(|track| track.end - track.start > 1.0));
        // And past the point where the fixed track alone fits, nothing is negative.
        assert!(laid(4.0).iter().all(|track| track.end >= track.start));
    }
}
