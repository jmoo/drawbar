//! Views: immutable values the app renders from.

use std::collections::BTreeSet;
use std::ops::RangeBounds;
use std::sync::Arc;

use crate::binding::{Bindings, Scanned};
use crate::cow::CowMap;
use crate::ids::EntityId;
use crate::index::Index;
use crate::merge::Folded;
use crate::path::RelPath;
use crate::report::{Fork, Gap, WriterInfo};
use crate::schema::{Field, Keyed, Members, Raw, Register, Set, Value, Written};

/// The library as one reader sees it. Cheap to clone; never changes; can be sent
/// to and shared between threads.
///
/// Lookups by value, by path and of conflicts are indexed: each costs a search
/// plus a step per result. Values compare by their JSON text, as toshokan writes
/// it for the key's type.
#[derive(Clone)]
pub struct View(Arc<Parts>, Arc<Index>);

/// What a view is built from.
#[derive(Default)]
pub struct Parts {
    pub folded: Folded,
    pub bindings: Arc<Bindings>,
    /// The library files the last scan found; `None` until a scan of every file.
    pub listing: Option<CowMap<RelPath, Listed>>,
    pub writers: Vec<WriterInfo>,
    pub forks: Vec<Fork>,
    pub gaps: Vec<Gap>,
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

/// A library file as the last scan listed it.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Listed {
    pub len: u64,
    /// When the file last changed, as [`crate::io::Meta::modified`] gives it.
    pub modified: Option<u64>,
}

impl Listed {
    pub(crate) fn of(file: &Scanned) -> Self {
        Self {
            len: file.len,
            modified: file.modified,
        }
    }
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
        let index = Index::of(&parts.folded, &parts.bindings);
        Self(Arc::new(parts), Arc::new(index))
    }

    /// A view of `parts`, whose index `index` is.
    pub(crate) fn indexed(parts: Parts, index: Arc<Index>) -> Self {
        Self(Arc::new(parts), index)
    }

    pub(crate) fn index(&self) -> &Arc<Index> {
        &self.1
    }

    pub(crate) fn parts(&self) -> &Parts {
        &self.0
    }

    /// The merged facts the view shows.
    pub fn folded(&self) -> &Folded {
        &self.0.folded
    }

    /// Every entity that exists or whose deletion is in conflict, by id.
    pub fn entities(&self) -> Vec<EntityView<'_>> {
        self.0
            .folded
            .entities()
            .into_iter()
            .map(|id| EntityView { view: self, id })
            .collect()
    }

    pub fn entity(&self, id: EntityId) -> Option<EntityView<'_>> {
        self.0
            .folded
            .present(id)
            .then_some(EntityView { view: self, id })
    }

    /// The entities whose register or set `key` holds `value`, by id. A register
    /// holds the value of each write that survives, so each side of a conflict is
    /// found.
    pub fn find<K: Keyed>(&self, key: K, value: &K::Value) -> Vec<EntityId> {
        let key = key.key();
        match Raw::of(value) {
            Ok(raw) => self.1.find(key.kind, key.name, &raw),
            Err(_) => Vec::new(),
        }
    }

    /// The entities whose register or set `key` holds any value, readable or not,
    /// by id.
    pub fn with<K: Keyed>(&self, key: K) -> Vec<EntityId> {
        let key = key.key();
        self.1.with(key.kind, key.name)
    }

    /// The entities whose register or set `key` holds a value within `range`, by
    /// id. Each distinct value of the key is decoded to compare it.
    pub fn range<K: Keyed>(&self, key: K, range: impl RangeBounds<K::Value>) -> Vec<EntityId> {
        let key = key.key();
        let values = self.1.values(key.kind, key.name).into_iter();
        let within = values.filter(|(text, _)| {
            serde_json::from_str::<K::Value>(text).is_ok_and(|value| range.contains(&value))
        });
        self.1
            .holding(key.kind, key.name, within.map(|(text, _)| text))
    }

    /// Each distinct value of the register or set `key`, with how many entities
    /// hold it.
    pub fn values<K: Keyed>(&self, key: K) -> Counts<K::Value> {
        let key = key.key();
        let mut counts: Counts<K::Value> = Counts {
            values: Vec::new(),
            unreadable: Vec::new(),
        };
        for (text, count) in self.1.values(key.kind, key.name) {
            match serde_json::from_str(text) {
                Ok(value) => counts.values.push((value, count)),
                Err(_) => counts
                    .unreadable
                    .push((Raw::new(text).expect("an indexed value is JSON"), count)),
            }
        }
        counts.values.sort_by(|a, b| a.0.cmp(&b.0));
        counts
    }

    /// The entity bound to the file at `path`, in sync or changed outside. Paths
    /// compare exactly, as the view shows them.
    pub fn at(&self, path: &RelPath) -> Option<EntityView<'_>> {
        let mut bound = self
            .1
            .at(path)
            .filter(|(_, state)| matches!(state, FileState::InSync | FileState::ChangedOutside));
        bound.next().map(|(id, _)| EntityView { view: self, id })
    }

    /// The entities whose file, as [`EntityView::file`] gives it, is at `dir` or
    /// under it, in order of path and then id.
    pub fn under(&self, dir: &RelPath) -> Vec<EntityView<'_>> {
        let under = self.1.under(dir);
        under
            .map(|(_, id, _)| EntityView { view: self, id })
            .collect()
    }

    /// Every conflict among the entities shown, sorted.
    pub fn conflicts(&self) -> Vec<Conflicted> {
        self.1.conflicts()
    }

    pub fn forks(&self) -> &[Fork] {
        &self.0.forks
    }

    pub fn gaps(&self) -> &[Gap] {
        &self.0.gaps
    }

    pub fn writers(&self) -> &[WriterInfo] {
        &self.0.writers
    }

    /// Library files no entity is bound to. They get an entity only when an intent
    /// says something about them.
    pub fn unbound(&self) -> &[RelPath] {
        &self.0.bindings.unbound
    }

    /// The library file at `path` as the last scan listed it; `None` where it
    /// listed none, and for every file until a scan of every file has run.
    pub fn listed(&self, path: &RelPath) -> Option<Listed> {
        self.0.listing.as_ref()?.get(path).copied()
    }

    /// What differs from `before`, a view of the same library. Costs a step per
    /// change where this view was made from `before`, as each view an instance
    /// gives is made from the one before it, and a step per entity and file
    /// otherwise.
    pub fn changed_since(&self, before: &View) -> Changed {
        let (now, was) = (&*self.0, &*before.0);
        let mut entities: BTreeSet<EntityId> =
            now.folded.changed(&was.folded).into_iter().collect();
        let mut files = BTreeSet::new();
        if !Arc::ptr_eq(&now.bindings, &was.bindings) {
            entities.extend(differing(now.bindings.bound.diff(&was.bindings.bound)));
            files.extend(either(&now.bindings.unbound, &was.bindings.unbound));
        }
        match (&now.listing, &was.listing) {
            (Some(listing), Some(listed)) => files.extend(differing(listing.diff(listed))),
            (listing, listed) => {
                let every = |listing: &Option<CowMap<RelPath, Listed>>| {
                    let paths = listing.iter().flat_map(|listing| listing.iter());
                    paths.map(|(path, _)| path.clone()).collect::<Vec<_>>()
                };
                files.extend(every(listing).into_iter().chain(every(listed)));
            }
        }
        Changed { entities, files }
    }
}

