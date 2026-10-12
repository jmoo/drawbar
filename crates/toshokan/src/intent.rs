//! Building an intent: one user action, one commit, one undo step.
//!
//! ```ignore
//! let (intent, piano) = lib.intent("Import B3 Split").create(|e| {
//!     e.save(&piano_path, piano_bytes, Expect::Absent);
//! });
//! let (intent, program) = intent.create(|e| {
//!     e.set(ORIGIN, bundle).set(PLAYS, piano);
//! });
//! intent.add(program, TAGS, tag).commit()?;
//! ```
//!
//! Building checks nothing and writes nothing; it only draws the ids of the
//! entities the intent creates, so one intent can create entities that name each
//! other. Commit checks every precondition before anything is written; a refused
//! intent changes nothing.

use std::collections::BTreeSet;

use crate::error::Invalid;
use crate::ids::{EntityId, EntryHash};
use crate::log::Op;
use crate::merge::Folded;
use crate::path::RelPath;
use crate::plan::{Content, Expect, FactChange, FileChange, Plan};
use crate::schema::{KeyKind, Raw, Register, Schema, Set, Value};

/// The library an intent is built for and committed through: a driver's.
pub trait Driver {
    /// What the driver fills a saved file from.
    type Source;

    /// A source holding `bytes`.
    fn bytes(bytes: Vec<u8>) -> Self::Source;

    /// An id for an entity the intent creates, from the library's randomness.
    fn entity_id(&mut self) -> EntityId;
}

/// An intent being built for `library`, which commits it.
pub struct Intent<L: Driver> {
    library: L,
    plan: Plan,
    /// What each [`Content`] of the plan is filled from.
    sources: Vec<L::Source>,
    /// The first value that could not be written as JSON; commit refuses with it.
    invalid: Option<Invalid>,
}

/// An entity the intent creates, inside [`Intent::create`].
pub struct Creating<'p, S> {
    plan: &'p mut Plan,
    sources: &'p mut Vec<S>,
    /// The driver's [`Driver::bytes`].
    bytes: fn(Vec<u8>) -> S,
    entity: EntityId,
    invalid: &'p mut Option<Invalid>,
}

impl<L: Driver> Intent<L> {
    pub fn new(library: L, label: &str) -> Self {
        Self {
            library,
            plan: Plan {
                label: label.to_owned(),
                ..Plan::default()
            },
            sources: Vec::new(),
            invalid: None,
        }
    }

    /// The library, the plan so far or why commit will refuse it for an encoding,
    /// and what the plan's contents are filled from, in order.
    pub fn into_parts(self) -> (L, Result<Plan, Invalid>, Vec<L::Source>) {
        let plan = match self.invalid {
            Some(invalid) => Err(invalid),
            None => Ok(self.plan),
        };
        (self.library, plan, self.sources)
    }

