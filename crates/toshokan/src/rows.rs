//! Rows: every file a view shows, each with a [`FileId`] that continues from look
//! to look by the rules of Rows in `spec/protocol.md`.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::{Arc, OnceLock};

use serde::{Deserialize, Serialize};

use crate::binding::{self, Bindings, Scan, Unscanned};
use crate::effects::Applied;
use crate::env::Names;
use crate::ids::{EntityId, EntryHash, WriterId};
use crate::log::Ref;
use crate::merge::{side_by_side, Folded, Part, Shift, Since};
use crate::path::RelPath;
use crate::view::FileState;

/// A file as one `Library` value knows it from look to look: the value's random
/// epoch plus a counter, so a FileId of another value names no file of this one.
/// Never written anywhere.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug, Serialize, Deserialize)]
#[serde(transparent)]
pub struct FileId(u64);

impl FileId {
    pub const fn to_u64(self) -> u64 {
        self.0
    }
}

/// Draws the FileIds of one `Library` value.
#[derive(Clone, PartialEq, Debug)]
pub(crate) struct FileIds {
    next: u64,
}

impl FileIds {
    pub(crate) fn from_epoch(epoch: u64) -> Self {
        Self { next: epoch }
    }

    fn draw(&mut self) -> FileId {
        let id = FileId(self.next);
        self.next = self.next.wrapping_add(1);
        id
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub enum RowState {
    /// The listing holds the file.
    Present { len: u64, modified: Option<u64> },
    /// The path is under one the scan cannot list.
    Unknown,
    /// The row's entity's file is nowhere the binding finds it: the path is the
    /// logged one.
    Missing,
}

/// One file a view shows.
#[derive(Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub struct Row {
    pub file: FileId,
    pub path: RelPath,
    pub state: RowState,
    /// The entity the binding binds the file to.
    pub entity: Option<EntityId>,
}

/// Where a row is kept.
#[derive(Clone, Copy, Debug)]
enum Slot {
    Listed(usize),
    Absent(usize),
}

/// The rows of one look.
#[derive(Debug, Default)]
pub struct Rows {
    /// The Present rows, by path: the listing.
    listed: Vec<Row>,
    /// The Missing and Unknown rows, by FileId.
    absent: Vec<Row>,
    /// The directories below the root the listing holds.
    dirs: BTreeSet<RelPath>,
    /// Where each row is kept, built on first use: by FileId, by entity, and the
    /// absent rows by path.
    by_file: OnceLock<Vec<(FileId, Slot)>>,
    by_entity: OnceLock<Vec<(EntityId, Slot)>>,
    absent_at: OnceLock<Vec<usize>>,
}

impl Rows {
    /// The listing's rows by path, then the absent rows by FileId.
    pub fn iter(&self) -> impl Iterator<Item = &Row> {
        self.listed.iter().chain(&self.absent)
    }

    /// The listing's rows, by path.
    pub(crate) fn listing(&self) -> impl Iterator<Item = &Row> {
        self.listed.iter()
    }

    /// The Missing and Unknown rows, by FileId.
    pub(crate) fn absent(&self) -> impl Iterator<Item = &Row> {
        self.absent.iter()
    }

    pub fn dirs(&self) -> impl Iterator<Item = &RelPath> {
        self.dirs.iter()
    }

    /// The listing's row at exactly `path`.
    pub fn listed(&self, path: &RelPath) -> Option<&Row> {
        let at = self.listed.binary_search_by(|row| row.path.cmp(path));
        at.ok().map(|at| &self.listed[at])
    }

    /// The row at exactly `path`: the listing's, else the first absent one by FileId.
    pub fn at(&self, path: &RelPath) -> Option<&Row> {
        self.listed(path).or_else(|| self.absent_at(path).next())
    }

    /// The absent rows at exactly `path`, by FileId.
    fn absent_at(&self, path: &RelPath) -> impl Iterator<Item = &Row> {
        let absent_at = self.absent_at.get_or_init(|| {
            let mut absent_at: Vec<usize> = (0..self.absent.len()).collect();
            absent_at.sort_by(|a, b| self.absent[*a].path.cmp(&self.absent[*b].path));
            absent_at
        });
        let first = absent_at.partition_point(|at| self.absent[*at].path < *path);
        let end = absent_at.partition_point(|at| self.absent[*at].path <= *path);
        absent_at[first..end].iter().map(|at| &self.absent[*at])
    }

    pub fn row(&self, file: FileId) -> Option<&Row> {
        let by_file = self.by_file();
        let at = by_file
            .binary_search_by_key(&file, |(file, _)| *file)
            .ok()?;
        Some(self.slot(by_file[at].1))
    }

