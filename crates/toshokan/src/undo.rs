//! Undo and redo: compensating intents, per writer, for this session's intents.
//!
//! Undo plans the intent that reverses this writer's latest intent not yet undone:
//! facts are set back by new writes, files come back from this writer's trash
//! under a precondition. Redo reverses the latest undo the same way, while this
//! writer has committed nothing else since. Either is refused, with a reason,
//! where another writer has changed the same thing since: a register no longer
//! holds only what the intent wrote, a set member was added or removed by
//! someone else, or the entity was deleted or revived.
//!
//! Bindings an intent pinned are left alone. The entities an intent created or
//! revived are deleted again rather than having their fields cleared, and undoing
//! a delete revives an entity with the values the delete ended, so a redo
//! revives it whole.

use std::collections::{BTreeMap, BTreeSet};

use crate::contribution::{FieldKey, Name};
use crate::error::{Invalid, Refusal};
use crate::ids::{EntityId, EntryHash, WriterId};
use crate::log::{Displaced, Entry, EntryKind, FileFact, Op, Ref};
use crate::merge::{Exists, Folded, Write};
use crate::path::RelPath;
use crate::plan::{Expect, FactChange, FileChange, Plan};
use crate::report::HistoryItem;
use crate::schema::Raw;
use crate::view::View;

/// This session's intents, oldest first, with what is undone.
#[derive(Clone, PartialEq, Eq, Debug, Default)]
pub struct History {
    items: Vec<HistoryItem>,
    /// The entries the next undos reverse, latest last.
    done: Vec<EntryHash>,
    /// The entries the next redos reverse, latest last.
    undone: Vec<EntryHash>,
    /// The item of each intent, undo and redo.
    origins: BTreeMap<EntryHash, usize>,
    /// Each intent of this session, with what reversing it needs.
    kept: BTreeMap<EntryHash, (Entry, Before)>,
}

/// What an intent replaced, as reversing it needs: the state it was committed
/// over is gone from the merge once the intent replaced it.
#[derive(Clone, PartialEq, Eq, Debug, Default)]
pub struct Before {
    /// Per register the intent wrote, the latest value of the writes it
    /// replaced, `None` for a clear or for none.
    pub registers: BTreeMap<(EntityId, String), Option<Raw>>,
    /// Per entity whose file the intent wrote, the latest file of the writes it
    /// replaced.
    pub files: BTreeMap<EntityId, Option<FileFact>>,
    /// Per entity the intent deleted, the changes that give it back the values
    /// the delete ended: each register's latest value, each member and where its
    /// file is.
    pub revived: BTreeMap<EntityId, Vec<FactChange>>,
}

impl Before {
    /// What `ops` replace in `folded`, the state they were committed over.
    pub fn of(ops: &[Op], folded: &Folded) -> Self {
        fn latest<V: Clone>(writes: Vec<Write<Option<V>>>, replaces: &[Ref]) -> Option<V> {
            let replaced = writes
                .into_iter()
                .filter(|write| replaces.contains(&write.named()));
            replaced
                .max_by_key(|write| (write.at, write.by, write.entry))
                .and_then(|write| write.value)
        }
        let mut before = Self::default();
        for op in ops {
            match op {
                Op::Write {
                    entity,
                    key,
                    replaces,
                    ..
                } => {
                    let value = latest(folded.register_writes(*entity, key), replaces);
                    before.registers.insert((*entity, key.clone()), value);
                }
                Op::File {
                    entity, replaces, ..
                } => {
                    let file = latest(folded.file_writes(*entity), replaces);
                    before.files.insert(*entity, file);
                }
                Op::Delete { entity, .. } => {
                    before.revived.insert(*entity, values(folded, *entity));
                }
                _ => {}
            }
        }
        before
    }
}

/// The changes that write `entity`'s register values, members and file as
/// `folded` shows them.
fn values(folded: &Folded, entity: EntityId) -> Vec<FactChange> {
    let mut changes = Vec::new();
    let files = folded.file_writes(entity).into_iter();
    let file = files.max_by_key(|write| (write.at, write.by, write.entry));
    if let Some(file) = file.and_then(|write| write.value) {
        changes.push(FactChange::File { entity, file });
    }
    for key in folded.registers(entity) {
        let writes = folded.register_writes(entity, key);
        let latest = writes
            .into_iter()
            .max_by_key(|write| (write.at, write.by, write.entry));
        if let Some(value) = latest.and_then(|write| write.value) {
            let key = key.to_owned();
            changes.push(FactChange::Set { entity, key, value });
        }
    }
    let members: BTreeSet<(Name, Raw)> = folded
        .live_writes(entity)
        .into_iter()
        .filter_map(|(field, _)| match field {
            FieldKey::Member(_, key, member) => Some((key, member)),
            _ => None,
        })
        .collect();
    for (key, member) in members {
        if folded.members(entity, &key).contains(&member) {
            let key = key.to_string();
            changes.push(FactChange::Add {
                entity,
                key,
                value: member,
            });
        }
    }
    changes
}

