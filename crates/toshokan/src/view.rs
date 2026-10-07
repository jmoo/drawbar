//! Views: immutable values the app renders from.

use std::collections::BTreeSet;
use std::sync::Arc;

use crate::binding::Bindings;
use crate::ids::EntityId;
use crate::merge::Folded;
use crate::path::RelPath;
use crate::report::{Fork, Gap, WriterInfo};
use crate::schema::{Field, Members, Raw, Register, Set, Value, Written};

/// The library as one reader sees it. Cheap to clone; never changes; can be sent
/// to and shared between threads.
#[derive(Clone)]
pub struct View(Arc<Parts>);

/// What a view is built from.
pub struct Parts {
    pub folded: Folded,
    pub bindings: Arc<Bindings>,
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
        Self(Arc::new(parts))
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

    /// The entities whose set `key` holds `value`, by id.
    pub fn find<T: Value>(&self, key: Set<T>, value: &T) -> Vec<EntityId> {
        self.entities()
            .into_iter()
            .filter(|entity| entity.members(key).values.binary_search(value).is_ok())
            .map(|entity| entity.id)
            .collect()
    }

    /// Every conflict among the entities shown, sorted.
    pub fn conflicts(&self) -> Vec<Conflicted> {
        let folded = &self.0.folded;
        let mut conflicts = Vec::new();
        for entity in folded.entities() {
            for key in folded.registers(entity) {
                if distinct(&folded.register(entity, key)) > 1 {
                    conflicts.push(Conflicted::Field {
                        entity,
                        key: key.to_owned(),
                    });
                }
            }
            if folded.deletion_conflicted(entity) {
                conflicts.push(Conflicted::Existence { entity });
            }
        }
        for (entity, files) in folded.files() {
            if distinct(&files) > 1 {
                conflicts.push(Conflicted::File { entity });
            }
        }
        conflicts.sort();
        conflicts
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
}

fn distinct<T: PartialEq>(writes: &[Written<T>]) -> usize {
    let mut values: Vec<&T> = Vec::new();
    for write in writes {
        if !values.contains(&&write.value) {
            values.push(&write.value);
        }
    }
    values.len()
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
        let files = self.view.0.folded.file(self.id);
        let latest = files.last()?;
        Some(FileRef {
            path: latest.value.path.clone(),
            state: FileState::Missing,
        })
    }

    /// Whether a delete and a write it did not observe are both in effect.
    pub fn deletion_conflicted(&self) -> bool {
        self.view.0.folded.deletion_conflicted(self.id)
    }
}
