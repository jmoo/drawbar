//! Views: immutable values the app renders from.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::{Arc, OnceLock};

use crate::binding::{Bindings, Scanned};
use crate::ids::EntityId;
use crate::merge::{side_by_side, Folded, Paired, Shown};
use crate::path::RelPath;
use crate::report::{Fork, Gap, Item, Joined, WriterInfo};
use crate::schema::{
    Field, Key, KeyKind, Keyed, Members, Raw, Register, Relates, Set, Value, Written, JOINED,
};

/// The library as one reader sees it. Cheap to clone; never changes; can be sent
/// to and shared between threads.
///
/// A lookup by value or of a relation visits every entity shown; one by path is a
/// search. Values compare by their JSON text, as toshokan writes it for the key's
/// type.
#[derive(Clone)]
pub struct View(Arc<Shared>);

struct Shared {
    parts: Parts,
    /// The entity bound to each file in sync or changed outside.
    at: BTreeMap<RelPath, EntityId>,
    /// The lowest entity shown whose [`JOINED`] holds each id, from the first
    /// [`View::resolve`] of an id that is not shown.
    joined: OnceLock<BTreeMap<EntityId, EntityId>>,
}

/// What a view is built from.
#[derive(Default)]
pub struct Parts {
    pub folded: Folded,
    pub bindings: Arc<Bindings>,
    /// The library files the last scan found; `None` until a scan of every file.
    pub listing: Option<Arc<BTreeMap<RelPath, Listed>>>,
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

/// One value of a relation.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Related<T> {
    /// The entity that holds the value.
    pub from: EntityId,
    pub value: T,
    /// What the value's target [resolves](View::resolve) to; `None` while no
    /// entity shown is its target.
    pub to: Option<EntityId>,
}