impl History {
    /// Adds this writer's next entry, committed over what `before` says it
    /// replaced. An intent reversing the latest done intent is its undo; one
    /// reversing the latest undo is a redo, which the next undo reverses. Any
    /// other intent starts a new item and ends every redo.
    pub fn push(&mut self, entry: &Entry, reverses: Option<EntryHash>, before: Before) {
        let Some(label) = entry.intent() else {
            return;
        };
        let hash = entry.hash();
        self.kept.insert(hash, (entry.clone(), before));
        let reversed = reverses.and_then(|reverses| {
            let item = *self.origins.get(&reverses)?;
            if self.done.last() == Some(&reverses) {
                self.done.pop();
                self.undone.push(hash);
                Some((item, true))
            } else if self.undone.last() == Some(&reverses) {
                self.undone.pop();
                self.done.push(hash);
                Some((item, false))
            } else {
                None
            }
        });
        match reversed {
            Some((item, undone)) => {
                self.items[item].undone = undone;
                self.origins.insert(hash, item);
            }
            None => {
                self.origins.insert(hash, self.items.len());
                self.items.push(HistoryItem {
                    intent: hash,
                    label,
                    at: entry.at(),
                    undone: false,
                });
                self.done.push(hash);
                self.undone.clear();
            }
        }
    }

    pub fn items(&self) -> &[HistoryItem] {
        &self.items
    }

    /// The plan reversing the latest intent not undone, with [`Plan::reverses`]
    /// naming it. `writer` is this writer's id.
    pub fn plan_undo(&self, writer: WriterId, view: &View) -> Result<Plan, Refusal> {
        self.reverse_latest(&self.done, writer, view.folded())
    }

    /// The plan reapplying the latest undone intent, while nothing was committed
    /// since its undo.
    pub fn plan_redo(&self, writer: WriterId, view: &View) -> Result<Plan, Refusal> {
        self.reverse_latest(&self.undone, writer, view.folded())
    }

    fn reverse_latest(
        &self,
        stack: &[EntryHash],
        writer: WriterId,
        folded: &Folded,
    ) -> Result<Plan, Refusal> {
        let latest = stack.last().ok_or(Refusal::Nothing)?;
        let (entry, before) = self.kept.get(latest).ok_or(Refusal::Nothing)?;
        reverse(entry, before, writer, folded)
    }
}

/// The plan that compensates `entry`, one of `writer`'s intents, committed
/// over what `before` says it replaced.
pub fn reverse(
    entry: &Entry,
    before: &Before,
    writer: WriterId,
    folded: &Folded,
) -> Result<Plan, Refusal> {
    let EntryKind::Intent(logged) = &entry.kind() else {
        return Err(Refusal::Nothing);
    };
    let reversing = Reversing {
        writer,
        folded,
        before,
        displaced: &logged.displaced,
    };
    let recreated: BTreeSet<EntityId> = logged
        .ops
        .iter()
        .filter_map(|op| match op {
            Op::Create { entity, .. } => Some(*entity),
            _ => None,
        })
        .collect();
    let mut plan = Plan {
        label: logged.label.clone(),
        reverses: Some(entry.hash()),
        ..Plan::default()
    };
    for op in &logged.ops {
        match op {
            Op::Create { entity, .. } => {
                reversing.existence(*entity, Exists::Created)?;
                reversing.only_own_writes(*entity)?;
                plan.facts.push(FactChange::Delete { entity: *entity });
            }
            Op::Delete { entity, .. } => {
                reversing.existence(*entity, Exists::Deleted)?;
                plan.facts.push(FactChange::Revive { entity: *entity });
                let revived = before.revived.get(entity).into_iter().flatten();
                plan.facts.extend(revived.cloned());
            }
            Op::Write {
                entity, key, value, ..
            } if !recreated.contains(entity) => {
                plan.facts.push(reversing.write(*entity, key, value)?);
            }
            Op::Add {
                entity, key, value, ..
            } if !recreated.contains(entity) => {
                plan.facts.extend(reversing.add(*entity, key, value)?);
            }
            Op::Remove {
                entity, key, value, ..
            } if !recreated.contains(entity) => {
                plan.facts.push(reversing.remove(*entity, key, value)?);
            }
            Op::File { entity, file, .. } => {
                plan.files.extend(reversing.file(*entity, file.as_ref())?);
            }
            Op::Write { .. }
            | Op::Add { .. }
            | Op::Remove { .. }
            | Op::Pin { .. }
            | Op::Dismiss { .. }
            | Op::Unknown(_) => {}
        }
    }
    Ok(plan)
}

