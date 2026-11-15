//! The two questions drawbar asks about linked files: what changed on the keyboard since
//! it was last attached, and which of a file and its slot is current. Bringing a slot in
//! reads it and saves what it holds as the file linked to it.

use std::collections::BTreeSet;

use eframe::egui;
use nord_usb::{Location, ObjectClass};

use crate::device::{
    occupied, slot_label, standing_of, there, Device, DeviceCmd, DeviceState, Purpose,
};
use crate::link::{self, Held, Keyboard, Print, Spot, Standing, There};
use crate::log::{Log, Restored, Undo};
use crate::queue::{compare, Diff, Queue};
use crate::sheet;
use crate::strings::{counted, display_name, shown};
use crate::workspace::{print_of, LocalEntity, VerifyState, Workspace};

/// What happened on the keyboard to one linked file's sound while drawbar was away.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Change {
    /// The slot holds a version the file never had, under the name it had when the two
    /// last agreed, and the file has not changed.
    Changed(Location),
    /// The sound is in another slot of its class, which no file is linked to, or which
    /// holds the sound of the file linked there while this slot holds that one's.
    /// `agrees` where that slot holds the file's saved version.
    Moved {
        from: Location,
        to: Location,
        agrees: bool,
    },
    /// The slot holds another sound, named `by`, and the file's sound is not in exactly
    /// one other slot.
    Replaced { at: Location, by: String },
    /// The slot is empty and the sound is nowhere else. A piano or sample has no address.
    Removed(Option<Location>),
}

/// A linked file, and what happened to its sound on the keyboard.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Noticed {
    pub id: u64,
    pub class: ObjectClass,
    pub change: Change,
}

/// What changed on the attached keyboard to the files linked there, for the question
/// asked once its walk has finished. A file with unsaved edits is left to its mark. A
/// sound two files would move to has moved for neither.
pub fn since(state: &DeviceState, workspace: &Workspace) -> Vec<Noticed> {
    let Some(keyboard) = state.keyboard() else {
        return Vec::new();
    };
    let judged: Vec<(Noticed, &Held)> = workspace
        .listed()
        .filter(|entity| !entity.is_unsaved())
        .filter_map(|entity| {
            let held = entity.held_on(keyboard)?;
            let change = change(state, workspace, keyboard, entity, held)?;
            let (id, class) = (entity.id, held.class());
            Some((Noticed { id, class, change }, held))
        })
        .collect();
    let target = |found: &Noticed| match found.change {
        Change::Moved { to, .. } => Some((found.class, to)),
        Change::Changed(_) | Change::Replaced { .. } | Change::Removed(_) => None,
    };
    let contested = |found: &Noticed| {
        let mut claims = judged
            .iter()
            .filter(|(other, _)| target(other) == target(found));
        target(found).is_some() && claims.nth(1).is_some()
    };
    judged
        .iter()
        .filter_map(|(found, held)| match contested(found) {
            false => Some(found.clone()),
            true => Some(Noticed {
                change: unmoved(state, held)?,
                ..found.clone()
            }),
        })
        .collect()
}

/// What changed to one file's sound, judged by what its slot holds now and whether it
/// holds it under the name it had when the two last agreed. A link that kept no name
/// goes by the file's.
pub fn change(
    state: &DeviceState,
    workspace: &Workspace,
    keyboard: &Keyboard,
    entity: &LocalEntity,
    held: &Held,
) -> Option<Change> {
    let here = entity.saved_print()?;
    let (_, there) = there(state, held);
    let Some(from) = held.location() else {
        return (there == There::Vacant).then_some(Change::Removed(None));
    };
    let by = match there {
        There::Unread | There::Holds { crc: None, .. } => return None,
        There::Holds { crc: Some(crc), .. } if held.knew(crc) || crc == here.crc => return None,
        There::Vacant => None,
        There::Holds { .. } => Some(name_at(state, held.class(), from)),
    };
    let agreed = held.name().map(str::to_string);
    if by.is_some() && by == agreed.or_else(|| slot_label(&entity.name)) {
        let theirs = link::standing(held, Some(here), there) == Standing::Theirs;
        return theirs.then_some(Change::Changed(from));
    }
    let mut elsewhere = elsewhere(state, held, here, from);
    let unique = match (elsewhere.next(), elsewhere.next()) {
        (Some(to), None) => Some(to),
        (None, None) if by.is_none() => return Some(Change::Removed(Some(from))),
        _ => None,
    };
    let to = unique.filter(|to| movable(state, workspace, keyboard, held, from, *to));
    match (to, by) {
        (Some(to), _) => {
            let agrees = state
                .slot(held.class(), to)
                .flatten()
                .and_then(|info| info.crc32);
            Some(Change::Moved {
                from,
                to,
                agrees: agrees == Some(here.crc),
            })
        }
        (None, Some(by)) => Some(Change::Replaced { at: from, by }),
        (None, None) => None,
    }
}

/// The name the keyboard keeps the slot `at` of `class` under.
fn name_at(state: &DeviceState, class: ObjectClass, at: Location) -> String {
    let info = state.slot(class, at).flatten();
    info.map_or("", |info| info.name.trim()).to_string()
}

/// The slots of its class other than `from` holding what a link last agreed on, or what
/// its file is saved as.
fn elsewhere<'s>(
    state: &'s DeviceState,
    held: &'s Held,
    here: Print,
    from: Location,
) -> impl Iterator<Item = Location> + 's {
    let ours = move |crc: u32| crc == held.base.crc || crc == here.crc;
    occupied(state, held.class())
        .filter(move |(at, info)| *at != from && info.crc32.is_some_and(ours))
        .map(|(at, _)| at)
}

/// Whether a link at `from` may follow its sound to `to`: no file is linked there, or the
/// one linked there has its sound at `from`, which is a swap.
fn movable(
    state: &DeviceState,
    workspace: &Workspace,
    keyboard: &Keyboard,
    held: &Held,
    from: Location,
    to: Location,
) -> bool {
    let class = held.class();
    let Some(other) = workspace.linked_at(keyboard, class, &Spot::at(to)) else {
        return true;
    };
    let Some(other) = workspace.get(other) else {
        return false;
    };
    let theirs = |crc| {
        other.held_on(keyboard).is_some_and(|it| it.base.crc == crc)
            || other.saved_print().is_some_and(|it| it.crc == crc)
    };
    let at_from = state
        .slot(class, from)
        .flatten()
        .and_then(|info| info.crc32);
    at_from.is_some_and(theirs)
}