    pub fn of(&self, entity: EntityId) -> Option<&Row> {
        let by_entity = self.by_entity.get_or_init(|| {
            let mut by_entity: Vec<(EntityId, Slot)> = self
                .slots()
                .filter_map(|(row, slot)| Some((row.entity?, slot)))
                .collect();
            by_entity.sort_unstable_by_key(|(entity, _)| *entity);
            by_entity
        });
        let at = by_entity.binary_search_by_key(&entity, |(entity, _)| *entity);
        Some(self.slot(by_entity[at.ok()?].1))
    }

    fn slot(&self, slot: Slot) -> &Row {
        match slot {
            Slot::Listed(at) => &self.listed[at],
            Slot::Absent(at) => &self.absent[at],
        }
    }

    /// Every row with where it is kept.
    fn slots(&self) -> impl Iterator<Item = (&Row, Slot)> {
        let listed = self.listed.iter().enumerate();
        let listed = listed.map(|(at, row)| (row, Slot::Listed(at)));
        let absent = self.absent.iter().enumerate();
        listed.chain(absent.map(|(at, row)| (row, Slot::Absent(at))))
    }

    fn by_file(&self) -> &[(FileId, Slot)] {
        self.by_file.get_or_init(|| {
            let mut by_file: Vec<(FileId, Slot)> =
                self.slots().map(|(row, slot)| (row.file, slot)).collect();
            by_file.sort_unstable_by_key(|(file, _)| *file);
            by_file
        })
    }

    /// Every row, by FileId.
    fn in_file_order(&self) -> impl Iterator<Item = &Row> {
        self.by_file().iter().map(|(_, slot)| self.slot(*slot))
    }

    /// The rows at `path` or under it.
    fn under<'a>(&'a self, path: &'a RelPath) -> impl Iterator<Item = &'a Row> + 'a {
        let first = self.listed.partition_point(|row| row.path < *path);
        let listed = self.listed[first..].iter();
        let listed = listed.take_while(move |row| row.path.as_str().starts_with(path.as_str()));
        let absent = self
            .absent
            .iter()
            .filter(move |row| row.path.starts_with(path));
        listed
            .filter(move |row| row.path.starts_with(path))
            .chain(absent)
    }
}

/// What this instance's own effects did to rows since the last look: rule 1.
#[derive(Debug, Default)]
pub(crate) struct Carried {
    /// The row at each path the effects moved or wrote a file to.
    at: BTreeMap<RelPath, FileId>,
    /// The path of each row of `at`.
    paths: BTreeMap<FileId, RelPath>,
    /// The rows the effects took from the path the rows give them.
    left: BTreeSet<FileId>,
}

impl Carried {
    pub(crate) fn is_empty(&self) -> bool {
        self.at.is_empty() && self.left.is_empty()
    }

    /// The row at `path` now: one an effect put there, else the listing's there, else
    /// an absent one there with no entity. An absent entity's row is its own.
    fn at(&self, rows: &Rows, path: &RelPath) -> Option<FileId> {
        if let Some(id) = self.at.get(path) {
            return Some(*id);
        }
        let mut absent = rows.absent_at(path).filter(|row| row.entity.is_none());
        let row = rows.listed(path).or_else(|| absent.next());
        row.filter(|row| !self.left.contains(&row.file))
            .map(|row| row.file)
    }

    fn put(&mut self, id: FileId, path: &RelPath) {
        if let Some(was) = self.paths.insert(id, path.clone()) {
            self.at.remove(&was);
        }
        if let Some(had) = self.at.insert(path.clone(), id).filter(|had| *had != id) {
            self.paths.remove(&had);
        }
        self.left.insert(id);
    }

    fn end(&mut self, id: Option<FileId>) {
        if let Some(id) = id {
            if let Some(was) = self.paths.remove(&id) {
                self.at.remove(&was);
            }
            self.left.insert(id);
        }
    }

    /// Takes in what `applied` did, against `rows`, the last look's. Returns each
    /// file it moved or wrote, with its row.
    pub(crate) fn applied(
        &mut self,
        rows: &Rows,
        ids: &mut FileIds,
        applied: &Applied,
    ) -> Vec<(RelPath, FileId)> {
        let mut files = Vec::new();
        for tree in &applied.trees {
            // An entity's file arrives with `applied.files`, and a file the plan did
            // not know of moves only with its whole directory.
            let moved = |row: &&Row| {
                let listed = matches!(row.state, RowState::Present { .. });
                row.entity.is_none() && (listed || tree.whole) && !self.left.contains(&row.file)
            };
            let mut moved: Vec<(RelPath, FileId)> = rows
                .under(&tree.from)
                .filter(moved)
                .map(|row| (row.path.clone(), row.file))
                .collect();
            let carried = self
                .at
                .iter()
                .filter(|(path, _)| path.starts_with(&tree.from));
            moved.extend(carried.map(|(path, id)| (path.clone(), *id)));
            for (path, id) in moved {
                let path = path.rebase(&tree.from, &tree.to).expect("under the tree");
                self.put(id, &path);
                files.push((path, id));
            }
        }
        for (entity, file) in &applied.files {
            let row = rows.of(*entity).map(|row| row.file);
            let Some(file) = file else {
                self.end(row);
                continue;
            };
            let id = row.or_else(|| self.at(rows, &file.path));
            let id = id.unwrap_or_else(|| ids.draw());
            self.put(id, &file.path);
            files.push((file.path.clone(), id));
        }
        for (from, file) in &applied.unbound {
            let was = from.as_ref().and_then(|from| self.at(rows, from));
            let Some(file) = file else {
                self.end(was);
                continue;
            };
            let id = was.or_else(|| self.at(rows, &file.path));
            let id = id.unwrap_or_else(|| ids.draw());
            self.put(id, &file.path);
            files.push((file.path.clone(), id));
        }
        files
    }
}

