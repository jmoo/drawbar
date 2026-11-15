//! Sync: what differs between this computer and the attached keyboard, one row per sound,
//! and what pressing Sync does with each row. Bringing a slot in reads it and saves what
//! it holds as the file linked to it.

use std::collections::{BTreeMap, BTreeSet};

use nord_usb::{Location, ObjectClass};

use crate::device::{
    occupied, slot_label, standing_of, there, Device, DeviceCmd, DeviceState, Purpose,
};
use crate::link::{Held, Keyboard, Spot, Standing, There};
use crate::log::{Log, Restored, Undo};
use crate::queue::{compare, Diff, Queue};
use crate::strings::{counted, display_name, shown};
use crate::workspace::{print_of, LocalEntity, VerifyState, Workspace};

/// What one row of the sheet says differs.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Change {
    /// Sent by hand to a slot it is not linked to.
    Placed,
    /// The slot holds an earlier version of the file.
    Ahead,
    /// The slot holds a version the file never had, and the file has not changed.
    Theirs,
    /// The slot holds a version the file never had, and the file has changed too.
    Both,
    /// The sound is in another slot of its class, which no file is linked to, or which
    /// holds the sound of the file linked there while this slot holds that one's.
    /// `agrees` where that slot holds the file's saved version.
    Moved { to: Location, agrees: bool },
    /// The slot holds another sound, named `by`, and the file's sound is not in exactly
    /// one other slot.
    Replaced { by: String },
    /// The slot is empty, and the sound is not in exactly one other slot.
    Removed,
}

impl Change {
    /// Whether the keyboard's side changed, which opens the sheet on attach.
    fn on_the_keyboard(&self) -> bool {
        !matches!(self, Change::Placed | Change::Ahead)
    }
}

/// One sound the sheet lists.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Row {
    pub id: u64,
    pub class: ObjectClass,
    /// The slot. A piano or sample removed from the keyboard has none.
    pub at: Option<Location>,
    pub change: Change,
}

/// Which way Sync takes a row: the file to the keyboard, or the keyboard's side to the
/// file, which for a moved, replaced or removed sound is its link.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Way {
    Up,
    Down,
}

impl Row {
    /// The ways the row may go. A click on a row with no way yet picks the first.
    pub fn ways(&self, workspace: &Workspace) -> Vec<Way> {
        let down = self.down(workspace).is_none();
        match (&self.change, self.at) {
            (Change::Placed, _) => vec![Way::Up],
            (Change::Ahead, _) if down => vec![Way::Up, Way::Down],
            (Change::Theirs | Change::Both, _) if down => vec![Way::Down, Way::Up],
            (Change::Ahead | Change::Theirs | Change::Both, _) => vec![Way::Up],
            (Change::Removed, Some(_)) => vec![Way::Up, Way::Down],
            (Change::Removed | Change::Moved { .. } | Change::Replaced { .. }, _) => {
                vec![Way::Down]
            }
        }
    }

    /// The way Sync takes the row unless the user picks one.
    ///
    /// ⚠️ A sound changed on both sides, and one removed, take none: only the user
    /// settles them.
    pub fn taken(&self, workspace: &Workspace) -> Option<Way> {
        match self.change {
            Change::Both | Change::Removed => None,
            Change::Theirs if self.down(workspace).is_some() => None,
            _ => self.ways(workspace).first().copied(),
        }
    }

    /// Why the keyboard's version cannot be brought into the file, for a row that holds
    /// one on each side.
    pub fn down(&self, workspace: &Workspace) -> Option<&'static str> {
        let entity = workspace.get(self.id)?;
        match () {
            _ if self.class.is_library() => Some(COPY_INSTEAD),
            _ if entity.is_unsaved() => Some(SAVE_FIRST),
            _ => None,
        }
    }
}

/// Why a piano or sample is never brought in: its slot is never read whole.
const COPY_INSTEAD: &str = "Copy it to this computer instead.";

/// Why a file with unsaved edits is never brought in over.
const SAVE_FIRST: &str = "Save your edits first.";

/// Every sound Sync would list: each send placed by hand, then each linked file whose slot
/// differs from it. A send waiting for the file's own slot is its linked row, unless that
/// slot is empty. A sound two files would move to has moved for neither.
pub fn rows(state: &DeviceState, workspace: &Workspace, queue: &Queue) -> Vec<Row> {
    let Some(keyboard) = state.keyboard() else {
        return Vec::new();
    };
    let own = |id, class, at| {
        let linked = workspace.get(id).and_then(|it| standing_of(state, it));
        linked.is_some_and(|linked| {
            (linked.class, linked.at) == (class, Some(at)) && linked.standing != Standing::Gone
        })
    };
    let placed: Vec<Row> = queue
        .entries()
        .iter()
        .filter(|held| !own(held.id, held.class, held.at))
        .map(|held| Row {
            id: held.id,
            class: held.class,
            at: Some(held.at),
            change: Change::Placed,
        })
        .collect();
    let linked: Vec<Row> = workspace
        .listed()
        .filter(|entity| !placed.iter().any(|row| row.id == entity.id))
        .filter_map(|entity| linked_row(state, workspace, keyboard, entity))
        .collect();
    let target = |row: &Row| match row.change {
        Change::Moved { to, .. } => Some((row.class, to)),
        _ => None,
    };
    let contested = |row: &Row| {
        let mut claims = linked.iter().filter(|other| target(other) == target(row));
        target(row).is_some() && claims.nth(1).is_some()
    };
    let unmoved = |row: &Row| match row.at.and_then(|at| state.slot(row.class, at).flatten()) {
        Some(info) => Change::Replaced {
            by: info.name.trim().to_string(),
        },
        None => Change::Removed,
    };
    let linked = linked.iter().map(|row| match contested(row) {
        false => row.clone(),
        true => Row {
            change: unmoved(row),
            ..row.clone()
        },
    });
    placed
        .into_iter()
        .chain(linked.collect::<Vec<_>>())
        .collect()
}