/// What a link two files would move to the same slot says instead: its slot holds another
/// sound, or nothing.
fn unmoved(state: &DeviceState, held: &Held) -> Option<Change> {
    let at = held.location()?;
    let (_, there) = there(state, held);
    (there != There::Vacant).then(|| Change::Replaced {
        at,
        by: name_at(state, held.class(), at),
    })
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
            .filter_map(|entity| Some(entity.held_on(keyboard)?.class()))
            .filter(|class| classes.contains(class))
            .all(|class| state.read_whole(class) && !running(class))
}

/// Whether every saved file linked on the attached keyboard has a saved checksum, or never
/// will. Asks the library for each one still to be read or checked.
fn printed(state: &DeviceState, workspace: &Workspace) -> bool {
    let Some(keyboard) = state.keyboard() else {
        return false;
    };
    let unread = |entity: &LocalEntity| {
        let failed = matches!(entity.verify(), VerifyState::NotRead(_));
        entity.awaits_check() || (entity.unread() && !failed)
    };
    let waiting: Vec<u64> = workspace
        .listed()
        .filter(|entity| !entity.is_unsaved() && entity.held_on(keyboard).is_some())
        .filter(|entity| entity.saved_print().is_none() && unread(entity))
        .map(|entity| entity.id)
        .collect();
    waiting.iter().for_each(|id| workspace.hurry(*id));
    waiting.is_empty()
}

/// Whether the once-per-attach passes may run: every linked class is walked, and every
/// linked file's checksum is known.
fn ready(state: &DeviceState, workspace: &Workspace) -> bool {
    walked(state, workspace) && printed(state, workspace)
}

/// A slot read to be brought into the file linked to it.
struct Pull {
    class: ObjectClass,
    at: Location,
    id: u64,
    /// The file's link when the read was asked for. The bytes land only on a file still
    /// linked so.
    held: Held,
    /// Whether it is one of those Bring them in asked for, which share one log line.
    first: bool,
}

/// What drawbar asks, and has been told, while one keyboard is attached. A new attach
/// starts it over.
#[derive(Default)]
pub struct Asks {
    /// Whether this attach's question was asked, or found nothing to ask.
    settled: bool,
    /// Whether [`since`] found anything, as of the last [`follow`].
    changed: bool,
    /// Whether the user said Bring them in: a slot changed on the keyboard from then on is
    /// brought in without asking.
    bringing: bool,
    /// The question asked on attach, while it shows.
    pub since: Option<Vec<Noticed>>,
    /// The question asked about one file, while it shows.
    pub choosing: Option<Choosing>,
    /// Slots read to be brought in, until their bytes land. A read that fails, or lands
    /// on a file whose bringing in was refused, stays, so it is not asked for again.
    pulling: Vec<Pull>,
    /// The log line counting the slots Bring them in brought in.
    brought: Option<u64>,
    /// The files whose bringing in the user undid: none is brought in again on its own.
    undone: BTreeSet<u64>,
    /// Whether the files ahead of their slots were queued for this attach.
    queued: bool,
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
        self.pulling
            .retain(|pull| (pull.class, pull.at) != (class, at));
        if self
            .choosing
            .as_ref()
            .is_some_and(|it| (it.class, it.at) == (class, at))
        {
            self.choosing = None;
        }
    }

    /// The user undid bringing `id` in.
    pub fn undid(&mut self, id: u64) {
        self.undone.insert(id);
    }

    /// Whether this attach's question was answered, or found nothing to ask.
    pub fn answered(&self) -> bool {
        self.settled && self.since.is_none()
    }

    /// Whether anything changed on the keyboard to ask about again.
    pub fn changed(&self) -> bool {
        self.changed
    }
}

/// Settle the question for this attach once the walk allows, and bring in what changed
/// on the keyboard after the user said Bring them in. Call when the instrument or the
/// list may have changed.
///
/// ⚠️ Nothing is asked or brought in while a write is running or waiting: a send changes
/// the slots it is asked about.
pub fn follow(device: &mut Device, workspace: &Workspace, log: &mut Log) {
    let found = since(&device.state, workspace);
    device.asks.changed = !found.is_empty();
    if device.sending() {
        return;
    }
    if !device.asks.settled && ready(&device.state, workspace) {
        device.asks.settled = true;
        device.asks.since = (!found.is_empty()).then_some(found.clone());
    }
    if device.asks.bringing {
        let kept = found
            .into_iter()
            .filter(|found| !device.asks.undone.contains(&found.id));
        pull_changed(device, workspace, log, kept.collect(), false);
    }
}

/// Read each slot of `found` that changed on the keyboard, to bring it into its file.
fn pull_changed(
    device: &mut Device,
    workspace: &Workspace,
    log: &mut Log,
    found: Vec<Noticed>,
    first: bool,
) {
    let Some(keyboard) = device.state.keyboard().cloned() else {
        return;
    };
    for found in found {
        let Change::Changed(at) = found.change else {
            continue;
        };
        let held = workspace.get(found.id).and_then(|it| it.held_on(&keyboard));
        let (false, Some(held)) = (found.class.is_library(), held.cloned()) else {
            continue;
        };
        let (class, id) = (found.class, found.id);
        pull(
            device,
            log,
            Pull {
                class,
                at,
                id,
                held,
                first,
            },
        );
    }
}

/// Read a slot to bring it in, unless it was asked for already under the same link.
fn pull(device: &mut Device, log: &mut Log, pull: Pull) {
    let asked = |it: &Pull| (it.class, it.at, &it.held) == (pull.class, pull.at, &pull.held);
    if device.asks.pulling.iter().any(asked) {
        return;
    }
    let (class, at, why) = (pull.class, pull.at, Purpose::Pull);
    device.asks.pulling.push(pull);
    device.send(DeviceCmd::Get { class, at, why }, log);
}

/// Once per attach, after the walk of every class holding a linked file and the question
/// it asks: queue each file ahead of its slot for that slot, as saving it would have, and
/// say so in one line that Undo takes back. A slot something already waits for keeps it.
pub fn queue_ahead(device: &mut Device, workspace: &Workspace, queue: &mut Queue, log: &mut Log) {
    if device.asks.queued || !device.asks.answered() || device.sending() {
        return;
    }
    device.asks.queued = true;
    let mut queued = Vec::new();
    for (id, class, at) in crate::queue::changed(workspace, &device.state, queue) {
        if queue.waiting(class, at).is_none() {
            let entry = (id, class, at);
            queued.extend(crate::queue::enqueue_quietly(
                workspace, device, queue, log, entry,
            ));
        }
    }
    if queued.is_empty() {
        return;
    }
    log.say(format!(
        "{} waiting to send.",
        match queued.len() {
            1 => "1 sound is".to_string(),
            n => format!("{n} sounds are"),
        }
    ));
    log.offer(Undo::Unqueue(queued));
}

/// The command that asks again what changed on the keyboard.
pub const KEYBOARD_CHANGES: &str = "Keyboard changes…";