/// What a look knows of the folder, which the rows follow.
pub(crate) struct Look<'a> {
    /// The files and directories listed; `None` until a scan of every file.
    pub scan: Option<&'a Scan>,
    pub bindings: &'a Bindings,
    /// What no scan has listed since it changed.
    pub unknown: &'a Unscanned,
    pub names: &'a dyn Names,
}

/// A look, with the paths it does not know as [`binding::under`] takes them.
struct Known<'a> {
    look: &'a Look<'a>,
    unknown: BTreeSet<&'a str>,
}

impl Known<'_> {
    fn knows(&self, path: &RelPath) -> bool {
        self.look.scan.is_some()
            && (self.unknown.is_empty() || !binding::under(path, &self.unknown))
    }

    fn lists(&self, path: &RelPath) -> bool {
        let listed = self
            .look
            .scan
            .is_some_and(|scan| scan.files.contains_key(path));
        listed && self.knows(path)
    }
}

/// A row that ended, merged into another, as one look found it.
pub(crate) type Merge = (FileId, FileId);

impl Rows {
    /// The rows of `look`, continuing these by `carried` (rule 1), the binding (rule
    /// 2) and the same path (rule 3), with the rows that ended merged into another.
    /// `since` is the binding these rows followed, where `look` knows what they knew:
    /// the row of an entity whose binding, joins and own effects are as they were then
    /// continues as it was.
    pub(crate) fn continued(
        &self,
        carried: &Carried,
        look: &Look,
        ids: &mut FileIds,
        since: Option<&Bindings>,
    ) -> (Rows, Vec<Merge>) {
        let listed = |path: &RelPath| look.scan.is_some_and(|scan| scan.files.contains_key(path));
        let respelled = carried.at.iter().filter(|(path, _)| !listed(path));
        let respelled = respelled.map(|(path, id)| (look.names.key(path.as_str()), *id));
        let dirty = since.map(|was| self.dirty(carried, was, look.bindings));
        let mut continuing = Continuing {
            from: self,
            carried,
            kept: Kept(dirty.as_ref()),
            respelled: respelled.collect(),
            known: Known {
                look,
                unknown: look.unknown.paths.iter().map(RelPath::as_str).collect(),
            },
            ids,
            taken: BTreeSet::new(),
            merged: Vec::new(),
            absent: Vec::new(),
        };
        let tagged = continuing.entities();
        let listed = continuing.listed(&tagged);
        continuing.unknown();
        let Continuing {
            mut absent, merged, ..
        } = continuing;
        absent.sort_by_key(|row| row.file);
        let rows = Rows {
            listed,
            absent,
            dirs: look.scan.map(|scan| scan.dirs.clone()).unwrap_or_default(),
            ..Rows::default()
        };
        (rows, merged)
    }

    /// The entities whose rows may not continue as they were: those `was`, the
    /// binding these rows followed, and `now` bind or join otherwise, and those of the
    /// rows `carried` took or put a file over.
    fn dirty(&self, carried: &Carried, was: &Bindings, now: &Bindings) -> BTreeSet<EntityId> {
        let rebound = side_by_side(&now.bound, &was.bound)
            .filter(|(now, was)| now.map(|(_, file)| file) != was.map(|(_, file)| file));
        let mut dirty: BTreeSet<EntityId> = rebound
            .filter_map(|(now, was)| now.or(was).map(|(entity, _)| *entity))
            .collect();
        let joins = |bindings: &Bindings| -> BTreeSet<(EntityId, EntityId)> {
            bindings.shown_on().collect()
        };
        for (entity, into) in joins(now).symmetric_difference(&joins(was)) {
            dirty.extend([*entity, *into]);
        }
        let taken = carried.left.iter().filter_map(|id| self.row(*id));
        let under = carried.at.keys();
        let under =
            under.flat_map(|path| self.listed(path).into_iter().chain(self.absent_at(path)));
        dirty.extend(taken.chain(under).filter_map(|row| row.entity));
        dirty
    }
}

