//! The two questions drawbar asks about linked files: what changed on the keyboard since
//! it was last attached, and which of a file and its slot is current. Bringing a slot in
//! reads it and saves what it holds as the file linked to it.

use eframe::egui;
use nord_usb::{Location, ObjectClass};

use crate::device::{occupied, standing_of, there, Device, DeviceCmd, DeviceState, Purpose};
use crate::link::{self, Held, Keyboard, Spot, Standing, There};
use crate::log::{Log, Undo};
use crate::queue::{compare, Diff, Queue};
use crate::sheet;
use crate::strings::{counted, display_name, label, shown};
use crate::workspace::{print_of, LocalEntity, Workspace};

/// What happened on the keyboard to one linked file's sound while drawbar was away.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Change {
    /// The slot holds a version the file never had, and the file has not changed.
    Changed(Location),
    /// The sound is in another slot of its class, which no file is linked to. `agrees`
    /// where that slot holds the file's saved version.
    Moved {
        from: Location,
        to: Location,
        agrees: bool,
    },
    /// The slot is empty and the sound is nowhere else. A piano or sample has no address.
    Removed(Option<Location>),
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Found {
    pub id: u64,
    pub class: ObjectClass,
    pub change: Change,
}

/// What changed on the attached keyboard to the files linked there, for the question
/// asked once its walk has finished. A file with unsaved edits, and one changed here as
/// well as there, is left to its mark. A sound two files would move to has moved for
/// neither.
pub fn since(state: &DeviceState, workspace: &Workspace) -> Vec<Found> {
    let Some(keyboard) = state.keyboard() else {
        return Vec::new();
    };
    let judged: Vec<(Found, Standing)> = workspace
        .listed()
        .filter(|entity| !entity.is_unsaved())
        .filter_map(|entity| {
            let held = entity.held_on(&keyboard)?;
            let (change, standing) = change(state, workspace, &keyboard, entity, held)?;
            let (id, class) = (entity.id, held.class());
            Some((Found { id, class, change }, standing))
        })
        .collect();
    let target = |found: &Found| match found.change {
        Change::Moved { to, .. } => Some((found.class, to)),
        Change::Changed(_) | Change::Removed(_) => None,
    };
    let contested = |found: &Found| {
        let claims = judged
            .iter()
            .filter(|(other, _)| target(other) == target(found));
        target(found).is_some() && claims.count() > 1
    };
    judged
        .iter()
        .filter_map(|(found, standing)| match (contested(found), found.change) {
            (false, _) => Some(*found),
            (true, Change::Moved { from, .. }) if *standing == Standing::Theirs => Some(Found {
                change: Change::Changed(from),
                ..*found
            }),
            (true, _) => None,
        })
        .collect()
}

/// What changed to one file's sound, and how the file stands against its slot.
fn change(
    state: &DeviceState,
    workspace: &Workspace,
    keyboard: &Keyboard,
    entity: &LocalEntity,
    held: &Held,
) -> Option<(Change, Standing)> {
    let class = held.class();
    let here = entity.saved_print()?;
    let (_, there) = there(state, held);
    let standing = link::standing(held, Some(here), there);
    if matches!(standing, Standing::Both | Standing::Unread) {
        return None;
    }
    let Some(from) = held.location() else {
        return (there == There::Vacant).then_some((Change::Removed(None), standing));
    };
    let lost = match there {
        There::Vacant => true,
        There::Holds { crc: Some(crc), .. } => !held.knew(crc) && crc != here.crc,
        There::Holds { crc: None, .. } | There::Unread => false,
    };
    let ours = |crc: u32| crc == held.base.crc || crc == here.crc;
    let mut elsewhere = occupied(state, class)
        .filter(|(at, info)| *at != from && info.crc32.is_some_and(ours))
        .map(|(at, info)| (at, info.crc32 == Some(here.crc)));
    let moved = match (elsewhere.next(), elsewhere.next()) {
        (Some((to, agrees)), None) => Some(Change::Moved { from, to, agrees }),
        (None, _) if there == There::Vacant => {
            return Some((Change::Removed(Some(from)), standing))
        }
        _ => None,
    };
    let free = |change: &Change| match change {
        Change::Moved { to, .. } => workspace
            .linked_at(keyboard, class, &Spot::at(*to))
            .is_none(),
        Change::Changed(_) | Change::Removed(_) => false,
    };
    match moved.filter(|_| lost).filter(free) {
        Some(moved) => Some((moved, standing)),
        None => (standing == Standing::Theirs).then_some((Change::Changed(from), standing)),
    }
}

/// Whether the walk of every class holding a file linked on the attached keyboard has
/// finished.
fn walked(state: &DeviceState, workspace: &Workspace) -> bool {
    let (Some(keyboard), classes) = (state.keyboard(), state.classes()) else {
        return false;
    };
    let running = |class| state.scan.progress(class).is_some_and(|read| read.running);
    !classes.is_empty()
        && workspace
            .listed()
            .filter_map(|entity| Some(entity.held_on(&keyboard)?.class()))
            .filter(|class| classes.contains(class))
            .all(|class| state.read_whole(class) && !running(class))
}

