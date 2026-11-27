//! Sync: what differs between this computer and the attached keyboard, one row per sound,
//! and what pressing Sync does with each row. Bringing a slot in reads it and saves what
//! it holds as the file linked to it.

use std::collections::{BTreeMap, BTreeSet};
use std::rc::Rc;

use nord_usb::{Location, ObjectClass};

use crate::device::Expect;
use crate::device::{
    occupied, slot_label, standing_of, there, Device, DeviceCmd, DeviceState, Purpose,
};
use crate::link::{Held, Keyboard, Print, Spot, Standing, There};
use crate::log::{Log, Relinked, Restored, Undo};
use crate::queue::{compare, Diff, Queue};
use crate::strings::{counted, display_name, shown};
use crate::workspace::{print_of, LocalEntity, VerifyState, Workspace};

/// What one row of the sheet says differs.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Change {
    /// Sent by hand to a slot, unless `waits` says why it goes nowhere yet.
    Placed { waits: Option<Waits> },
    /// A piano or sample only this computer has, which the programs `by` play and which
    /// goes up with any of them, to a free slot.
    Plays { by: Vec<u64> },
    /// The slot holds an earlier version of the file.
    Ahead,
    /// The slot holds a version the file never had, and the file has not changed.
    Theirs,
    /// The slot holds a version the file never had, and the file has changed too.
    Both,
    /// The sound is in another slot of its class, which no file is linked to, or which
    /// holds the sound of the file `with`, linked there, while this slot holds that one's.
    /// `agrees` where that slot holds the file's saved version.
    Moved {
        to: Location,
        agrees: bool,
        with: Option<u64>,
    },
    /// The slot holds another sound, named `by`, and the file's sound is not in exactly
    /// one other slot.
    Replaced { by: String },
    /// The slot is empty, and the sound is not in exactly one other slot.
    Removed,
}

/// Why a send placed by hand goes nowhere yet.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Waits {
    /// The keyboard attached now refuses it.
    Refused,
    /// The file's own slot changed on the keyboard, which its own row settles first: a
    /// file has one row with a way.
    OwnRow,
}

/// One sound the sheet lists.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Row {
    pub id: u64,
    pub class: ObjectClass,
    /// The slot. A piano or sample removed from the keyboard, or one with no free slot to
    /// go to, has none.
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

/// The way a row goes unless the user picks one, and the ways it may go.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Ways {
    pub taken: Option<Way>,
    pub allowed: Vec<Way>,
}

/// What a mark before a file's name claims about its row.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Sign {
    /// Sync writes it unless told otherwise.
    Up,
    /// Sync brings its slot in unless told otherwise.
    Down,
    /// It waits for the user, or its link changes.
    Attention,
}

impl Row {
    /// The brief's table: each change's own way and the ways it may go. A slot's version
    /// is brought in only where [`Row::down_blocked`] says nothing. A send the keyboard
    /// refuses, and a piano or sample with no room, go nowhere.
    pub fn ways(&self, workspace: &Workspace) -> Ways {
        use Way::{Down, Up};
        let down = self.down_blocked(workspace).is_none();
        let (taken, allowed) = match (&self.change, down, self.at) {
            (Change::Placed { waits: Some(_) }, ..) | (Change::Plays { .. }, _, None) => {
                (None, vec![])
            }
            (Change::Placed { waits: None } | Change::Plays { .. }, ..) => (Some(Up), vec![Up]),
            (Change::Ahead, true, _) => (Some(Up), vec![Up, Down]),
            (Change::Ahead, false, _) => (Some(Up), vec![Up]),
            (Change::Theirs, true, _) => (Some(Down), vec![Down, Up]),
            (Change::Theirs | Change::Both, false, _) => (None, vec![Up]),
            (Change::Both, true, _) => (None, vec![Down, Up]),
            (Change::Removed, _, Some(_)) => (None, vec![Up, Down]),
            (Change::Removed, _, None) => (None, vec![Down]),
            (Change::Moved { .. } | Change::Replaced { .. }, ..) => (Some(Down), vec![Down]),
        };
        Ways { taken, allowed }
    }

    /// Why the keyboard's version cannot be brought into the file, if it cannot.
    pub fn down_blocked(&self, workspace: &Workspace) -> Option<Blocked> {
        let unsaved = workspace.get(self.id).is_some_and(LocalEntity::is_unsaved);
        match (self.class.is_library(), unsaved) {
            (true, _) => Some(Blocked::Library),
            (false, true) => Some(Blocked::Unsaved),
            (false, false) => None,
        }
    }

    /// The mark the library shows for the file: its own way where it reads true to a
    /// player, and else `!`. A moved or replaced sound has a way in the sheet, its link,
    /// but needs the player's eye all the same.
    pub fn sign(&self, workspace: &Workspace) -> Sign {
        match (self.ways(workspace).taken, &self.change) {
            (_, Change::Moved { .. } | Change::Replaced { .. }) => Sign::Attention,
            (Some(Way::Up), _) => Sign::Up,
            (Some(Way::Down), _) => Sign::Down,
            (None, _) => Sign::Attention,
        }
    }

    /// The row's word line, as the sheet and the inspector say it; a hover capitalizes it
    /// with [`said_as_hover`].
    pub fn said(&self, workspace: &Workspace, queue: &Queue) -> String {
        let blocked = match (&self.change, self.down_blocked(workspace)) {
            (Change::Theirs | Change::Both, Some(blocked)) => blocked.words(),
            _ => "",
        };
        let said = match &self.change {
            Change::Placed {
                waits: Some(Waits::OwnRow),
            } => {
                let name = workspace
                    .get(self.id)
                    .map_or("", |it| display_name(&it.name));
                format!("sync “{name}” first")
            }
            Change::Placed { .. } => {
                let to = self.at.map(shown).unwrap_or_default();
                format!("to {to} · {}", crate::queue::placed_words(self.id, queue))
            }
            Change::Plays { .. } if self.at.is_none() => "no room on the keyboard".into(),
            Change::Plays { by } => match by[..] {
                [program] => {
                    let name = workspace.get(program).map_or("", |it| it.name.as_str());
                    format!("new · “{}” plays it", display_name(name))
                }
                _ => format!("new · {} programs play it", by.len()),
            },
            Change::Ahead => "newer here".into(),
            Change::Theirs => "newer on the keyboard".into(),
            Change::Both => "both changed".into(),
            Change::Moved { to, .. } => format!("moved to {}", shown(*to)),
            Change::Replaced { by } => format!("replaced by “{by}”"),
            Change::Removed => "removed on the keyboard".into(),
        };
        format!("{said}{blocked}")
    }
}

/// [`Row::said`] for a hover: its first letter capitalized, and only that.
pub fn said_as_hover(said: &str) -> String {
    let mut chars = said.chars();
    chars.next().map_or_else(String::new, |first| {
        first.to_uppercase().chain(chars).collect()
    })
}

/// Why a file cannot take the keyboard's version.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Blocked {
    /// A piano or sample: its slot is never read whole.
    Library,
    /// It holds edits not saved, which would be lost.
    Unsaved,
}

impl Blocked {
    /// What the row's word line adds.
    fn words(self) -> &'static str {
        match self {
            Blocked::Library => " · copy it here instead",
            Blocked::Unsaved => " · save first",
        }
    }
}

/// Every row, and each file's mark and words, as one calculation.
#[derive(Default)]
pub struct Rows {
    pub list: Vec<Row>,
    marks: BTreeMap<u64, (Sign, String)>,
}

impl Rows {
    /// The rows, and a mark for each but a piano or sample whose program does not go up
    /// as it stands.
    pub fn of(state: &DeviceState, workspace: &Workspace, queue: &Queue) -> Rows {
        let list = rows(state, workspace, queue);
        let up = |row: &Row| row.ways(workspace).taken == Some(Way::Up);
        let marked = list.iter().filter(|row| match &row.change {
            Change::Plays { by } => list.iter().any(|it| by.contains(&it.id) && up(it)),
            _ => true,
        });
        let marks = marked.map(|row| {
            let said = row.said(workspace, queue);
            (row.id, (row.sign(workspace), said))
        });
        let marks = marks.collect();
        Rows { list, marks }
    }

    /// The mark and words of the file `id`'s row, where it has one.
    pub fn mark(&self, id: u64) -> Option<(Sign, &str)> {
        let (sign, said) = self.marks.get(&id)?;
        Some((*sign, said.as_str()))
    }

    /// How many files Sync acts on as they stand, and how many wait for the user.
    pub fn counts(&self) -> [usize; 2] {
        let attention = self
            .marks
            .values()
            .filter(|(sign, _)| *sign == Sign::Attention);
        let attention = attention.count();
        [self.marks.len() - attention, attention]
    }
}