/// The entities whose rows continue as they were: every one but those of the set
/// it holds, and none without one.
#[derive(Clone, Copy)]
struct Kept<'a>(Option<&'a BTreeSet<EntityId>>);

impl Kept<'_> {
    fn entity(self, entity: EntityId) -> bool {
        self.0.is_some_and(|dirty| !dirty.contains(&entity))
    }

    fn row(self, row: &Row) -> bool {
        row.entity.is_some_and(|entity| self.entity(entity))
    }
}

/// The rows of one look being derived from the last look's.
struct Continuing<'a> {
    from: &'a Rows,
    carried: &'a Carried,
    kept: Kept<'a>,
    /// The rows own effects carried to a path the listing does not hold, by the
    /// form the volume takes the path under.
    respelled: BTreeMap<String, FileId>,
    known: Known<'a>,
    ids: &'a mut FileIds,
    /// The last look's rows that continue as a row of an entity, or as Unknown.
    taken: BTreeSet<FileId>,
    merged: Vec<Merge>,
    absent: Vec<Row>,
}

impl<'a> Continuing<'a> {
    /// Each bound entity's row, by rule 2: the listed ones by path, and the others
    /// absent. Where rows meet on one file, the row of the entity bound to it
    /// survives, else the row there, and the others merge into it.
    fn entities(&mut self) -> BTreeMap<&'a RelPath, (FileId, EntityId)> {
        let bindings = self.known.look.bindings;
        let from = self.from;
        let of_entity = |entity: &EntityId| from.of(*entity).map(|row| row.file);
        let mut joined: BTreeMap<EntityId, Vec<EntityId>> = BTreeMap::new();
        for (entity, into) in bindings.shown_on() {
            joined.entry(into).or_default().push(entity);
        }
        let joins = |entity: &EntityId| {
            let joins = joined.get(entity).into_iter().flatten();
            joins.filter_map(of_entity)
        };
        let claims = |entity: &EntityId| of_entity(entity).into_iter().chain(joins(entity));
        let kept = self.kept;
        let rederived = bindings
            .bound
            .iter()
            .filter(|(entity, _)| !kept.entity(**entity));
        let claimed: BTreeSet<FileId> = rederived.clone().flat_map(|(e, _)| claims(e)).collect();
        // A row an entity kept as it was is its own.
        let owned = |id: &FileId| {
            claimed.contains(id) || (kept.0.is_some() && from.row(*id).is_some_and(|r| kept.row(r)))
        };
        let mut tagged = BTreeMap::new();
        for (entity, file) in rederived {
            let free = |id: &FileId| !self.taken.contains(id);
            let own = of_entity(entity).filter(free);
            let listed = matches!(file.state, FileState::InSync | FileState::ChangedOutside)
                && self.known.lists(&file.path);
            let carried = self.carried_to(&file.path);
            let there = match (file.state, listed) {
                (_, true) | (FileState::Unscanned, false) => {
                    carried.or_else(|| self.carried.at(self.from, &file.path))
                }
                // A file listed at the path of an entity bound to none is not its file.
                _ if self.known.lists(&file.path) => None,
                _ => carried,
            };
            let there = there.filter(|id| !owned(id) && free(id));
            let mut rows: Vec<FileId> = own.into_iter().chain(there).collect();
            rows.extend(joins(entity).filter(free));
            let survivor = rows.first().copied().unwrap_or_else(|| self.ids.draw());
            let merged = rows.iter().skip(1).map(|id| (*id, survivor));
            self.merged.extend(merged);
            self.taken.extend(rows);
            self.taken.insert(survivor);
            let state = match (file.state, listed) {
                (_, true) => {
                    tagged.insert(&file.path, (survivor, *entity));
                    continue;
                }
                (FileState::Missing, false) => RowState::Missing,
                (FileState::InSync | FileState::ChangedOutside | FileState::Unscanned, false) => {
                    RowState::Unknown
                }
            };
            self.absent.push(Row {
                file: survivor,
                path: file.path.clone(),
                state,
                entity: Some(*entity),
            });
        }
        let unchanged = from.absent.iter().filter(|row| kept.row(row)).cloned();
        self.absent.extend(unchanged);
        tagged
    }

    /// The listing's rows, by path: the entities' of `tagged`, else the row an own
    /// effect put there (rule 1), else the row at that path (rule 3), else at the
    /// path the volume takes for it, else a new one.
    fn listed(&mut self, tagged: &BTreeMap<&'a RelPath, (FileId, EntityId)>) -> Vec<Row> {
        let scan = self.known.look.scan;
        let files = scan.map(|scan| &scan.files).into_iter().flatten();
        let mut listed = Vec::with_capacity(scan.map_or(0, |scan| scan.files.len()));
        let mut unmatched = Vec::new();
        let mut was = self.from.listed.iter().peekable();
        let mut tagged = tagged.iter().peekable();
        // The rows of the last listing at a path this one does not list.
        let mut away: Vec<&Row> = Vec::new();
        for (path, found) in files.filter(|(path, _)| self.known.knows(path)) {
            while let Some(row) = was.next_if(|row| row.path < *path) {
                away.push(row);
            }
            let exact = was.next_if(|row| row.path == *path);
            let bound = tagged
                .next_if(|(at, _)| **at == path)
                .map(|(_, bound)| *bound);
            let kept = exact.filter(|row| self.kept.row(row));
            let (file, entity) = match (bound, kept) {
                (Some((file, entity)), _) => (file, Some(entity)),
                (None, Some(row)) => (row.file, row.entity),
                (None, None) => match self.there(path, exact) {
                    Some(file) => (file, None),
                    None => {
                        unmatched.push(listed.len());
                        (self.ids.draw(), None)
                    }
                },
            };
            listed.push(Row {
                file,
                path: path.clone(),
                state: RowState::Present {
                    len: found.len,
                    modified: found.modified,
                },
                entity,
            });
        }
        if unmatched.is_empty() {
            return listed;
        }
        away.extend(was);
        away.extend(&self.from.absent);
        let (carried, kept) = (self.carried, self.kept);
        let gone = |row: &&&Row| {
            !self.taken.contains(&row.file) && !carried.left.contains(&row.file) && !kept.row(row)
        };
        let away = away
            .iter()
            .filter(gone)
            .filter(|row| self.known.knows(&row.path));
        let paths = unmatched.iter().map(|at| &listed[*at].path);
        let mut renamed = respelled(away, paths, &self.known);
        for at in unmatched {
            if let Some(file) = renamed.remove(&listed[at].path) {
                listed[at].file = file;
            }
        }
        listed
    }

    /// The row a file at `path` no entity is bound to continues: one an own effect
    /// put there, else `exact`, the last listing's there, else an absent one there.
    fn there(&self, path: &RelPath, exact: Option<&Row>) -> Option<FileId> {
        let carried = self.carried;
        let there = self.carried_to(path).or_else(|| {
            let row = exact.filter(|row| !carried.left.contains(&row.file));
            row.map(|row| row.file)
                .or_else(|| carried.at(self.from, path))
        });
        there.filter(|id| !self.taken.contains(id))
    }

    /// The row an own effect carried to `path`, or to a path the listing does not
    /// hold that the volume takes for it.
    fn carried_to(&self, path: &RelPath) -> Option<FileId> {
        if let Some(id) = self.carried.at.get(path) {
            return Some(*id);
        }
        if self.respelled.is_empty() {
            return None;
        }
        let key = self.known.look.names.key(path.as_str());
        self.respelled.get(&key).copied()
    }

    /// Each row left at a path no scan can list now, Unknown there. A row the
    /// listing continued is at a listed path, or one an own effect carried.
    fn unknown(&mut self) {
        let (carried, kept) = (self.carried, self.kept);
        let rows = self
            .from
            .iter()
            .filter(|row| !carried.left.contains(&row.file));
        let rows = rows
            .filter(|row| !kept.row(row))
            .map(|row| (&row.path, row.file));
        let rows = rows.chain(self.carried.at.iter().map(|(path, id)| (path, *id)));
        for (path, id) in rows {
            if !self.taken.contains(&id) && !self.known.knows(path) {
                self.taken.insert(id);
                self.absent.push(Row {
                    file: id,
                    path: path.clone(),
                    state: RowState::Unknown,
                    entity: None,
                });
            }
        }
    }
}