/// Ask again what changed on the attached keyboard, after Not now.
pub fn ask_again(device: &mut Device, workspace: &Workspace) {
    let found = since(&device.state, workspace);
    device.asks.since = (!found.is_empty()).then_some(found);
}

/// Bring them in: every slot listed as changed on the keyboard is brought in, though an
/// earlier bringing in of it was undone, and once one was listed, a slot changed later
/// while this keyboard stays attached is brought in unasked; a moved sound's link follows
/// it; a removed or replaced sound's file is unlinked and kept. What changed since the
/// question was asked is left as it is.
pub fn bring_in(device: &mut Device, workspace: &mut Workspace, log: &mut Log) {
    let asked = device.asks.since.take().unwrap_or_default();
    let Some(keyboard) = device.state.keyboard().cloned() else {
        return;
    };
    let changed = |found: &Noticed| matches!(found.change, Change::Changed(_));
    device.asks.bringing |= asked.iter().any(changed);
    device.asks.brought = None;
    let now: Vec<Noticed> = since(&device.state, workspace)
        .into_iter()
        .filter(|found| asked.contains(found))
        .collect();
    follow_moves(&device.state, workspace, log, &keyboard, &now);
    let unlinked: Vec<(u64, Held)> = now
        .iter()
        .filter(|found| matches!(found.change, Change::Removed(_) | Change::Replaced { .. }))
        .filter_map(|found| {
            Some((
                found.id,
                workspace.get(found.id)?.held_on(&keyboard)?.clone(),
            ))
        })
        .collect();
    for (id, _) in &unlinked {
        workspace.unlink(*id, &keyboard);
    }
    if let [(id, _), ..] = unlinked.as_slice() {
        let name = workspace.get(*id).map_or("", |it| it.name.as_str());
        log.say(match unlinked.len() {
            1 => format!("“{}” unlinked from the keyboard.", display_name(name)),
            n => format!("{n} files unlinked from the keyboard."),
        });
        let links = unlinked;
        log.offer(Undo::Relink {
            keyboard: keyboard.clone(),
            links,
        });
    }
    for found in now.iter().filter(|found| changed(found)) {
        device.asks.undone.remove(&found.id);
    }
    if !device.sending() {
        pull_changed(device, workspace, log, now, true);
    }
}

/// Move each moved sound's link to the slot it moved to, all of a class at once so a swap
/// moves both, in one log line whose Undo puts them back.
fn follow_moves(
    state: &DeviceState,
    workspace: &mut Workspace,
    log: &mut Log,
    keyboard: &Keyboard,
    found: &[Noticed],
) {
    let moved: Vec<(&Noticed, Location, bool, Held)> = found
        .iter()
        .filter_map(|found| match found.change {
            Change::Moved { to, agrees, .. } => {
                let held = workspace.get(found.id)?.held_on(keyboard)?.clone();
                Some((found, to, agrees, held))
            }
            _ => None,
        })
        .collect();
    let mut classes: Vec<ObjectClass> = Vec::new();
    for (found, ..) in &moved {
        if !classes.contains(&found.class) {
            classes.push(found.class);
        }
    }
    for class in classes {
        let moves: Vec<crate::workspace::Moved> = moved
            .iter()
            .filter(|(found, ..)| found.class == class)
            .map(|(_, to, _, held)| (held.spot.clone(), Some(Spot::at(*to)), None))
            .collect();
        workspace.respot(keyboard, class, &moves);
    }
    for (found, to, agrees, _) in &moved {
        let entity = workspace.get(found.id);
        let saved = entity.and_then(LocalEntity::saved_print);
        let now = entity.and_then(|it| it.held_on(keyboard)).cloned();
        if let (true, Some(print), Some(now)) = (agrees, saved, now) {
            let name = name_at(state, found.class, *to);
            workspace.link(found.id, keyboard, now.agreed(print, &name));
        }
    }
    let Some((first, to, ..)) = moved.first() else {
        return;
    };
    let name = workspace.get(first.id).map_or("", |it| it.name.as_str());
    log.say(match moved.len() {
        1 => format!("“{}” is now at {}.", display_name(name), shown(*to)),
        n => format!("{n} files follow their sounds to other slots."),
    });
    let links = moved
        .into_iter()
        .map(|(found, _, _, held)| (found.id, held));
    log.offer(Undo::Relink {
        keyboard: keyboard.clone(),
        links: links.collect(),
    });
}

/// Save `bytes`, read from the slot `at` of `class`, as the file `id` linked to that slot
/// on the attached keyboard, and agree the link on them. Returns what it held before.
///
/// ⚠️ A file with unsaved edits is never brought in over: they would be lost.
fn bring(
    device: &mut Device,
    workspace: &mut Workspace,
    log: &mut Log,
    (class, at, id): (ObjectClass, Location, u64),
    bytes: Vec<u8>,
) -> Option<Restored> {
    let keyboard = device.state.keyboard()?;
    let spot = Spot::at(at);
    if workspace.linked_at(keyboard, class, &spot) != Some(id) {
        return None;
    }
    let entity = workspace.get(id)?;
    let before = entity.held_on(keyboard)?.clone();
    if entity.is_unsaved() || entity.unread() || entity.rests().is_some() {
        return None;
    }
    let after = before.agreed(print_of(&bytes)?, &name_at(&device.state, class, at));
    let restored = Restored {
        id,
        bytes: entity.bytes.to_vec(),
        keyboard: keyboard.clone(),
        before,
        after: after.clone(),
    };
    workspace.replace_bytes(id, bytes, log);
    workspace.mark_saved(id);
    workspace.link(id, keyboard, after);
    Some(restored)
}

/// A slot read to be brought in landed. It is brought in only over the file it was read
/// for, while that file is linked as it was when the read was asked for and is saved as
/// what that link last agreed on.
pub fn landed(
    device: &mut Device,
    workspace: &mut Workspace,
    log: &mut Log,
    class: ObjectClass,
    at: Location,
    bytes: Vec<u8>,
) {
    let Some(keyboard) = device.state.keyboard() else {
        return;
    };
    let still = |pull: &Pull| {
        let entity = workspace.get(pull.id);
        (pull.class, pull.at) == (class, at)
            && entity.is_some_and(|entity| {
                entity.held_on(keyboard) == Some(&pull.held)
                    && entity.saved_print() == Some(pull.held.base)
            })
    };
    let Some(index) = device.asks.pulling.iter().position(still) else {
        return;
    };
    let (id, first) = (
        device.asks.pulling[index].id,
        device.asks.pulling[index].first,
    );
    if let Some(restored) = bring(device, workspace, log, (class, at, id), bytes) {
        device.asks.pulling.remove(index);
        brought(&mut device.asks, workspace, log, restored, first);
    }
}