/// [`Rows`], worked out again only when the list, the keyboard or the queue changed.
#[derive(Default)]
pub struct Synced(Option<Taken>);

/// The rows, and what they were worked out from: the workspace's and the keyboard's
/// revisions, and the queue's shape.
type Taken = ((u64, u64), Vec<(crate::queue::Waiting, bool)>, Rc<Rows>);

impl Synced {
    /// ⚠️ The browser asks for every row it draws, so a hit must not walk the list.
    pub fn of(&mut self, workspace: &Workspace, device: &Device, queue: &Queue) -> Rc<Rows> {
        let at = (workspace.revision(), device.revision());
        let held = self
            .0
            .as_ref()
            .filter(|(was, shape, _)| *was == at && queue.shape().eq(shape.iter().copied()));
        if let Some((_, _, rows)) = held {
            return rows.clone();
        }
        let rows = Rc::new(Rows::of(&device.state, workspace, queue));
        self.0 = Some((at, queue.shape().collect(), rows.clone()));
        rows
    }
}

/// Every sound Sync would list: each send placed by hand, each linked file whose slot
/// differs from it, unless a send placed by hand goes to that slot or is the file's own
/// and its row is on this computer's side, and each piano or sample a program that may go
/// up plays that only this computer has. A sound two files would move to has moved for
/// neither.
pub fn rows(state: &DeviceState, workspace: &Workspace, queue: &Queue) -> Vec<Row> {
    let Some(keyboard) = state.keyboard() else {
        return Vec::new();
    };
    let own: Vec<Row> = workspace
        .listed()
        .filter_map(|entity| linked_row(state, workspace, keyboard, entity))
        .collect();
    let keyboard_side = |id: u64| {
        own.iter()
            .any(|row| row.id == id && row.change != Change::Ahead)
    };
    let placed: Vec<Row> = queue
        .entries()
        .iter()
        .map(|held| Row {
            id: held.id,
            class: held.class,
            at: Some(held.at),
            change: Change::Placed {
                waits: match (held.failure.is_some(), keyboard_side(held.id)) {
                    (true, _) => Some(Waits::Refused),
                    (false, true) => Some(Waits::OwnRow),
                    (false, false) => None,
                },
            },
        })
        .collect();
    let covered = |row: &Row| {
        let file_side = row.change == Change::Ahead;
        placed
            .iter()
            .any(|it| (it.id == row.id && file_side) || (it.at == row.at && it.class == row.class))
    };
    let linked: Vec<Row> = own.into_iter().filter(|row| !covered(row)).collect();
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
    let mut rows: Vec<Row> = placed.iter().cloned().chain(linked).collect();
    let played = played(state, workspace, queue, &rows);
    rows.extend(played);
    rows
}