/// The rows `away`, at paths no longer listed, that continue at a path of
/// `unmatched` the volume takes for theirs, the first of each by path: a change of
/// spelling outside.
fn respelled<'a, 'r>(
    away: impl Iterator<Item = &'r &'r Row>,
    unmatched: impl Iterator<Item = &'a RelPath>,
    known: &Known,
) -> BTreeMap<RelPath, FileId> {
    let away = away.filter(|row| !known.lists(&row.path));
    let mut by_key: BTreeMap<String, Vec<FileId>> = BTreeMap::new();
    for row in away {
        let key = known.look.names.key(row.path.as_str());
        by_key.entry(key).or_default().push(row.file);
    }
    let mut renamed = BTreeMap::new();
    if by_key.is_empty() {
        return renamed;
    }
    for path in unmatched {
        let rows = by_key.get_mut(&known.look.names.key(path.as_str()));
        if let Some(id) = rows
            .filter(|rows| !rows.is_empty())
            .map(|rows| rows.remove(0))
        {
            renamed.insert(path.clone(), id);
        }
    }
    renamed
}

/// The rows each look merged, linked to the next look's: what lets a view say
/// which rows of an earlier one merged, however many looks lie between.
#[derive(Debug, Default)]
pub struct Merges {
    merged: Vec<Merge>,
    next: OnceLock<Arc<Merges>>,
}