/// The slots holding a version their linked file never had, where the file has not
/// changed and can be brought in.
fn theirs(state: &DeviceState, workspace: &Workspace) -> Vec<(ObjectClass, Location)> {
    workspace
        .listed()
        .filter(|entity| !entity.is_unsaved())
        .filter_map(|entity| standing_of(state, entity))
        .filter(|linked| linked.standing == Standing::Theirs && !linked.class.is_library())
        .filter_map(|linked| Some((linked.class, linked.at?)))
        .collect()
}

/// What drawbar asks, and has been told, while one keyboard is attached. A new attach
/// starts it over.
#[derive(Default)]
pub struct Asks {
    /// Whether this attach's question was asked, or found nothing to ask.
    settled: bool,
    /// Whether the user said Bring them in: a slot changed on the keyboard from then on is
    /// brought in without asking.
    bringing: bool,
    /// The question asked on attach, while it shows.
    pub since: Option<Vec<Found>>,
    /// The question asked about one file, while it shows.
    pub choosing: Option<Choosing>,
    /// Slots read to be brought in, until their bytes land. A read that fails stays, so
    /// it is not asked for again.
    pulling: Vec<(ObjectClass, Location)>,
    /// The log line counting the slots brought in now.
    brought: Option<u64>,
}

impl Asks {
    /// The bytes read for the question about one file.
    pub fn read(
        &mut self,
        class: ObjectClass,
        at: Location,
        bytes: Vec<u8>,
        workspace: &Workspace,
    ) {
        let Some(choosing) = self
            .choosing
            .as_mut()
            .filter(|it| (it.class, it.at) == (class, at))
        else {
            return;
        };
        if let Some(entity) = workspace.get(choosing.id) {
            choosing.diff = compare(&entity.bytes, &bytes);
            choosing.there = Some(bytes);
        }
    }

    /// A read for a question found its slot empty, so there is nothing to bring in.
    pub fn vacant(&mut self, class: ObjectClass, at: Location) {
        self.pulling.retain(|slot| *slot != (class, at));
        if self
            .choosing
            .as_ref()
            .is_some_and(|it| (it.class, it.at) == (class, at))
        {
            self.choosing = None;
        }
    }
}

/// Settle the question for this attach once the walk allows, and bring in what changed
/// on the keyboard after the user said Bring them in. Call when the instrument or the
/// list may have changed.
///
/// ⚠️ Never while a write is running or waiting: a send changes the slots it is asked
/// about.
pub fn follow(device: &mut Device, workspace: &Workspace, log: &mut Log) {
    if !device.asks.settled && !device.sending() && walked(&device.state, workspace) {
        let found = since(&device.state, workspace);
        device.asks.settled = true;
        device.asks.since = (!found.is_empty()).then_some(found);
    }
    if device.asks.bringing {
        for (class, at) in theirs(&device.state, workspace) {
            pull(device, log, class, at);
        }
    }
}

/// Read a slot to bring it in, unless it is being read already.
fn pull(device: &mut Device, log: &mut Log, class: ObjectClass, at: Location) {
    if device.asks.pulling.contains(&(class, at)) {
        return;
    }
    device.asks.pulling.push((class, at));
    let why = Purpose::Pull;
    device.send(DeviceCmd::Get { class, at, why }, log);
}

/// Bring them in: every slot changed on the keyboard is brought in, now and while this
/// keyboard stays attached; a moved sound's link follows it; a removed sound's file is
/// unlinked and kept. What changed since the question was asked is left as it is.
pub fn bring_in(device: &mut Device, workspace: &mut Workspace, log: &mut Log) {
    let asked = device.asks.since.take().unwrap_or_default();
    let Some(keyboard) = device.state.keyboard() else {
        return;
    };
    device.asks.bringing = true;
    device.asks.brought = None;
    let now = since(&device.state, workspace);
    for found in now.iter().filter(|found| asked.contains(found)) {
        let Some(held) = workspace.get(found.id).and_then(|it| it.held_on(&keyboard)) else {
            continue;
        };
        match found.change {
            Change::Changed(_) => {}
            Change::Moved { to, agrees, .. } => {
                let mut moved = held.moved(Spot::at(to));
                if let (true, Some(print)) = (
                    agrees,
                    workspace.get(found.id).and_then(LocalEntity::saved_print),
                ) {
                    moved = moved.agreed(print);
                }
                let before = held.clone();
                workspace.link(found.id, &keyboard, moved);
                let name = workspace
                    .get(found.id)
                    .map(|it| it.name.clone())
                    .unwrap_or_default();
                log.say(format!(
                    "“{}” is now at {}.",
                    display_name(&name),
                    shown(to)
                ));
                log.offer(Undo::Relink {
                    id: found.id,
                    keyboard: keyboard.clone(),
                    held: before,
                });
            }
            Change::Removed(_) => workspace.unlink(found.id, &keyboard, false),
        }
    }
    follow(device, workspace, log);
}

