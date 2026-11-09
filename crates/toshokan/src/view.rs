//! Views: immutable values the app renders from.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use crate::binding::Bindings;
use crate::ids::{EntityId, WriterId};
use crate::log::Ref;
use crate::merge::Folded;
use crate::path::RelPath;
use crate::report::{Fork, Gap, Item, Joined, Presence, WriterInfo};
use crate::rows::{self, FileId, Merges, Row, RowChanges, RowState, Rows};
use crate::schema::{Field, Key, KeyKind, Keyed, Members, Raw, Register, Set, Value, Written};

/// The library as one reader sees it. Cheap to clone; never changes; can be sent
/// to and shared between threads.
///
/// A lookup by value visits every entity shown; one by path is a search. Values
/// compare by their JSON text, as toshokan writes it for the key's type.
#[derive(Clone)]
pub struct View(Arc<Shared>);

struct Shared {
    parts: Parts,
    /// The entity bound to each file in sync or changed outside.
    at: BTreeMap<RelPath, EntityId>,
}

/// What a view is built from.
#[derive(Default)]
pub struct Parts {
    pub folded: Folded,
    pub bindings: Arc<Bindings>,
    pub rows: Arc<Rows>,
    /// What the looks since this one's merged: the rows that ended in another.
    pub merges: Arc<Merges>,
    /// Every writer seen, in the order first seen, whatever the view.
    pub table: Arc<[WriterId]>,
    /// The writes the merged state lost because their entries stopped being read,
    /// and does not hold again.
    pub away: Arc<BTreeSet<Ref>>,
    pub writers: Vec<WriterInfo>,
    pub forks: Vec<Fork>,
    pub gaps: Vec<Gap>,
    pub items: Vec<Item>,
}

/// One entity of a view.
#[derive(Clone, Copy)]
pub struct EntityView<'v> {
    view: &'v View,
    id: EntityId,
}

/// Where an entity's file is, as the view binds it.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct FileRef {
    pub path: RelPath,
    pub state: FileState,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum FileState {
    /// The file holds what the entity's file register says.
    InSync,
    /// The file is there with other contents, changed without an entry.
    ChangedOutside,
    /// No file the entity can be bound to is there.
    Missing,
    /// Unknown: no scan of every file has succeeded since the library opened, or
    /// a scan of the path failed after this instance's own commit moved files there
    /// or from there. The path is the logged one. A scan of every file binds it
    /// again.
    Unscanned,
}

/// A library file's length and time, as a listing or a check read them.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Listed {
    pub len: u64,
    /// When the file last changed, as [`crate::io::Meta::modified`] gives it.
    pub modified: Option<u64>,
}

/// The distinct values of a key, each with how many entities hold it.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Counts<T> {
    /// Sorted.
    pub values: Vec<(T, usize)>,
    /// Values that do not decode as the key's type, sorted by text.
    pub unreadable: Vec<(Raw, usize)>,
}

/// A key of an entity whose surviving writes disagree.
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Debug)]
pub enum Conflicted {
    Field { entity: EntityId, key: String },
    Existence { entity: EntityId },
    File { entity: EntityId },
}

impl View {
    pub fn new(parts: Parts) -> Self {
        let mut at = BTreeMap::new();
        for (entity, file) in parts.bindings.bound.iter() {
            if matches!(file.state, FileState::InSync | FileState::ChangedOutside) {
                at.entry(file.path.clone()).or_insert(*entity);
            }
        }
        Self(Arc::new(Shared { parts, at }))
    }

    pub(crate) fn parts(&self) -> &Parts {
        &self.0.parts
    }

    /// Each entity shown holding a value under `key`, by id, with the values it
    /// holds there, sorted and once each.
    fn holding(&self, key: Key) -> impl Iterator<Item = (EntityId, Vec<&Raw>)> {
        let shown = self.parts().folded.each_shown();
        shown.filter_map(move |(entity, shown)| {
            let values = shown?.values.into_iter();
            let under = values.filter(|(kind, name, _)| *kind == key.kind && *name == key.name);
            let held: Vec<&Raw> = under.map(|(_, _, value)| value).collect();
            (!held.is_empty()).then_some((entity, held))
        })
    }

    /// The merged facts the view shows.
    pub fn folded(&self) -> &Folded {
        &self.parts().folded
    }