impl Merges {
    /// The look after this one, which merged `merged`.
    pub(crate) fn then(self: &Arc<Self>, merged: Vec<Merge>) -> Arc<Merges> {
        let next = Arc::new(Merges {
            merged,
            next: OnceLock::new(),
        });
        match self.next.set(Arc::clone(&next)) {
            Ok(()) => next,
            Err(_) => unreachable!("only the latest look is followed"),
        }
    }

    /// Each row merged after `self` up to `until`, with the row it merged into;
    /// nothing where `until` does not follow `self`.
    fn until(self: &Arc<Self>, until: &Arc<Merges>) -> BTreeMap<FileId, FileId> {
        let mut found = BTreeMap::new();
        let mut at = Arc::clone(self);
        while !Arc::ptr_eq(&at, until) {
            let Some(next) = at.next.get().cloned() else {
                return BTreeMap::new();
            };
            found.extend(next.merged.iter().copied());
            at = next;
        }
        found
    }
}

/// Who changed a row of an entity.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug, Serialize, Deserialize)]
pub enum Who {
    This,
    /// The writer at this index of [`crate::View::writer_table`].
    Writer(usize),
    /// Found by a scan with no entry behind it.
    Outside,
}

/// How a row changed, where that is known.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug, Serialize, Deserialize)]
pub enum How {
    Moved,
    Trashed,
    Saved,
    Made,
    Tagged,
}

#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Debug, Serialize, Deserialize)]
pub enum RowWhat {
    Appeared,
    Moved {
        from: RelPath,
    },
    /// Present at the same path before and after, with another length or time.
    Changed,
    Gone,
    Merged {
        into: FileId,
    },
    Present,
    Unknown,
    Missing,
    Gained(EntityId),
    Lost(EntityId),
}

/// One change of one row.
#[derive(Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub struct RowChange {
    pub file: FileId,
    /// Where the row is, or was last for one gone or merged.
    pub path: RelPath,
    pub what: RowWhat,
    /// Of a row with an entity, before or after, from the merged state; `None`
    /// for a row with none.
    pub by: Option<Who>,
    pub how: Option<How>,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub enum DirWhat {
    Made,
    Removed,
}

#[derive(Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub struct DirChange {
    pub path: RelPath,
    pub what: DirWhat,
}

/// What changed between two views' rows.
#[derive(Clone, PartialEq, Eq, Debug, Default, Serialize, Deserialize)]
pub struct RowChanges {
    /// By FileId, each row's changes in the order of [`RowWhat`].
    pub files: Vec<RowChange>,
    /// By path.
    pub dirs: Vec<DirChange>,
}

/// One side of [`changes`]: a view's rows and what explains them.
pub(crate) struct Side<'a> {
    pub rows: &'a Arc<Rows>,
    pub merges: &'a Arc<Merges>,
    pub folded: &'a Folded,
    pub bindings: &'a Bindings,
    /// The writes lost because their entries stopped being read, which explain no
    /// change, nor does their return.
    pub away: &'a BTreeSet<Ref>,
    pub own: Option<WriterId>,
}

