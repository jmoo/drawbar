//! What is waiting to go to the instrument, and where each of it lands.
//!
//! One entry per destination and one per asset: queueing a second asset for a slot
//! displaces the first, and queueing an asset that is already waiting moves it. Being in
//! here is what being owed to the instrument means — [`Queue::holds`] is the flag
//! `LocalEntity` used to carry.

use eframe::egui;
use nord_usb::wire::ProgramInfo;
use nord_usb::{Location, ObjectClass};

use crate::app::{bad, good, ui as ui_text, warn};
use crate::browser::{cell_ink, Kind};
use crate::icon::{painted, Glyph};
use crate::strings::place;
use crate::workspace::{LocalEntity, Workspace};

/// One asset waiting to be written, and the slot it is waiting for.
pub struct Queued {
    /// The asset on this computer.
    pub id: u64,
    pub class: ObjectClass,
    pub at: Location,
    /// What the slot held when this was queued.
    pub replaces: Option<Occupant>,
    /// Why the last attempt to write it stopped. Cleared when it is queued again.
    pub failure: Option<String>,
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

/// What an enqueue moved out of the way, for the lines the log gets.
#[derive(Default)]
pub struct Displaced {
    /// Where this asset was waiting to go before.
    pub from: Option<(ObjectClass, Location)>,
    /// The asset that was waiting for this destination and is no longer.
    pub instead_of: Option<u64>,
}

/// Everything owed to the instrument, in the order it was asked for.
#[derive(Default)]
pub struct Queue {
    list: Vec<Queued>,
    /// The entry the dock shows in detail.
    picked: Option<u64>,
}

impl Queue {
    /// Wait for `entity` to be written to a slot, displacing whatever else was waiting
    /// for that slot and moving it if it was waiting for another.
    pub fn enqueue(
        &mut self,
        entity: &LocalEntity,
        class: ObjectClass,
        at: Location,
        occupant: Option<&ProgramInfo>,
    ) -> Displaced {
        let displaced = Displaced {
            from: self
                .list
                .iter()
                .find(|held| held.id == entity.id)
                .map(|held| (held.class, held.at))
                .filter(|held| *held != (class, at)),
            instead_of: self
                .list
                .iter()
                .find(|held| (held.class, held.at) == (class, at) && held.id != entity.id)
                .map(|held| held.id),
        };
        self.list
            .retain(|held| held.id != entity.id && (held.class, held.at) != (class, at));
        self.list.push(Queued {
            id: entity.id,
            class,
            at,
            replaces: occupant.map(Occupant::of),
            failure: None,
        });
        self.picked = Some(entity.id);
        displaced
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
    if let Some(id) = clicked {
        queue.picked = Some(id);
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

/// The glyph that says what this write runs into, and the sentence behind it.
fn state(held: &Queued, visuals: &egui::Visuals) -> (Glyph, egui::Color32, String) {
    if let Some(why) = &held.failure {
        return (Glyph::CircleAlert, bad(visuals), why.clone());
    }
    let where_ = place(held.class, held.at);
    match &held.replaces {
        Some(occupant) => (
            Glyph::Replace,
            warn(visuals),
            format!("{where_} holds “{}”, which this replaces", occupant.name),
        ),
        None => (
            Glyph::CircleCheck,
            good(visuals),
            format!("{where_} is free"),
        ),
    }
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

        let landed = queue.enqueue(workspace.get(first).unwrap(), class, at(0), None);
        assert!(landed.instead_of.is_none() && landed.from.is_none());

        let landed = queue.enqueue(workspace.get(second).unwrap(), class, at(0), None);
        assert_eq!(landed.instead_of, Some(first), "one asset per slot");
        assert_eq!(queue.ids(), vec![second]);

        queue.enqueue(workspace.get(first).unwrap(), class, at(1), None);
        let landed = queue.enqueue(workspace.get(first).unwrap(), class, at(2), None);
        assert_eq!(landed.from, Some((class, at(1))), "one slot per asset");
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
            queue.enqueue(workspace.get(*id).unwrap(), class, at(slot as u32), None);
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
                0 => queue.enqueue(entity, class, at(slot), Some(&held)),
                _ => queue.enqueue(entity, class, at(slot), None),
            };
        }
        assert_eq!(queue.summary(), "3 writes · 1 replace");

        let alone = workspace.ingest("alone".into(), Origin::Fresh, bytes, &mut log);
        let mut queue = Queue::default();
        queue.enqueue(workspace.get(alone).unwrap(), class, at(0), None);
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

        queue.enqueue(
            workspace.get(id).unwrap(),
            ObjectClass::Program,
            at(3),
            None,
        );
        assert!(queue.holds(id));

        queue.forget(id);
        assert!(!queue.holds(id) && queue.is_empty());
    }
}