    /// Creates an entity, describes it, and returns the intent with the entity's
    /// id, which later changes of this intent, other entities' fields among them,
    /// may name.
    pub fn create(
        mut self,
        describe: impl FnOnce(&mut Creating<'_, L::Source>),
    ) -> (Self, EntityId) {
        let entity = self.library.entity_id();
        self.plan.created.push(entity);
        describe(&mut Creating {
            plan: &mut self.plan,
            sources: &mut self.sources,
            bytes: L::bytes,
            entity,
            invalid: &mut self.invalid,
        });
        (self, entity)
    }

    /// Replaces every write of `key` this writer has observed, which resolves a
    /// conflict.
    pub fn set<T: Value>(mut self, entity: EntityId, key: Register<T>, value: T) -> Self {
        set(&mut self.plan, &mut self.invalid, entity, key, &value);
        self
    }

    pub fn clear<T: Value>(mut self, entity: EntityId, key: Register<T>) -> Self {
        self.plan.facts.push(FactChange::Clear {
            entity,
            key: key.name().to_owned(),
        });
        self
    }

    pub fn add<T: Value>(mut self, entity: EntityId, key: Set<T>, value: T) -> Self {
        add(&mut self.plan, &mut self.invalid, entity, key, &value);
        self
    }

    /// Removes every add of `value` this writer has observed.
    pub fn remove<T: Value>(mut self, entity: EntityId, key: Set<T>, value: &T) -> Self {
        if let Some(value) = encode(&mut self.invalid, key.name(), value) {
            self.plan.facts.push(FactChange::Remove {
                entity,
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

    /// Writes `bytes` as the entity's file at `path`.
    pub fn save(self, entity: EntityId, path: &RelPath, bytes: Vec<u8>, expect: Expect) -> Self {
        self.save_from(entity, path, L::bytes(bytes), expect)
    }

    /// Writes the entity's file at `path` from `source`, which the driver streams
    /// into this writer's staging as the commit runs.
    pub fn save_from(
        mut self,
        entity: EntityId,
        path: &RelPath,
        source: L::Source,
        expect: Expect,
    ) -> Self {
        save(
            &mut self.plan,
            &mut self.sources,
            entity,
            path,
            source,
            expect,
        );
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

impl<S> Creating<'_, S> {
    /// The new entity's id.
    pub fn id(&self) -> EntityId {
        self.entity
    }

    pub fn set<T: Value>(&mut self, key: Register<T>, value: T) -> &mut Self {
        set(self.plan, self.invalid, self.entity, key, &value);
        self
    }

    pub fn add<T: Value>(&mut self, key: Set<T>, value: T) -> &mut Self {
        add(self.plan, self.invalid, self.entity, key, &value);
        self
    }

    /// Gives the new entity the library file at `path`, which holds what `expect`
    /// says: a file no entity is bound to, such as a copy.
    pub fn adopt(&mut self, path: &RelPath, expect: Expect) -> &mut Self {
        self.plan.files.push(FileChange::Adopt {
            entity: self.entity,
            path: path.clone(),
            expect,
        });
        self
    }

    /// Writes `bytes` as the new entity's file.
    pub fn save(&mut self, path: &RelPath, bytes: Vec<u8>, expect: Expect) -> &mut Self {
        let source = (self.bytes)(bytes);
        self.save_from(path, source, expect)
    }

    /// Writes the new entity's file from `source`; see [`Intent::save_from`].
    pub fn save_from(&mut self, path: &RelPath, source: S, expect: Expect) -> &mut Self {
        save(self.plan, self.sources, self.entity, path, source, expect);
        self
    }
}

fn save<S>(
    plan: &mut Plan,
    sources: &mut Vec<S>,
    entity: EntityId,
    path: &RelPath,
    source: S,
    expect: Expect,
) {
    plan.files.push(FileChange::Save {
        entity,
        path: path.clone(),
        content: Content(sources.len()),
        expect,
    });
    sources.push(source);
}

fn set<T: Value>(
    plan: &mut Plan,
    invalid: &mut Option<Invalid>,
    entity: EntityId,
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
    entity: EntityId,
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
/// `folded`: a create for each entity it creates, then each fact change in order.
/// A write replaces every surviving write of its register, a remove names every
/// live add of its value, and a delete observes every live field write of its
/// entity.
///
/// Refuses a key the schema does not declare with the change's kind, and a change
/// to an entity that is neither shown, created nor revived by the plan. Of several
/// changes to one register of one entity, or to one entity's existence, the last
/// is logged. A plan that changes nothing is refused unless it reverses an entry.
pub fn ops(plan: &Plan, folded: &Folded, schema: &Schema) -> Result<Vec<Op>, Invalid> {
    let nothing = plan.created.is_empty() && plan.facts.is_empty() && plan.files.is_empty();
    if nothing && plan.reverses.is_none() {
        return Err(Invalid::Empty);
    }
    let revived: BTreeSet<EntityId> = plan
        .facts
        .iter()
        .filter_map(|change| match change {
            FactChange::Revive { entity } => Some(*entity),
            _ => None,
        })
        .collect();
    let writable = |entity: EntityId| {
        let known =
            plan.created.contains(&entity) || folded.present(entity) || revived.contains(&entity);
        match known {
            true => Ok(entity),
            false => Err(Invalid::NoEntity(entity)),
        }
    };
    let declared = |key: &str, kind: KeyKind| match schema.kind(key) == Some(kind) {
        true => Ok(key.to_owned()),
        false => Err(Invalid::UndeclaredKey(key.to_owned())),
    };

    let mut ops: Vec<Op> = plan
        .created
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
        FactChange::Delete { entity } | FactChange::Revive { entity } => Some((*entity, None)),
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