/// Save `bytes`, read from the slot `at` of `class`, as the file linked to that slot on
/// the attached keyboard, and agree the link on them. Returns the file and the bytes it
/// was saved as before.
///
/// ⚠️ A file with unsaved edits is never brought in over: they would be lost.
fn bring(
    state: &DeviceState,
    workspace: &mut Workspace,
    log: &mut Log,
    class: ObjectClass,
    at: Location,
    bytes: Vec<u8>,
) -> Option<(u64, Vec<u8>)> {
    let keyboard = state.keyboard()?;
    let id = workspace.linked_at(&keyboard, class, &Spot::at(at))?;
    let entity = workspace.get(id)?;
    if entity.is_unsaved() || entity.unread() || entity.rests().is_some() {
        return None;
    }
    let held = entity.held_on(&keyboard)?.agreed(print_of(&bytes)?);
    let before = entity.bytes.to_vec();
    workspace.replace_bytes(id, bytes, log);
    workspace.mark_saved(id);
    workspace.link(id, &keyboard, held);
    Some((id, before))
}

/// A slot read to be brought in landed.
pub fn landed(
    device: &mut Device,
    workspace: &mut Workspace,
    log: &mut Log,
    class: ObjectClass,
    at: Location,
    bytes: Vec<u8>,
) {
    if let Some((id, before)) = bring(&device.state, workspace, log, class, at, bytes) {
        device.asks.pulling.retain(|slot| *slot != (class, at));
        brought(&mut device.asks, workspace, log, id, before);
    }
}

/// Count a file brought in on the log line of the ones brought in with it, or start one.
fn brought(asks: &mut Asks, workspace: &Workspace, log: &mut Log, id: u64, before: Vec<u8>) {
    let counted_on = asks.brought.and_then(|serial| log.entry_mut(serial));
    if let Some(entry) = counted_on {
        if let Some(Undo::Restore(kept)) = &mut entry.undo {
            kept.push((id, before));
            entry.text = format!(
                "Brought in {} from the keyboard.",
                counted(kept.len(), "sound", "sounds")
            );
            if entry
                .undo
                .as_ref()
                .is_some_and(|undo| undo.weight() > crate::log::KEPT)
            {
                entry.undo = None;
            }
            return;
        }
    }
    let name = workspace.get(id).map_or("", |it| it.name.as_str());
    log.say(format!(
        "Brought in “{}” from the keyboard.",
        display_name(name)
    ));
    asks.brought = log.offer(Undo::Restore(vec![(id, before)]));
}

/// The question about one file whose slot holds a version it never had.
pub struct Choosing {
    pub id: u64,
    pub class: ObjectClass,
    pub at: Location,
    /// How the file differs from the slot, once the slot is read. A piano or sample is
    /// never read whole, so its difference stays [`Diff::Checksum`].
    pub diff: Diff,
    there: Option<Vec<u8>>,
}

/// Ask which of a file and its slot is current, and read the slot for the answer.
pub fn choose(device: &mut Device, workspace: &Workspace, log: &mut Log, id: u64) {
    let Some(linked) = workspace
        .get(id)
        .and_then(|entity| standing_of(&device.state, entity))
        .filter(|linked| linked.standing.needs_attention())
    else {
        return;
    };
    let (Some(at), class) = (linked.at, linked.class) else {
        return;
    };
    let diff = match class.is_library() {
        true => Diff::Checksum,
        false => {
            let why = Purpose::Choose;
            device.send(DeviceCmd::Get { class, at, why }, log);
            Diff::Pending
        }
    };
    device.asks.choosing = Some(Choosing {
        id,
        class,
        at,
        diff,
        there: None,
    });
}

/// Use the keyboard's: bring the slot's bytes in.
pub fn use_keyboards(device: &mut Device, workspace: &mut Workspace, log: &mut Log) {
    let Some(Choosing {
        class,
        at,
        there: Some(bytes),
        ..
    }) = device.asks.choosing.take()
    else {
        return;
    };
    device.asks.brought = None;
    if let Some((id, before)) = bring(&device.state, workspace, log, class, at, bytes) {
        brought(&mut device.asks, workspace, log, id, before);
    }
}

/// Use mine: the slot's version counts as an earlier one, so the file is ahead of it, and
/// the file is queued for the slot.
pub fn use_mine(device: &mut Device, workspace: &mut Workspace, queue: &mut Queue, log: &mut Log) {
    let Some(Choosing { id, class, at, .. }) = device.asks.choosing.take() else {
        return;
    };
    let Some(keyboard) = device.state.keyboard() else {
        return;
    };
    let Some(held) = workspace.get(id).and_then(|it| it.held_on(&keyboard)) else {
        return;
    };
    let (_, there) = there(&device.state, held);
    workspace.link(id, &keyboard, held.superseding(there));
    crate::queue::enqueue(workspace, device, queue, log, id, class, at);
}

/// How the user answered a question.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Answer {
    Yes,
    Other,
    No,
}

const WIDTH: f32 = 440.0;

