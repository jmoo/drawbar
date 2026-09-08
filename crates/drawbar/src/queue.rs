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
use crate::browser::{cell_ink, Kind};
use crate::device::{Device, DeviceCmd, Purpose};
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
    pub replaces: Option<Occupant>,
    /// How what is waiting differs from what the slot holds.
    pub diff: Diff,
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

/// What a scanned slot holds, as the walk that read it reported.
pub struct Occupant {
    pub name: String,
    /// ⚠️ `None` where the class reports no checksum, which is *not comparable* rather
    /// than *the same*.
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
    /// The entry the dock shows in detail.
    picked: Option<u64>,
}

/// Wait for an asset to be written to a slot, and say in the log what that displaced.
///
/// One entry per asset and one per destination, so this both moves what was waiting
/// somewhere else and drops what was waiting for this slot. What the slot holds is read
/// again only when the two bodies are not already known to agree.
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
    let occupant = device.state.slot(class, at).flatten();
    let (moved, instead_of) = queue.put(entity, class, at, occupant);

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
        occupant: Option<&ProgramInfo>,
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

        let replaces = occupant.map(Occupant::of);
        // The container's own CRC-32 is the number the instrument reports for a slot, so
        // two bodies that agree are known to before either is read again.
        let here = entity.container.as_ref().and_then(|held| held.body_crc32);
        let diff = match &replaces {
            None => Diff::Empty,
            Some(held) if held.crc.is_some() && held.crc == here => Diff::Identical,
            Some(_) => Diff::Pending,
        };
        self.list.push(Queued {
            id: entity.id,
            class,
            at,
            replaces,
            diff,
            failure: None,
        });
        self.picked = Some(entity.id);
        (moved, instead_of)
    }

    /// The occupant of a slot something is waiting for, read at last.
    pub fn arrived(
        &mut self,
        class: ObjectClass,
        at: Location,
        there: &[u8],
        workspace: &Workspace,
    ) {
        let Some(held) = self
            .list
            .iter_mut()
            .find(|held| (held.class, held.at) == (class, at))
        else {
            return;
        };
        let Some(entity) = workspace.get(held.id) else {
            return;
        };
        held.diff = compare(&entity.bytes, there);
    }

    /// It landed on the instrument, or it is not here to send any more.
    pub fn forget(&mut self, id: u64) {
        self.list.retain(|held| held.id != id);
        if self.picked == Some(id) {
            self.picked = self.list.first().map(|held| held.id);
        }
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
            .filter(|held| held.replaces.is_some())
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

/// The faces a row paints in.
const NAME: f32 = 12.0;
const MONO: f32 = 10.5;

/// The item list's width, and the geometry of the diff beside it.
const ITEMS: f32 = 250.0;
const HEAD: f32 = 20.0;
const DIFF_ROW: f32 = 22.0;
const DIFF_MONO: f32 = 11.0;

/// Everything waiting, and what each of it runs into.
pub fn page(ui: &mut egui::Ui, queue: &mut Queue, workspace: &Workspace) {
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
                        if item(ui, held, entity, picked == Some(held.id)).clicked() {
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
    table(ui, &held.diff);
}

/// The four column heads, and under them either the fields two bodies do not agree on or
/// the one line every other shape of difference comes to.
pub fn table(ui: &mut egui::Ui, diff: &Diff) {
    let width = ui.available_width() - PAD;
    let tracks = crate::panel::tracks(width, &DIFF_TRACKS, GAP);
    diff_head(ui, width, &tracks);

    let Diff::Fields(fields) = diff else {
        let (glyph, tint, said) = summarise(diff, ui.visuals());
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
fn summarise(diff: &Diff, visuals: &egui::Visuals) -> (Glyph, egui::Color32, String) {
    let quiet = visuals.weak_text_color();
    match diff {
        Diff::Pending => (Glyph::Gauge, quiet, "reading what is there…".to_string()),
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
fn item(ui: &mut egui::Ui, held: &Queued, entity: &LocalEntity, selected: bool) -> egui::Response {
    let (rect, response) =
        ui.allocate_exact_size(egui::vec2(ui.available_width(), ROW), egui::Sense::click());
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
    let write = |right: f32, text: String, font: egui::FontId, tint: egui::Color32| -> f32 {
        let mut job = egui::text::LayoutJob::simple_singleline(text, font, tint);
        job.wrap = egui::text::TextWrapping::truncate_at_width((right - rect.left()).max(0.0));
        let galley = painter.layout_job(job);
        let width = galley.size().x;
        painter.galley(
            egui::pos2(right - width, rect.center().y - galley.size().y / 2.0),
            galley,
            egui::Color32::PLACEHOLDER,
        );
        right - width - GAP
    };

    let (glyph, tint, why) = state(held, &visuals);
    painted(
        ui,
        glyph,
        egui::Rect::from_center_size(
            egui::pos2(rect.right() - PAD - SMALL / 2.0, rect.center().y),
            egui::Vec2::splat(SMALL),
        ),
        cell_ink(selected, tint, &visuals),
    );
    let right = write(
        rect.right() - PAD - SMALL - GAP,
        place(held.class, held.at),
        egui::FontId::monospace(MONO),
        quiet,
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
    response.on_hover_text(why)
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
    let ink = visuals.widgets.noninteractive.fg_stroke.color;
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
    match (&held.diff, &held.replaces) {
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
    egui::RichText::new(queue.summary())
        .monospace()
        .size(9.5)
        .color(warn(visuals))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::log::Log;
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

        let landed = queue.put(workspace.get(first).unwrap(), class, at(0), None);
        assert_eq!(landed, (None, None));

        let landed = queue.put(workspace.get(second).unwrap(), class, at(0), None);
        assert_eq!(landed.1, Some(first), "one asset per slot");
        assert_eq!(queue.ids(), vec![second]);

        queue.put(workspace.get(first).unwrap(), class, at(1), None);
        let landed = queue.put(workspace.get(first).unwrap(), class, at(2), None);
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
            queue.put(workspace.get(*id).unwrap(), class, at(slot as u32), None);
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
                0 => queue.put(entity, class, at(slot), Some(&held)),
                _ => queue.put(entity, class, at(slot), None),
            };
        }
        assert_eq!(queue.summary(), "3 writes · 1 replace");

        let alone = workspace.ingest("alone".into(), Origin::Fresh, bytes, &mut log);
        let mut queue = Queue::default();
        queue.put(workspace.get(alone).unwrap(), class, at(0), None);
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
            None,
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

        queue.arrived(class, at(0), &bytes, &workspace);
        queue.arrived(class, at(1), &bytes, &workspace);
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
        queue.arrived(class, at(1), &bytes, &workspace);

        for width in [430.0_f32, 900.0] {
            for picked in queue.ids() {
                queue.picked = Some(picked);
                let _ = ctx.run(egui::RawInput::default(), |ctx| {
                    egui::TopBottomPanel::bottom("dock")
                        .exact_height(crate::shell::DOCK_BODY)
                        .frame(egui::Frame::new())
                        .show(ctx, |ui| {
                            ui.set_width(width);
                            page(ui, &mut queue, &workspace);
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