/// What changed from `was` to `now`, with who changed each entity's row, as
/// `table` numbers the writers.
pub(crate) fn changes(now: &Side, was: &Side, table: &[WriterId]) -> RowChanges {
    let dirs = now.rows.dirs.symmetric_difference(&was.rows.dirs);
    let dirs = dirs.map(|path| DirChange {
        path: path.clone(),
        what: match now.rows.dirs.contains(path) {
            true => DirWhat::Made,
            false => DirWhat::Removed,
        },
    });
    let mut changes = RowChanges {
        files: Vec::new(),
        dirs: dirs.collect(),
    };
    if Arc::ptr_eq(now.rows, was.rows) {
        return changes;
    }
    let merged = was.merges.until(now.merges);
    let mut causes: Option<BTreeMap<EntityId, (Cause, WriterId)>> = None;
    let mut cause = |entity: EntityId| {
        let causes = causes.get_or_insert_with(|| {
            let mut first: BTreeMap<EntityId, &Since> = BTreeMap::new();
            let all = now.folded.since(was.folded);
            let lost =
                |since: &&Since| since.shift != Shift::Gained && now.away.contains(&since.write);
            let evicted: Vec<&Since> = all.iter().filter(lost).collect();
            for since in &all {
                let unread = match since.shift {
                    Shift::Gained => {
                        was.away.contains(&since.write)
                            || evicted.iter().any(|lost| since.given_back_by(lost))
                    }
                    Shift::Lost | Shift::Replaced => now.away.contains(&since.write),
                };
                if unread || !since.causes() {
                    continue;
                }
                let rank = cause_rank(&since.part);
                let earlier = first
                    .get(&since.entity)
                    .is_none_or(|had| rank < cause_rank(&had.part));
                if rank.is_some() && earlier {
                    first.insert(since.entity, since);
                }
            }
            let trashed = trashed(first.values().copied(), now.folded, was.folded);
            let causes = first.into_iter().map(|(entity, since)| {
                let cause = Cause::of(since, now.folded, was.folded, &trashed);
                (entity, (cause, since.write.writer))
            });
            causes.collect()
        });
        causes.get(&entity).cloned()
    };
    let who = |writer: WriterId| match writer {
        _ if Some(writer) == now.own || Some(writer) == was.own => Who::This,
        _ => match table.iter().position(|known| *known == writer) {
            Some(at) => Who::Writer(at),
            None => Who::Outside,
        },
    };
    let mut shown_on: Option<BTreeMap<EntityId, EntityId>> = None;
    for (now_row, was_row) in paired(now.rows, was.rows) {
        let whats = whats(now_row, was_row, now.rows, &merged);
        if whats.is_empty() {
            continue;
        }
        let row = now_row.or(was_row).expect("one side holds the row");
        let entity = now_row.and_then(|row| row.entity);
        let entity = entity.or(was_row.and_then(|row| row.entity));
        let mut found = entity.map(&mut cause);
        let mut host = None;
        if let (Some(None), Some(entity)) = (&found, entity) {
            let shown_on = shown_on.get_or_insert_with(|| was.bindings.shown_on().collect());
            if let Some((into, what, path)) = hosted(entity, shown_on, now.rows, was.rows, &merged)
            {
                found = Some(cause(into));
                host = Some((what, path));
            }
        }
        for what in whats {
            let (by, how) = match &found {
                None => (
                    None,
                    matches!(what, RowWhat::Moved { .. }).then_some(How::Moved),
                ),
                Some(None) => (Some(Who::Outside), outside(&what)),
                Some(Some((cause, writer))) => {
                    let how = match &host {
                        Some((what, path)) => cause.how(what, Some(path)),
                        None => cause.how(&what, was_row.map(|row| &row.path)),
                    };
                    (Some(who(*writer)), Some(how))
                }
            };
            changes.files.push(RowChange {
                file: row.file,
                path: row.path.clone(),
                what,
                by,
                how,
            });
        }
    }
    changes
}

/// The entity whose row `was` showed `entity` on, with how that row changed, a rewrite
/// in place where it shows no change, and where it was: what changed `entity`'s row.
fn hosted<'a>(
    entity: EntityId,
    shown_on: &BTreeMap<EntityId, EntityId>,
    now: &Rows,
    was: &'a Rows,
    merged: &BTreeMap<FileId, FileId>,
) -> Option<(EntityId, RowWhat, &'a RelPath)> {
    let into = *shown_on.get(&entity)?;
    let row = was.of(into)?;
    let what = whats(now.row(row.file), Some(row), now, merged)
        .into_iter()
        .next();
    Some((into, what.unwrap_or(RowWhat::Changed), &row.path))
}

/// Which change of an entity's state explains a change of its row first: its making or
/// deletion, which also drops the writes of its file, then its file's; none for a field.
fn cause_rank(part: &Part) -> Option<u8> {
    match part {
        Part::Created | Part::Deleted => Some(0),
        Part::File => Some(1),
        Part::Field(_) => None,
    }
}

/// Each row of `now` or `was`, by FileId, with its row in each.
fn paired<'a>(
    now: &'a Rows,
    was: &'a Rows,
) -> impl Iterator<Item = (Option<&'a Row>, Option<&'a Row>)> + 'a {
    let (mut now, mut was) = (
        now.in_file_order().peekable(),
        was.in_file_order().peekable(),
    );
    std::iter::from_fn(move || {
        let order = match (now.peek(), was.peek()) {
            (None, None) => return None,
            (Some(_), None) => std::cmp::Ordering::Less,
            (None, Some(_)) => std::cmp::Ordering::Greater,
            (Some(a), Some(b)) => a.file.cmp(&b.file),
        };
        let a = (order.is_le()).then(|| now.next()).flatten();
        let b = (order.is_ge()).then(|| was.next()).flatten();
        Some((a, b))
    })
}