/// Log a file brought in. Those Bring them in asked for share one line; any other gets
/// its own.
fn brought(asks: &mut Asks, workspace: &Workspace, log: &mut Log, restored: Restored, first: bool) {
    let counted_on = asks
        .brought
        .filter(|_| first)
        .and_then(|serial| log.entry_mut(serial));
    if let Some(entry) = counted_on {
        if let Some(Undo::Restore(kept)) = &mut entry.undo {
            kept.push(restored);
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
    let name = workspace.get(restored.id).map_or("", |it| it.name.as_str());
    log.say(format!(
        "Brought in “{}” from the keyboard.",
        display_name(name)
    ));
    let serial = log.offer(Undo::Restore(vec![restored]));
    if first {
        asks.brought = serial;
    }
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
        id,
        class,
        at,
        there: Some(bytes),
        ..
    }) = device.asks.choosing.take()
    else {
        return;
    };
    if let Some(restored) = bring(device, workspace, log, (class, at, id), bytes) {
        brought(&mut device.asks, workspace, log, restored, false);
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
    let Some(held) = workspace.get(id).and_then(|it| it.held_on(keyboard)) else {
        return;
    };
    let (_, there) = there(&device.state, held);
    workspace.link(id, keyboard, held.superseding(there));
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

/// The question asked once after an attach, while it shows: "Bring them in", or "Update
/// links" where no slot is listed as changed, is [`Answer::Yes`], "Not now" [`Answer::No`].
pub fn since_sheet(
    ctx: &egui::Context,
    found: &[Noticed],
    workspace: &Workspace,
) -> Option<Answer> {
    let name = |id| {
        workspace
            .get(id)
            .map_or("", |it: &LocalEntity| display_name(&it.name))
            .to_string()
    };
    let mut groups: [Vec<(String, String)>; 4] = Default::default();
    for found in found {
        let name = name(found.id);
        let (group, line) = match &found.change {
            Change::Changed(at) => (0, (name, shown(*at))),
            Change::Moved { from, to, .. } => {
                (1, (name, format!("{} → {}", shown(*from), shown(*to))))
            }
            Change::Replaced { at, by } => (2, (format!("{name} → {by}"), shown(*at))),
            Change::Removed(at) => (3, (name, at.map(shown).unwrap_or_default())),
        };
        groups[group].push(line);
    }
    let [changed, moved, replaced, removed] = groups;
    let yes = match changed.is_empty() {
        true => UPDATE_LINKS,
        false => BRING_THEM_IN,
    };
    let groups = [
        (format!("{} changed", changed.len()), changed),
        (format!("{} moved", moved.len()), moved),
        (format!("{} replaced", replaced.len()), replaced),
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
                    if sheet::primary(ui, None, yes).clicked() {
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
    device: &DeviceState,
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
                fields(ui, choosing, workspace, device);
            });
            sheet::foot(
                ui,
                |_| {},
                |ui| {
                    if sheet::primary(ui, None, "Use mine").clicked() {
                        answer = Some(Answer::Yes);
                    }
                    let mut keyboards = ui
                        .add_enabled_ui(theirs, |ui| {
                            sheet::secondary(ui, None, "Use the keyboard’s")
                        })
                        .inner;
                    if choosing.class.is_library() {
                        keyboards = keyboards.on_disabled_hover_text(UNREAD);
                    } else if entity.is_unsaved() {
                        keyboards = keyboards.on_disabled_hover_text(UNSAVED);
                    }
                    if keyboards.clicked() {
                        answer = Some(Answer::Other);
                    }
                    if sheet::secondary(ui, None, "Cancel").clicked() {
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

/// Why a piano or sample cannot be brought in from the question about it: its slot is
/// never read whole.
const UNREAD: &str = "Copy it to this computer instead.";

/// Why a file with unsaved edits cannot be brought in over.
const UNSAVED: &str = "Save your edits first.";

/// The answer to the question asked on attach that brings changed slots in, and the one
/// for a list with none, whose answer only moves and removes links.
const BRING_THEM_IN: &str = "Bring them in";
const UPDATE_LINKS: &str = "Update links";

/// What the question says of two versions that differ in no field it can name.
pub const DIFFERS: &str = "differs";

/// The fields that differ, Yours beside the Keyboard's, or one line for a difference that
/// is not a list of fields.
fn fields(ui: &mut egui::Ui, choosing: &Choosing, workspace: &Workspace, device: &DeviceState) {
    let fields = match &choosing.diff {
        Diff::Fields(fields) => fields,
        Diff::Bytes { .. } | Diff::Checksum => {
            ui.label(egui::RichText::new(DIFFERS).color(crate::app::warn(ui.visuals())));
            return;
        }
        Diff::Pending | Diff::Identical | Diff::Empty => {
            let (_, tint, said) =
                crate::queue::summary(&choosing.diff, choosing.class, choosing.at, ui.visuals());
            ui.label(egui::RichText::new(said).color(tint));
            return;
        }
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
                        let [label, there, here] = field.said(workspace, device);
                        ui.label(label);
                        ui.label(egui::RichText::new(here).monospace());
                        ui.label(egui::RichText::new(there).monospace());
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
    /// checksum under the next letter's name, so the file `A.ne5p` names slot 1.
    fn walked(bench: &mut Bench, slots: &[Option<u32>]) {
        const NAMES: [&str; 5] = ["A", "B", "C", "D", "E"];
        let slots: Vec<Option<(&str, u32)>> = slots
            .iter()
            .zip(NAMES)
            .map(|(crc, name)| crc.map(|crc| (name, crc)))
            .collect();
        walked_named(bench, &slots);
    }

    /// Programs as a finished walk of one bank reported them, each slot `None` or a name
    /// and a checksum.
    fn walked_named(bench: &mut Bench, slots: &[Option<(&str, u32)>]) {
        bench.device.pretend_partitions(&ELECTRO5);
        let banks = [("A", slots.len() as u32)];
        bench.device.pretend_geometry(PROGRAM, &banks);
        bench.device.pretend_bodies(PROGRAM, 1, slots);
    }

    /// A file holding `bytes`, linked to `slot` as last agreed on `base`.
    fn linked(bench: &mut Bench, name: &str, bytes: Vec<u8>, slot: u32, base: &[u8]) -> u64 {
        let origin = Origin::File(name.into());
        let log = &mut bench.log;
        let id = bench.workspace.ingest(name.into(), origin, bytes, log);
        bench.workspace.settle_files(log);
        let keyboard = bench.device.state.keyboard().cloned().expect("attached");
        let held = Held::new(PROGRAM, at(slot), "", print(base)).unwrap();
        bench.workspace.link(id, &keyboard, held);
        id
    }

    fn standing(bench: &Bench, id: u64) -> Option<Standing> {
        let entity = bench.workspace.get(id)?;
        Some(standing_of(&bench.device.state, entity)?.standing)
    }

    fn slot_of(bench: &Bench, id: u64) -> Option<Spot> {
        let keyboard = bench.device.state.keyboard()?;
        Some(bench.workspace.get(id)?.held_on(keyboard)?.spot.clone())
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

    fn found(id: u64, change: Change) -> Noticed {
        let class = PROGRAM;
        Noticed { id, class, change }
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
    fn a_sound_in_one_other_free_slot_has_moved_and_one_in_two_was_replaced() {
        let mut bench = Bench::new();
        let (other, ours) = (crc(&gained("40")), crc(&body()));
        walked_named(
            &mut bench,
            &[Some(("Clav", other)), Some(("A", ours)), None],
        );
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

        let slots = [Some(("Clav", other)), Some(("A", ours)), Some(("A", ours))];
        walked_named(&mut bench, &slots);
        let replaced = Change::Replaced {
            at: at(1),
            by: "Clav".into(),
        };
        assert_eq!(
            since(&bench.device.state, &bench.workspace),
            [found(id, replaced)],
            "two slots hold it"
        );
    }

    #[test]
    fn a_slot_edited_under_its_name_has_changed_though_a_copy_is_elsewhere() {
        let mut bench = Bench::new();
        let slots = [Some(("A", crc(&gained("40")))), Some(("A", crc(&body())))];
        walked_named(&mut bench, &slots);
        let id = linked(&mut bench, "A.ne5p", body(), 1, &body());
        assert_eq!(
            since(&bench.device.state, &bench.workspace),
            [found(id, Change::Changed(at(1)))]
        );
    }

    /// A file named `file`, linked to slot 1 while the slot was called `slot` or, for
    /// `None`, by a link that kept no name, whose slot was then edited under `now`.
    fn edited_under(file: &str, slot: Option<&str>, now: &str) -> Option<Change> {
        let mut bench = Bench::new();
        walked_named(&mut bench, &[Some((now, crc(&gained("40"))))]);
        let id = linked(&mut bench, file, body(), 1, &body());
        let keyboard = bench.device.state.keyboard().cloned().unwrap();
        if let Some(slot) = slot {
            let held = Held::new(PROGRAM, at(1), slot, print(&body())).unwrap();
            bench.workspace.link(id, &keyboard, held);
        }
        let entity = bench.workspace.get(id).unwrap();
        let held = entity.held_on(&keyboard).unwrap();
        change(
            &bench.device.state,
            &bench.workspace,
            &keyboard,
            entity,
            held,
        )
    }

    #[test]
    fn a_slot_edited_under_the_name_it_had_when_last_agreed_has_changed() {
        let changed = Some(Change::Changed(at(1)));
        assert_eq!(edited_under("Renamed.ne5p", Some("Tine"), "Tine"), changed);
        assert_eq!(edited_under("Tine.ne5p", None, "Tine"), changed);
    }

    #[test]
    fn a_slot_now_under_another_name_was_replaced() {
        let replaced = |by: &str| {
            Some(Change::Replaced {
                at: at(1),
                by: by.into(),
            })
        };
        assert_eq!(
            edited_under("Tine.ne5p", Some("Tine"), "Clav"),
            replaced("Clav")
        );
        assert_eq!(
            edited_under("Renamed.ne5p", None, "Tine"),
            replaced("Tine"),
            "a link that kept no name goes by the file's"
        );
    }

    #[test]
    fn two_linked_sounds_that_swapped_slots_have_both_moved() {
        let mut bench = Bench::new();
        let (a, b) = (body(), gained("40"));
        walked_named(&mut bench, &[Some(("B", crc(&b))), Some(("A", crc(&a)))]);
        let first = linked(&mut bench, "A.ne5p", a.clone(), 1, &a);
        let second = linked(&mut bench, "B.ne5p", b.clone(), 2, &b);
        let moved = |from, to| Change::Moved {
            from: at(from),
            to: at(to),
            agrees: true,
        };
        assert_eq!(
            since(&bench.device.state, &bench.workspace),
            [found(first, moved(1, 2)), found(second, moved(2, 1))]
        );
        heard(&mut bench);
        bring_in(&mut bench.device, &mut bench.workspace, &mut bench.log);
        assert_eq!(slot_of(&bench, first), Some(Spot::at(at(2))));
        assert_eq!(slot_of(&bench, second), Some(Spot::at(at(1))));
    }

    #[test]
    fn a_sound_two_files_would_move_to_has_moved_for_neither() {
        let mut bench = Bench::new();
        let slots = [
            Some(("Clav", crc(&gained("40")))),
            None,
            Some(("C", crc(&body()))),
        ];
        walked_named(&mut bench, &slots);
        let replaced = linked(&mut bench, "A.ne5p", body(), 1, &body());
        linked(&mut bench, "B.ne5p", body(), 2, &body());
        let by = "Clav".into();
        assert_eq!(
            since(&bench.device.state, &bench.workspace),
            [found(replaced, Change::Replaced { at: at(1), by })]
        );
    }

    #[test]
    fn a_replaced_sound_is_unlinked_by_bring_them_in_and_never_pulled() {
        let mut bench = Bench::new();
        walked_named(&mut bench, &[Some(("Clav", crc(&gained("40"))))]);
        let id = linked(&mut bench, "Tine.ne5p", body(), 1, &body());
        heard(&mut bench);
        assert!(bench.device.asks.since.is_some());
        bring_in(&mut bench.device, &mut bench.workspace, &mut bench.log);
        assert_eq!(slot_of(&bench, id), None);
        assert!(pulls(&bench).is_empty());
        assert_eq!(bench.workspace.get(id).unwrap().bytes.to_vec(), body());
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
    fn keyboard_changes_asks_again_after_not_now_while_anything_changed() {
        let mut bench = Bench::new();
        walked(&mut bench, &[Some(crc(&gained("40")))]);
        let id = linked(&mut bench, "A.ne5p", body(), 1, &body());
        heard(&mut bench);
        bench.device.asks.since = None;
        bench.act(vec![crate::browser::Act::KeyboardChanges]);
        let asked = Some(vec![found(id, Change::Changed(at(1)))]);
        assert_eq!(bench.device.asks.since, asked);

        bench.device.asks.since = None;
        walked(&mut bench, &[Some(crc(&body()))]);
        bench.act(vec![crate::browser::Act::KeyboardChanges]);
        assert_eq!(bench.device.asks.since, None, "nothing to ask");
    }

    #[test]
    fn the_choice_says_differs_for_unnamed_bytes_and_cancel_asks_nothing() {
        use crate::testing::{click, run, screen, where_};
        let mut bench = Bench::new();
        let id = linked_off(&mut bench);
        let choosing = Choosing {
            id,
            class: PROGRAM,
            at: at(1),
            diff: Diff::Bytes { first_at: 0x40 },
            there: None,
        };
        let ctx = crate::testing::context();
        let size = egui::vec2(800.0, 600.0);
        let mut answer = None;
        let mut frame = |events| {
            run(&ctx, screen(size, events), |ctx| {
                answer = answer.or(choice_sheet(
                    ctx,
                    &choosing,
                    &bench.workspace,
                    &bench.device.state,
                ));
            })
        };
        frame(Vec::new());
        let said = crate::testing::painted(&frame(Vec::new()));
        let words: Vec<&str> = said.iter().map(|word| word.text.as_str()).collect();
        assert!(words.contains(&DIFFERS), "{words:?}");
        assert!(!words.iter().any(|word| word.contains("0x")), "{words:?}");
        let cancel = where_(&said, "Cancel").center();
        frame(click(cancel));
        frame(Vec::new());
        assert_eq!(answer, Some(Answer::No));
    }

    /// A file on this computer the choice can name.
    fn linked_off(bench: &mut Bench) -> u64 {
        let origin = Origin::File("A.ne5p".into());
        let log = &mut bench.log;
        bench.workspace.ingest("A.ne5p".into(), origin, body(), log)
    }

    #[test]
    fn once_read_the_files_ahead_of_their_slots_are_queued_in_one_line_undo_takes_back() {
        let mut bench = Bench::new();
        walked(&mut bench, &[Some(crc(&body())), Some(crc(&body()))]);
        let ahead = linked(&mut bench, "A.ne5p", gained("40"), 1, &body());
        let also = linked(&mut bench, "B.ne5p", gained("41"), 2, &body());
        heard(&mut bench);
        assert!(bench.queue.holds(ahead) && bench.queue.holds(also));
        assert!(bench.log.press("2 sounds are waiting to send."));
        bench.log.undo_pressed(
            &mut bench.workspace,
            &mut bench.queue,
            &mut bench.device.asks,
        );
        assert!(bench.queue.is_empty());
        bench.device.pretend(DeviceEvent::Finished);
        heard(&mut bench);
        assert!(bench.queue.is_empty(), "once per attach");
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
                    let keyboard = bench.device.state.keyboard().cloned().unwrap();
                    let entity = bench.workspace.get(id).unwrap();
                    let held = entity.held_on(&keyboard).unwrap();
                    assert!(held.knew(crc(&theirs)), "the keyboard's counts as earlier");
                    let unchosen = Held::new(PROGRAM, at(1), "", print(&body())).unwrap();
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
            bench.log.undo_pressed(
                &mut bench.workspace,
                &mut bench.queue,
                &mut bench.device.asks,
            );
            pressed
        };
        assert!(undo(&mut bench, "is now at 1:3"));
        assert_eq!(slot_of(&bench, moved), Some(Spot::at(at(2))));
        assert!(!undo(&mut bench, "is now at 1:3"), "once");

        assert!(undo(&mut bench, "Brought in"));
        assert_eq!(bench.workspace.get(changed).unwrap().bytes.to_vec(), body());
        assert_eq!(
            standing(&bench, changed),
            Some(Standing::Theirs),
            "the link is as it was too"
        );
        heard(&mut bench);
        assert_eq!(pulls(&bench), [at(1)], "not brought in again");
        assert!(!undo(&mut bench, "Brought in"), "once");
    }

    #[test]
    fn each_later_bringing_in_has_its_own_line_and_undo() {
        let mut bench = Bench::new();
        let (one, two) = (gained("40"), gained("41"));
        walked(&mut bench, &[Some(crc(&one)), Some(crc(&two))]);
        let first = linked(&mut bench, "A.ne5p", body(), 1, &body());
        let second = linked(&mut bench, "B.ne5p", body(), 2, &body());
        heard(&mut bench);
        bring_in(&mut bench.device, &mut bench.workspace, &mut bench.log);
        got(&mut bench, 1, one.clone(), Purpose::Pull);
        got(&mut bench, 2, two.clone(), Purpose::Pull);
        assert!(bench.log.transcript().contains("Brought in 2 sounds"));

        let three = gained("42");
        walked(&mut bench, &[Some(crc(&three)), Some(crc(&two))]);
        bench.device.pretend(DeviceEvent::Finished);
        heard(&mut bench);
        got(&mut bench, 1, three.clone(), Purpose::Pull);
        assert_eq!(bench.workspace.get(first).unwrap().bytes.to_vec(), three);
        assert!(bench.log.press("Brought in “A”"));
        bench.log.undo_pressed(
            &mut bench.workspace,
            &mut bench.queue,
            &mut bench.device.asks,
        );
        assert_eq!(bench.workspace.get(first).unwrap().bytes.to_vec(), one);
        assert_eq!(bench.workspace.get(second).unwrap().bytes.to_vec(), two);
        assert!(
            bench.log.press("Brought in 2 sounds"),
            "the first batch keeps its undo"
        );
    }

    #[test]
    fn a_pull_lands_only_on_a_file_still_as_its_slot_last_agreed() {
        let mut bench = Bench::new();
        let one = gained("40");
        walked(&mut bench, &[Some(crc(&one))]);
        let id = linked(&mut bench, "A.ne5p", body(), 1, &body());
        heard(&mut bench);
        bring_in(&mut bench.device, &mut bench.workspace, &mut bench.log);
        let log = &mut bench.log;
        bench.workspace.replace_bytes(id, gained("60"), log);
        bench.workspace.mark_saved(id);
        got(&mut bench, 1, one, Purpose::Pull);
        assert_eq!(
            bench.workspace.get(id).unwrap().bytes.to_vec(),
            gained("60")
        );
        assert!(standing(&bench, id).is_some_and(Standing::needs_attention));
    }

    #[test]
    fn nothing_is_pulled_while_a_send_waits() {
        let mut bench = Bench::new();
        walked(&mut bench, &[Some(crc(&gained("40"))), None]);
        let id = linked(&mut bench, "A.ne5p", body(), 1, &body());
        let log = &mut bench.log;
        let other = bench
            .workspace
            .ingest("B.ne5p".into(), Origin::Fresh, gained("41"), log);
        heard(&mut bench);
        bench.act(vec![crate::browser::Act::Send {
            id: other,
            class: PROGRAM,
            at: at(2),
        }]);
        bench.act(vec![crate::browser::Act::SendAll]);
        assert!(bench.device.sending());
        bring_in(&mut bench.device, &mut bench.workspace, &mut bench.log);
        heard(&mut bench);
        assert!(pulls(&bench).is_empty());
        assert_eq!(standing(&bench, id), Some(Standing::Theirs));
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
        bench.log.undo_pressed(
            &mut bench.workspace,
            &mut bench.queue,
            &mut bench.device.asks,
        );
        heard(&mut bench);
        let keyboard = bench.device.state.keyboard().cloned().unwrap();
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
        bench.log.undo_pressed(
            &mut bench.workspace,
            &mut bench.queue,
            &mut bench.device.asks,
        );
        assert!(!bench.queue.holds(id));
        assert_eq!(standing(&bench, id), Some(Standing::Ahead), "still saved");
    }

    fn undo(bench: &mut Bench, line: &str) -> String {
        assert!(bench.log.press(line), "{line} has an Undo");
        bench.log.undo_pressed(
            &mut bench.workspace,
            &mut bench.queue,
            &mut bench.device.asks,
        );
        bench.log.status().1.to_string()
    }

    fn brought_in(bench: &mut Bench, theirs: &[u8]) -> u64 {
        walked(bench, &[Some(crc(theirs))]);
        let id = linked(bench, "A.ne5p", body(), 1, &body());
        heard(bench);
        bring_in(&mut bench.device, &mut bench.workspace, &mut bench.log);
        got(bench, 1, theirs.to_vec(), Purpose::Pull);
        assert_eq!(bench.workspace.get(id).unwrap().bytes.to_vec(), theirs);
        id
    }

    #[test]
    fn undoing_a_bringing_in_leaves_a_file_changed_since_and_says_so() {
        let mut bench = Bench::new();
        let id = brought_in(&mut bench, &gained("40"));
        let log = &mut bench.log;
        bench.workspace.replace_bytes(id, gained("60"), log);
        bench.workspace.mark_saved(id);
        let said = undo(&mut bench, "Brought in");
        assert_eq!(said, "Not undone: “A” changed since.");
        let bytes = bench.workspace.get(id).unwrap().bytes.to_vec();
        assert_eq!(bytes, gained("60"));
    }

    #[test]
    fn a_file_brought_in_then_sent_is_brought_in_again_when_the_keyboard_changes_it() {
        let mut bench = Bench::new();
        let id = brought_in(&mut bench, &gained("40"));
        bench.device.pretend(DeviceEvent::Sent {
            id,
            class: PROGRAM,
            at: at(1),
            name: "A.ne5p".into(),
            sent: crate::device::Payload::Bytes(gained("40")),
        });
        heard(&mut bench);
        let three = gained("42");
        walked(&mut bench, &[Some(crc(&three))]);
        bench.device.pretend(DeviceEvent::Finished);
        heard(&mut bench);
        assert_eq!(pulls(&bench), [at(1), at(1)]);
        got(&mut bench, 1, three.clone(), Purpose::Pull);
        assert_eq!(bench.workspace.get(id).unwrap().bytes.to_vec(), three);
    }

    #[test]
    fn bring_them_in_asked_again_brings_in_a_file_whose_bringing_in_was_undone() {
        let mut bench = Bench::new();
        let one = gained("40");
        let id = brought_in(&mut bench, &one);
        assert_eq!(undo(&mut bench, "Brought in"), "Put back “A”.");
        heard(&mut bench);
        assert_eq!(pulls(&bench), [at(1)], "not on its own");
        assert!(bench.device.asks.changed());

        bench.act(vec![crate::browser::Act::KeyboardChanges]);
        bring_in(&mut bench.device, &mut bench.workspace, &mut bench.log);
        assert_eq!(pulls(&bench), [at(1), at(1)]);
        got(&mut bench, 1, one.clone(), Purpose::Pull);
        assert_eq!(bench.workspace.get(id).unwrap().bytes.to_vec(), one);
    }

    #[test]
    fn a_pull_the_user_answered_before_it_landed_does_not_land() {
        let mut bench = Bench::new();
        let theirs = gained("40");
        walked(&mut bench, &[Some(crc(&theirs))]);
        let id = linked(&mut bench, "A.ne5p", body(), 1, &body());
        heard(&mut bench);
        bring_in(&mut bench.device, &mut bench.workspace, &mut bench.log);
        choose(&mut bench.device, &bench.workspace, &mut bench.log, id);
        got(&mut bench, 1, theirs.clone(), Purpose::Choose);
        let Bench {
            device,
            workspace,
            queue,
            log,
            ..
        } = &mut bench;
        use_mine(device, workspace, queue, log);
        got(&mut bench, 1, theirs, Purpose::Pull);
        assert_eq!(bench.workspace.get(id).unwrap().bytes.to_vec(), body());
        assert_eq!(standing(&bench, id), Some(Standing::Ahead));
    }

    #[test]
    fn a_swap_brought_in_is_one_line_whose_undo_puts_both_links_back() {
        let mut bench = Bench::new();
        let (a, b) = (body(), gained("40"));
        walked_named(&mut bench, &[Some(("B", crc(&b))), Some(("A", crc(&a)))]);
        let first = linked(&mut bench, "A.ne5p", a.clone(), 1, &a);
        let second = linked(&mut bench, "B.ne5p", b.clone(), 2, &b);
        heard(&mut bench);
        bring_in(&mut bench.device, &mut bench.workspace, &mut bench.log);
        let line = "2 files follow their sounds to other slots.";
        assert_eq!(bench.log.transcript().matches(line).count(), 1);
        assert_eq!(undo(&mut bench, line), "Linked 2 files again.");
        assert_eq!(slot_of(&bench, first), Some(Spot::at(at(1))));
        assert_eq!(slot_of(&bench, second), Some(Spot::at(at(2))));
    }

    /// Two files whose slots hold other sounds now, unlinked by Bring them in.
    fn replaced(bench: &mut Bench) -> [u64; 2] {
        let slots = [
            Some(("Clav", crc(&gained("40")))),
            Some(("Wurl", crc(&gained("41")))),
        ];
        walked_named(bench, &slots);
        let first = linked(bench, "Tine.ne5p", body(), 1, &body());
        let second = linked(bench, "Rhodes.ne5p", body(), 2, &body());
        heard(bench);
        bring_in(&mut bench.device, &mut bench.workspace, &mut bench.log);
        assert_eq!(
            (slot_of(bench, first), slot_of(bench, second)),
            (None, None)
        );
        [first, second]
    }

    #[test]
    fn the_files_bring_them_in_unlinks_share_one_line_whose_undo_links_them_back() {
        let mut bench = Bench::new();
        let [first, second] = replaced(&mut bench);
        let line = "2 files unlinked from the keyboard.";
        assert_eq!(undo(&mut bench, line), "Linked 2 files again.");
        assert_eq!(slot_of(&bench, first), Some(Spot::at(at(1))));
        assert_eq!(slot_of(&bench, second), Some(Spot::at(at(2))));
    }

    #[test]
    fn undoing_an_unlink_never_unlinks_a_file_linked_there_since() {
        let mut bench = Bench::new();
        let [first, second] = replaced(&mut bench);
        let since = linked(&mut bench, "Wurl.ne5p", gained("41"), 2, &gained("41"));
        let said = undo(&mut bench, "2 files unlinked from the keyboard.");
        assert_eq!(said, "Not undone: “Wurl” is linked to Programs 1:2 now.");
        assert_eq!(slot_of(&bench, since), Some(Spot::at(at(2))));
        assert_eq!(
            (slot_of(&bench, first), slot_of(&bench, second)),
            (None, None)
        );
    }

    #[test]
    fn queueing_on_attach_leaves_a_slot_something_waits_for_and_undoes_only_its_own() {
        use crate::browser::Act;
        let mut bench = Bench::new();
        walked(&mut bench, &[Some(crc(&body())), Some(crc(&body())), None]);
        let waited = linked(&mut bench, "A.ne5p", gained("40"), 1, &body());
        let ahead = linked(&mut bench, "B.ne5p", gained("41"), 2, &body());
        let log = &mut bench.log;
        let other = bench
            .workspace
            .ingest("C.ne5p".into(), Origin::Fresh, gained("42"), log);
        let send = |id, slot| Act::Send {
            id,
            class: PROGRAM,
            at: at(slot),
        };
        bench.act(vec![send(other, 1)]);
        heard(&mut bench);
        assert!(!bench.queue.holds(waited), "1:1 keeps what waits for it");
        assert_eq!(bench.queue.entry(ahead).map(|held| held.at), Some(at(2)));

        bench.act(vec![send(ahead, 3)]);
        let said = undo(&mut bench, "1 sound is waiting to send.");
        assert_eq!(said, "Not undone: the queue changed since.");
        assert_eq!(bench.queue.entry(ahead).map(|held| held.at), Some(at(3)));
        assert!(bench.queue.holds(other));
    }

    #[test]
    fn queueing_on_attach_counts_and_undoes_the_piano_a_program_takes_along() {
        let mut bench = Bench::new();
        walked(&mut bench, &[Some(crc(&body()))]);
        let piano = ObjectClass::Piano;
        bench
            .device
            .pretend_geometry(piano, &[("Grand", 1), ("Upright", 1)]);
        for bank in [1, 2] {
            bench.device.pretend_lengths(piano, bank, &[None]);
        }
        let log = &mut bench.log;
        let tine = bench.workspace.ingest(
            "Tine.npno".into(),
            Origin::Fresh,
            crate::testing::piano(1),
            log,
        );
        let key = crate::plays::slot_key(piano, "Tine").1;
        let plays = [("piano_panel.id".into(), key.to_string())];
        let keys = crate::fields::apply(&body(), &plays).unwrap().1;
        let program = linked(&mut bench, "A.ne5p", keys, 1, &body());
        heard(&mut bench);
        assert!(bench.queue.holds(program) && bench.queue.holds(tine));
        assert_eq!(
            undo(&mut bench, "2 sounds are waiting to send."),
            "Took 2 sounds out of the queue."
        );
        assert!(bench.queue.is_empty());
    }

    #[test]
    fn a_save_that_queues_nothing_new_offers_no_undo() {
        let mut bench = Bench::new();
        walked(&mut bench, &[Some(crc(&body()))]);
        let id = linked(&mut bench, "A.ne5p", body(), 1, &body());
        crate::queue::enqueue(
            &bench.workspace,
            &mut bench.device,
            &mut bench.queue,
            &mut bench.log,
            id,
            PROGRAM,
            at(1),
        );
        bench.device.pretend_serial("B2");
        let keyboard = bench.device.state.keyboard().cloned().unwrap();
        let held = Held::new(PROGRAM, at(1), "", print(&body())).unwrap();
        bench.workspace.link(id, &keyboard, held);
        let log = &mut bench.log;
        bench.workspace.replace_bytes(id, gained("40"), log);
        let undos = |bench: &Bench| bench.log.iter().filter(|it| it.undo.is_some()).count();
        let before = undos(&bench);
        bench.act(vec![crate::browser::Act::SaveDoc(id)]);
        assert!(bench.queue.holds(id));
        assert_eq!(undos(&bench), before);
    }

    #[test]
    fn the_question_waits_for_every_linked_file_to_be_read_or_found_unreadable() {
        let mut bench = Bench::new();
        walked(&mut bench, &[Some(crc(&gained("40"))), Some(crc(&body()))]);
        let keyboard = bench.device.state.keyboard().cloned().unwrap();
        let ids = [("A.ne5p", 1), ("B.ne5p", 2)].map(|(name, slot)| {
            let id = bench.workspace.next_id();
            let saved = crate::workspace::Saved {
                id,
                name: name.into(),
                path: None,
                origin: Origin::File(name.into()),
                content: crate::workspace::Content::unread(body().len() as u64),
                unsaved: None,
            };
            bench.workspace.restore(vec![saved], None, &mut bench.log);
            let held = Held::new(PROGRAM, at(slot), "", print(&body())).unwrap();
            bench.workspace.link(id, &keyboard, held);
            id
        });
        heard(&mut bench);
        assert!(bench.device.asks.since.is_none());
        assert!(
            ids.iter().all(|id| bench.workspace.wanted(*id)),
            "asked for"
        );

        bench.workspace.took(ids[0], Some(body()), None);
        bench.workspace.settle_files(&mut bench.log);
        heard(&mut bench);
        assert!(bench.device.asks.since.is_none(), "B is not read yet");

        bench
            .workspace
            .unreadable(ids[1], "gone".into(), &mut bench.log);
        heard(&mut bench);
        let asked = Some(vec![found(ids[0], Change::Changed(at(1)))]);
        assert_eq!(bench.device.asks.since, asked);
    }

    #[test]
    fn the_question_offers_to_bring_in_only_a_list_with_a_changed_slot() {
        use crate::testing::{painted, run, screen};
        let mut bench = Bench::new();
        let id = linked_off(&mut bench);
        let ctx = crate::testing::context();
        let words = |found: &[Noticed]| {
            let frame = || {
                let size = egui::vec2(800.0, 600.0);
                run(&ctx, screen(size, Vec::new()), |ctx| {
                    since_sheet(ctx, found, &bench.workspace);
                })
            };
            frame();
            let said = painted(&frame());
            said.into_iter().map(|word| word.text).collect::<Vec<_>>()
        };
        let by = "Clav".to_string();
        let replaced = [found(id, Change::Replaced { at: at(1), by })];
        let said = words(&replaced);
        assert!(said.iter().any(|it| it == UPDATE_LINKS), "{said:?}");
        assert!(!said.iter().any(|it| it == BRING_THEM_IN), "{said:?}");
        let said = words(&[found(id, Change::Changed(at(1)))]);
        assert!(said.iter().any(|it| it == BRING_THEM_IN), "{said:?}");
    }
}