/// The values of a relation, as [`View::relations`] and
/// [`EntityView::relations`] read them.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Relations<T> {
    /// By holder, then by the value's JSON text.
    pub related: Vec<Related<T>>,
    /// Values that do not decode as the key's type, each with its holder. What
    /// they name is unknown.
    pub unreadable: Vec<(EntityId, Raw)>,
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
        Self(Arc::new(Shared {
            parts,
            at,
            joined: OnceLock::new(),
        }))
    }

    pub(crate) fn parts(&self) -> &Parts {
        &self.0.parts
    }

    /// Each entity shown holding a value under `key`, by id, with the values it
    /// holds there, sorted and once each.
    fn holding(&self, key: Key) -> impl Iterator<Item = (EntityId, Vec<&Raw>)> {
        let shown = self.parts().folded.each_shown();
        shown.filter_map(move |(entity, shown)| {
            let held = held(shown?, key);
            (!held.is_empty()).then_some((entity, held))
        })
    }

    /// `held`, the values `from` holds under a relation's key, each with what it
    /// names now.
    fn relate<T: Relates>(&self, relations: &mut Relations<T>, from: EntityId, held: Vec<&Raw>) {
        for raw in held {
            match raw.decode::<T>() {
                Ok(value) => relations.related.push(Related {
                    from,
                    to: self.resolve(value.target()),
                    value,
                }),
                Err(_) => relations.unreadable.push((from, raw.clone())),
            }
        }
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

    /// The entity shown that `named` names now: itself while it is shown, or the
    /// entity it was joined into ([`crate::intent::Intent::join`]); and, of either,
    /// the entity a scan found it joined into ([`View::joined`]). `None` while no
    /// entity shown is it: it was deleted, or its writer's entries have not
    /// arrived. A deleted entity brought back resolves again.
    pub fn resolve(&self, named: EntityId) -> Option<EntityId> {
        let shown = match self.parts().folded.present(named) {
            true => named,
            false => *self.joined_into().get(&named)?,
        };
        let scanned = &self.parts().bindings.report.joined;
        let found = scanned.binary_search_by_key(&shown, |joined| joined.entity);
        Some(found.map_or(shown, |at| scanned[at].into))
    }

    fn joined_into(&self) -> &BTreeMap<EntityId, EntityId> {
        self.0.joined.get_or_init(|| {
            let mut into = BTreeMap::new();
            for (holder, member) in self.parts().folded.holding(JOINED.name()) {
                if let Ok(joined) = member.decode::<EntityId>() {
                    into.entry(joined).or_insert(holder);
                }
            }
            into
        })
    }

    /// Every value of the relation `key`, a register or a set, with what each
    /// names now. A register holds the value of each write that survives, so each
    /// side of a conflict is read.
    pub fn relations<K>(&self, key: K) -> Relations<K::Value>
    where
        K: Keyed<Value: Relates>,
    {
        let mut relations = Relations {
            related: Vec::new(),
            unreadable: Vec::new(),
        };
        for (from, held) in self.holding(key.key()) {
            self.relate(&mut relations, from, held);
        }
        relations
    }

    /// The entities whose relation `key` names `target`, or the entity `target`
    /// [resolves](View::resolve) to, by id: each as it resolves, once. A value
    /// that does not decode names nothing here; [`View::relations`] lists those.
    pub fn referrers<K>(&self, key: K, target: EntityId) -> Vec<EntityId>
    where
        K: Keyed<Value: Relates>,
    {
        let now = self.resolve(target);
        let related = self.relations(key).related.into_iter();
        let naming = related.filter(|related| {
            related.value.target() == target || (now.is_some() && related.to == now)
        });
        let from: BTreeSet<EntityId> = naming
            .map(|related| self.resolve(related.from).unwrap_or(related.from))
            .collect();
        from.into_iter().collect()
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

    /// Whether the library file at `path` is one no entity is bound to.
    pub fn is_unbound(&self, path: &RelPath) -> bool {
        self.parts().bindings.unbound.contains(path)
    }

    /// The library file at `path` as the last scan listed it; `None` where it
    /// listed none, and for every file until a scan of every file has run.
    pub fn listed(&self, path: &RelPath) -> Option<Listed> {
        self.parts().listing.as_ref()?.get(path).copied()
    }

    /// What differs from `before`, a view of the same library. Costs a step per
    /// entity and file. An entity whose relation names one that differs is not
    /// listed for that.
    pub fn changed_since(&self, before: &View) -> Changed {
        let (now, was) = (self.parts(), before.parts());
        let mut entities: BTreeSet<EntityId> =
            now.folded.changed(&was.folded).into_iter().collect();
        let mut files = BTreeSet::new();
        if !Arc::ptr_eq(&now.bindings, &was.bindings) {
            let (bound, unbound) = (&now.bindings.unbound, &was.bindings.unbound);
            entities.extend(differing(side_by_side(
                &now.bindings.bound,
                &was.bindings.bound,
            )));
            files.extend(bound.symmetric_difference(unbound).cloned());
            let joined = |parts: &'_ Parts| -> BTreeSet<Joined> {
                parts.bindings.report.joined.iter().copied().collect()
            };
            let (joined, was_joined) = (joined(now), joined(was));
            for Joined { entity, into } in joined.symmetric_difference(&was_joined) {
                entities.extend([*entity, *into]);
            }
        }
        match (&now.listing, &was.listing) {
            (Some(listing), Some(listed)) if Arc::ptr_eq(listing, listed) => {}
            (Some(listing), Some(listed)) => files.extend(differing(side_by_side(listing, listed))),
            (listing, listed) => {
                let every = |listing: &Option<Arc<BTreeMap<RelPath, Listed>>>| {
                    let paths = listing.iter().flat_map(|listing| listing.keys());
                    paths.cloned().collect::<Vec<_>>()
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

/// The values `shown` holds under `key`, sorted and once each.
fn held<'a>(shown: Shown<'a>, key: Key) -> Vec<&'a Raw> {
    let values = shown.values.into_iter();
    let under = values.filter(|(kind, name, _)| *kind == key.kind && *name == key.name);
    under.map(|(_, _, value)| value).collect()
}

/// The keys whose entries differ, of two maps [`side_by_side`].
fn differing<'a, K: Clone + 'a, V: PartialEq + 'a>(
    pairs: impl Iterator<Item = Paired<'a, K, V>> + 'a,
) -> impl Iterator<Item = K> + 'a {
    pairs.filter_map(|pair| match pair {
        (Some((_, now)), Some((_, was))) if now == was => None,
        (Some((key, _)), _) | (None, Some((key, _))) => Some(key.clone()),
        (None, None) => None,
    })
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

    /// The entity's values of the relation `key`, a register or a set, with what
    /// each names now, as [`View::relations`] reads them.
    pub fn relations<K>(&self, key: K) -> Relations<K::Value>
    where
        K: Keyed<Value: Relates>,
    {
        let mut relations = Relations {
            related: Vec::new(),
            unreadable: Vec::new(),
        };
        if let Some(shown) = self.view.parts().folded.shown(self.id) {
            self.view
                .relate(&mut relations, self.id, held(shown, key.key()));
        }
        relations
    }

    /// Where the view binds the entity's file; [`FileState::Missing`] at the
    /// logged path when no binding was derived for it, and `None` where it has no
    /// file fact or is one of [`View::joined`].
    pub fn file(&self) -> Option<FileRef> {
        let bindings = &self.view.parts().bindings;
        if let Some(bound) = bindings.bound.get(&self.id) {
            return Some(bound.clone());
        }
        let joined = &bindings.report.joined;
        let joined = joined.binary_search_by_key(&self.id, |joined| joined.entity);
        if joined.is_ok() {
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
