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

use std::collections::BTreeSet;

use crate::error::Invalid;
use crate::ids::{EntityId, EntryHash};
use crate::log::Op;
use crate::merge::Folded;
use crate::path::RelPath;
use crate::plan::{Expect, FactChange, FileChange, Plan, Target};
use crate::schema::{KeyKind, Raw, Register, Schema, Set, Value};

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
    pub fn create(mut self, describe: impl FnOnce(&mut Creating<'_>)) -> Self {
        let target = Target::New(self.plan.creates);
        self.plan.creates += 1;
        describe(&mut Creating {
            plan: &mut self.plan,
            target,
            invalid: &mut self.invalid,
        });
        self
    }

    /// Replaces every write of `key` this writer has observed, which resolves a
    /// conflict.
    pub fn set<T: Value>(mut self, entity: EntityId, key: Register<T>, value: T) -> Self {
        set(
            &mut self.plan,
            &mut self.invalid,
            Target::Existing(entity),
            key,
            &value,
        );
        self
    }

    pub fn clear<T: Value>(mut self, entity: EntityId, key: Register<T>) -> Self {
        self.plan.facts.push(FactChange::Clear {
            entity: Target::Existing(entity),
            key: key.name().to_owned(),
        });
        self
    }

    pub fn add<T: Value>(mut self, entity: EntityId, key: Set<T>, value: T) -> Self {
        add(
            &mut self.plan,
            &mut self.invalid,
            Target::Existing(entity),
            key,
            &value,
        );
        self
    }

    /// Removes every add of `value` this writer has observed.
    pub fn remove<T: Value>(mut self, entity: EntityId, key: Set<T>, value: &T) -> Self {
        if let Some(value) = encode(&mut self.invalid, key.name(), value) {
            self.plan.facts.push(FactChange::Remove {
                entity: Target::Existing(entity),
                key: key.name().to_owned(),
                value,
            });
        }
        self
    }

    /// Deletes the entity, observing every field write this writer has seen. Its
    /// file stays where it is unless the intent also trashes it.
    pub fn delete(mut self, entity: EntityId) -> Self {
        self.plan.facts.push(FactChange::Delete { entity });
        self
    }

    /// Brings a deleted entity back with the fields it had, or settles a deletion
    /// conflict in favor of keeping it.
    pub fn revive(mut self, entity: EntityId) -> Self {
        self.plan.facts.push(FactChange::Revive { entity });
        self
    }

    pub fn save(
        mut self,
        entity: EntityId,
        path: &RelPath,
        bytes: Vec<u8>,
        expect: Expect,
    ) -> Self {
        self.plan.files.push(FileChange::Save {
            entity: Target::Existing(entity),
            path: path.clone(),
            bytes,
            expect,
        });
        self
    }

    pub fn rename(mut self, entity: EntityId, to: &RelPath, expect: Expect) -> Self {
        self.plan.files.push(FileChange::Rename {
            entity,
            to: to.clone(),
            expect,
        });
        self
    }

    /// Moves the entity's file to the trash; the entity stays.
    pub fn trash(mut self, entity: EntityId, expect: Expect) -> Self {
        self.plan.files.push(FileChange::Trash { entity, expect });
        self
    }

    pub fn move_tree(mut self, from: &RelPath, to: &RelPath) -> Self {
        self.plan.files.push(FileChange::MoveTree {
            from: from.clone(),
            to: to.clone(),
        });
        self
    }
}

impl Creating<'_> {
    pub fn set<T: Value>(&mut self, key: Register<T>, value: T) -> &mut Self {
        set(self.plan, self.invalid, self.target, key, &value);
        self
    }

    pub fn add<T: Value>(&mut self, key: Set<T>, value: T) -> &mut Self {
        add(self.plan, self.invalid, self.target, key, &value);
        self
    }

    /// Writes the new entity's file.
    pub fn save(&mut self, path: &RelPath, bytes: Vec<u8>, expect: Expect) -> &mut Self {
        self.plan.files.push(FileChange::Save {
            entity: self.target,
            path: path.clone(),
            bytes,
            expect,
        });
        self
    }
}

fn set<T: Value>(
    plan: &mut Plan,
    invalid: &mut Option<Invalid>,
    entity: Target,
    key: Register<T>,
    value: &T,
) {
    if let Some(value) = encode(invalid, key.name(), value) {
        plan.facts.push(FactChange::Set {
            entity,
            key: key.name().to_owned(),
            value,
        });
    }
}

fn add<T: Value>(
    plan: &mut Plan,
    invalid: &mut Option<Invalid>,
    entity: Target,
    key: Set<T>,
    value: &T,
) {
    if let Some(value) = encode(invalid, key.name(), value) {
        plan.facts.push(FactChange::Add {
            entity,
            key: key.name().to_owned(),
            value,
        });
    }
}

/// The value's JSON, or `None` after keeping the first failure in `invalid`.
fn encode<T: Value>(invalid: &mut Option<Invalid>, key: &str, value: &T) -> Option<Raw> {
    match Raw::of(value) {
        Ok(raw) => Some(raw),
        Err(error) => {
            invalid.get_or_insert(Invalid::Unencodable {
                key: key.to_owned(),
                reason: error.to_string(),
            });
            None
        }
    }
}