    /// Every entity that exists or whose deletion is in conflict, by id.
    pub fn entities(&self) -> Vec<EntityView<'_>> {
        self.parts()
            .folded
            .entities()
            .into_iter()
            .map(|id| EntityView { view: self, id })
            .collect()
    }

    pub fn entity(&self, id: EntityId) -> Option<EntityView<'_>> {
        self.parts()
            .folded
            .present(id)
            .then_some(EntityView { view: self, id })
    }

    /// The entities whose register or set `key` holds `value`, by id. A register
    /// holds the value of each write that survives, so each side of a conflict is
    /// found.
    pub fn find<K: Keyed>(&self, key: K, value: &K::Value) -> Vec<EntityId> {
        let Ok(raw) = Raw::of(value) else {
            return Vec::new();
        };
        let holding = self.holding(key.key());
        let found = holding.filter(|(_, held)| held.binary_search(&&raw).is_ok());
        found.map(|(entity, _)| entity).collect()
    }

    /// The entities whose register or set `key` holds any value, readable or not,
    /// by id.
    pub fn with<K: Keyed>(&self, key: K) -> Vec<EntityId> {
        let holding = self.holding(key.key());
        holding.map(|(entity, _)| entity).collect()
    }

    /// Each distinct value of the register or set `key`, with how many entities
    /// hold it.
    pub fn values<K: Keyed>(&self, key: K) -> Counts<K::Value> {
        let mut held: BTreeMap<&Raw, usize> = BTreeMap::new();
        for (_, values) in self.holding(key.key()) {
            for value in values {
                *held.entry(value).or_default() += 1;
            }
        }
        let mut counts: Counts<K::Value> = Counts {
            values: Vec::new(),
            unreadable: Vec::new(),
        };
        for (raw, count) in held {
            match raw.decode() {
                Ok(value) => counts.values.push((value, count)),
                Err(_) => counts.unreadable.push((raw.clone(), count)),
            }
        }
        counts.values.sort_by(|a, b| a.0.cmp(&b.0));
        counts
    }

    /// The entity bound to the file at `path`, in sync or changed outside. Paths
    /// compare exactly, as the view shows them.
    pub fn at(&self, path: &RelPath) -> Option<EntityView<'_>> {
        let id = *self.0.at.get(path)?;
        Some(EntityView { view: self, id })
    }

    /// Every conflict among the entities shown, sorted.
    pub fn conflicts(&self) -> Vec<Conflicted> {
        let mut conflicts = Vec::new();
        for (entity, shown) in self.parts().folded.each_shown() {
            let Some(shown) = shown else {
                continue;
            };
            let registers = shown
                .values
                .iter()
                .filter(|(kind, ..)| *kind == KeyKind::Register);
            let keys: Vec<&str> = registers.map(|(_, key, _)| *key).collect();
            let written = keys
                .chunk_by(|a, b| a == b)
                .filter(|values| values.len() > 1);
            conflicts.extend(written.map(|values| Conflicted::Field {
                entity,
                key: values[0].to_owned(),
            }));
            if shown.deletion_conflicted {
                conflicts.push(Conflicted::Existence { entity });
            }
            if shown.file_conflicted {
                conflicts.push(Conflicted::File { entity });
            }
        }
        conflicts.sort();
        conflicts
    }

    pub fn forks(&self) -> &[Fork] {
        &self.parts().forks
    }

    pub fn gaps(&self) -> &[Gap] {
        &self.parts().gaps
    }

    /// What the view shows apart from the merged state: runs, gaps and unplaced
    /// lines.
    pub fn items(&self) -> &[Item] {
        &self.parts().items
    }

    pub fn writers(&self) -> &[WriterInfo] {
        &self.parts().writers
    }

    /// Library files no entity is bound to, in order. They get an entity only when
    /// an intent says something about them.
    pub fn unbound(&self) -> impl Iterator<Item = &RelPath> {
        self.parts().bindings.unbound.iter()
    }

    /// The entities whose file facts hold the file bound to another, by id. Each
    /// is bound to no file, and is not missing, until an intent joins it into
    /// that one.
    pub fn joined(&self) -> &[Joined] {
        &self.parts().bindings.report.joined
    }

    /// The library file at `path` as the last scan listed it; `None` where it
    /// listed none, where a scan since could not list it, and for every file until
    /// a scan of every file has run.
    pub fn listed(&self, path: &RelPath) -> Option<Listed> {
        match self.parts().rows.listed(path)?.state {
            RowState::Present { len, modified } => Some(Listed { len, modified }),
            RowState::Unknown | RowState::Missing => None,
        }
    }

    /// Every row: the listing's by path, then the absent rows by FileId.
    pub fn rows(&self) -> impl Iterator<Item = &Row> {
        self.parts().rows.iter()
    }

    pub fn row(&self, file: FileId) -> Option<&Row> {
        self.parts().rows.row(file)
    }

    /// The row at `path`: the listing's, else an absent one. Paths compare
    /// exactly, as the view shows them.
    pub fn row_at(&self, path: &RelPath) -> Option<&Row> {
        self.parts().rows.at(path)
    }

    /// The row of `entity`'s file; `None` where it has no file fact or another's
    /// row shows it, as for each of [`View::joined`].
    pub fn row_of(&self, entity: EntityId) -> Option<&Row> {
        self.parts().rows.of(entity)
    }

    /// The directories below the root the listing holds, empty ones included, in
    /// order.
    pub fn dirs(&self) -> impl Iterator<Item = &RelPath> {
        self.parts().rows.dirs()
    }

    /// The writers [`rows::Who::Writer`] numbers: each index names one writer in
    /// every view of one library.
    pub fn writer_table(&self) -> &[WriterId] {
        &self.parts().table
    }

    /// What changed of each row and directory since `earlier`, a view of the same
    /// library, with who changed each entity's row as the merged state says. Costs
    /// a step per row.
    pub fn changes_since(&self, earlier: &View) -> RowChanges {
        fn side(view: &View) -> rows::Side<'_> {
            let parts = view.parts();
            let here = parts
                .writers
                .iter()
                .find(|info| info.here == Presence::This);
            rows::Side {
                rows: &parts.rows,
                merges: &parts.merges,
                folded: &parts.folded,
                bindings: &parts.bindings,
                away: &parts.away,
                own: here.map(|info| info.writer),
            }
        }
        rows::changes(&side(self), &side(earlier), &self.parts().table)
    }
}