/// What differs between two views of one library, as [`View::changed_since`]
/// finds it.
#[derive(Clone, Default, PartialEq, Eq, Debug)]
pub struct Changed {
    /// The entities whose state, or whose file's binding, differs.
    pub entities: BTreeSet<EntityId>,
    /// The library files whose listing differs, or that one view has as a file no
    /// entity is bound to and the other does not.
    pub files: BTreeSet<RelPath>,
}

/// The keys whose entries differ in a [`CowMap::diff`].
fn differing<'a, K: Clone + 'a, V: PartialEq + 'a>(
    pairs: impl Iterator<Item = (Option<&'a (K, V)>, Option<&'a (K, V)>)> + 'a,
) -> impl Iterator<Item = K> + 'a {
    pairs.filter_map(|pair| match pair {
        (Some((_, now)), Some((_, was))) if now == was => None,
        (Some((key, _)), _) | (None, Some((key, _))) => Some(key.clone()),
        (None, None) => None,
    })
}

/// The paths in one of two sorted lists and not the other.
fn either(now: &[RelPath], was: &[RelPath]) -> Vec<RelPath> {
    let (mut now, mut was) = (now.iter().peekable(), was.iter().peekable());
    let mut either = Vec::new();
    loop {
        let path = match (now.peek(), was.peek()) {
            (None, None) => return either,
            (Some(a), Some(b)) if a == b => {
                now.next();
                was.next();
                continue;
            }
            (Some(a), Some(b)) if a < b => now.next(),
            (Some(_), None) => now.next(),
            (_, Some(_)) => was.next(),
        };
        either.extend(path.cloned());
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
        let writes = self.view.0.folded.register(self.id, key.name());
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
        for raw in self.view.0.folded.members(self.id, key.name()) {
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
    /// logged path when no binding was derived for it.
    pub fn file(&self) -> Option<FileRef> {
        if let Some(bound) = self.view.0.bindings.bound.get(&self.id) {
            return Some(bound.clone());
        }
        Some(FileRef {
            path: self.view.0.folded.logged_path(self.id)?.clone(),
            state: FileState::Missing,
        })
    }

    /// Whether a delete and a write it did not observe are both in effect.
    pub fn deletion_conflicted(&self) -> bool {
        self.view.0.folded.deletion_conflicted(self.id)
    }
}