/// The row of a file linked on the attached keyboard, unless it and its slot agree.
fn linked_row(
    state: &DeviceState,
    workspace: &Workspace,
    keyboard: &Keyboard,
    entity: &LocalEntity,
) -> Option<Row> {
    let linked = standing_of(state, entity)?;
    let held = entity.held_on(keyboard)?;
    let change = match linked.standing {
        Standing::Same | Standing::Unread => return None,
        Standing::Ahead => Change::Ahead,
        Standing::Theirs | Standing::Both | Standing::Gone => {
            change(state, workspace, keyboard, entity, held).unwrap_or(match linked.standing {
                Standing::Theirs => Change::Theirs,
                Standing::Both => Change::Both,
                _ => Change::Removed,
            })
        }
    };
    Some(Row {
        id: entity.id,
        class: linked.class,
        at: linked.at.or(held.location()),
        change,
    })
}

/// Where a linked file's sound went, judged by what its slot holds now and whether it
/// holds it under the name it had when the two last agreed. A link that kept no name
/// goes by the file's. `None` where the slot holds the sound under that name, or where
/// this cannot be judged.
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
        return (there == There::Vacant).then_some(Change::Removed);
    };
    let by = match there {
        There::Unread | There::Holds { crc: None, .. } => return None,
        There::Holds { crc: Some(crc), .. } if held.knew(crc) || crc == here.crc => return None,
        There::Vacant => None,
        There::Holds { .. } => Some(name_at(state, held.class(), from)),
    };
    let agreed = held.name().map(str::to_string);
    if by.is_some() && by == agreed.or_else(|| slot_label(&entity.name)) {
        return None;
    }
    let ours = |crc: u32| crc == held.base.crc || crc == here.crc;
    let mut elsewhere = occupied(state, held.class())
        .filter(|(at, info)| *at != from && info.crc32.is_some_and(ours))
        .map(|(at, info)| (at, info.crc32 == Some(here.crc)));
    let unique = match (elsewhere.next(), elsewhere.next()) {
        (Some(to), None) => Some(to),
        (None, None) if by.is_none() => return Some(Change::Removed),
        _ => None,
    };
    let to = unique.filter(|(to, _)| movable(state, workspace, keyboard, held, from, *to));
    match (to, by) {
        (Some((to, agrees)), _) => Some(Change::Moved { to, agrees }),
        (None, Some(by)) => Some(Change::Replaced { by }),
        (None, None) => None,
    }
}

