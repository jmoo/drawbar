//! Building an intent: one user action, one commit, one undo step.
//!
//! ```ignore
//! lib.intent("Import B3 Split")
//!     .create(|e| {
//!         e.set(ORIGIN, bundle).add(TAGS, sunday);
//!     })
//!     .rename(program, &to, Expect::Holds(identity))
//!     .commit()?;
//! ```
//!
//! Building checks nothing and writes nothing. Commit checks every precondition
//! before anything is written; a refused intent changes nothing.

#![expect(
    dead_code,
    unused_variables,
    reason = "the skeleton's bodies are todo!()"
)]

use crate::error::Invalid;
use crate::ids::EntityId;
use crate::path::RelPath;
use crate::plan::{Expect, Plan, Target};
use crate::schema::{Register, Set, Value};

/// An intent being built for `library`, which commits it: a driver's library.
pub struct Intent<L> {
    library: L,
    plan: Plan,
    /// The first value that could not be written as JSON; commit refuses with it.
    invalid: Option<Invalid>,
}

/// An entity the intent creates, inside [`Intent::create`].
pub struct Creating<'p> {
    plan: &'p mut Plan,
    target: Target,
    invalid: &'p mut Option<Invalid>,
}

impl<L> Intent<L> {
    pub fn new(library: L, label: &str) -> Self {
        Self {
            library,
            plan: Plan {
                label: label.to_owned(),
                ..Plan::default()
            },
            invalid: None,
        }
    }

    /// The plan so far, and why commit will refuse it if it will for an encoding.
    pub fn into_parts(self) -> (L, Result<Plan, Invalid>) {
        let plan = match self.invalid {
            Some(invalid) => Err(invalid),
            None => Ok(self.plan),
        };
        (self.library, plan)
    }

    /// Creates an entity and describes it. [`crate::Committed::created`] lists the
    /// ids of created entities in order.
    pub fn create(self, describe: impl FnOnce(&mut Creating<'_>)) -> Self {
        todo!()
    }

    /// Replaces every write of `key` this writer has observed.
    pub fn set<T: Value>(self, entity: EntityId, key: Register<T>, value: T) -> Self {
        todo!()
    }

    pub fn clear<T: Value>(self, entity: EntityId, key: Register<T>) -> Self {
        todo!()
    }

    pub fn add<T: Value>(self, entity: EntityId, key: Set<T>, value: T) -> Self {
        todo!()
    }

    /// Removes every add of `value` this writer has observed.
    pub fn remove<T: Value>(self, entity: EntityId, key: Set<T>, value: &T) -> Self {
        todo!()
    }

    /// Deletes the entity and moves its file, if it has one, to the trash.
    pub fn delete(self, entity: EntityId) -> Self {
        todo!()
    }

    pub fn save(self, entity: EntityId, path: &RelPath, bytes: Vec<u8>, expect: Expect) -> Self {
        todo!()
    }

    pub fn rename(self, entity: EntityId, to: &RelPath, expect: Expect) -> Self {
        todo!()
    }

    /// Moves the entity's file to the trash; the entity stays.
    pub fn trash(self, entity: EntityId, expect: Expect) -> Self {
        todo!()
    }

    pub fn move_tree(self, from: &RelPath, to: &RelPath) -> Self {
        todo!()
    }
}

impl Creating<'_> {
    pub fn set<T: Value>(&mut self, key: Register<T>, value: T) -> &mut Self {
        todo!()
    }

    pub fn add<T: Value>(&mut self, key: Set<T>, value: T) -> &mut Self {
        todo!()
    }

    /// Writes the new entity's file.
    pub fn save(&mut self, path: &RelPath, bytes: Vec<u8>, expect: Expect) -> &mut Self {
        todo!()
    }
}
