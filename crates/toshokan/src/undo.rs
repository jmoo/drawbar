//! Undo and redo: compensating intents, per writer.
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
//! revived are deleted again rather than having their fields cleared, so a redo
//! revives them whole.

use std::collections::BTreeSet;

use crate::error::{Invalid, Refusal};
use crate::ids::{EntityId, EntryHash, WriterId};
use crate::log::{Displaced, Entry, EntryKind, FileFact, Op};
use crate::merge::{Exists, Folded, Write};
use crate::path::RelPath;
use crate::plan::{Expect, FactChange, FileChange, Plan, Target};
use crate::report::HistoryItem;
use crate::schema::Raw;
use crate::view::View;

/// This writer's intents, oldest first, with what is undone.
#[derive(Clone, PartialEq, Eq, Debug, Default)]
pub struct History {
    items: Vec<HistoryItem>,
    /// The entries the next undos reverse, latest last.
    done: Vec<EntryHash>,
    /// The entries the next redos reverse, latest last.
    undone: Vec<EntryHash>,
}

impl History {
    /// From this writer's own entries in chain order. Intents a snapshot folded
    /// are out of reach: compaction ends what can be undone.
    ///
    /// An entry reversing the latest done intent is its undo; one reversing the
    /// latest undo is a redo, which the next undo reverses. Any other intent
    /// starts a new item and ends every redo.
    pub fn of<'a>(own: impl IntoIterator<Item = &'a Entry>) -> Self {
        let mut history = Self::default();
        let mut origins: Vec<(EntryHash, usize)> = Vec::new();
        for entry in own {
            let EntryKind::Intent(logged) = &entry.kind else {
                continue;
            };
            let hash = entry.hash();
            let origin = |of: EntryHash| {
                origins
                    .iter()
                    .find(|(entry, _)| *entry == of)
                    .map(|&(_, item)| item)
            };
            let reversed = logged.reverses.and_then(|reverses| {
                let item = origin(reverses)?;
                if history.done.last() == Some(&reverses) {
                    history.done.pop();
                    history.undone.push(hash);
                    Some((item, true))
                } else if history.undone.last() == Some(&reverses) {
                    history.undone.pop();
                    history.done.push(hash);
                    Some((item, false))
                } else {
                    None
                }
            });
            match reversed {
                Some((item, undone)) => {
                    history.items[item].undone = undone;
                    origins.push((hash, item));
                }
                None => {
                    origins.push((hash, history.items.len()));
                    history.items.push(HistoryItem {
                        intent: hash,
                        label: logged.label.clone(),
                        at: entry.at,
                        undone: false,
                    });
                    history.done.push(hash);
                    history.undone.clear();
                }
            }
        }
        history
    }

    pub fn items(&self) -> &[HistoryItem] {
        &self.items
    }

    /// The plan reversing the latest intent not undone, with [`Plan::reverses`]
    /// naming it. `own` and `writer` are this writer's entries and id.
    pub fn plan_undo(&self, own: &[Entry], writer: WriterId, view: &View) -> Result<Plan, Refusal> {
        reverse_latest(&self.done, own, writer, view.folded())
    }

    /// The plan reapplying the latest undone intent, while nothing was committed
    /// since its undo.
    pub fn plan_redo(&self, own: &[Entry], writer: WriterId, view: &View) -> Result<Plan, Refusal> {
        reverse_latest(&self.undone, own, writer, view.folded())
    }
}

fn reverse_latest(
    stack: &[EntryHash],
    own: &[Entry],
    writer: WriterId,
    folded: &Folded,
) -> Result<Plan, Refusal> {
    let latest = stack.last().ok_or(Refusal::Nothing)?;
    let entry = own
        .iter()
        .find(|entry| entry.hash() == *latest)
        .ok_or(Refusal::Nothing)?;
    reverse(entry, writer, folded)
}

/// The plan that compensates `entry`, one of `writer`'s intents.
pub fn reverse(entry: &Entry, writer: WriterId, folded: &Folded) -> Result<Plan, Refusal> {
    let EntryKind::Intent(logged) = &entry.kind else {
        return Err(Refusal::Nothing);
    };
    let reversing = Reversing {
        entry: entry.hash(),
        writer,
        folded,
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
            }
            Op::Write {
                entity,
                key,
                value,
                replaces,
            } if !recreated.contains(entity) => {
                plan.facts
                    .push(reversing.write(*entity, key, value, replaces)?);
            }
            Op::Add { entity, key, value } if !recreated.contains(entity) => {
                plan.facts.extend(reversing.add(*entity, key, value)?);
            }
            Op::Remove {
                entity, key, value, ..
            } if !recreated.contains(entity) => {
                plan.facts.push(reversing.remove(*entity, key, value)?);
            }
            Op::File {
                entity,
                file,
                replaces,
            } => {
                plan.files
                    .extend(reversing.file(*entity, file.as_ref(), replaces)?);
            }
            Op::Write { .. }
            | Op::Add { .. }
            | Op::Remove { .. }
            | Op::Pin { .. }
            | Op::Unknown(_) => {}
        }
    }
    Ok(plan)
}

struct Reversing<'a> {
    entry: EntryHash,
    writer: WriterId,
    folded: &'a Folded,
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
        match live.iter().rev().find(|write| write.by != self.writer) {
            Some(write) => Err(changed(write)),
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
        replaces: &[EntryHash],
    ) -> Result<FactChange, Refusal> {
        self.shown(entity)?;
        let survivors = self.folded.register_writes(entity, key);
        if let Some(write) = survivors.iter().rev().find(|write| write.value != *value) {
            return Err(changed(write));
        }
        let previous = replaces
            .iter()
            .filter_map(|&entry| self.folded.register_write(entity, key, entry))
            .max_by_key(|write| (write.at, write.by, write.entry))
            .and_then(|write| write.value);
        let entity = Target::Existing(entity);
        let key = key.to_owned();
        Ok(match previous {
            Some(value) => FactChange::Set { entity, key, value },
            None => FactChange::Clear { entity, key },
        })
    }

    /// Removes `value` again, unless another writer added it too or removed this
    /// add. `None` when the add is gone and only this writer removed it.
    fn add(&self, entity: EntityId, key: &str, value: &Raw) -> Result<Option<FactChange>, Refusal> {
        self.shown(entity)?;
        let tags = self.folded.tags(entity, key, value);
        if let Some(tag) = tags.iter().rev().find(|tag| tag.by != self.writer) {
            return Err(changed(tag));
        }
        if tags.is_empty() {
            return match self.folded.removal(entity, key, value, self.entry) {
                Some(removal) if removal.by != self.writer => Err(changed(&removal)),
                _ => Ok(None),
            };
        }
        Ok(Some(FactChange::Remove {
            entity: Target::Existing(entity),
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
            entity: Target::Existing(entity),
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
        replaces: &[EntryHash],
    ) -> Result<Option<FileChange>, Refusal> {
        let survivors = self.folded.file_writes(entity);
        if let Some(write) = survivors
            .iter()
            .rev()
            .find(|write| write.value.as_ref() != file)
        {
            return Err(changed(write));
        }
        let old = replaces
            .iter()
            .filter_map(|&entry| self.folded.file_write(entity, entry))
            .max_by_key(|write| (write.at, write.by, write.entry))
            .and_then(|write| write.value);
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