/// The name the keyboard keeps the slot `at` of `class` under.
fn name_at(state: &DeviceState, class: ObjectClass, at: Location) -> String {
    let info = state.slot(class, at).flatten();
    info.map_or("", |info| info.name.trim()).to_string()
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

/// Whether the walk of every class holding a file linked on the attached keyboard has
/// finished.
fn walked(state: &DeviceState, workspace: &Workspace) -> bool {
    let (Some(keyboard), classes) = (state.keyboard(), state.classes()) else {
        return false;
    };
    !classes.is_empty()
        && workspace
            .listed()
            .filter_map(|entity| Some(entity.held_on(keyboard)?.class()))
            .filter(|class| classes.contains(class))
            .all(|class| state.read_whole(class) && !running(state, class))
}

fn running(state: &DeviceState, class: ObjectClass) -> bool {
    state.scan.progress(class).is_some_and(|read| read.running)
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

/// A slot read to be brought into the file linked to it.
struct Pull {
    class: ObjectClass,
    at: Location,
    id: u64,
    /// The file's link when Sync was pressed. The bytes land only on a file still linked
    /// so.
    held: Held,
}

/// The sheet, while it shows.
pub struct Sheet {
    /// The classes read for it, until each walk has finished.
    reading: Vec<ObjectClass>,
    pub picked: Option<u64>,
    /// The ways the user picked, for this showing only.
    ways: BTreeMap<u64, Way>,
    /// The rows the user left out of this Sync.
    skipped: BTreeSet<u64>,
    /// How the picked row's file differs from its slot, once the slot is read.
    pub shown: Option<Shown>,
}

/// A linked row's slot as read for the sheet.
pub struct Shown {
    pub id: u64,
    pub class: ObjectClass,
    pub at: Location,
    /// A piano or sample is never read whole, so its difference stays [`Diff::Checksum`].
    pub diff: Diff,
}

impl Sheet {
    fn new(reading: Vec<ObjectClass>, picked: Option<u64>) -> Sheet {
        Sheet {
            reading,
            picked,
            ways: BTreeMap::new(),
            skipped: BTreeSet::new(),
            shown: None,
        }
    }

    /// Whether every walk asked for when it opened has finished: its rows are what the
    /// keyboard holds since then.
    pub fn read(&self, state: &DeviceState) -> bool {
        !self.reading.iter().any(|class| running(state, *class))
    }

    /// The way Sync takes `row`: the one the user picked, or else its own.
    pub fn way(&self, row: &Row, workspace: &Workspace) -> Option<Way> {
        if self.skipped.contains(&row.id) {
            return None;
        }
        let picked = self.ways.get(&row.id).copied();
        let picked = picked.filter(|way| row.ways(workspace).contains(way));
        picked.or_else(|| row.taken(workspace))
    }

    /// Turn `row` to its next way.
    pub fn flip(&mut self, row: &Row, workspace: &Workspace) {
        let ways = row.ways(workspace);
        let next = match self.way(row, workspace) {
            Some(way) => ways.iter().find(|it| **it != way).copied(),
            None => ways.first().copied(),
        };
        self.skipped.remove(&row.id);
        if let Some(next) = next {
            self.ways.insert(row.id, next);
        }
    }

    pub fn skip(&mut self, id: u64) {
        self.skipped.insert(id);
    }
}

/// The sheet and what Sync is bringing in, while one keyboard is attached. A new attach
/// starts it over.
#[derive(Default)]
pub struct Asks {
    /// Whether this attach's walk was looked at for the sheet to open by itself.
    settled: bool,
    pub sheet: Option<Sheet>,
    /// Slots read to be brought in, until their bytes land. A read that fails, or lands
    /// on a file whose bringing in was refused, stays, so it is not asked for again.
    pulling: Vec<Pull>,
    /// The log line counting the slots the last Sync brought in.
    brought: Option<u64>,
}

impl Asks {
    /// The bytes read for the picked row.
    pub fn read(&mut self, class: ObjectClass, at: Location, bytes: &[u8], workspace: &Workspace) {
        let shown = self.sheet.as_mut().and_then(|sheet| sheet.shown.as_mut());
        let Some(shown) = shown.filter(|it| (it.class, it.at) == (class, at)) else {
            return;
        };
        if let Some(entity) = workspace.get(shown.id) {
            shown.diff = compare(&entity.bytes, bytes);
        }
    }

    /// A read for Sync found its slot empty.
    pub fn vacant(&mut self, class: ObjectClass, at: Location) {
        self.pulling
            .retain(|pull| (pull.class, pull.at) != (class, at));
        let shown = self.sheet.as_mut().and_then(|sheet| sheet.shown.as_mut());
        if let Some(shown) = shown.filter(|it| (it.class, it.at) == (class, at)) {
            shown.diff = Diff::Empty;
        }
    }
}

/// Read the whole keyboard, and show the sheet with `picked` picked where it is a row.
pub fn open(device: &mut Device, picked: Option<u64>) {
    if !device.state.connected() {
        return;
    }
    device.resync();
    device.asks.sheet = Some(Sheet::new(device.state.classes(), picked));
}

/// Open the sheet once per attach, once the walk allows, where the keyboard's side has
/// something to say; and close one whose read found nothing to sync, saying so. Call when
/// the instrument or the list may have changed.
///
/// ⚠️ Nothing opens while a write is running or waiting: a send changes the slots it
/// would list.
pub fn follow(device: &mut Device, workspace: &Workspace, queue: &Queue, log: &mut Log) {
    if device.sending() {
        return;
    }
    let state = &device.state;
    if let Some(sheet) = device.asks.sheet.as_ref().filter(|sheet| sheet.read(state)) {
        if !sheet.reading.is_empty() && rows(state, workspace, queue).is_empty() {
            device.asks.sheet = None;
            log.say(NOTHING);
        }
        return;
    }
    if device.asks.settled || !(walked(state, workspace) && printed(state, workspace)) {
        return;
    }
    device.asks.settled = true;
    let rows = rows(state, workspace, queue);
    if rows.iter().any(|row| row.change.on_the_keyboard()) {
        device.asks.sheet = Some(Sheet::new(Vec::new(), None));
    }
}

/// What the status line says when the keyboard and this computer agree.
const NOTHING: &str = "Nothing to sync.";

/// Ask for the picked row's slot, to show how its file differs from it.
pub fn show_picked(device: &mut Device, workspace: &Workspace, queue: &Queue, log: &mut Log) {
    let state = &device.state;
    let Some(sheet) = device.asks.sheet.as_ref().filter(|sheet| sheet.read(state)) else {
        return;
    };
    let rows = rows(state, workspace, queue);
    let picked = rows.iter().find(|row| Some(row.id) == sheet.picked);
    let Some((row, at)) = picked.and_then(|row| Some((row, row.at?))) else {
        return;
    };
    let both = matches!(row.change, Change::Ahead | Change::Theirs | Change::Both);
    let asked = sheet.shown.as_ref();
    if !both || asked.is_some_and(|it| (it.id, it.class, it.at) == (row.id, row.class, at)) {
        return;
    }
    let (id, class) = (row.id, row.class);
    let diff = match class.is_library() {
        true => Diff::Checksum,
        false => {
            let why = Purpose::Compare;
            device.send(DeviceCmd::Get { class, at, why }, log);
            Diff::Pending
        }
    };
    if let Some(sheet) = device.asks.sheet.as_mut() {
        sheet.shown = Some(Shown {
            id,
            class,
            at,
            diff,
        });
    }
}

/// Sync, pressed: each row goes the way the sheet shows, and the sheet closes. Returns
/// whether it ran, leaving the files to write in the queue for the caller to send.
/// Nothing runs while the sheet's read is still out.
///
/// ⚠️ The rows are worked out again from what the keyboard reported since the sheet
/// opened. A row with no way is left as it is.
pub fn apply(
    device: &mut Device,
    workspace: &mut Workspace,
    queue: &mut Queue,
    log: &mut Log,
) -> bool {
    let (Some(sheet), Some(keyboard)) =
        (device.asks.sheet.take(), device.state.keyboard().cloned())
    else {
        return false;
    };
    if !sheet.read(&device.state) {
        device.asks.sheet = Some(sheet);
        return false;
    }
    let rows = rows(&device.state, workspace, queue);
    let going: Vec<(Row, Way)> = rows
        .into_iter()
        .filter_map(|row| Some((sheet.way(&row, workspace)?, row)))
        .map(|(way, row)| (row, way))
        .collect();
    let up: Vec<&Row> = going
        .iter()
        .filter(|(_, way)| *way == Way::Up)
        .map(|(row, _)| row)
        .collect();
    for id in queue.ids() {
        if !up.iter().any(|row| row.id == id) {
            queue.forget(id);
        }
    }
    let moved: Vec<&Row> = going
        .iter()
        .filter(|(row, _)| matches!(row.change, Change::Moved { .. }))
        .map(|(row, _)| row)
        .collect();
    follow_moves(&device.state, workspace, queue, log, &keyboard, &moved);
    let unlinked: Vec<u64> = going
        .iter()
        .filter(|(row, way)| match row.change {
            Change::Replaced { .. } => true,
            Change::Removed => *way == Way::Down,
            _ => false,
        })
        .map(|(row, _)| row.id)
        .collect();
    unlink(workspace, log, &keyboard, &unlinked);
    device.asks.brought = None;
    for (row, way) in &going {
        let held = workspace.get(row.id).and_then(|it| it.held_on(&keyboard));
        let (Some(at), Some(held)) = (row.at, held.cloned()) else {
            continue;
        };
        match (way, &row.change) {
            (Way::Down, Change::Ahead | Change::Theirs | Change::Both) => {
                let (class, id) = (row.class, row.id);
                pull(
                    device,
                    log,
                    Pull {
                        class,
                        at,
                        id,
                        held,
                    },
                );
            }
            (Way::Up, Change::Theirs | Change::Both) => {
                let (_, there) = there(&device.state, &held);
                workspace.link(row.id, &keyboard, held.superseding(there));
            }
            _ => {}
        }
    }
    for row in up.iter().filter(|row| row.change != Change::Placed) {
        if let Some(at) = row.at {
            let waiting = (row.id, row.class, at);
            crate::queue::enqueue_quietly(workspace, device, queue, log, waiting);
        }
    }
    true
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

/// Unlink each file whose sound was replaced or removed on the keyboard, in one log line
/// whose Undo links them back.
fn unlink(workspace: &mut Workspace, log: &mut Log, keyboard: &Keyboard, ids: &[u64]) {
    let links: Vec<(u64, Held)> = ids
        .iter()
        .filter_map(|id| Some((*id, workspace.get(*id)?.held_on(keyboard)?.clone())))
        .collect();
    for (id, _) in &links {
        workspace.unlink(*id, keyboard);
    }
    let Some((id, _)) = links.first() else {
        return;
    };
    let name = workspace.get(*id).map_or("", |it| it.name.as_str());
    log.say(match links.len() {
        1 => format!("“{}” unlinked from the keyboard.", display_name(name)),
        n => format!("{n} files unlinked from the keyboard."),
    });
    let keyboard = keyboard.clone();
    log.offer(Undo::Relink { keyboard, links });
}

/// Move each moved sound's link to the slot it moved to, all of a class at once so a swap
/// moves both, in one log line whose Undo puts them back.
fn follow_moves(
    state: &DeviceState,
    workspace: &mut Workspace,
    queue: &mut Queue,
    log: &mut Log,
    keyboard: &Keyboard,
    rows: &[&Row],
) {
    let moved: Vec<(&Row, Location, bool, Held)> = rows
        .iter()
        .filter_map(|row| match row.change {
            Change::Moved { to, agrees } => {
                let held = workspace.get(row.id)?.held_on(keyboard)?.clone();
                Some((*row, to, agrees, held))
            }
            _ => None,
        })
        .collect();
    let mut classes: Vec<ObjectClass> = Vec::new();
    for (row, ..) in &moved {
        if !classes.contains(&row.class) {
            classes.push(row.class);
        }
    }
    let respotted: Vec<(ObjectClass, Vec<crate::workspace::Moved>)> = classes
        .into_iter()
        .map(|class| {
            let moves = moved.iter().filter(|(row, ..)| row.class == class);
            let moves =
                moves.map(|(_, to, _, held)| (held.spot.clone(), Some(Spot::at(*to)), None));
            (class, moves.collect())
        })
        .collect();
    for (class, moves) in &respotted {
        workspace.respot(keyboard, *class, moves);
    }
    for (row, to, agrees, _) in &moved {
        let entity = workspace.get(row.id);
        let saved = entity.and_then(LocalEntity::saved_print);
        let now = entity.and_then(|it| it.held_on(keyboard)).cloned();
        if let (true, Some(print), Some(now)) = (agrees, saved, now) {
            let name = name_at(state, row.class, *to);
            workspace.link(row.id, keyboard, now.agreed(print, &name));
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
    let links = moved.into_iter().map(|(row, _, _, held)| (row.id, held));
    log.offer(Undo::Relink {
        keyboard: keyboard.clone(),
        links: links.collect(),
    });
    crate::queue::unqueue_moved(workspace, queue, log, &respotted);
}

/// Save `bytes`, read from the slot `at` of `class`, as the file `id` linked to that slot
/// on the attached keyboard, and agree the link on them. Returns what it held before.
///
/// ⚠️ A file with unsaved edits is never brought in over: they would be lost.
fn bring(
    device: &mut Device,
    workspace: &mut Workspace,
    queue: &mut Queue,
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
    if queue.waiting(class, at).is_some_and(|held| held.id == id) {
        queue.forget(id);
    }
    Some(restored)
}

/// A slot read to be brought in landed. It is brought in only over the file it was read
/// for, while that file is linked as it was when Sync was pressed.
pub fn landed(
    device: &mut Device,
    workspace: &mut Workspace,
    queue: &mut Queue,
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
            && entity.is_some_and(|entity| entity.held_on(keyboard) == Some(&pull.held))
    };
    let Some(index) = device.asks.pulling.iter().position(still) else {
        return;
    };
    let id = device.asks.pulling[index].id;
    if let Some(restored) = bring(device, workspace, queue, log, (class, at, id), bytes) {
        device.asks.pulling.remove(index);
        brought(&mut device.asks, workspace, log, restored);
    }
}

/// Log a file brought in. Those one Sync brought in share one line.
fn brought(asks: &mut Asks, workspace: &Workspace, log: &mut Log, restored: Restored) {
    let counted_on = asks.brought.and_then(|serial| log.entry_mut(serial));
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
    asks.brought = log.offer(Undo::Restore(vec![restored]));
}

/// What the sheet says of two versions that differ in no field it can name.
pub const DIFFERS: &str = "differs";

#[cfg(test)]
mod tests {
    use super::*;
    use crate::browser::Act;
    use crate::device::{DeviceEvent, Expect, ELECTRO5};
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

    fn listed(bench: &Bench) -> Vec<(u64, Change)> {
        let rows = rows(&bench.device.state, &bench.workspace, &bench.queue);
        rows.into_iter().map(|row| (row.id, row.change)).collect()
    }

    fn row(bench: &Bench, id: u64) -> Row {
        let rows = rows(&bench.device.state, &bench.workspace, &bench.queue);
        rows.into_iter().find(|row| row.id == id).expect("a row")
    }

    /// Press Sync, and let its read land.
    fn open_read(bench: &mut Bench) {
        bench.act(vec![Act::Sync(None)]);
        bench.land();
    }

    /// Turn `id`'s row in the open sheet until it goes `way`.
    fn pick(bench: &mut Bench, id: u64, way: Way) {
        let row = row(bench, id);
        let sheet = bench.device.asks.sheet.as_mut().expect("open");
        for _ in 0..3 {
            if sheet.way(&row, &bench.workspace) == Some(way) {
                return;
            }
            sheet.flip(&row, &bench.workspace);
        }
        panic!("{row:?} cannot go {way:?}");
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

    /// Each write asked for, with what it expects its slot to hold.
    fn written(bench: &Bench) -> Vec<(u64, Expect)> {
        let queued = bench.device.queued().into_iter();
        queued
            .flat_map(|cmd| match cmd {
                DeviceCmd::SendAll { items, .. } => items
                    .iter()
                    .map(|item| (item.id, item.expect.clone()))
                    .collect(),
                _ => Vec::new(),
            })
            .collect()
    }

    fn got(bench: &mut Bench, slot: u32, bytes: Vec<u8>, why: Purpose) {
        let origin = Origin::Device {
            class: PROGRAM,
            at: at(slot),
        };
        bench.device.pretend(DeviceEvent::Got {
            info: crate::device::reported(at(slot), "P.ne5p"),
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

    fn undo(bench: &mut Bench, line: &str) -> String {
        assert!(bench.log.press(line), "{line} has an Undo");
        bench.log.undo_pressed(&mut bench.workspace);
        bench.log.status().1.to_string()
    }

    #[test]
    fn each_standing_and_keyboard_change_is_one_row() {
        let mut bench = Bench::new();
        let (theirs, both, other) = (gained("40"), gained("41"), gained("42"));
        walked_named(
            &mut bench,
            &[
                Some(("A", crc(&body()))),
                Some(("B", crc(&theirs))),
                Some(("C", crc(&both))),
                None,
                Some(("Clav", crc(&other))),
            ],
        );
        let ahead = linked(&mut bench, "A.ne5p", gained("50"), 1, &body());
        let changed = linked(&mut bench, "B.ne5p", body(), 2, &body());
        let twice = linked(&mut bench, "C.ne5p", gained("51"), 3, &body());
        let removed = linked(&mut bench, "D.ne5p", body(), 4, &body());
        let replaced = linked(&mut bench, "E.ne5p", body(), 5, &body());
        let by = "Clav".to_string();
        assert_eq!(
            listed(&bench),
            [
                (ahead, Change::Ahead),
                (changed, Change::Theirs),
                (twice, Change::Both),
                (removed, Change::Removed),
                (replaced, Change::Replaced { by }),
            ]
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
            to: at(2),
            agrees: true,
        };
        assert_eq!(listed(&bench), [(id, moved)]);

        let slots = [Some(("Clav", other)), Some(("A", ours)), Some(("A", ours))];
        walked_named(&mut bench, &slots);
        let by = "Clav".into();
        assert_eq!(
            listed(&bench),
            [(id, Change::Replaced { by })],
            "two hold it"
        );
    }

    #[test]
    fn a_slot_edited_under_its_name_is_newer_there_though_a_copy_is_elsewhere() {
        let mut bench = Bench::new();
        let slots = [Some(("A", crc(&gained("40")))), Some(("A", crc(&body())))];
        walked_named(&mut bench, &slots);
        let id = linked(&mut bench, "A.ne5p", body(), 1, &body());
        assert_eq!(listed(&bench), [(id, Change::Theirs)]);
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
        let emptied = linked(&mut bench, "B.ne5p", body(), 2, &body());
        let by = "Clav".into();
        assert_eq!(
            listed(&bench),
            [
                (replaced, Change::Replaced { by }),
                (emptied, Change::Removed)
            ]
        );
    }

    #[test]
    fn a_send_placed_by_hand_is_its_own_row_and_one_to_its_own_slot_is_the_linked_row() {
        let mut bench = Bench::new();
        walked(&mut bench, &[Some(crc(&body())), None]);
        let ahead = linked(&mut bench, "A.ne5p", gained("40"), 1, &body());
        let log = &mut bench.log;
        let placed = bench
            .workspace
            .ingest("B.ne5p".into(), Origin::Fresh, gained("41"), log);
        bench.workspace.settle_files(log);
        for (id, slot) in [(ahead, 1), (placed, 2)] {
            let (class, at) = (PROGRAM, at(slot));
            bench.act(vec![Act::Send { id, class, at }]);
        }
        assert_eq!(
            listed(&bench),
            [(placed, Change::Placed), (ahead, Change::Ahead)]
        );
    }

    #[test]
    fn each_row_takes_its_own_way_and_only_the_user_settles_both_changed_and_removed() {
        let mut bench = Bench::new();
        walked(&mut bench, &[Some(crc(&body()))]);
        let id = linked(&mut bench, "A.ne5p", body(), 1, &body());
        let workspace = &bench.workspace;
        let taken = |change: Change| {
            let row = Row {
                id,
                class: PROGRAM,
                at: Some(at(1)),
                change,
            };
            row.taken(workspace)
        };
        assert_eq!(taken(Change::Placed), Some(Way::Up));
        assert_eq!(taken(Change::Ahead), Some(Way::Up));
        assert_eq!(taken(Change::Theirs), Some(Way::Down));
        assert_eq!(taken(Change::Both), None);
        assert_eq!(taken(Change::Removed), None);
        let moved = Change::Moved {
            to: at(2),
            agrees: true,
        };
        assert_eq!(taken(moved), Some(Way::Down));
        let by = "Clav".into();
        assert_eq!(taken(Change::Replaced { by }), Some(Way::Down));
    }

    #[test]
    fn a_piano_and_a_file_with_unsaved_edits_never_take_the_keyboards_version() {
        let mut bench = Bench::new();
        walked(&mut bench, &[Some(crc(&gained("40")))]);
        let id = linked(&mut bench, "A.ne5p", body(), 1, &body());
        let row = row(&bench, id);
        assert_eq!(row.ways(&bench.workspace), [Way::Down, Way::Up]);

        let piano = Row {
            class: ObjectClass::Piano,
            ..row.clone()
        };
        assert_eq!(piano.ways(&bench.workspace), [Way::Up]);
        assert_eq!(
            piano.taken(&bench.workspace),
            None,
            "nothing is picked for it"
        );
        assert_eq!(piano.down(&bench.workspace), Some(COPY_INSTEAD));

        let log = &mut bench.log;
        bench.workspace.replace_bytes(id, gained("60"), log);
        assert_eq!(row.ways(&bench.workspace), [Way::Up]);
        assert_eq!(row.down(&bench.workspace), Some(SAVE_FIRST));
    }

    #[test]
    fn sync_asks_for_a_read_and_does_nothing_until_it_has_landed() {
        let mut bench = Bench::new();
        walked(&mut bench, &[Some(crc(&body()))]);
        let id = linked(&mut bench, "A.ne5p", gained("40"), 1, &body());
        bench.act(vec![Act::Sync(None)]);
        let running = bench.device.state.scan.progress(PROGRAM);
        assert!(running.is_some_and(|it| it.running), "the keyboard is read");
        assert_eq!(listed(&bench), [(id, Change::Ahead)]);

        bench.act(vec![Act::SyncAll]);
        assert_eq!(written(&bench), [], "nothing before the read lands");
        assert!(bench.device.asks.sheet.is_some(), "and the sheet stays");

        bench.land();
        bench.act(vec![Act::SyncAll]);
        assert_eq!(written(&bench).len(), 1);
        assert!(bench.device.asks.sheet.is_none());
    }

    #[test]
    fn a_write_expects_what_the_read_after_the_press_found() {
        let mut bench = Bench::new();
        walked(&mut bench, &[None]);
        let log = &mut bench.log;
        let id = bench
            .workspace
            .ingest("B.ne5p".into(), Origin::Fresh, gained("41"), log);
        bench.workspace.settle_files(log);
        let (class, at1) = (PROGRAM, at(1));
        bench.act(vec![Act::Send { id, class, at: at1 }]);
        bench.act(vec![Act::Sync(None)]);
        let now = crc(&gained("42"));
        bench.device.pretend(DeviceEvent::BankScanned {
            class,
            bank: 1,
            slots: vec![Some(program_info(1, "Someone", now))],
        });
        bench.land();
        bench.act(vec![Act::SyncAll]);
        let expect = Expect::Holds {
            crc: Some(now),
            len: 121,
            name: "Someone".into(),
        };
        assert_eq!(written(&bench), [(id, expect)]);
    }

    #[test]
    fn a_panel_edit_after_the_read_is_refused_by_the_write_and_the_row_comes_back() {
        let mut bench = Bench::new();
        walked(&mut bench, &[Some(crc(&body()))]);
        let id = linked(&mut bench, "A.ne5p", gained("40"), 1, &body());
        bench.sync();
        let expect = Expect::Holds {
            crc: Some(crc(&body())),
            len: 121,
            name: "A".into(),
        };
        assert_eq!(written(&bench), [(id, expect)]);

        let panel = program_info(1, "A", crc(&gained("43")));
        bench.device.pretend(DeviceEvent::Changed {
            id,
            class: PROGRAM,
            at: at(1),
            holds: Some(panel.clone()),
        });
        bench.device.pretend(DeviceEvent::BankScanned {
            class: PROGRAM,
            bank: 1,
            slots: vec![Some(panel)],
        });
        heard(&mut bench);
        assert_eq!(listed(&bench), [(id, Change::Both)]);
    }

    #[test]
    fn the_keyboards_way_brings_the_slot_in_and_nothing_else() {
        for (file, change) in [(body(), Change::Theirs), (gained("50"), Change::Ahead)] {
            let mut bench = Bench::new();
            let theirs = gained("40");
            let slot = match change {
                Change::Theirs => crc(&theirs),
                _ => crc(&body()),
            };
            walked(&mut bench, &[Some(slot)]);
            let id = linked(&mut bench, "A.ne5p", file, 1, &body());
            assert_eq!(listed(&bench), [(id, change.clone())]);
            open_read(&mut bench);
            pick(&mut bench, id, Way::Down);
            bench.act(vec![Act::SyncAll]);
            assert_eq!(pulls(&bench), [at(1)], "{change:?}");
            assert_eq!(written(&bench), [], "{change:?}");

            let there = match change {
                Change::Theirs => theirs,
                _ => body(),
            };
            got(&mut bench, 1, there.clone(), Purpose::Pull);
            assert_eq!(bench.workspace.get(id).unwrap().bytes.to_vec(), there);
            assert_eq!(standing(&bench, id), Some(Standing::Same), "{change:?}");
        }
    }

    #[test]
    fn this_computers_way_writes_the_file_and_brings_nothing_in() {
        for (file, slot, change) in [
            (gained("50"), body(), Change::Ahead),
            (body(), gained("40"), Change::Theirs),
            (gained("50"), gained("40"), Change::Both),
        ] {
            let mut bench = Bench::new();
            walked(&mut bench, &[Some(crc(&slot))]);
            let id = linked(&mut bench, "A.ne5p", file, 1, &body());
            assert_eq!(listed(&bench), [(id, change.clone())]);
            open_read(&mut bench);
            pick(&mut bench, id, Way::Up);
            bench.act(vec![Act::SyncAll]);
            let ids: Vec<u64> = written(&bench).iter().map(|(id, _)| *id).collect();
            assert_eq!(ids, [id], "{change:?}");
            assert_eq!(pulls(&bench), [], "{change:?}");
            assert_eq!(standing(&bench, id), Some(Standing::Ahead), "{change:?}");
        }
    }

    #[test]
    fn only_a_picked_up_writes_a_file_over_its_own_changed_slot() {
        let mut bench = Bench::new();
        walked(&mut bench, &[Some(crc(&gained("40"))), None]);
        let id = linked(&mut bench, "A.ne5p", body(), 1, &body());
        let own = Act::Send {
            id,
            class: PROGRAM,
            at: at(1),
        };
        for act in [Act::SendChecked(vec![id]), own, Act::SaveDoc(id)] {
            let said = format!("{act:?}");
            bench.act(vec![act]);
            assert!(!bench.queue.holds(id), "{said} queued it");
        }
        bench.sync();
        assert_eq!(written(&bench), [], "its own way is the keyboard's");

        bench.device.pretend(DeviceEvent::Finished);
        heard(&mut bench);
        open_read(&mut bench);
        pick(&mut bench, id, Way::Up);
        bench.act(vec![Act::SyncAll]);
        let ids: Vec<u64> = written(&bench).iter().map(|(id, _)| *id).collect();
        assert_eq!(ids, [id]);
    }

    #[test]
    fn a_row_left_without_a_way_or_left_out_is_untouched_by_sync() {
        let mut bench = Bench::new();
        walked(
            &mut bench,
            &[Some(crc(&gained("40"))), Some(crc(&gained("41")))],
        );
        let both = linked(&mut bench, "A.ne5p", gained("50"), 1, &body());
        let skipped = linked(&mut bench, "B.ne5p", body(), 2, &body());
        open_read(&mut bench);
        bench.device.asks.sheet.as_mut().unwrap().skip(skipped);
        bench.act(vec![Act::SyncAll]);
        assert_eq!((written(&bench), pulls(&bench)), (vec![], vec![]));
        assert_eq!(standing(&bench, both), Some(Standing::Both));
        assert_eq!(standing(&bench, skipped), Some(Standing::Theirs));
    }

    #[test]
    fn a_removed_sound_goes_back_up_or_is_unlinked_as_picked() {
        for way in [Way::Up, Way::Down] {
            let mut bench = Bench::new();
            walked(&mut bench, &[None, Some(crc(&gained("40")))]);
            let id = linked(&mut bench, "A.ne5p", body(), 1, &body());
            assert_eq!(listed(&bench), [(id, Change::Removed)]);
            open_read(&mut bench);
            pick(&mut bench, id, way);
            bench.act(vec![Act::SyncAll]);
            let ids: Vec<u64> = written(&bench).iter().map(|(id, _)| *id).collect();
            match way {
                Way::Up => assert_eq!(
                    (ids, slot_of(&bench, id)),
                    (vec![id], Some(Spot::at(at(1))))
                ),
                Way::Down => {
                    assert_eq!((ids, slot_of(&bench, id)), (vec![], None));
                    assert!(bench.workspace.get(id).is_some(), "kept on this computer");
                    assert_eq!(undo(&mut bench, "unlinked"), "Linked “A” again.");
                    assert_eq!(slot_of(&bench, id), Some(Spot::at(at(1))));
                }
            }
        }
    }

    #[test]
    fn sync_follows_moves_and_unlinks_replaced_sounds_and_undo_takes_each_back() {
        let mut bench = Bench::new();
        let (a, b, clav) = (body(), gained("40"), gained("41"));
        walked_named(
            &mut bench,
            &[
                Some(("B", crc(&b))),
                Some(("A", crc(&a))),
                Some(("Clav", crc(&clav))),
            ],
        );
        let first = linked(&mut bench, "A.ne5p", a.clone(), 1, &a);
        let second = linked(&mut bench, "B.ne5p", b.clone(), 2, &b);
        let replaced = linked(&mut bench, "C.ne5p", gained("42"), 3, &gained("42"));
        bench.sync();
        assert_eq!(slot_of(&bench, first), Some(Spot::at(at(2))), "a swap");
        assert_eq!(slot_of(&bench, second), Some(Spot::at(at(1))));
        assert_eq!(slot_of(&bench, replaced), None);
        assert_eq!(written(&bench), []);

        undo(&mut bench, "files follow their sounds");
        assert_eq!(slot_of(&bench, first), Some(Spot::at(at(1))));
        assert_eq!(slot_of(&bench, second), Some(Spot::at(at(2))));
        undo(&mut bench, "unlinked from the keyboard");
        assert_eq!(slot_of(&bench, replaced), Some(Spot::at(at(3))));
    }

    #[test]
    fn a_slot_is_brought_in_only_over_the_file_as_linked_and_saved_when_sync_was_pressed() {
        for change in ["edited", "relinked"] {
            let mut bench = Bench::new();
            let theirs = gained("40");
            walked(&mut bench, &[Some(crc(&theirs)), None]);
            let id = linked(&mut bench, "A.ne5p", body(), 1, &body());
            bench.sync();
            assert_eq!(pulls(&bench), [at(1)]);
            let log = &mut bench.log;
            match change {
                "edited" => bench.workspace.replace_bytes(id, gained("60"), log),
                _ => {
                    let keyboard = bench.device.state.keyboard().cloned().unwrap();
                    let held = Held::new(PROGRAM, at(2), "", print(&body())).unwrap();
                    bench.workspace.link(id, &keyboard, held);
                }
            }
            let before = bench.workspace.get(id).unwrap().bytes.to_vec();
            got(&mut bench, 1, theirs, Purpose::Pull);
            assert_eq!(
                bench.workspace.get(id).unwrap().bytes.to_vec(),
                before,
                "{change}"
            );
        }
    }

    #[test]
    fn undoing_a_bringing_in_puts_the_file_back_and_nothing_brings_it_in_again() {
        let mut bench = Bench::new();
        let theirs = gained("40");
        walked(&mut bench, &[Some(crc(&theirs))]);
        let id = linked(&mut bench, "A.ne5p", body(), 1, &body());
        bench.sync();
        got(&mut bench, 1, theirs, Purpose::Pull);
        assert_eq!(standing(&bench, id), Some(Standing::Same));

        assert_eq!(undo(&mut bench, "Brought in"), "Put back “A”.");
        assert_eq!(bench.workspace.get(id).unwrap().bytes.to_vec(), body());
        assert_eq!(standing(&bench, id), Some(Standing::Theirs));
        bench.device.pretend(DeviceEvent::Finished);
        heard(&mut bench);
        assert_eq!(pulls(&bench), [at(1)], "not brought in again");
    }

    #[test]
    fn the_sheet_opens_on_attach_once_and_only_for_what_changed_on_the_keyboard() {
        let mut bench = Bench::new();
        walked(&mut bench, &[Some(crc(&body())), Some(crc(&body()))]);
        let ahead = linked(&mut bench, "A.ne5p", gained("40"), 1, &body());
        heard(&mut bench);
        assert_eq!(listed(&bench), [(ahead, Change::Ahead)]);
        assert!(
            bench.device.asks.sheet.is_none(),
            "what this computer saved"
        );

        let card = bench.device.state.card().unwrap().clone();
        bench
            .device
            .pretend(DeviceEvent::Disconnected { lost: false });
        bench.device.pretend(DeviceEvent::Connected(card));
        heard(&mut bench);
        walked(&mut bench, &[Some(crc(&body())), Some(crc(&gained("41")))]);
        linked(&mut bench, "B.ne5p", body(), 2, &body());
        heard(&mut bench);
        assert!(
            bench.device.asks.sheet.is_some(),
            "the keyboard changed one"
        );

        bench.device.asks.sheet = None;
        bench.device.pretend(DeviceEvent::Finished);
        heard(&mut bench);
        assert!(bench.device.asks.sheet.is_none(), "once per attach");
    }

    #[test]
    fn sync_on_a_file_opens_the_sheet_on_its_row_and_shows_how_its_slot_differs() {
        let mut bench = Bench::new();
        let theirs = gained("40");
        walked(&mut bench, &[Some(crc(&theirs))]);
        let id = linked(&mut bench, "A.ne5p", body(), 1, &body());
        bench.act(vec![Act::Sync(Some(id))]);
        bench.land();
        let sheet = bench.device.asks.sheet.as_ref().expect("open");
        assert_eq!(sheet.picked, Some(id));

        let Bench {
            device,
            workspace,
            queue,
            log,
            ..
        } = &mut bench;
        show_picked(device, workspace, queue, log);
        show_picked(device, workspace, queue, log);
        let reads = device.queued().into_iter();
        let reads = reads.filter(|cmd| {
            matches!(
                cmd,
                DeviceCmd::Get {
                    why: Purpose::Compare,
                    ..
                }
            )
        });
        assert_eq!(reads.count(), 1, "read once");
        got(&mut bench, 1, theirs, Purpose::Compare);
        let sheet = bench.device.asks.sheet.as_ref().expect("open");
        let Some(Diff::Fields(fields)) = sheet.shown.as_ref().map(|it| &it.diff) else {
            panic!("a program differs by its fields")
        };
        assert!(fields.iter().any(|field| field.path == "center_panel.gain"));
    }

    #[test]
    fn the_sheets_sync_button_does_what_its_rows_say() {
        use crate::testing::{click, painted, run, screen, where_};
        let mut bench = Bench::new();
        walked(&mut bench, &[Some(crc(&body()))]);
        let id = linked(&mut bench, "A.ne5p", gained("40"), 1, &body());
        open_read(&mut bench);
        let ctx = bench.ctx.clone();
        let mut acts = Vec::new();
        let mut said = Vec::new();
        for events in [Vec::new(), Vec::new(), Vec::new()] {
            let events = match said.is_empty() {
                true => events,
                false => click(where_(&said, "Sync 1").center()),
            };
            let output = run(
                &ctx,
                screen(eframe::egui::vec2(1280.0, 800.0), events),
                |ctx| {
                    let (device, queue) = (&mut bench.device, &bench.queue);
                    crate::queue::sheet(ctx, device, queue, &bench.workspace, &mut acts);
                },
            );
            said = painted(&output);
        }
        assert_eq!(acts, [Act::SyncAll]);
        bench.act(acts);
        let ids: Vec<u64> = written(&bench).iter().map(|(id, _)| *id).collect();
        assert_eq!(ids, [id]);
    }

    #[test]
    fn sync_with_nothing_to_do_closes_and_says_so() {
        let mut bench = Bench::new();
        walked(&mut bench, &[Some(crc(&body()))]);
        linked(&mut bench, "A.ne5p", body(), 1, &body());
        open_read(&mut bench);
        assert!(bench.device.asks.sheet.is_none());
        assert_eq!(bench.log.status().1, NOTHING);
    }
}