/// What changed of one row, `merged` naming the rows that ended merged.
fn whats(
    now: Option<&Row>,
    was: Option<&Row>,
    rows: &Rows,
    merged: &BTreeMap<FileId, FileId>,
) -> Vec<RowWhat> {
    let (now, was) = match (now, was) {
        (Some(_), None) => return vec![RowWhat::Appeared],
        (None, Some(was)) => return vec![gone(was.file, rows, merged)],
        (Some(now), Some(was)) => (now, was),
        (None, None) => return Vec::new(),
    };
    let mut whats = Vec::new();
    if now.path != was.path {
        whats.push(RowWhat::Moved {
            from: was.path.clone(),
        });
    }
    match (now.state, was.state) {
        (RowState::Present { .. }, RowState::Present { .. }) => {
            if now.path == was.path && now.state != was.state {
                whats.push(RowWhat::Changed);
            }
        }
        (now, was) if std::mem::discriminant(&now) == std::mem::discriminant(&was) => {}
        (RowState::Present { .. }, _) => whats.push(RowWhat::Present),
        (RowState::Unknown, _) => whats.push(RowWhat::Unknown),
        (RowState::Missing, _) => whats.push(RowWhat::Missing),
    }
    if now.entity != was.entity {
        whats.extend(was.entity.map(RowWhat::Lost));
        whats.extend(now.entity.map(RowWhat::Gained));
    }
    whats
}

/// How `file`, which `rows` no longer hold, ended: merged into the row it merged
/// into last that `rows` hold, or gone.
fn gone(file: FileId, rows: &Rows, merged: &BTreeMap<FileId, FileId>) -> RowWhat {
    let mut into = file;
    for _ in 0..=merged.len() {
        match merged.get(&into) {
            Some(next) => into = *next,
            None => break,
        }
        if rows.row(into).is_some() {
            return RowWhat::Merged { into };
        }
    }
    RowWhat::Gone
}

/// The paths each deletion among `causes` moved files from into its writer's trash,
/// by whether the later state holds the deletion and by its entry.
fn trashed<'a>(
    causes: impl Iterator<Item = &'a Since>,
    now: &Folded,
    was: &Folded,
) -> BTreeMap<(bool, EntryHash), Arc<BTreeSet<RelPath>>> {
    let mut entries: BTreeMap<(bool, WriterId), BTreeSet<EntryHash>> = BTreeMap::new();
    for since in causes.filter(|since| since.part == Part::Deleted) {
        let side = (since.shift == Shift::Gained, since.write.writer);
        entries.entry(side).or_default().insert(since.write.hash);
    }
    let mut trashed: BTreeMap<(bool, EntryHash), BTreeSet<RelPath>> = BTreeMap::new();
    for ((gained, writer), by) in entries {
        let folded = if gained { now } else { was };
        for (entry, from) in folded.trashed(writer, &by) {
            trashed.entry((gained, entry)).or_default().insert(from);
        }
    }
    trashed
        .into_iter()
        .map(|(at, from)| (at, Arc::new(from)))
        .collect()
}

/// The change of an entity's state that explains a change of its row.
#[derive(Clone)]
struct Cause {
    part: Part,
    /// Whether the change is a file write that leaves the entity no file.
    fileless: bool,
    /// The paths the entry of a deletion moved files from into the trash.
    trashed: Arc<BTreeSet<RelPath>>,
}

impl Cause {
    fn of(
        since: &Since,
        now: &Folded,
        was: &Folded,
        trashed: &BTreeMap<(bool, EntryHash), Arc<BTreeSet<RelPath>>>,
    ) -> Self {
        let folded = match since.shift {
            Shift::Gained => now,
            Shift::Lost | Shift::Replaced => was,
        };
        let fileless = since.part == Part::File && {
            let writes = folded.file_writes(since.entity);
            let write = writes.iter().find(|write| write.named() == since.write);
            write.is_some_and(|write| write.value.is_none())
        };
        let gained = since.shift == Shift::Gained;
        let trashed = trashed.get(&(gained, since.write.hash)).cloned();
        Self {
            part: since.part.clone(),
            fileless,
            trashed: trashed.unwrap_or_default(),
        }
    }

    /// How an entity's row, at `was` before, changed `what`, as this change says: a
    /// file write that leaves the file at another path moves it, wherever the row ends,
    /// and a deletion that trashed no file from the row's path takes its tags off.
    fn how(&self, what: &RowWhat, was: Option<&RelPath>) -> How {
        match (&self.part, what) {
            (Part::Deleted, _) if was.is_some_and(|was| self.trashed.contains(was)) => How::Trashed,
            (Part::Deleted, _) => How::Tagged,
            (Part::File, RowWhat::Gone) if self.fileless => How::Trashed,
            (Part::Created, RowWhat::Appeared | RowWhat::Missing) => How::Made,
            (Part::Created, _) => How::Tagged,
            (_, RowWhat::Gained(_) | RowWhat::Lost(_)) => How::Tagged,
            (_, RowWhat::Changed) => How::Saved,
            (_, RowWhat::Appeared) => How::Made,
            _ => How::Moved,
        }
    }
}

/// How an entity's row changed `what` with no write behind it, where that is known.
fn outside(what: &RowWhat) -> Option<How> {
    match what {
        RowWhat::Moved { .. } => Some(How::Moved),
        RowWhat::Changed => Some(How::Saved),
        _ => None,
    }
}