/// The question asked once after an attach, while it shows: "Bring them in" is
/// [`Answer::Yes`], "Not now" [`Answer::No`].
pub fn since_sheet(ctx: &egui::Context, found: &[Found], workspace: &Workspace) -> Option<Answer> {
    let name = |id| {
        workspace
            .get(id)
            .map_or("", |it: &LocalEntity| display_name(&it.name))
    };
    let changed: Vec<(&str, String)> = found
        .iter()
        .filter_map(|found| match found.change {
            Change::Changed(at) => Some((name(found.id), shown(at))),
            _ => None,
        })
        .collect();
    let moved: Vec<(&str, String)> = found
        .iter()
        .filter_map(|found| match found.change {
            Change::Moved { from, to, .. } => {
                Some((name(found.id), format!("{} → {}", shown(from), shown(to))))
            }
            _ => None,
        })
        .collect();
    let removed: Vec<(&str, String)> = found
        .iter()
        .filter_map(|found| match found.change {
            Change::Removed(at) => Some((name(found.id), at.map(shown).unwrap_or_default())),
            _ => None,
        })
        .collect();
    let groups = [
        (format!("{} changed", changed.len()), changed),
        (format!("{} moved", moved.len()), moved),
        (
            format!("{} removed, kept on this computer", removed.len()),
            removed,
        ),
    ];
    let mut answer = None;
    egui::Modal::new(egui::Id::new("since"))
        .frame(sheet::frame(&ctx.style().visuals))
        .show(ctx, |ui| {
            ui.set_width(sheet::width(ui.ctx(), WIDTH));
            ui.add_space(sheet::GAP * 4.0);
            sheet::section(ui, |ui| {
                ui.label(egui::RichText::new("Since last time, on the keyboard").strong());
                egui::ScrollArea::vertical()
                    .max_height(sheet::middle(ui.ctx(), 200.0, 120.0))
                    .show(ui, |ui| {
                        for (head, items) in groups.iter().filter(|(_, items)| !items.is_empty()) {
                            ui.add_space(sheet::GAP * 3.0);
                            ui.label(egui::RichText::new(head).weak());
                            for (name, place) in items {
                                listed(ui, name, place);
                            }
                        }
                    });
            });
            sheet::foot(
                ui,
                |_| {},
                |ui| {
                    if sheet::primary(ui, None, "Bring them in").clicked() {
                        answer = Some(Answer::Yes);
                    }
                    if sheet::secondary(ui, None, "Not now").clicked() {
                        answer = Some(Answer::No);
                    }
                },
            );
            if ui.input(|input| input.key_pressed(egui::Key::Escape)) {
                answer = Some(Answer::No);
            }
        });
    answer
}

/// One file in a question: its name, and its place at the right.
fn listed(ui: &mut egui::Ui, name: &str, place: &str) {
    ui.horizontal(|ui| {
        ui.label(name);
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            ui.label(egui::RichText::new(place).monospace().weak());
        });
    });
}

/// The question about one file, while it shows: "Use mine" is [`Answer::Yes`], "Use the
/// keyboard's" [`Answer::Other`], and Escape [`Answer::No`].
pub fn choice_sheet(
    ctx: &egui::Context,
    choosing: &Choosing,
    workspace: &Workspace,
) -> Option<Answer> {
    let entity = workspace.get(choosing.id)?;
    let theirs = choosing.there.is_some() && !entity.is_unsaved();
    let mut answer = None;
    egui::Modal::new(egui::Id::new("choice"))
        .frame(sheet::frame(&ctx.style().visuals))
        .show(ctx, |ui| {
            ui.set_width(sheet::width(ui.ctx(), WIDTH));
            ui.add_space(sheet::GAP * 4.0);
            sheet::section(ui, |ui| {
                let title = format!("Which “{}” is current?", display_name(&entity.name));
                ui.add(egui::Label::new(egui::RichText::new(title).strong()).wrap());
                ui.add_space(sheet::GAP * 3.0);
                fields(ui, choosing);
            });
            sheet::foot(
                ui,
                |_| {},
                |ui| {
                    if sheet::primary(ui, None, "Use mine").clicked() {
                        answer = Some(Answer::Yes);
                    }
                    let keyboards = ui
                        .add_enabled_ui(theirs, |ui| {
                            sheet::secondary(ui, None, "Use the keyboard’s")
                        })
                        .inner;
                    if keyboards.clicked() {
                        answer = Some(Answer::Other);
                    }
                },
            );
            if ui.input(|input| input.key_pressed(egui::Key::Escape)) {
                answer = Some(Answer::No);
            }
        });
    answer
}