struct Reversing<'a> {
    writer: WriterId,
    folded: &'a Folded,
    before: &'a Before,
    displaced: &'a [Displaced],
}

fn changed<V>(write: &Write<V>) -> Refusal {
    Refusal::ChangedSince {
        by: write.by,
        entry: write.entry,
    }
}

impl Reversing<'_> {
    /// Refuses unless every surviving existence write of `entity` is as `left`.
    fn existence(&self, entity: EntityId, left: Exists) -> Result<(), Refusal> {
        let survivors = self.folded.existence(entity);
        match survivors.iter().rev().find(|write| write.value != left) {
            Some(write) => Err(changed(write)),
            None => Ok(()),
        }
    }

    /// Refuses when another writer's field write on `entity` is live.
    fn only_own_writes(&self, entity: EntityId) -> Result<(), Refusal> {
        let live = self.folded.live_writes(entity);
        match live.iter().rev().find(|(_, write)| write.by != self.writer) {
            Some((_, write)) => Err(changed(write)),
            None => Ok(()),
        }
    }

    /// Refuses unless `entity` is shown and no delete of it survives, naming the
    /// latest delete.
    fn shown(&self, entity: EntityId) -> Result<(), Refusal> {
        let existence = self.folded.existence(entity);
        let delete = existence
            .iter()
            .rev()
            .find(|write| write.value == Exists::Deleted);
        match (delete, existence.is_empty()) {
            (Some(write), _) => Err(changed(write)),
            (None, true) => Err(Refusal::Invalid(Invalid::NoEntity(entity))),
            (None, false) => Ok(()),
        }
    }

    /// Writes back the latest value the write replaced, once the register holds
    /// only what the write left.
    fn write(
        &self,
        entity: EntityId,
        key: &str,
        value: &Option<Raw>,
    ) -> Result<FactChange, Refusal> {
        self.shown(entity)?;
        let survivors = self.folded.register_writes(entity, key);
        if let Some(write) = survivors.iter().rev().find(|write| write.value != *value) {
            return Err(changed(write));
        }
        let key = key.to_owned();
        let previous = self
            .before
            .registers
            .get(&(entity, key.clone()))
            .cloned()
            .flatten();
        Ok(match previous {
            Some(value) => FactChange::Set { entity, key, value },
            None => FactChange::Clear { entity, key },
        })
    }

    /// Removes `value` again, unless another writer added or removed it since.
    /// `None` when only this writer's removal of it is live.
    fn add(&self, entity: EntityId, key: &str, value: &Raw) -> Result<Option<FactChange>, Refusal> {
        self.shown(entity)?;
        let tags = self.folded.tags(entity, key, value);
        if let Some(tag) = tags.iter().rev().find(|tag| tag.by != self.writer) {
            return Err(changed(tag));
        }
        if !tags.iter().any(|tag| tag.value) {
            return Ok(None);
        }
        Ok(Some(FactChange::Remove {
            entity,
            key: key.to_owned(),
            value: value.clone(),
        }))
    }

    /// Adds `value` back, unless another writer added it since.
    fn remove(&self, entity: EntityId, key: &str, value: &Raw) -> Result<FactChange, Refusal> {
        self.shown(entity)?;
        let tags = self.folded.tags(entity, key, value);
        if let Some(tag) = tags.iter().rev().find(|tag| tag.by != self.writer) {
            return Err(changed(tag));
        }
        Ok(FactChange::Add {
            entity,
            key: key.to_owned(),
            value: value.clone(),
        })
    }

    /// The file effect that puts back what a file write changed: the displaced
    /// bytes from the trash, the old name, or the trash for a file the write
    /// added.
    fn file(
        &self,
        entity: EntityId,
        file: Option<&FileFact>,
    ) -> Result<Option<FileChange>, Refusal> {
        let survivors = self.folded.file_writes(entity);
        if let Some(write) = survivors
            .iter()
            .rev()
            .find(|write| write.value.as_ref() != file)
        {
            return Err(changed(write));
        }
        let old = self.before.files.get(&entity).cloned().flatten();
        let restore = |path: &RelPath, expect| {
            let item = self.displaced.iter().find(|item| item.from == *path)?;
            Some(FileChange::Restore {
                entity,
                item: item.item,
                to: path.clone(),
                expect,
            })
        };
        Ok(match (file, old) {
            (Some(new), old) => {
                let back = match old {
                    Some(old) if old.path != new.path => Some(FileChange::Rename {
                        entity,
                        to: old.path,
                        expect: Expect::Holds(new.identity),
                    }),
                    Some(_) => None,
                    None => Some(FileChange::Trash {
                        entity,
                        expect: Expect::Holds(new.identity),
                    }),
                };
                restore(&new.path, Expect::Holds(new.identity)).or(back)
            }
            (None, Some(old)) => restore(&old.path, Expect::Absent),
            (None, None) => None,
        })
    }
}