impl EntityView<'_> {
    pub fn id(&self) -> EntityId {
        self.id
    }

    /// The register as this view merges it. Writes of equal values are one value.
    /// A value that does not decode as `T` makes the whole field unreadable, so an
    /// app never shows part of a conflict as if it were all of it.
    pub fn get<T: Value>(&self, key: Register<T>) -> Field<T> {
        let writes = self.view.parts().folded.register(self.id, key.name());
        let mut all = Vec::with_capacity(writes.len());
        for write in writes.iter().rev() {
            match write.value.decode::<T>() {
                Ok(value) => all.push(Written {
                    value,
                    by: write.by,
                    at: write.at,
                    entry: write.entry,
                }),
                Err(_) => return Field::Unreadable(write.value.clone()),
            }
        }
        all.reverse();
        let values: BTreeSet<&T> = all.iter().map(|write| &write.value).collect();
        match (values.len(), all.last()) {
            (_, None) => Field::Unset,
            (1, Some(write)) => Field::Value(write.value.clone()),
            (_, Some(write)) => Field::Conflict {
                shown: write.value.clone(),
                all,
            },
        }
    }

    pub fn members<T: Value>(&self, key: Set<T>) -> Members<T> {
        let mut values = BTreeSet::new();
        let mut unreadable: Vec<Raw> = Vec::new();
        for raw in self.view.parts().folded.members(self.id, key.name()) {
            match raw.decode::<T>() {
                Ok(value) => {
                    values.insert(value);
                }
                Err(_) => unreadable.push(raw),
            }
        }
        Members {
            values: values.into_iter().collect(),
            unreadable,
        }
    }

    /// Where the view binds the entity's file; [`FileState::Missing`] at the
    /// logged path when no binding was derived for it, and `None` where it has no
    /// file fact or another's row shows it: it is one of [`View::joined`] or of
    /// [`Bindings::alike`](crate::binding::Bindings::alike).
    pub fn file(&self) -> Option<FileRef> {
        let bindings = &self.view.parts().bindings;
        if let Some(bound) = bindings.bound.get(&self.id) {
            return Some(bound.clone());
        }
        let joined = &bindings.report.joined;
        let joined = joined.binary_search_by_key(&self.id, |joined| joined.entity);
        if joined.is_ok() || bindings.alike.contains_key(&self.id) {
            return None;
        }
        Some(FileRef {
            path: self.view.parts().folded.logged_path(self.id)?.clone(),
            state: FileState::Missing,
        })
    }

    /// Whether a delete and a write it did not observe are both in effect.
    pub fn deletion_conflicted(&self) -> bool {
        self.view.parts().folded.deletion_conflicted(self.id)
    }
}