/// The ops that log `plan`'s facts against what this writer has observed in
/// `folded`: a create for each of `created`, then each fact change in order. A
/// write replaces every surviving write of its register, a remove names every live
/// add of its value, and a delete observes every live field write of its entity.
///
/// Refuses a key the schema does not declare with the change's kind, and a change
/// to an entity that is neither shown nor revived by the plan. Of several changes
/// to one register of one entity, or to one entity's existence, the last is
/// logged. A plan that changes nothing is refused unless it reverses an entry.
///
/// # Panics
///
/// When a target is [`Target::New`] beyond `created`.
pub fn ops(
    plan: &Plan,
    created: &[EntityId],
    folded: &Folded,
    schema: &Schema,
) -> Result<Vec<Op>, Invalid> {
    let nothing = plan.creates == 0 && plan.facts.is_empty() && plan.files.is_empty();
    if nothing && plan.reverses.is_none() {
        return Err(Invalid::Empty);
    }
    let resolve = |target: Target| match target {
        Target::Existing(entity) => entity,
        Target::New(index) => created[index],
    };
    let revived: BTreeSet<EntityId> = plan
        .facts
        .iter()
        .filter_map(|change| match change {
            FactChange::Revive { entity } => Some(*entity),
            _ => None,
        })
        .collect();
    let writable = |target: Target| {
        let entity = resolve(target);
        match target {
            Target::New(_) => Ok(entity),
            Target::Existing(_) if folded.present(entity) || revived.contains(&entity) => {
                Ok(entity)
            }
            Target::Existing(_) => Err(Invalid::NoEntity(entity)),
        }
    };
    let declared = |key: &str, kind: KeyKind| match schema.kind(key) == Some(kind) {
        true => Ok(key.to_owned()),
        false => Err(Invalid::UndeclaredKey(key.to_owned())),
    };

    let mut ops: Vec<Op> = created
        .iter()
        .map(|&entity| Op::Create {
            entity,
            replaces: Vec::new(),
        })
        .collect();
    for (index, change) in plan.facts.iter().enumerate() {
        if superseded(&plan.facts[index + 1..], change) {
            continue;
        }
        let op = match change {
            FactChange::Set { entity, key, value } => {
                let entity = writable(*entity)?;
                register_write(
                    folded,
                    entity,
                    declared(key, KeyKind::Register)?,
                    Some(value),
                )
            }
            FactChange::Clear { entity, key } => {
                let entity = writable(*entity)?;
                register_write(folded, entity, declared(key, KeyKind::Register)?, None)
            }
            FactChange::Add { entity, key, value } => Op::Add {
                entity: writable(*entity)?,
                key: declared(key, KeyKind::Set)?,
                value: value.clone(),
            },
            FactChange::Remove { entity, key, value } => {
                let entity = writable(*entity)?;
                let key = declared(key, KeyKind::Set)?;
                let tags: Vec<_> = folded
                    .tags(entity, &key, value)
                    .into_iter()
                    .map(|tag| tag.entry)
                    .collect();
                if tags.is_empty() {
                    continue;
                }
                Op::Remove {
                    entity,
                    key,
                    value: value.clone(),
                    tags,
                }
            }
            FactChange::Delete { entity } => {
                if !folded.present(*entity) && !revived.contains(entity) {
                    return Err(Invalid::NoEntity(*entity));
                }
                Op::Delete {
                    entity: *entity,
                    replaces: existence(folded, *entity),
                    observed: folded
                        .live_writes(*entity)
                        .into_iter()
                        .map(|write| write.entry)
                        .collect(),
                }
            }
            FactChange::Revive { entity } => {
                let replaces = existence(folded, *entity);
                if replaces.is_empty() {
                    return Err(Invalid::NoEntity(*entity));
                }
                Op::Create {
                    entity: *entity,
                    replaces,
                }
            }
        };
        ops.push(op);
    }
    Ok(ops)
}

/// Whether a later change in `rest` writes the same register, or the same
/// entity's existence, as `change`.
fn superseded(rest: &[FactChange], change: &FactChange) -> bool {
    let slot = |change: &FactChange| match change {
        FactChange::Set { entity, key, .. } | FactChange::Clear { entity, key } => {
            Some((*entity, Some(key.clone())))
        }
        FactChange::Delete { entity } | FactChange::Revive { entity } => {
            Some((Target::Existing(*entity), None))
        }
        FactChange::Add { .. } | FactChange::Remove { .. } => None,
    };
    let Some(own) = slot(change) else {
        return false;
    };
    rest.iter().any(|later| slot(later).as_ref() == Some(&own))
}

fn register_write(folded: &Folded, entity: EntityId, key: String, value: Option<&Raw>) -> Op {
    let replaces = folded
        .register_writes(entity, &key)
        .into_iter()
        .map(|write| write.entry)
        .collect();
    Op::Write {
        entity,
        key,
        value: value.cloned(),
        replaces,
    }
}

fn existence(folded: &Folded, entity: EntityId) -> Vec<EntryHash> {
    folded
        .existence(entity)
        .into_iter()
        .map(|write| write.entry)
        .collect()
}