/// The fields that differ, Yours beside the Keyboard's, or the one line the send queue's
/// review says for a difference that is not a list of fields.
fn fields(ui: &mut egui::Ui, choosing: &Choosing) {
    let Diff::Fields(fields) = &choosing.diff else {
        let (_, tint, said) =
            crate::queue::summary(&choosing.diff, choosing.class, choosing.at, ui.visuals());
        ui.label(egui::RichText::new(said).color(tint));
        return;
    };
    egui::ScrollArea::vertical()
        .max_height(sheet::middle(ui.ctx(), 220.0, 120.0))
        .show(ui, |ui| {
            egui::Grid::new("choice_fields")
                .num_columns(3)
                .striped(true)
                .show(ui, |ui| {
                    for head in ["Field", "Yours", "Keyboard’s"] {
                        ui.label(egui::RichText::new(head).weak());
                    }
                    ui.end_row();
                    for field in fields {
                        ui.label(label(&field.path));
                        ui.label(egui::RichText::new(&field.here).monospace());
                        ui.label(egui::RichText::new(&field.there).monospace());
                        ui.end_row();
                    }
                });
        });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::device::{DeviceEvent, ELECTRO5};
    use crate::link::Print;
    use crate::testing::Bench;
    use crate::workspace::Origin;

    const PROGRAM: ObjectClass = ObjectClass::Program;

    fn body() -> Vec<u8> {
        crate::workspace::Fresh::Program.bytes().unwrap()
    }

    fn gained(gain: &str) -> Vec<u8> {
        let gain = [("center_panel.gain".into(), gain.into())];
        crate::fields::apply(&body(), &gain).unwrap().1
    }

    fn print(bytes: &[u8]) -> Print {
        print_of(bytes).unwrap()
    }

    fn crc(bytes: &[u8]) -> u32 {
        print(bytes).crc
    }

    fn at(slot: u32) -> Location {
        Location::from_user(1, slot)
    }

    fn heard(bench: &mut Bench) {
        bench.device.poll(
            &mut bench.log,
            &mut bench.workspace,
            &mut bench.tabs,
            &mut bench.queue,
        );
    }

    /// Programs as a finished walk of one bank reported them, each slot `None` or a
    /// checksum.
    fn walked(bench: &mut Bench, slots: &[Option<u32>]) {
        bench.device.pretend_partitions(&ELECTRO5);
        let banks = [("A", slots.len() as u32)];
        bench.device.pretend_geometry(PROGRAM, &banks);
        let slots: Vec<Option<(&str, u32)>> =
            slots.iter().map(|crc| crc.map(|crc| ("P", crc))).collect();
        bench.device.pretend_bodies(PROGRAM, 1, &slots);
    }

    /// A file holding `bytes`, linked to `slot` as last agreed on `base`.
    fn linked(bench: &mut Bench, name: &str, bytes: Vec<u8>, slot: u32, base: &[u8]) -> u64 {
        let origin = Origin::File(name.into());
        let log = &mut bench.log;
        let id = bench.workspace.ingest(name.into(), origin, bytes, log);
        bench.workspace.settle_files(log);
        let keyboard = bench.device.state.keyboard().expect("attached");
        let held = Held::new(PROGRAM, Spot::at(at(slot)), print(base)).unwrap();
        bench.workspace.link(id, &keyboard, held);
        id
    }

    fn standing(bench: &Bench, id: u64) -> Option<Standing> {
        let entity = bench.workspace.get(id)?;
        Some(standing_of(&bench.device.state, entity)?.standing)
    }

    fn slot_of(bench: &Bench, id: u64) -> Option<Spot> {
        let keyboard = bench.device.state.keyboard()?;
        Some(bench.workspace.get(id)?.held_on(&keyboard)?.spot.clone())
    }

    fn pulls(bench: &Bench) -> Vec<Location> {
        let queued = bench.device.queued().into_iter();
        queued
            .filter_map(|cmd| match cmd {
                DeviceCmd::Get {
                    at,
                    why: Purpose::Pull,
                    ..
                } => Some(*at),
                _ => None,
            })
            .collect()
    }

    fn got(bench: &mut Bench, slot: u32, bytes: Vec<u8>, why: Purpose) {
        let origin = Origin::Device {
            class: PROGRAM,
            at: at(slot),
        };
        let name = "P.ne5p".into();
        bench.device.pretend(DeviceEvent::Got {
            name,
            origin,
            bytes,
            why,
        });
        heard(bench);
    }

    fn program_info(slot: u32, name: &str, crc: u32) -> nord_usb::wire::ProgramInfo {
        nord_usb::wire::ProgramInfo {
            location: at(slot),
            body_len: 121,
            format: "ne5p".into(),
            version: 4,
            crc32: Some(crc),
            modified: None,
            name: name.into(),
        }
    }

    fn found(id: u64, change: Change) -> Found {
        let class = PROGRAM;
        Found { id, class, change }
    }

    #[test]
    fn a_slot_changed_on_the_keyboard_is_asked_about_and_one_changed_here_too_is_not() {
        let mut bench = Bench::new();
        walked(
            &mut bench,
            &[
                Some(crc(&gained("40"))),
                Some(crc(&gained("41"))),
                Some(crc(&gained("42"))),
            ],
        );
        let theirs = linked(&mut bench, "A.ne5p", body(), 1, &body());
        let both = linked(&mut bench, "B.ne5p", gained("50"), 2, &body());
        let edited = linked(&mut bench, "C.ne5p", body(), 3, &body());
        let log = &mut bench.log;
        bench.workspace.replace_bytes(edited, gained("60"), log);
        assert_eq!(standing(&bench, both), Some(Standing::Both));
        assert_eq!(
            since(&bench.device.state, &bench.workspace),
            [found(theirs, Change::Changed(at(1)))]
        );
    }

    #[test]
    fn a_sound_in_one_other_free_slot_has_moved_and_one_in_two_has_not() {
        let mut bench = Bench::new();
        let other = crc(&gained("40"));
        walked(&mut bench, &[Some(other), Some(crc(&body())), None]);
        let id = linked(&mut bench, "A.ne5p", body(), 1, &body());
        let moved = Change::Moved {
            from: at(1),
            to: at(2),
            agrees: true,
        };
        assert_eq!(
            since(&bench.device.state, &bench.workspace),
            [found(id, moved)]
        );

        walked(
            &mut bench,
            &[Some(other), Some(crc(&body())), Some(crc(&body()))],
        );
        assert_eq!(
            since(&bench.device.state, &bench.workspace),
            [found(id, Change::Changed(at(1)))],
            "two slots hold it"
        );
    }

    #[test]
    fn a_sound_two_files_would_move_to_has_moved_for_neither() {
        let mut bench = Bench::new();
        walked(
            &mut bench,
            &[Some(crc(&gained("40"))), None, Some(crc(&body()))],
        );
        let theirs = linked(&mut bench, "A.ne5p", body(), 1, &body());
        linked(&mut bench, "B.ne5p", body(), 2, &body());
        assert_eq!(
            since(&bench.device.state, &bench.workspace),
            [found(theirs, Change::Changed(at(1)))]
        );
    }

    #[test]
    fn a_sound_in_a_slot_another_file_is_linked_to_has_not_moved() {
        let mut bench = Bench::new();
        walked(&mut bench, &[None, Some(crc(&body()))]);
        let id = linked(&mut bench, "A.ne5p", body(), 1, &body());
        linked(&mut bench, "B.ne5p", body(), 2, &body());
        assert_eq!(standing(&bench, id), Some(Standing::Gone));
        assert_eq!(since(&bench.device.state, &bench.workspace), []);
    }

    #[test]
    fn an_emptied_slot_is_put_back_only_by_its_explicit_send() {
        let mut bench = Bench::new();
        walked(&mut bench, &[None]);
        let id = linked(&mut bench, "A.ne5p", gained("40"), 1, &body());
        assert_eq!(standing(&bench, id), Some(Standing::Gone));
        let state = &bench.device.state;
        assert_eq!(
            crate::queue::offer(&bench.workspace, state, &bench.queue),
            None
        );
        let entity = bench.workspace.get(id).unwrap();
        let loud = crate::document::action(entity, state).unwrap();
        assert_eq!(loud.label, "Send to 1:1");
        assert_eq!(loud.click, crate::document::Click::Send(PROGRAM, at(1)));

        bench.act(vec![crate::browser::Act::SaveDoc(id)]);
        assert!(!bench.queue.holds(id), "Save queues nothing");
        bench.act(vec![crate::browser::Act::Send {
            id,
            class: PROGRAM,
            at: at(1),
        }]);
        assert!(bench.queue.holds(id));
    }

    #[test]
    fn nothing_but_choose_queues_a_file_over_its_own_changed_slot() {
        use crate::browser::Act;
        let mut bench = Bench::new();
        walked(&mut bench, &[Some(crc(&gained("40"))), None]);
        let id = linked(&mut bench, "A.ne5p", body(), 1, &body());
        assert_eq!(standing(&bench, id), Some(Standing::Theirs));
        let own = Act::Send {
            id,
            class: PROGRAM,
            at: at(1),
        };
        for act in [
            Act::SendChecked(vec![id]),
            own,
            Act::SaveDoc(id),
            Act::QueueChanged,
        ] {
            let said = format!("{act:?}");
            bench.act(vec![act]);
            assert!(!bench.queue.holds(id), "{said} queued it");
        }
        let pointed = bench.log.transcript().matches("Choose…").count();
        assert_eq!(pointed, 2, "a bulk queue and a drag say why");

        bench.act(vec![Act::Send {
            id,
            class: PROGRAM,
            at: at(2),
        }]);
        assert_eq!(bench.queue.entry(id).map(|held| held.at), Some(at(2)));
    }

    #[test]
    fn a_queued_write_whose_slot_changed_is_held_back_and_the_rest_go() {
        use crate::browser::Act;
        let mut bench = Bench::new();
        walked(&mut bench, &[Some(crc(&body())), Some(crc(&body())), None]);
        let theirs = linked(&mut bench, "A.ne5p", body(), 1, &body());
        let log = &mut bench.log;
        let other = bench
            .workspace
            .ingest("B.ne5p".into(), Origin::Fresh, gained("41"), log);
        let free = bench
            .workspace
            .ingest("C.ne5p".into(), Origin::Fresh, gained("42"), log);
        bench.workspace.settle_files(log);
        bench.workspace.replace_bytes(theirs, gained("50"), log);
        bench.act(vec![Act::SaveDoc(theirs)]);
        for (id, slot) in [(other, 2), (free, 3)] {
            bench.act(vec![Act::Send {
                id,
                class: PROGRAM,
                at: at(slot),
            }]);
        }
        assert_eq!(bench.queue.len(), 3);

        let slots = vec![
            Some(program_info(1, "A", crc(&gained("43")))),
            Some(program_info(2, "Someone", crc(&gained("44")))),
            None,
        ];
        bench.device.pretend(DeviceEvent::BankScanned {
            class: PROGRAM,
            bank: 1,
            slots,
        });
        heard(&mut bench);
        let failure = |bench: &Bench, id| bench.queue.entry(id).unwrap().failure.clone();
        assert!(failure(&bench, theirs).unwrap().contains("Choose…"));
        assert!(
            failure(&bench, other).is_some(),
            "it replaces another sound now"
        );
        let Some(crate::queue::Occupancy::Held(now)) =
            bench.queue.entry(other).map(|held| &held.replaces)
        else {
            panic!("the review shows what the slot holds");
        };
        assert_eq!(now.name, "Someone");
        assert_eq!(failure(&bench, free), None);

        bench.act(vec![Act::SendAll]);
        let ids: Vec<u64> = bench
            .device
            .queued()
            .into_iter()
            .flat_map(|cmd| match cmd {
                DeviceCmd::SendAll { items, .. } => items.iter().map(|item| item.id).collect(),
                _ => Vec::new(),
            })
            .collect();
        assert_eq!(ids, [free]);
        assert!(bench.queue.holds(theirs) && bench.queue.holds(other));
    }

    #[test]
    fn an_empty_slot_whose_sound_is_nowhere_was_removed() {
        let mut bench = Bench::new();
        walked(&mut bench, &[None, Some(crc(&gained("40")))]);
        let id = linked(&mut bench, "A.ne5p", body(), 1, &body());
        assert_eq!(
            since(&bench.device.state, &bench.workspace),
            [found(id, Change::Removed(Some(at(1))))]
        );
    }

    #[test]
    fn the_question_waits_for_the_walk_and_is_asked_once_per_attach() {
        let mut bench = Bench::new();
        walked(&mut bench, &[Some(crc(&gained("40")))]);
        bench
            .device
            .pretend_geometry(PROGRAM, &[("A", 1), ("B", 1)]);
        let id = linked(&mut bench, "A.ne5p", body(), 1, &body());
        heard(&mut bench);
        assert!(bench.device.asks.since.is_none(), "bank 2 is unread");

        let slots = vec![None];
        bench.device.pretend(DeviceEvent::BankScanned {
            class: PROGRAM,
            bank: 2,
            slots,
        });
        heard(&mut bench);
        let asked = Some(vec![found(id, Change::Changed(at(1)))]);
        assert_eq!(bench.device.asks.since, asked);

        bench.device.asks.since = None;
        bench.device.pretend(DeviceEvent::BankScanned {
            class: PROGRAM,
            bank: 2,
            slots: vec![None],
        });
        heard(&mut bench);
        assert!(bench.device.asks.since.is_none(), "asked once");
        assert_eq!(standing(&bench, id), Some(Standing::Theirs), "Not now");

        let card = bench.device.state.card().unwrap().clone();
        bench
            .device
            .pretend(DeviceEvent::Disconnected { lost: false });
        bench.device.pretend(DeviceEvent::Connected(card));
        heard(&mut bench);
        walked(&mut bench, &[Some(crc(&gained("40")))]);
        bench.device.pretend(DeviceEvent::Finished);
        heard(&mut bench);
        assert_eq!(bench.device.asks.since, asked, "a new attach asks again");
    }

    #[test]
    fn nothing_changed_asks_nothing() {
        let mut bench = Bench::new();
        walked(&mut bench, &[Some(crc(&body()))]);
        linked(&mut bench, "A.ne5p", body(), 1, &body());
        heard(&mut bench);
        assert!(bench.device.asks.since.is_none());
    }

    #[test]
    fn bring_them_in_brings_in_moves_and_unlinks_and_brings_in_later_changes_unasked() {
        let mut bench = Bench::new();
        let (one, two) = (gained("40"), gained("41"));
        walked(
            &mut bench,
            &[Some(crc(&one)), None, None, Some(crc(&two)), None],
        );
        let changed = linked(&mut bench, "A.ne5p", body(), 1, &body());
        let removed = linked(&mut bench, "B.ne5p", gained("50"), 2, &gained("50"));
        let moved = linked(&mut bench, "C.ne5p", two.clone(), 3, &two);
        heard(&mut bench);
        assert_eq!(bench.device.asks.since.as_ref().map(Vec::len), Some(3));

        bring_in(&mut bench.device, &mut bench.workspace, &mut bench.log);
        assert_eq!(pulls(&bench), [at(1)]);
        assert_eq!(slot_of(&bench, moved), Some(Spot::at(at(4))));
        assert_eq!(standing(&bench, moved), Some(Standing::Same));
        assert_eq!(slot_of(&bench, removed), None);
        assert!(
            bench.workspace.get(removed).is_some(),
            "kept on this computer"
        );

        got(&mut bench, 1, one.clone(), Purpose::Pull);
        assert_eq!(bench.workspace.get(changed).unwrap().bytes.to_vec(), one);
        assert_eq!(standing(&bench, changed), Some(Standing::Same));
        assert!(bench.device.asks.since.is_none());

        let three = gained("42");
        walked(
            &mut bench,
            &[Some(crc(&three)), None, None, Some(crc(&two)), None],
        );
        heard(&mut bench);
        bench.device.pretend(DeviceEvent::Finished);
        heard(&mut bench);
        assert_eq!(pulls(&bench), [at(1), at(1)], "brought in without asking");
        got(&mut bench, 1, three.clone(), Purpose::Pull);
        assert_eq!(bench.workspace.get(changed).unwrap().bytes.to_vec(), three);
        assert!(bench.device.asks.since.is_none());
    }

    #[test]
    fn a_file_edited_since_its_slot_was_read_is_not_brought_in_over() {
        let mut bench = Bench::new();
        let one = gained("40");
        walked(&mut bench, &[Some(crc(&one))]);
        let id = linked(&mut bench, "A.ne5p", body(), 1, &body());
        heard(&mut bench);
        bring_in(&mut bench.device, &mut bench.workspace, &mut bench.log);
        let log = &mut bench.log;
        bench.workspace.replace_bytes(id, gained("60"), log);
        got(&mut bench, 1, one, Purpose::Pull);
        assert_eq!(
            bench.workspace.get(id).unwrap().bytes.to_vec(),
            gained("60")
        );
    }

    #[test]
    fn use_mine_leaves_the_file_ahead_and_queued_and_use_the_keyboards_leaves_it_the_same() {
        let mut bench = Bench::new();
        let theirs = gained("40");
        walked(&mut bench, &[Some(crc(&theirs))]);
        let id = linked(&mut bench, "A.ne5p", body(), 1, &body());
        heard(&mut bench);
        for answer in [Answer::Yes, Answer::Other] {
            assert_eq!(standing(&bench, id), Some(Standing::Theirs));
            choose(&mut bench.device, &bench.workspace, &mut bench.log, id);
            got(&mut bench, 1, theirs.clone(), Purpose::Choose);
            let choosing = bench.device.asks.choosing.as_ref().expect("asked");
            let Diff::Fields(fields) = &choosing.diff else {
                panic!("a program differs by its fields")
            };
            assert!(fields.iter().any(|field| field.path == "center_panel.gain"));
            let Bench {
                device,
                workspace,
                queue,
                log,
                ..
            } = &mut bench;
            match answer {
                Answer::Yes => {
                    use_mine(device, workspace, queue, log);
                    assert_eq!(standing(&bench, id), Some(Standing::Ahead));
                    assert!(bench.queue.holds(id), "queued for its slot");
                    let keyboard = bench.device.state.keyboard().unwrap();
                    let entity = bench.workspace.get(id).unwrap();
                    let held = entity.held_on(&keyboard).unwrap();
                    assert!(held.knew(crc(&theirs)), "the keyboard's counts as earlier");
                    let unchosen = Held::new(PROGRAM, Spot::at(at(1)), print(&body())).unwrap();
                    bench.workspace.link(id, &keyboard, unchosen);
                    bench.queue.forget(id);
                }
                Answer::Other | Answer::No => {
                    use_keyboards(device, workspace, log);
                    assert_eq!(standing(&bench, id), Some(Standing::Same));
                }
            }
            assert!(bench.device.asks.choosing.is_none());
        }
    }

    #[test]
    fn each_undo_in_the_log_takes_its_change_back_once() {
        let mut bench = Bench::new();
        let (theirs, other) = (gained("40"), gained("41"));
        walked(&mut bench, &[Some(crc(&theirs)), None, Some(crc(&other))]);
        let changed = linked(&mut bench, "A.ne5p", body(), 1, &body());
        let moved = linked(&mut bench, "B.ne5p", other.clone(), 2, &other);
        heard(&mut bench);

        bring_in(&mut bench.device, &mut bench.workspace, &mut bench.log);
        got(&mut bench, 1, theirs, Purpose::Pull);
        let undo = |bench: &mut Bench, line: &str| {
            let pressed = bench.log.press(line);
            bench
                .log
                .undo_pressed(&mut bench.workspace, &mut bench.queue);
            pressed
        };
        assert!(undo(&mut bench, "is now at 1:3"));
        assert_eq!(slot_of(&bench, moved), Some(Spot::at(at(2))));
        assert!(!undo(&mut bench, "is now at 1:3"), "once");

        assert!(undo(&mut bench, "Brought in"));
        assert_eq!(bench.workspace.get(changed).unwrap().bytes.to_vec(), body());
        assert_eq!(standing(&bench, changed), Some(Standing::Ahead));
        heard(&mut bench);
        assert_eq!(pulls(&bench), [at(1)], "not brought in again");
        assert!(!undo(&mut bench, "Brought in"), "once");
    }

    #[test]
    fn undoing_files_linked_on_their_own_keeps_them_off_the_keyboard() {
        let mut bench = Bench::new();
        walked(&mut bench, &[Some(crc(&body()))]);
        let origin = Origin::Device {
            class: PROGRAM,
            at: at(1),
        };
        let log = &mut bench.log;
        let id = bench.workspace.ingest("A.ne5p".into(), origin, body(), log);
        bench.workspace.settle_files(log);
        heard(&mut bench);
        assert_eq!(slot_of(&bench, id), Some(Spot::at(at(1))));
        assert!(bench.log.press("linked to the slot"));
        bench
            .log
            .undo_pressed(&mut bench.workspace, &mut bench.queue);
        heard(&mut bench);
        let keyboard = bench.device.state.keyboard().unwrap();
        assert!(bench.workspace.get(id).unwrap().kept_off(&keyboard));
        assert!(!bench.log.press("linked to the slot"), "once");
    }

    #[test]
    fn undoing_a_save_that_queued_a_send_takes_it_out_of_the_queue() {
        let mut bench = Bench::new();
        walked(&mut bench, &[Some(crc(&body()))]);
        let id = linked(&mut bench, "A.ne5p", body(), 1, &body());
        let log = &mut bench.log;
        bench.workspace.replace_bytes(id, gained("40"), log);
        bench.act(vec![crate::browser::Act::SaveDoc(id)]);
        assert!(bench.queue.holds(id));
        assert!(bench.log.press("is waiting to be sent"));
        bench
            .log
            .undo_pressed(&mut bench.workspace, &mut bench.queue);
        assert!(!bench.queue.holds(id));
        assert_eq!(standing(&bench, id), Some(Standing::Ahead), "still saved");
    }
}