/// The piano and sample files each program that may go up plays that the keyboard does not
/// hold and that have no row of their own, each as a row to the first free slot of its
/// class.
///
/// ⚠️ Only for a class whose every bank has been read: a slot missing from a partial
/// read may still be on the keyboard.
fn played(state: &DeviceState, workspace: &Workspace, queue: &Queue, rows: &[Row]) -> Vec<Row> {
    let mut found: Vec<Row> = Vec::new();
    let going = rows
        .iter()
        .filter(|row| row.ways(workspace).allowed.contains(&Way::Up));
    for program in going.filter_map(|row| workspace.get(row.id)) {
        for (class, key) in crate::plays::named(program)
            .iter()
            .map(|plays| (plays.key().0, plays.key()))
        {
            let held = |info: &nord_usb::wire::ProgramInfo| {
                crate::plays::slot_key(class, &info.name) == key
            };
            if !state.read_whole(class) || occupied(state, class).any(|(_, info)| held(info)) {
                continue;
            }
            let file = workspace
                .listed()
                .find(|file| crate::plays::key_of(file) == Some(key));
            let Some(file) = file.filter(|file| !rows.iter().any(|row| row.id == file.id)) else {
                continue;
            };
            if let Some(row) = found.iter_mut().find(|row| row.id == file.id) {
                if let Change::Plays { by } = &mut row.change {
                    by.extend((!by.contains(&program.id)).then_some(program.id));
                }
                continue;
            }
            let mut taken = queue.waiting_in(class);
            taken.extend(
                found
                    .iter()
                    .filter(|row| row.class == class)
                    .filter_map(|row| row.at),
            );
            found.push(Row {
                id: file.id,
                class,
                at: state.first_free(class, &taken),
                change: Change::Plays {
                    by: vec![program.id],
                },
            });
        }
    }
    found
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
    let to = unique.filter(|(to, _)| movable(state, workspace, keyboard, held, *to));
    let with = |to| workspace.linked_at(keyboard, held.class(), &Spot::at(to));
    match (to, by) {
        (Some((to, agrees)), _) => Some(Change::Moved {
            to,
            agrees,
            with: with(to),
        }),
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
/// one linked there has its sound elsewhere now, as in a swap or a rotation.
fn movable(
    state: &DeviceState,
    workspace: &Workspace,
    keyboard: &Keyboard,
    held: &Held,
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
    let at_to = state.slot(class, to).flatten().and_then(|info| info.crc32);
    at_to.is_some_and(|crc| !theirs(crc))
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
    /// What the slot held in the sheet's read. Bytes of another version land nowhere.
    expect: Expect,
    /// The file as the sheet showed it, linked so. It lands on nothing else.
    file: Seen,
}

/// How the sheet came to show.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Opened {
    /// By itself on attach, over the walk attaching made.
    OnAttach,
    /// By a press, which read these classes again.
    Pressed { reading: Vec<ObjectClass> },
}

/// The sheet, while it shows.
pub struct Sheet {
    pub opened: Opened,
    pub picked: Option<u64>,
    /// Where the user turned a row from its own way, for this showing only: to the way
    /// held, or out of this Sync where it holds none.
    picks: BTreeMap<RowKey, Option<Way>>,
    /// The plan Sync was pressed on, until it runs.
    accepted: Option<Vec<Step>>,
    /// How the picked row's file differs from its slot, once the slot is read.
    pub shown: Option<Shown>,
}

/// The picked row's slot as read for the sheet.
pub struct Shown {
    pub id: u64,
    pub class: ObjectClass,
    pub at: Location,
    pub diff: Diff,
}

/// A row as the sheet tells it apart: its file and its slot.
type RowKey = (u64, Option<(u32, u32)>);

fn key(row: &Row) -> RowKey {
    (row.id, row.at.map(|at| (at.bank, at.slot)))
}

impl Sheet {
    fn new(opened: Opened, picked: Option<u64>) -> Sheet {
        Sheet {
            opened,
            picked,
            picks: BTreeMap::new(),
            accepted: None,
            shown: None,
        }
    }

    /// Whether every walk asked for when it opened has finished: its rows are what the
    /// keyboard holds since then.
    pub fn read(&self, state: &DeviceState) -> bool {
        match &self.opened {
            Opened::OnAttach => true,
            Opened::Pressed { reading } => !reading.iter().any(|class| running(state, *class)),
        }
    }

    /// Whether the sheet lists `row`: a piano or sample programs play only while one of
    /// them goes up.
    pub fn shows(&self, row: &Row, rows: &[Row], workspace: &Workspace) -> bool {
        let Change::Plays { by } = &row.change else {
            return true;
        };
        let mut programs = rows.iter().filter(|it| by.contains(&it.id));
        programs.any(|it| self.way(it, rows, workspace) == Some(Way::Up))
    }

    /// The way Sync takes `row`: the one the user picked, or else its own. A move goes
    /// only with every move it depends on: a swap or a rotation goes whole or not at all.
    pub fn way(&self, row: &Row, rows: &[Row], workspace: &Workspace) -> Option<Way> {
        if matches!(row.change, Change::Moved { .. }) && !self.moving(rows).contains(&row.id) {
            return None;
        }
        let ways = row.ways(workspace);
        match self.picks.get(&key(row)) {
            Some(None) => None,
            Some(Some(way)) if ways.allowed.contains(way) => Some(*way),
            Some(Some(_)) | None => ways.taken,
        }
    }

    /// Whether `row` waits for the user: it goes no way, and the user did not leave it
    /// out.
    pub fn waits(&self, row: &Row, rows: &[Row], workspace: &Workspace) -> bool {
        self.way(row, rows, workspace).is_none() && self.picks.get(&key(row)) != Some(&None)
    }

    /// Whether `row` can be turned to `way`: it may go that way, and every move it goes
    /// only with is listed.
    pub fn takes(&self, row: &Row, way: Way, rows: &[Row], workspace: &Workspace) -> bool {
        row.ways(workspace).allowed.contains(&way) && Sheet::with(row, rows).is_some()
    }

    /// The files whose moves go: the largest set of moves not left out in which every
    /// move onto another file's slot has that file's move too, so a swap or a rotation
    /// goes whole or not at all.
    fn moving(&self, rows: &[Row]) -> BTreeSet<u64> {
        let moves = rows
            .iter()
            .filter(|row| self.picks.get(&key(row)) != Some(&None));
        let moves: Vec<(u64, Option<u64>)> = moves
            .filter_map(|row| match row.change {
                Change::Moved { with, .. } => Some((row.id, with)),
                _ => None,
            })
            .collect();
        let mut going: BTreeSet<u64> = moves.iter().map(|(id, _)| *id).collect();
        loop {
            let stuck = moves.iter().find(|(id, with)| {
                going.contains(id) && with.is_some_and(|it| !going.contains(&it))
            });
            match stuck {
                Some((id, _)) => going.remove(id),
                None => return going,
            };
        }
    }

    /// `row` and each move it goes only with, in turn. `None` where one of them is not
    /// listed, so the row cannot go.
    fn with<'r>(row: &'r Row, rows: &'r [Row]) -> Option<Vec<&'r Row>> {
        let mut found = vec![row];
        while let Some(Change::Moved {
            with: Some(with), ..
        }) = found.last().map(|row| &row.change)
        {
            if found.iter().any(|it| it.id == *with) {
                break;
            }
            let moved = |it: &&Row| it.id == *with && matches!(it.change, Change::Moved { .. });
            found.push(rows.iter().find(moved)?);
        }
        Some(found)
    }

    /// Turn `row` to `way`, with every move it goes only with, or out of this Sync where
    /// it went that way already.
    pub fn turn(&mut self, row: &Row, way: Way, rows: &[Row], workspace: &Workspace) {
        let Some(with) = Sheet::with(row, rows).filter(|_| self.takes(row, way, rows, workspace))
        else {
            return;
        };
        if self.way(row, rows, workspace) == Some(way) {
            return self.pick(row, None, workspace);
        }
        for row in with {
            self.pick(row, Some(way), workspace);
        }
    }

    /// Keep `pick` for `row` where it is not the row's own way.
    fn pick(&mut self, row: &Row, pick: Option<Way>, workspace: &Workspace) {
        match pick == row.ways(workspace).taken {
            true => self.picks.remove(&key(row)),
            false => self.picks.insert(key(row), pick),
        };
    }

    /// Sync, pressed on `plan`: [`apply`] runs it.
    pub fn accept(&mut self, plan: Vec<Step>) {
        self.accepted = Some(plan);
    }

    /// How many steps the plan Sync was pressed on holds, until it runs. While it waits,
    /// the sheet takes no more choices.
    pub fn pressed(&self) -> Option<usize> {
        self.accepted.as_ref().map(Vec::len)
    }
}

/// The sheet and what Sync is bringing in, while one keyboard is attached. A new attach
/// starts it over.
#[derive(Default)]
pub struct Asks {
    /// Whether this attach's walk was looked at for the sheet to open by itself.
    settled: bool,
    pub sheet: Option<Sheet>,
    /// The slots the last Sync asked to bring in, each until its bytes land, its read
    /// fails or finds the slot empty, or the next Sync.
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

    /// A read for the sheet found its slot empty.
    pub fn vacant(&mut self, class: ObjectClass, at: Location) {
        let shown = self.sheet.as_mut().and_then(|sheet| sheet.shown.as_mut());
        if let Some(shown) = shown.filter(|it| (it.class, it.at) == (class, at)) {
            shown.diff = Diff::Empty;
        }
    }

    /// A read to bring the slot `at` in failed or found it empty: nothing lands there.
    pub fn unpull(&mut self, class: ObjectClass, at: Location) {
        self.pulling
            .retain(|pull| (pull.class, pull.at) != (class, at));
    }
}

/// Read the whole keyboard, and show the sheet with `picked` picked where it is a row.
pub fn open(device: &mut Device, picked: Option<u64>) {
    if !device.state.connected() {
        return;
    }
    device.resync();
    let reading = device.state.classes();
    device.asks.sheet = Some(Sheet::new(Opened::Pressed { reading }, picked));
}

/// Open the sheet once per attach, once the walk allows, where the keyboard's side has
/// something to say; and close a pressed one whose read found nothing to sync, saying
/// so. Call when the instrument or the list may have changed.
///
/// ⚠️ Nothing opens while a write is running or waiting: a send changes the slots it
/// would list.
pub fn follow(device: &mut Device, workspace: &Workspace, queue: &Queue, log: &mut Log) {
    let state = &device.state;
    if let Some(sheet) = &device.asks.sheet {
        let pressed = matches!(sheet.opened, Opened::Pressed { .. });
        if pressed && sheet.read(state) && rows(state, workspace, queue).is_empty() {
            device.asks.sheet = None;
            log.say(NOTHING);
        }
        return;
    }
    if device.sending()
        || device.asks.settled
        || !(walked(state, workspace) && printed(state, workspace))
    {
        return;
    }
    device.asks.settled = true;
    let keyboard_side = |row: &Row| {
        !matches!(
            row.change,
            Change::Placed { .. } | Change::Plays { .. } | Change::Ahead
        )
    };
    if rows(state, workspace, queue).iter().any(keyboard_side) {
        device.asks.sheet = Some(Sheet::new(Opened::OnAttach, None));
    }
}

/// What the status line says when the keyboard and this computer agree.
const NOTHING: &str = "Nothing to sync.";

/// Ask for the picked row's slot, to show how its file differs from it.
pub fn show_picked(device: &mut Device, sync: &Rows, log: &mut Log) {
    let state = &device.state;
    let Some(sheet) = device.asks.sheet.as_ref().filter(|sheet| sheet.read(state)) else {
        return;
    };
    let picked = sync.list.iter().find(|row| Some(row.id) == sheet.picked);
    let Some((row, at)) = picked.and_then(|row| Some((row, row.at?))) else {
        return;
    };
    let asked = sheet.shown.as_ref();
    if asked.is_some_and(|it| (it.id, it.class, it.at) == (row.id, row.class, at)) {
        return;
    }
    let (id, class) = (row.id, row.class);
    let diff = match (class.is_library(), state.slot(class, at), &row.change) {
        (_, _, Change::Moved { .. }) | (true, Some(Some(_)) | None, _) => return,
        (_, Some(None), _) => Diff::Empty,
        (false, _, _) => {
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

/// A file as the sheet showed it.
#[derive(Clone, PartialEq, Eq, Debug)]
struct Seen {
    /// The stamp of its bytes, which any edit changes.
    stamp: u64,
    /// What a slot holding its bytes as they are would report: what a write sends.
    body: Option<Print>,
    saved: Option<Print>,
    /// Whether it held edits not saved: a write sends them, and saves them once it lands.
    unsaved: bool,
    /// Its link on the attached keyboard.
    held: Option<Held>,
}

impl Seen {
    fn of(entity: &LocalEntity, keyboard: &Keyboard) -> Seen {
        Seen {
            stamp: entity.stamp,
            body: entity.container.as_ref().and_then(|it| it.print()),
            saved: entity.saved_print(),
            unsaved: entity.is_unsaved(),
            held: entity.held_on(keyboard).cloned(),
        }
    }

    /// Whether `now` is still the file shown for going `way`. A write sends the body as
    /// it is, or as Sync saves the edits it was shown with; bringing a slot in replaces
    /// the file, so nothing about it may have moved.
    fn still(&self, now: &Seen, way: Way) -> bool {
        match (way, self.unsaved) {
            (Way::Up, true) => self.held == now.held,
            (Way::Up, false) => (self.body, &self.held) == (now.body, &now.held),
            (Way::Down, _) => self == now,
        }
    }
}

/// One row the user accepted, with the versions the sheet showed it with. It runs only
/// while those still hold.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Step {
    pub row: Row,
    pub way: Way,
    /// The file as shown.
    file: Seen,
    /// What the row's slot held in the read the sheet was built from; `None` while unread.
    pub slot: Option<Expect>,
    /// What a move's destination held in that read.
    to: Option<Expect>,
}

impl Step {
    fn of(row: &Row, way: Way, state: &DeviceState, workspace: &Workspace) -> Option<Step> {
        let keyboard = state.keyboard()?;
        let entity = workspace.get(row.id)?;
        let read = |at: Option<Location>| Some(Expect::of(state.slot(row.class, at?)?));
        let to = match row.change {
            Change::Moved { to, .. } => read(Some(to)),
            _ => None,
        };
        Some(Step {
            row: row.clone(),
            way,
            file: Seen::of(entity, keyboard),
            slot: read(row.at),
            to,
        })
    }

    /// Whether the file, its link, and what the keyboard reports for the slots it
    /// touches are all still as the sheet showed them.
    fn holds(&self, state: &DeviceState, workspace: &Workspace) -> bool {
        let (Some(keyboard), Some(entity)) = (state.keyboard(), workspace.get(self.row.id)) else {
            return false;
        };
        let file = Seen::of(entity, keyboard);
        let read = |at: Option<Location>| Some(Expect::of(state.slot(self.row.class, at?)?));
        let to = match self.row.change {
            Change::Moved { to, .. } => read(Some(to)),
            _ => None,
        };
        self.file.still(&file, self.way) && read(self.row.at) == self.slot && to == self.to
    }
}

/// What the sheet's Sync does as the sheet stands: each row it shows with a way, and the
/// versions it shows them with, one step to a file. The footer counts it, and pressing
/// Sync freezes it.
pub fn plan(sheet: &Sheet, rows: &[Row], state: &DeviceState, workspace: &Workspace) -> Vec<Step> {
    let shown = rows.iter().filter(|row| sheet.shows(row, rows, workspace));
    let mut files = BTreeSet::new();
    shown
        .filter_map(|row| Step::of(row, sheet.way(row, rows, workspace)?, state, workspace))
        .filter(|step| files.insert(step.row.id))
        .collect()
}

/// Sync, pressed where nothing paints the sheet: freeze the plan it shows now, unless
/// one was pressed on already.
pub fn accept(device: &mut Device, sync: &Rows, workspace: &Workspace) {
    let state = &device.state;
    let sheet = device.asks.sheet.as_mut();
    if let Some(sheet) = sheet.filter(|sheet| sheet.read(state) && sheet.accepted.is_none()) {
        sheet.accept(plan(sheet, &sync.list, state, workspace));
    }
}

/// Run the plan the sheet's Sync froze, and close the sheet: check that every write can
/// start, then run each step whose versions still hold. A step that no longer holds
/// is named in the log, and the sheet opens again on the first one, reading the keyboard
/// again.
///
/// ⚠️ Nothing changes, neither a link nor a slot, if a write cannot start.
pub fn apply(device: &mut Device, workspace: &mut Workspace, log: &mut Log) {
    let Some(mut sheet) = device.asks.sheet.take() else {
        return;
    };
    let (Some(steps), Some(keyboard)) = (sheet.accepted.take(), device.state.keyboard().cloned())
    else {
        device.asks.sheet = Some(sheet);
        return;
    };
    let (going, stale): (Vec<Step>, Vec<Step>) = steps
        .into_iter()
        .partition(|step| step.holds(&device.state, workspace));
    let writes: Vec<&Step> = going.iter().filter(|step| step.way == Way::Up).collect();
    let batches = match crate::browser::batches(&writes, workspace) {
        Ok(batches) => batches,
        Err(why) => {
            log.trouble(format!("Nothing was synced: {why}"));
            device.asks.sheet = Some(sheet);
            return;
        }
    };
    device.asks.pulling.clear();
    device.asks.brought = None;
    let (mut moves, mut unlinks) = (Vec::new(), Vec::new());
    for step in &going {
        match (
            step.way,
            &step.row.change,
            step.row.at,
            &step.file.held,
            &step.slot,
        ) {
            (Way::Up, ..) => {}
            (Way::Down, Change::Moved { .. }, ..) => moves.push(&step.row),
            (Way::Down, Change::Replaced { .. } | Change::Removed, ..) => unlinks.push(step.row.id),
            (
                Way::Down,
                Change::Ahead | Change::Theirs | Change::Both,
                Some(at),
                Some(_),
                Some(expect),
            ) => {
                let (class, id) = (step.row.class, step.row.id);
                let (expect, file) = (expect.clone(), step.file.clone());
                pull(
                    device,
                    log,
                    Pull {
                        class,
                        at,
                        id,
                        expect,
                        file,
                    },
                );
            }
            (Way::Down, Change::Ahead | Change::Theirs | Change::Both, ..)
            | (Way::Down, Change::Placed { .. } | Change::Plays { .. }, ..) => {}
        }
    }
    follow_moves(&device.state, workspace, log, &keyboard, &moves);
    unlink(workspace, log, &keyboard, &unlinks);
    for (class, items) in batches {
        device.send(DeviceCmd::SendAll { class, items }, log);
    }
    let Some(first) = stale.first() else {
        return;
    };
    for step in &stale {
        let name = workspace.get(step.row.id).map_or("", |it| it.name.as_str());
        log.trouble(format!(
            "“{}” was not synced: it changed since the sheet showed it.",
            display_name(name)
        ));
    }
    open(device, Some(first.row.id));
}

/// Read a slot to bring it in.
fn pull(device: &mut Device, log: &mut Log, pull: Pull) {
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
    let links = links.into_iter().map(|(id, before)| Relinked {
        id,
        before,
        left: None,
    });
    log.offer(Undo::Relink {
        keyboard,
        links: links.collect(),
    });
}

/// Move each moved sound's link to the slot it moved to, all of a class at once so a swap
/// moves both, in one log line whose Undo puts them back. The keyboard is unchanged, so
/// nothing waiting to be written moves.
fn follow_moves(
    state: &DeviceState,
    workspace: &mut Workspace,
    log: &mut Log,
    keyboard: &Keyboard,
    rows: &[&Row],
) {
    let moved: Vec<(&Row, Location, bool, Held)> = rows
        .iter()
        .filter_map(|row| match row.change {
            Change::Moved { to, agrees, .. } => {
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
    for class in classes {
        let moves = moved.iter().filter(|(row, ..)| row.class == class);
        let moves: Vec<crate::workspace::Moved> = moves
            .map(|(_, to, _, held)| (held.spot.clone(), Some(Spot::at(*to)), None))
            .collect();
        workspace.respot(keyboard, class, &moves);
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
        n => format!("{n} files moved with their sounds."),
    });
    let links = moved.into_iter().map(|(row, _, _, before)| Relinked {
        id: row.id,
        before,
        left: workspace
            .get(row.id)
            .and_then(|it| it.held_on(keyboard))
            .cloned(),
    });
    log.offer(Undo::Relink {
        keyboard: keyboard.clone(),
        links: links.collect(),
    });
}

/// Save `bytes`, read from the slot `at` of `class` the keyboard calls `name`, as the file
/// `id` linked to that slot on the attached keyboard, and agree the link on them. Returns
/// what it held before.
///
/// ⚠️ A file with unsaved edits is never brought in over: they would be lost.
fn bring(
    device: &mut Device,
    workspace: &mut Workspace,
    log: &mut Log,
    (class, at, id): (ObjectClass, Location, u64),
    (bytes, name): (Vec<u8>, &str),
) -> Option<Restored> {
    let keyboard = device.state.keyboard()?;
    if workspace.linked_at(keyboard, class, &Spot::at(at)) != Some(id) {
        return None;
    }
    let entity = workspace.get(id)?;
    let before = entity.held_on(keyboard)?.clone();
    if entity.is_unsaved() || entity.unread() || entity.rests().is_some() {
        return None;
    }
    let after = before.agreed(print_of(&bytes)?, name.trim());
    let restored = Restored {
        id,
        bytes: entity.bytes.to_vec(),
        keyboard: keyboard.clone(),
        before,
        after: after.clone(),
    };
    workspace.save_over(id, bytes, None, log);
    workspace.link(id, keyboard, after);
    Some(restored)
}

/// A slot read to be brought in landed, as the keyboard calls it `name`. It is brought in
/// only over the file it was read for, while that file is linked as it was when Sync was
/// pressed, and only as the version its row showed. Either way the read is done with.
#[allow(clippy::too_many_arguments)]
pub fn landed(
    device: &mut Device,
    workspace: &mut Workspace,
    log: &mut Log,
    (class, at): (ObjectClass, Location),
    bytes: Vec<u8>,
    name: &str,
) {
    let Some(keyboard) = device.state.keyboard() else {
        return;
    };
    let file = |pull: &Pull| Some(Seen::of(workspace.get(pull.id)?, keyboard));
    let Some(index) = device
        .asks
        .pulling
        .iter()
        .position(|pull| (pull.class, pull.at) == (class, at))
    else {
        return;
    };
    let pull = device.asks.pulling.remove(index);
    let read = print_of(&bytes).map(|print| Expect::Holds {
        crc: Some(print.crc),
        len: print.len,
        name: name.trim().to_string(),
    });
    let shown = file(&pull).is_some_and(|now| pull.file.still(&now, Way::Down));
    let landed = shown
        && read.is_some_and(|read| pull.expect.matches(&read))
        && bring(device, workspace, log, (class, at, pull.id), (bytes, name))
            .map(|restored| brought(&mut device.asks, workspace, log, restored))
            .is_some();
    if landed {
        return;
    }
    let file = workspace
        .get(pull.id)
        .map_or("", |it| display_name(&it.name));
    log.trouble(format!(
        "“{file}” was not synced: it changed since the sheet showed it."
    ));
    if device.asks.sheet.is_none() {
        open(device, Some(pull.id));
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
    use crate::bring::Rows;
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

    /// Turn `id`'s row in the open sheet to `way`, as a click on that arrow does.
    fn pick(bench: &mut Bench, id: u64, way: Way) {
        let all = rows(&bench.device.state, &bench.workspace, &bench.queue);
        let row = row(bench, id);
        let sheet = bench.device.asks.sheet.as_mut().expect("open");
        if sheet.way(&row, &all, &bench.workspace) != Some(way) {
            sheet.turn(&row, way, &all, &bench.workspace);
        }
        let goes = sheet.way(&row, &all, &bench.workspace);
        assert_eq!(goes, Some(way), "{row:?}");
    }

    /// Click the lit arrow of `id`'s row in the open sheet, which leaves it out.
    fn turn_off(bench: &mut Bench, id: u64) {
        let all = rows(&bench.device.state, &bench.workspace, &bench.queue);
        let row = row(bench, id);
        let sheet = bench.device.asks.sheet.as_mut().expect("open");
        let lit = sheet.way(&row, &all, &bench.workspace).expect("a way");
        sheet.turn(&row, lit, &all, &bench.workspace);
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
            with: None,
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
            [
                (placed, Change::Placed { waits: None }),
                (ahead, Change::Ahead)
            ]
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
            row.ways(workspace).taken
        };
        assert_eq!(taken(Change::Placed { waits: None }), Some(Way::Up));
        assert_eq!(taken(Change::Ahead), Some(Way::Up));
        assert_eq!(taken(Change::Theirs), Some(Way::Down));
        assert_eq!(taken(Change::Both), None);
        assert_eq!(taken(Change::Removed), None);
        let moved = Change::Moved {
            to: at(2),
            agrees: true,
            with: None,
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
        assert_eq!(row.ways(&bench.workspace).allowed, [Way::Down, Way::Up]);

        let piano = Row {
            class: ObjectClass::Piano,
            ..row.clone()
        };
        let ways = piano.ways(&bench.workspace);
        assert_eq!(
            (ways.taken, ways.allowed),
            (None, vec![Way::Up]),
            "nothing is picked"
        );
        assert_eq!(piano.down_blocked(&bench.workspace), Some(Blocked::Library));

        let log = &mut bench.log;
        bench.workspace.replace_bytes(id, gained("60"), log);
        assert_eq!(row.ways(&bench.workspace).allowed, [Way::Up]);
        assert_eq!(row.down_blocked(&bench.workspace), Some(Blocked::Unsaved));
    }

    #[test]
    fn the_cached_rows_follow_an_edit_into_the_file_that_came_to_hold_it() {
        let mut bench = Bench::new();
        walked(&mut bench, &[Some(crc(&gained("40")))]);
        let id = linked(&mut bench, "A.ne5p", body(), 1, &body());
        bench
            .workspace
            .replace_bytes(id, gained("60"), &mut bench.log);
        let mut synced = Synced::default();
        let mut said = |bench: &Bench| {
            let rows = synced.of(&bench.workspace, &bench.device, &bench.queue);
            rows.mark(id).map(|(_, said)| said.to_string())
        };
        assert_eq!(
            said(&bench).as_deref(),
            Some("newer on the keyboard · save first")
        );

        bench.workspace.rebase(id, gained("60"), &mut bench.log);
        assert_eq!(said(&bench).as_deref(), Some("both changed"));
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

        bench.press();
        assert_eq!(written(&bench), [], "nothing before the read lands");
        assert!(bench.device.asks.sheet.is_some(), "and the sheet stays");

        bench.land();
        bench.press();
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
        bench.press();
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
            bench.press();
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
            let before = standing(&bench, id);
            open_read(&mut bench);
            pick(&mut bench, id, Way::Up);
            bench.press();
            let ids: Vec<u64> = written(&bench).iter().map(|(id, _)| *id).collect();
            assert_eq!(ids, [id], "{change:?}");
            assert_eq!(pulls(&bench), [], "{change:?}");
            assert_eq!(
                standing(&bench, id),
                before,
                "{change:?}: nothing stored yet"
            );
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
        bench.press();
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
        turn_off(&mut bench, skipped);
        let all = rows(&bench.device.state, &bench.workspace, &bench.queue);
        let sheet = bench.device.asks.sheet.as_ref().unwrap();
        let waits = [both, skipped].map(|id| sheet.waits(&row(&bench, id), &all, &bench.workspace));
        assert_eq!(waits, [true, false], "only the row with no way waits");
        bench.press();
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
            bench.press();
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

        undo(&mut bench, "files moved with their sounds");
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
    fn undoing_a_bringing_in_puts_the_file_back() {
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

        let sync = Rows::of(&bench.device.state, &bench.workspace, &bench.queue);
        let Bench { device, log, .. } = &mut bench;
        show_picked(device, &sync, log);
        show_picked(device, &sync, log);
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
                    let sync = Rows::of(&bench.device.state, &bench.workspace, &bench.queue);
                    let (device, queue) = (&mut bench.device, &bench.queue);
                    crate::queue::sheet(ctx, device, (queue, &sync), &bench.workspace, &mut acts);
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

    /// A bring-in lands only the version its row showed: the panel storing another sound
    /// over the slot after the press brings nothing in, says so, and reads again.
    #[test]
    fn a_bring_in_lands_only_the_version_its_row_showed() {
        let mut bench = Bench::new();
        let theirs = gained("40");
        walked(&mut bench, &[Some(crc(&theirs))]);
        let id = linked(&mut bench, "A.ne5p", body(), 1, &body());
        bench.sync();
        assert_eq!(pulls(&bench), [at(1)]);
        got(&mut bench, 1, gained("41"), Purpose::Pull);
        assert_eq!(bench.workspace.get(id).unwrap().bytes.to_vec(), body());
        let said = bench.log.status().1.to_string();
        assert_eq!(
            said,
            "“A” was not synced: it changed since the sheet showed it."
        );
        let sheet = bench.device.asks.sheet.as_ref().expect("read again");
        assert_eq!(sheet.picked, Some(id));
    }

    /// A bring-in that is refused, or whose read finds the slot empty, is done with: the
    /// next Sync reads the slot again and brings it in.
    #[test]
    fn a_refused_or_empty_bring_in_is_forgotten_and_the_next_sync_asks_again() {
        let mut bench = Bench::new();
        let theirs = gained("40");
        walked(&mut bench, &[Some(crc(&theirs))]);
        let id = linked(&mut bench, "A.ne5p", body(), 1, &body());
        bench.sync();
        bench.device.pretend(DeviceEvent::Vacant {
            class: PROGRAM,
            at: at(1),
            why: Purpose::Pull,
        });
        heard(&mut bench);
        got(&mut bench, 1, theirs.clone(), Purpose::Pull);
        assert_eq!(
            bench.workspace.get(id).unwrap().bytes.to_vec(),
            body(),
            "forgotten"
        );

        bench.sync();
        bench
            .workspace
            .replace_bytes(id, gained("60"), &mut bench.log);
        got(&mut bench, 1, theirs.clone(), Purpose::Pull);
        assert_eq!(
            bench.log.status().1,
            "“A” was not synced: it changed since the sheet showed it."
        );
        bench.act(vec![Act::Revert(id)]);

        bench.sync();
        assert_eq!(pulls(&bench), [at(1), at(1), at(1)], "each Sync reads it");
        got(&mut bench, 1, theirs.clone(), Purpose::Pull);
        assert_eq!(bench.workspace.get(id).unwrap().bytes.to_vec(), theirs);
    }

    /// A send placed by hand on a slot hides the row of the file linked there, and Sync
    /// writes it.
    #[test]
    fn a_send_placed_on_a_linked_files_slot_is_the_only_row_and_is_written() {
        let mut bench = Bench::new();
        walked(&mut bench, &[Some(crc(&body()))]);
        linked(&mut bench, "A.ne5p", gained("40"), 1, &body());
        let log = &mut bench.log;
        let placed = bench
            .workspace
            .ingest("B.ne5p".into(), Origin::Fresh, gained("41"), log);
        bench.workspace.settle_files(log);
        let (class, at1) = (PROGRAM, at(1));
        bench.act(vec![Act::Send {
            id: placed,
            class,
            at: at1,
        }]);
        assert_eq!(listed(&bench), [(placed, Change::Placed { waits: None })]);
        bench.sync();
        let ids: Vec<u64> = written(&bench).iter().map(|(id, _)| *id).collect();
        assert_eq!(ids, [placed]);
    }

    /// Following a moved sound changes links alone: a send placed on the slot it moved to
    /// is still written.
    #[test]
    fn following_a_move_leaves_a_send_placed_on_the_slot_it_moved_to() {
        let mut bench = Bench::new();
        walked_named(&mut bench, &[None, Some(("A", crc(&body())))]);
        let moved = linked(&mut bench, "A.ne5p", body(), 1, &body());
        let log = &mut bench.log;
        let placed = bench
            .workspace
            .ingest("B.ne5p".into(), Origin::Fresh, gained("41"), log);
        bench.workspace.settle_files(log);
        let (class, at2) = (PROGRAM, at(2));
        bench.act(vec![Act::Send {
            id: placed,
            class,
            at: at2,
        }]);
        bench.sync();
        assert_eq!(slot_of(&bench, moved), Some(Spot::at(at(2))));
        let ids: Vec<u64> = written(&bench).iter().map(|(id, _)| *id).collect();
        assert_eq!(ids, [placed]);
    }

    /// Picking ↑ over a version the keyboard changed stores nothing. A write the keyboard
    /// refuses leaves the link as it was, takes its entry out of the queue, and opens the
    /// sheet again on that row; a write that lands agrees the link.
    #[test]
    fn up_over_a_changed_slot_stores_nothing_until_its_write_lands() {
        let mut bench = Bench::new();
        let theirs = gained("40");
        walked(&mut bench, &[Some(crc(&theirs))]);
        let id = linked(&mut bench, "A.ne5p", body(), 1, &body());
        let keyboard = bench.device.state.keyboard().cloned().unwrap();
        let before = bench.workspace.get(id).unwrap().held_on(&keyboard).cloned();
        open_read(&mut bench);
        pick(&mut bench, id, Way::Up);
        bench.press();
        assert_eq!(written(&bench).len(), 1);
        let now = bench.workspace.get(id).unwrap().held_on(&keyboard).cloned();
        assert_eq!(now, before, "nothing stored at the press");
        bench.device.pump();

        let panel = program_info(1, "A", crc(&gained("42")));
        bench.device.pretend(DeviceEvent::Changed {
            id,
            class: PROGRAM,
            at: at(1),
            holds: Some(panel),
        });
        heard(&mut bench);
        let now = bench.workspace.get(id).unwrap().held_on(&keyboard).cloned();
        assert_eq!(now, before, "nor when the write is refused");
        assert!(
            !bench.queue.holds(id),
            "the row stands for it, not the queue"
        );
        let sheet = bench
            .device
            .asks
            .sheet
            .as_ref()
            .expect("the sheet opens again");
        assert_eq!(sheet.picked, Some(id));

        bench.device.pretend(DeviceEvent::Sent {
            id,
            class: PROGRAM,
            at: at(1),
            name: "A.ne5p".into(),
            sent: crate::device::Payload::Bytes(body()),
        });
        heard(&mut bench);
        let now = bench.workspace.get(id).unwrap().held_on(&keyboard).cloned();
        assert_eq!(
            now.map(|held| held.base),
            Some(print(&body())),
            "a landed write agrees"
        );
    }

    /// Pressing Sync during the first read after attaching keeps the pressed sheet: the
    /// sheet that opens by itself never takes its place.
    #[test]
    fn a_sheet_pressed_during_the_attach_read_is_not_replaced() {
        let mut bench = Bench::new();
        walked(&mut bench, &[Some(crc(&gained("40")))]);
        let id = linked(&mut bench, "A.ne5p", body(), 1, &body());
        bench.act(vec![Act::Sync(Some(id))]);
        heard(&mut bench);
        let sheet = bench.device.asks.sheet.as_ref().expect("open");
        assert!(matches!(sheet.opened, Opened::Pressed { .. }));
        assert!(!sheet.read(&bench.device.state), "still reading");
        assert_eq!(sheet.picked, Some(id));
    }

    /// The sheet warns of what every write going up disturbs, a linked file's as much as
    /// a send placed by hand.
    #[test]
    fn the_sheet_warns_of_writing_a_linked_settings_file() {
        use crate::testing::{painted, run, screen};
        let mut bench = Bench::new();
        bench.device.pretend_partitions(&ELECTRO5);
        let class = ObjectClass::Settings;
        bench.device.pretend_geometry(class, &[("A", 1)]);
        let settings = crate::workspace::Fresh::Settings.bytes().unwrap();
        bench
            .device
            .pretend_bodies(class, 1, &[Some(("Settings", crc(&settings) ^ 1))]);
        let log = &mut bench.log;
        let id = bench
            .workspace
            .ingest("S.nsys".into(), Origin::Fresh, settings, log);
        bench.workspace.settle_files(log);
        let keyboard = bench.device.state.keyboard().cloned().unwrap();
        let mut held = Held::new(
            class,
            at(1),
            "",
            Print {
                crc: crc(&gained("1")),
                len: 1,
            },
        )
        .unwrap();
        held.past
            .push(crc(&bench.workspace.get(id).unwrap().bytes) ^ 1);
        bench.workspace.link(id, &keyboard, held);
        assert_eq!(listed(&bench), [(id, Change::Ahead)]);
        open_read(&mut bench);
        let sync = Rows::of(&bench.device.state, &bench.workspace, &bench.queue);
        let mut said = Vec::new();
        for _ in 0..2 {
            let output = run(
                &bench.ctx,
                screen(eframe::egui::vec2(1280.0, 800.0), Vec::new()),
                |ctx| {
                    let (device, queue) = (&mut bench.device, &bench.queue);
                    crate::queue::sheet(
                        ctx,
                        device,
                        (queue, &sync),
                        &bench.workspace,
                        &mut Vec::new(),
                    );
                },
            );
            said = painted(&output);
        }
        let warning = crate::device::write_warning(class).expect("Settings warns");
        let first = warning.split(' ').next().unwrap();
        assert!(
            said.iter().any(|word| word.text.starts_with(first)),
            "{said:?}"
        );
    }

    fn ids(steps: &[(u64, Expect)]) -> Vec<u64> {
        steps.iter().map(|(id, _)| *id).collect()
    }

    /// A send placed by hand that the keyboard refused because its slot changed comes
    /// back in the sheet saying what the slot holds now, counted, and nothing is written
    /// until Sync is pressed; then exactly the rows counted are written.
    #[test]
    fn a_refused_placed_send_is_shown_again_counted_and_written_only_when_pressed() {
        let mut bench = Bench::new();
        walked(&mut bench, &[None, Some(crc(&body()))]);
        let ahead = linked(&mut bench, "A.ne5p", gained("40"), 2, &body());
        let log = &mut bench.log;
        let placed = bench
            .workspace
            .ingest("B.ne5p".into(), Origin::Fresh, gained("41"), log);
        bench.workspace.settle_files(log);
        let (class, at1) = (PROGRAM, at(1));
        bench.act(vec![Act::Send {
            id: placed,
            class,
            at: at1,
        }]);
        bench.sync();
        bench.device.pump();
        let panel = program_info(1, "Clav", crc(&gained("42")));
        bench.device.pretend(DeviceEvent::Changed {
            id: placed,
            class,
            at: at1,
            holds: Some(panel.clone()),
        });
        bench.device.pretend(DeviceEvent::Finished);
        heard(&mut bench);
        assert!(bench.device.asks.sheet.is_some(), "the sheet reads again");
        let slots = vec![Some(panel), Some(program_info(2, "B", crc(&body())))];
        bench.device.pretend(DeviceEvent::BankScanned {
            class,
            bank: 1,
            slots,
        });
        bench.land();
        let sync = Rows::of(&bench.device.state, &bench.workspace, &bench.queue);
        let row = sync.list.iter().find(|row| row.id == placed).unwrap();
        assert_eq!(
            row.said(&bench.workspace, &bench.queue),
            "to 1:1 · replaces “Clav”"
        );
        let sheet = bench.device.asks.sheet.as_ref().unwrap();
        let counted = plan(sheet, &sync.list, &bench.device.state, &bench.workspace);
        assert_eq!(counted.len(), 2);
        let before = written(&bench).len();
        bench.press();
        let now = written(&bench);
        assert_eq!(
            ids(&now[before..]),
            [placed, ahead],
            "exactly the rows counted"
        );
    }

    /// A linked file changed after the sheet was painted is not written by that Sync, as
    /// a row the sheet showed or as one it did not, and the sheet comes back for it.
    #[test]
    fn a_file_changed_after_the_sheet_was_painted_is_not_written() {
        let mut bench = Bench::new();
        walked(&mut bench, &[Some(crc(&body())), Some(crc(&body()))]);
        let ahead = linked(&mut bench, "A.ne5p", gained("40"), 1, &body());
        let agreed = linked(&mut bench, "B.ne5p", body(), 2, &body());
        open_read(&mut bench);
        let sync = Rows::of(&bench.device.state, &bench.workspace, &bench.queue);
        accept(&mut bench.device, &sync, &bench.workspace);
        for (id, gain) in [(ahead, "50"), (agreed, "51")] {
            bench
                .workspace
                .replace_bytes(id, gained(gain), &mut bench.log);
            bench.workspace.mark_saved(id);
        }
        bench.act(vec![Act::SyncAll]);
        assert_eq!(written(&bench), []);
        let sheet = bench
            .device
            .asks
            .sheet
            .as_ref()
            .expect("back for a second look");
        assert_eq!(sheet.picked, Some(ahead));
    }

    /// A file saved again after its ↓ row was accepted, before the slot's bytes arrive,
    /// is not overwritten.
    #[test]
    fn a_file_saved_again_before_its_slot_arrives_is_not_overwritten() {
        let mut bench = Bench::new();
        let theirs = gained("40");
        walked(&mut bench, &[Some(crc(&theirs))]);
        let id = linked(&mut bench, "A.ne5p", body(), 1, &body());
        bench.sync();
        assert_eq!(pulls(&bench), [at(1)]);
        bench
            .workspace
            .replace_bytes(id, gained("60"), &mut bench.log);
        bench.workspace.mark_saved(id);
        got(&mut bench, 1, theirs, Purpose::Pull);
        assert_eq!(
            bench.workspace.get(id).unwrap().bytes.to_vec(),
            gained("60")
        );
    }

    /// Where a write cannot start, nothing changes: no link moves, nothing is unlinked or
    /// read, and the log says nothing was synced.
    #[test]
    fn nothing_changes_when_the_batch_cannot_start() {
        let mut bench = Bench::new();
        walked_named(&mut bench, &[None, Some(("A", crc(&body())))]);
        let moved = linked(&mut bench, "A.ne5p", body(), 1, &body());
        let log = &mut bench.log;
        let placed = bench
            .workspace
            .ingest("B.ne5p".into(), Origin::Fresh, gained("41"), log);
        bench.workspace.settle_files(log);
        let unread = Location::from_user(2, 1);
        bench.act(vec![Act::Send {
            id: placed,
            class: PROGRAM,
            at: unread,
        }]);
        bench.sync();
        assert_eq!(
            slot_of(&bench, moved),
            Some(Spot::at(at(1))),
            "the link stays"
        );
        assert_eq!((written(&bench), pulls(&bench)), (vec![], vec![]));
        assert!(bench.log.status().1.starts_with("Nothing was synced"));
    }

    /// Where the slot reports no checksum, the bytes brought in are checked by their
    /// length and the name the slot reports.
    #[test]
    fn a_slot_that_reports_no_checksum_is_brought_in_by_length_and_name() {
        let mut bench = Bench::new();
        let theirs = crate::workspace::Fresh::Stage4Program.bytes().unwrap();
        let len = print(&theirs).len;
        bench.device.pretend_partitions(&ELECTRO5);
        bench.device.pretend_geometry(PROGRAM, &[("A", 1)]);
        bench
            .device
            .pretend_lengths(PROGRAM, 1, &[Some(("A", len))]);
        let id = linked(&mut bench, "A.ne5p", body(), 1, &body());
        assert_eq!(listed(&bench), [(id, Change::Theirs)]);
        bench.sync();
        let origin = Origin::Device {
            class: PROGRAM,
            at: at(1),
        };
        let mut info = crate::device::reported(at(1), "A");
        info.body_len = len;
        bench.device.pretend(DeviceEvent::Got {
            info,
            origin,
            bytes: theirs.clone(),
            why: Purpose::Pull,
        });
        heard(&mut bench);
        assert_eq!(bench.workspace.get(id).unwrap().bytes.to_vec(), theirs);
    }

    /// A batch that fails leaves nothing behind in the queue: the file is its own row
    /// again, never one placed by hand.
    #[test]
    fn a_failed_batch_leaves_nothing_behind() {
        let mut bench = Bench::new();
        walked(&mut bench, &[Some(crc(&body()))]);
        let id = linked(&mut bench, "A.ne5p", gained("40"), 1, &body());
        bench.sync();
        bench.device.pump();
        bench
            .device
            .pretend(DeviceEvent::OpFailed("refused".into()));
        let slots = vec![Some(program_info(1, "A", crc(&body())))];
        bench.device.pretend(DeviceEvent::BankScanned {
            class: PROGRAM,
            bank: 1,
            slots,
        });
        heard(&mut bench);
        assert!(bench.queue.is_empty());
        assert_eq!(listed(&bench), [(id, Change::Ahead)]);
    }

    /// Leaving out one move of a swap leaves out both.
    #[test]
    fn half_a_swap_cannot_go() {
        let mut bench = Bench::new();
        let (a, b) = (body(), gained("40"));
        walked_named(&mut bench, &[Some(("B", crc(&b))), Some(("A", crc(&a)))]);
        let first = linked(&mut bench, "A.ne5p", a.clone(), 1, &a);
        let second = linked(&mut bench, "B.ne5p", b.clone(), 2, &b);
        open_read(&mut bench);
        turn_off(&mut bench, second);
        let sync = Rows::of(&bench.device.state, &bench.workspace, &bench.queue);
        let sheet = bench.device.asks.sheet.as_ref().unwrap();
        let mine = row(&bench, first);
        assert_eq!(sheet.way(&mine, &sync.list, &bench.workspace), None);
        assert_eq!(
            plan(sheet, &sync.list, &bench.device.state, &bench.workspace),
            []
        );
        bench.press();
        assert_eq!(slot_of(&bench, first), Some(Spot::at(at(1))));
        assert_eq!(slot_of(&bench, second), Some(Spot::at(at(2))));
    }

    #[test]
    fn undoing_a_move_leaves_a_file_linked_elsewhere_since() {
        let mut bench = Bench::new();
        let (a, b) = (body(), gained("40"));
        walked_named(
            &mut bench,
            &[Some(("B", crc(&b))), Some(("A", crc(&a))), None],
        );
        let first = linked(&mut bench, "A.ne5p", a.clone(), 1, &a);
        let second = linked(&mut bench, "B.ne5p", b.clone(), 2, &b);
        bench.sync();
        let keyboard = bench.device.state.keyboard().cloned().unwrap();
        let elsewhere = Held::new(PROGRAM, at(3), "A", print(&a)).unwrap();
        bench.workspace.link(first, &keyboard, elsewhere);

        let said = undo(&mut bench, "files moved with their sounds");
        assert_eq!(said, "Linked “B” again; “A” relinked since.");
        assert_eq!(slot_of(&bench, first), Some(Spot::at(at(3))));
        assert_eq!(slot_of(&bench, second), Some(Spot::at(at(2))));
    }

    #[test]
    fn turning_one_move_of_a_swap_back_on_takes_the_other() {
        let mut bench = Bench::new();
        let (a, b) = (body(), gained("40"));
        walked_named(&mut bench, &[Some(("B", crc(&b))), Some(("A", crc(&a)))]);
        let first = linked(&mut bench, "A.ne5p", a.clone(), 1, &a);
        let second = linked(&mut bench, "B.ne5p", b.clone(), 2, &b);
        open_read(&mut bench);
        turn_off(&mut bench, second);
        pick(&mut bench, first, Way::Down);
        let sync = Rows::of(&bench.device.state, &bench.workspace, &bench.queue);
        let sheet = bench.device.asks.sheet.as_ref().unwrap();
        let ways = [first, second].map(|id| {
            let row = row(&bench, id);
            sheet.way(&row, &sync.list, &bench.workspace)
        });
        assert_eq!(ways, [Some(Way::Down); 2]);
    }

    /// A file with unsaved edits whose slot changed on the keyboard may only go up.
    #[test]
    fn a_row_with_one_way_is_turned_on_and_off_by_the_same_arrow() {
        let mut bench = Bench::new();
        walked(&mut bench, &[Some(crc(&gained("40")))]);
        let id = linked(&mut bench, "A.ne5p", body(), 1, &body());
        let log = &mut bench.log;
        bench.workspace.replace_bytes(id, gained("60"), log);
        open_read(&mut bench);
        let all = rows(&bench.device.state, &bench.workspace, &bench.queue);
        let row = row(&bench, id);
        let workspace = &bench.workspace;
        let sheet = bench.device.asks.sheet.as_mut().unwrap();
        assert!(sheet.waits(&row, &all, workspace));

        sheet.turn(&row, Way::Down, &all, workspace);
        assert_eq!(sheet.way(&row, &all, workspace), None, "it may not go down");
        sheet.turn(&row, Way::Up, &all, workspace);
        assert_eq!(sheet.way(&row, &all, workspace), Some(Way::Up));
        assert!(!sheet.waits(&row, &all, workspace));
        sheet.turn(&row, Way::Up, &all, workspace);
        assert_eq!(sheet.way(&row, &all, workspace), None);
        assert!(sheet.waits(&row, &all, workspace), "it waits again");
    }

    /// One frame of the sheet, given `events`, as the text it painted.
    fn frame(bench: &mut Bench, events: Vec<eframe::egui::Event>) -> Vec<crate::testing::Word> {
        use crate::testing::{painted, run, screen};
        let sync = Rows::of(&bench.device.state, &bench.workspace, &bench.queue);
        let input = screen(eframe::egui::vec2(1280.0, 800.0), events);
        let output = run(&bench.ctx, input, |ctx| {
            let (device, queue) = (&mut bench.device, &bench.queue);
            crate::queue::sheet(
                ctx,
                device,
                (queue, &sync),
                &bench.workspace,
                &mut Vec::new(),
            );
        });
        painted(&output)
    }

    /// Every string the sheet paints, after a frame to settle its layout.
    fn painted_sheet(bench: &mut Bench) -> Vec<String> {
        frame(bench, Vec::new());
        let said = frame(bench, Vec::new());
        said.into_iter().map(|word| word.text).collect()
    }

    /// A file with unsaved edits whose slot changed on the keyboard may only go up: its
    /// `↓` takes no click, and its `↑` is a switch.
    #[test]
    fn the_one_arrow_of_a_waiting_row_lights_on_a_click_and_goes_out_on_the_next() {
        use crate::testing::{click, where_};
        let mut bench = Bench::new();
        walked(&mut bench, &[Some(crc(&gained("40")))]);
        let id = linked(&mut bench, "A.ne5p", body(), 1, &body());
        let log = &mut bench.log;
        bench.workspace.replace_bytes(id, gained("60"), log);
        open_read(&mut bench);
        frame(&mut bench, Vec::new());
        let said = frame(&mut bench, Vec::new());
        let (up, down) = (where_(&said, "↑").center(), where_(&said, "↓").center());
        let shown = |bench: &mut Bench, at| {
            frame(bench, click(at));
            let said = painted_sheet(bench);
            let has = |text: &str| said.iter().any(|it| it == text);
            (has("!"), has("Sync 1"))
        };
        assert_eq!(shown(&mut bench, down), (true, false), "it may not go down");
        assert_eq!(shown(&mut bench, up), (false, true), "lit, and counted");
        assert_eq!(shown(&mut bench, up), (true, false), "it waits again");
    }

    /// A file has one row with a way: placed by hand on another slot while its own slot
    /// changes at the panel, Sync brings the panel's version in and writes nothing, and
    /// the placed send goes once the file's own row is settled.
    #[test]
    fn a_file_placed_elsewhere_whose_slot_the_panel_changed_is_brought_in_first() {
        let mut bench = Bench::new();
        walked(&mut bench, &[Some(crc(&body())), None]);
        let id = linked(&mut bench, "A.ne5p", body(), 1, &body());
        let (class, at2) = (PROGRAM, at(2));
        bench.act(vec![Act::Send { id, class, at: at2 }]);
        assert!(bench.queue.holds(id));
        let theirs = gained("40");
        let slots = vec![Some(program_info(1, "A", crc(&theirs))), None];
        bench.device.pretend(DeviceEvent::BankScanned {
            class,
            bank: 1,
            slots,
        });
        heard(&mut bench);
        let placed = row(&bench, id);
        assert_eq!(
            placed.said(&bench.workspace, &bench.queue),
            "sync “A” first"
        );
        bench.sync();
        assert_eq!((pulls(&bench), written(&bench)), (vec![at(1)], vec![]));
        got(&mut bench, 1, theirs.clone(), Purpose::Pull);
        assert_eq!(bench.workspace.get(id).unwrap().bytes.to_vec(), theirs);
        assert_eq!(slot_of(&bench, id), Some(Spot::at(at(1))));
        assert_eq!(listed(&bench), [(id, Change::Placed { waits: None })]);
    }

    /// A move onto the slot of a file that is not moving back has no way: its row is
    /// marked `!`, and the footer counts what Sync runs.
    #[test]
    fn a_move_onto_a_file_that_stays_paints_no_way_and_is_not_counted() {
        let mut bench = Bench::new();
        let (a, b) = (body(), gained("40"));
        walked_named(
            &mut bench,
            &[Some(("B", crc(&b))), Some(("A", crc(&a))), None],
        );
        let first = linked(&mut bench, "A.ne5p", a.clone(), 1, &a);
        linked(&mut bench, "B.ne5p", b.clone(), 2, &b);
        let log = &mut bench.log;
        let placed = bench
            .workspace
            .ingest("C.ne5p".into(), Origin::Fresh, gained("41"), log);
        bench.workspace.settle_files(log);
        let (class, at2) = (PROGRAM, at(2));
        bench.act(vec![Act::Send {
            id: placed,
            class,
            at: at2,
        }]);
        open_read(&mut bench);
        let sync = Rows::of(&bench.device.state, &bench.workspace, &bench.queue);
        let sheet = bench.device.asks.sheet.as_ref().unwrap();
        let mine = row(&bench, first);
        assert_eq!(sheet.way(&mine, &sync.list, &bench.workspace), None);
        let steps = plan(sheet, &sync.list, &bench.device.state, &bench.workspace);
        let ids: Vec<u64> = steps.iter().map(|step| step.row.id).collect();
        assert_eq!(ids, [placed]);
        let said = painted_sheet(&mut bench);
        assert!(
            said.iter().any(|it| it == "!") && said.iter().any(|it| it == "Sync 1"),
            "{said:?}"
        );
    }

    /// Once Sync is pressed the sheet takes no more choices: it says it is syncing, lists
    /// no row to turn, and what runs is the plan it was pressed on.
    #[test]
    fn a_pressed_sheet_takes_no_more_choices_and_runs_what_was_pressed() {
        let mut bench = Bench::new();
        walked(&mut bench, &[Some(crc(&body())), Some(crc(&body()))]);
        let first = linked(&mut bench, "A.ne5p", gained("40"), 1, &body());
        let second = linked(&mut bench, "B.ne5p", gained("41"), 2, &body());
        open_read(&mut bench);
        turn_off(&mut bench, second);
        let sync = Rows::of(&bench.device.state, &bench.workspace, &bench.queue);
        accept(&mut bench.device, &sync, &bench.workspace);
        let said = painted_sheet(&mut bench);
        assert!(said.iter().any(|it| it == "Syncing…"), "{said:?}");
        assert!(!said.iter().any(|it| it == "B" || it == "↑"), "{said:?}");
        bench.act(vec![Act::SyncAll]);
        assert_eq!(ids(&written(&bench)), [first], "the plan pressed on");
    }

    /// A rotation on the keyboard moves every link with its sound, or none: with one move
    /// left out, none of them has a way, Sync counts none, and no link changes.
    #[test]
    fn a_rotation_goes_whole_or_not_at_all() {
        let (a, b, c) = (body(), gained("40"), gained("41"));
        let rotated = [
            Some(("C", crc(&c))),
            Some(("A", crc(&a))),
            Some(("B", crc(&b))),
        ];
        for leave_out in [false, true] {
            let mut bench = Bench::new();
            walked_named(&mut bench, &rotated);
            let files = [
                linked(&mut bench, "A.ne5p", a.clone(), 1, &a),
                linked(&mut bench, "B.ne5p", b.clone(), 2, &b),
                linked(&mut bench, "C.ne5p", c.clone(), 3, &c),
            ];
            open_read(&mut bench);
            if leave_out {
                turn_off(&mut bench, files[2]);
            }
            let sync = Rows::of(&bench.device.state, &bench.workspace, &bench.queue);
            let sheet = bench.device.asks.sheet.as_ref().unwrap();
            let steps = plan(sheet, &sync.list, &bench.device.state, &bench.workspace);
            bench.press();
            let slots: Vec<Option<Spot>> = files.iter().map(|id| slot_of(&bench, *id)).collect();
            let expected = match leave_out {
                false => [2, 3, 1],
                true => [1, 2, 3],
            };
            let expected: Vec<Option<Spot>> = expected
                .iter()
                .map(|slot| Some(Spot::at(at(*slot))))
                .collect();
            assert_eq!(
                (steps.len(), slots),
                (3 * usize::from(!leave_out), expected)
            );
        }
    }
}
