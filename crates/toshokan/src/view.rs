//! Views: immutable values the app renders from.

#![expect(
    dead_code,
    unused_variables,
    reason = "the skeleton's bodies are todo!()"
)]

use std::rc::Rc;

use crate::binding::Bindings;
use crate::ids::EntityId;
use crate::merge::Folded;
use crate::path::RelPath;
use crate::report::{Fork, Gap, WriterInfo};
use crate::schema::{Field, Members, Register, Set, Value};

/// The library as one reader sees it. Cheap to clone; never changes.
#[derive(Clone)]
pub struct View(Rc<Parts>);

/// What a view is built from.
pub struct Parts {
    pub folded: Folded,
    pub bindings: Bindings,
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
        todo!()
    }

    /// Every entity that exists or whose deletion is in conflict, by id.
    pub fn entities(&self) -> Vec<EntityView<'_>> {
        todo!()
    }

    pub fn entity(&self, id: EntityId) -> Option<EntityView<'_>> {
        todo!()
    }

    /// The entities whose set `key` holds `value`, by id.
    pub fn find<T: Value>(&self, key: Set<T>, value: &T) -> Vec<EntityId> {
        todo!()
    }

    pub fn conflicts(&self) -> Vec<Conflicted> {
        todo!()
    }

    pub fn forks(&self) -> &[Fork] {
        todo!()
    }

    pub fn gaps(&self) -> &[Gap] {
        todo!()
    }

    pub fn writers(&self) -> &[WriterInfo] {
        todo!()
    }

    /// Library files no entity is bound to. They get an entity only when an intent
    /// says something about them.
    pub fn unbound(&self) -> &[RelPath] {
        todo!()
    }
}

impl EntityView<'_> {
    pub fn id(&self) -> EntityId {
        todo!()
    }

    pub fn get<T: Value>(&self, key: Register<T>) -> Field<T> {
        todo!()
    }

    pub fn members<T: Value>(&self, key: Set<T>) -> Members<T> {
        todo!()
    }

    pub fn file(&self) -> Option<FileRef> {
        todo!()
    }

    /// Whether a delete and a write it did not observe are both in effect.
    pub fn deletion_conflicted(&self) -> bool {
        todo!()
    }
}
