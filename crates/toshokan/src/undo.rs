//! Undo and redo: compensating intents, per writer, for this session's intents.

use std::cell::OnceCell;
use std::collections::{BTreeMap, BTreeSet};

use crate::contribution::{self, FieldKey, Name};
use crate::derive::Placed;
use crate::env::Names;
use crate::error::{Invalid, Refusal};
use crate::ids::{EntityId, EntryHash, Hlc, WriterId};
use crate::log::{Displaced, Entry, EntryKind, FileFact, Logged, Op, Ref};
use crate::merge::{Exists, Folded, Write};
use crate::path::RelPath;
use crate::plan::{Expect, FactChange, FileChange, Plan};
use crate::report::HistoryItem;
use crate::schema::Raw;
use crate::view::{FileRef, FileState, View};

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
    kept: BTreeMap<EntryHash, (Hlc, Logged, Before)>,
    /// The entry of the latest intent, undo or redo.
    last: Option<EntryHash>,
    /// The final intents, whose writes undo takes for another writer's: a join
    /// holds what the joined entity held, a settlement what another writer did.
    finals: BTreeSet<EntryHash>,
}

/// How an intent being committed enters this session's history.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum Entering {
    /// As a step of its own, which ends every redo.
    #[default]
    Step,
    /// As the undo or redo of this entry.
    Reversing(EntryHash),
    /// Not at all: it only joins entities, or settles another writer's record.
    Final,
}

impl Entering {
    /// How committing `plan` enters the history.
    pub fn of(plan: &Plan) -> Self {
        let only_joins = plan.created.is_empty() && plan.facts.is_empty() && plan.files.is_empty();
        match plan.reverses {
            Some(entry) => Entering::Reversing(entry),
            None if only_joins && !plan.joins.is_empty() => Entering::Final,
            None => Entering::Step,
        }
    }
}

/// What an intent replaced, as reversing it needs: the state it was committed
/// over is gone from the merge once the intent replaced it.
#[derive(Clone, PartialEq, Eq, Debug, Default)]
pub struct Before {
    /// Per register the intent wrote, the latest value of the writes it
    /// replaced, `None` for a clear or for none.
    pub registers: BTreeMap<(EntityId, String), Option<Raw>>,
    /// Per entity whose file the intent wrote, the file of the writes it replaced
    /// that the binding found it at, or where the binding found that file moved, else
    /// the latest.
    pub files: BTreeMap<EntityId, Option<FileFact>>,
    /// The entities of `files` whose file the binding found nowhere, so that the
    /// intent's file is one it added.
    pub missing: BTreeSet<EntityId>,
    /// Per entity the intent deleted, the changes that give it back the values
    /// the delete ended: each register's latest value, each member and where its
    /// file is.
    pub revived: BTreeMap<EntityId, Vec<FactChange>>,
    /// Per other writer, the number of its latest entry the intent was committed
    /// over: its entries above that one the intent did not see.
    pub seen: BTreeMap<WriterId, u64>,
}

impl Before {
    /// What `ops` replace in `folded`, the state they were committed over, with
    /// each file where `moved`, the pins of the moves the binding found then,
    /// says it was, else where `bound`, the binding then, found it.
    pub fn of(
        ops: &[Op],
        folded: &Folded,
        moved: &[Op],
        bound: &BTreeMap<EntityId, FileRef>,
        seen: BTreeMap<WriterId, u64>,
    ) -> Self {
        fn latest<V: Clone>(writes: Vec<Write<Option<V>>>, replaces: &[Ref]) -> Option<V> {
            let replaced = writes
                .into_iter()
                .filter(|write| replaces.contains(&write.named()));
            replaced
                .max_by_key(|write| (write.at, write.by, write.entry))
                .and_then(|write| write.value)
        }
        let mut before = Self {
            seen,
            ..Self::default()
        };
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
                    let at = bound.get(entity);
                    if at.is_some_and(|file| file.state == FileState::Missing) {
                        before.missing.insert(*entity);
                    }
                    let at = at.filter(|file| file.state != FileState::Missing);
                    let writes = folded.file_writes(*entity);
                    let there = writes
                        .iter()
                        .filter(|write| replaces.contains(&write.named()));
                    let there = there.filter_map(|write| write.value.clone());
                    let mut there = there.filter(|fact| at.is_some_and(|at| at.path == fact.path));
                    let file = there.next().or_else(|| latest(writes, replaces));
                    let file = file.map(|fact| found_at(fact, at));
                    before.files.insert(*entity, found(moved, *entity).or(file));
                }
                Op::Delete { entity, .. } => {
                    let at = bound.get(entity);
                    let values = values(folded, moved, at, *entity);
                    before.revived.insert(*entity, values);
                }
                _ => {}
            }
        }
        before
    }
}

/// The number of the last entry of each writer's history in `writers`.
pub fn seen(writers: &BTreeMap<WriterId, Placed>) -> BTreeMap<WriterId, u64> {
    let heads = writers.iter();
    let heads = heads.filter_map(|(by, placed)| Some((*by, placed.head()?.n)));
    heads.collect()
}

/// Where the pin of `entity` among `moved` says its file is.
fn found(moved: &[Op], entity: EntityId) -> Option<FileFact> {
    moved.iter().find_map(|op| match op {
        Op::Pin {
            entity: pinned,
            file,
            ..
        } if *pinned == entity => Some(file.clone()),
        _ => None,
    })
}

/// `fact` at the path the binding found its file at, `at`, where it found it: a move
/// no pin says yet.
fn found_at(fact: FileFact, at: Option<&FileRef>) -> FileFact {
    match at.filter(|file| file.state != FileState::Missing) {
        Some(at) => FileFact {
            path: at.path.clone(),
            ..fact
        },
        None => fact,
    }
}

/// The changes that write `entity`'s register values, members and file as
/// `folded` shows them, its file where `moved` says it is, else where the binding
/// found it, `at`.
fn values(
    folded: &Folded,
    moved: &[Op],
    at: Option<&FileRef>,
    entity: EntityId,
) -> Vec<FactChange> {
    let mut changes = Vec::new();
    let files = folded.file_writes(entity).into_iter();
    let file = files.max_by_key(|write| (write.at, write.by, write.entry));
    let file = file
        .and_then(|write| write.value)
        .map(|fact| found_at(fact, at));
    if let Some(file) = found(moved, entity).or(file) {
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

/// Whether an intent whose ops are `ops` may be a step of the history: one that
/// only dismisses runs is not.
pub fn is_step<'a>(ops: impl IntoIterator<Item = &'a Op>) -> bool {
    let mut ops = ops.into_iter().peekable();
    ops.peek().is_none() || !ops.all(|op| matches!(op, Op::Dismiss { .. }))
}

impl History {
    /// Adds this writer's next intent `entry`, which logs `logged`, committed over
    /// what `before` says it replaced. An intent reversing the latest done intent
    /// is its undo; one reversing the latest undo is a redo, which the next undo
    /// reverses. Any other intent starts a new item and ends every redo. An intent
    /// that only dismisses runs, or enters as [`Entering::Final`], is no item.
    pub fn push(&mut self, entry: &Entry, logged: Logged, entering: Entering, before: Before) {
        let reverses = match entering {
            Entering::Final => {
                self.finals.insert(entry.hash());
                return;
            }
            Entering::Step => None,
            Entering::Reversing(reverses) => Some(reverses),
        };
        if !is_step(&logged.ops) {
            return;
        }
        let label = logged.label.clone();
        let hash = entry.hash();
        self.last = Some(hash);
        self.kept.insert(hash, (entry.at(), logged, before));
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

    /// The entry of this session's latest intent, undo or redo.
    pub fn last(&self) -> Option<EntryHash> {
        self.last
    }

    /// Lets go of the intent the next undo would reverse, as once its undo is
    /// refused: the next undo reaches the one before it.
    pub fn forget_undo(&mut self) {
        self.done.pop();
    }

    /// Lets go of the undo the next redo would reverse, as once its redo is refused.
    pub fn forget_redo(&mut self) {
        self.undone.pop();
    }

    /// The plan reversing the latest intent not undone, with [`Plan::reverses`]
    /// naming it. `writer` is this writer's id, and `writers` every writer's
    /// entries as the reader places them.
    pub fn plan_undo(
        &self,
        writer: WriterId,
        view: &View,
        writers: &BTreeMap<WriterId, Placed>,
        names: &dyn Names,
    ) -> Result<Plan, Refusal> {
        self.reverse_latest(&self.done, writer, view, writers, names)
    }

    /// The plan reapplying the latest undone intent, while nothing was committed
    /// since its undo.
    pub fn plan_redo(
        &self,
        writer: WriterId,
        view: &View,
        writers: &BTreeMap<WriterId, Placed>,
        names: &dyn Names,
    ) -> Result<Plan, Refusal> {
        self.reverse_latest(&self.undone, writer, view, writers, names)
    }

    fn reverse_latest(
        &self,
        stack: &[EntryHash],
        writer: WriterId,
        view: &View,
        writers: &BTreeMap<WriterId, Placed>,
        names: &dyn Names,
    ) -> Result<Plan, Refusal> {
        let latest = stack.last().ok_or(Refusal::Nothing)?;
        let (at, logged, before) = self.kept.get(latest).ok_or(Refusal::Nothing)?;
        let reversing = Reversing {
            writer,
            intent: *latest,
            at: *at,
            finals: &self.finals,
            folded: view.folded(),
            view,
            writers,
            before,
            displaced: &logged.displaced,
            names,
            later: OnceCell::new(),
        };
        reverse(*latest, logged, &reversing)
    }
}

/// The plan that compensates `intent`, one of the reversing writer's intents,
/// which logged `logged`.
fn reverse(intent: EntryHash, logged: &Logged, reversing: &Reversing) -> Result<Plan, Refusal> {
    let before = reversing.before;
    let recreated: BTreeSet<EntityId> = logged
        .ops
        .iter()
        .filter_map(|op| match op {
            Op::Create { entity, .. } => Some(*entity),
            _ => None,
        })
        .collect();
    let made_or_deleted: BTreeSet<EntityId> = logged
        .ops
        .iter()
        .filter_map(|op| match op {
            Op::Create { entity, .. } | Op::Delete { entity, .. } => Some(*entity),
            _ => None,
        })
        .collect();
    let mut plan = Plan {
        label: logged.label.clone(),
        reverses: Some(intent),
        ..Plan::default()
    };
    let mut trees: Vec<MovedBack> = logged
        .ops
        .iter()
        .filter_map(|op| match op {
            Op::Tree { from, to } => Some(MovedBack {
                from: to.clone(),
                to: from.clone(),
                held: Vec::new(),
            }),
            _ => None,
        })
        .collect();
    let mut undone: Vec<Option<FileChange>> = Vec::new();
    let mut facts = Vec::new();
    for op in &logged.ops {
        match op {
            Op::Create { entity, .. } => {
                reversing.existence(*entity, Exists::Created)?;
                reversing.only_own_writes(*entity)?;
                facts.push(FactChange::Delete { entity: *entity });
            }
            Op::Delete { entity, .. } => {
                reversing.existence(*entity, Exists::Deleted)?;
                facts.push(FactChange::Revive { entity: *entity });
                let revived = before.revived.get(entity).into_iter().flatten();
                facts.extend(revived.cloned());
            }
            Op::Write {
                entity, key, value, ..
            } if !recreated.contains(entity) => {
                facts.push(reversing.write(*entity, key, value)?);
            }
            Op::Add {
                entity, key, value, ..
            } if !recreated.contains(entity) => {
                facts.extend(reversing.add(*entity, key, value)?);
            }
            Op::Remove {
                entity, key, value, ..
            } if !recreated.contains(entity) => {
                facts.push(reversing.remove(*entity, key, value)?);
            }
            Op::File { entity, file, .. } => {
                if !made_or_deleted.contains(entity) {
                    reversing.undeleted(*entity)?;
                }
                let change = reversing.file(*entity, file.as_ref())?;
                undone.extend(carried(&mut trees, file.as_ref(), change).map(Some));
            }
            Op::Path { from, file } => {
                let change = reversing.path(from.as_ref(), file.as_ref())?;
                undone.extend(carried(&mut trees, file.as_ref(), change).map(Some));
            }
            Op::Dir { path, made } => {
                reversing.unchanged_under(path)?;
                undone.push(Some(match made {
                    true => FileChange::RemoveDir { path: path.clone() },
                    false => FileChange::MakeDir { path: path.clone() },
                }));
            }
            Op::Tree { to, .. } => {
                reversing.unchanged_under(to)?;
                undone.push(None);
            }
            Op::Write { .. }
            | Op::Add { .. }
            | Op::Remove { .. }
            | Op::Pin { .. }
            | Op::Dismiss { .. }
            | Op::Unknown(_) => {}
        }
    }
    let mut trees = trees.into_iter().map(|tree| FileChange::MoveBack {
        from: tree.from,
        to: tree.to,
        held: tree.held,
    });
    let in_order: Vec<FileChange> = undone
        .into_iter()
        .filter_map(|change| change.or_else(|| trees.next()))
        .collect();
    plan.files.extend(in_order.into_iter().rev());
    // ⚠️ The log keeps trees, then directories, then files, whatever order the intent
    // made them in, so a directory the intent removed is made again before anything
    // moves back into it, and one it made is removed after everything moved out.
    plan.files.sort_by_key(|change| match change {
        FileChange::MakeDir { .. } => 0,
        FileChange::RemoveDir { .. } => 2,
        _ => 1,
    });
    plan.facts = facts.into_iter().map(|change| (change, None)).collect();
    Ok(plan)
}

/// A tree move being undone as one move back, with the files it carries back.
struct MovedBack {
    from: RelPath,
    to: RelPath,
    held: Vec<(RelPath, Expect)>,
}

impl MovedBack {
    /// Takes `change` when it renames the file at `at` to where the move back
    /// carries it, keeping only what it expects there.
    fn takes(&mut self, at: &RelPath, change: &FileChange) -> bool {
        let (FileChange::Rename { to, expect, .. } | FileChange::RenameFile { to, expect, .. }) =
            change
        else {
            return false;
        };
        let carried = at.rebase(&self.from, &self.to).as_ref() == Some(to);
        if carried {
            self.held.push((at.clone(), *expect));
        }
        carried
    }
}

/// `change` to the file `file` describes, unless one of `trees` carries it back.
fn carried(
    trees: &mut [MovedBack],
    file: Option<&FileFact>,
    change: Option<FileChange>,
) -> Option<FileChange> {
    let change = change?;
    let taken =
        file.is_some_and(|file| trees.iter_mut().any(|tree| tree.takes(&file.path, &change)));
    (!taken).then_some(change)
}

struct Reversing<'a> {
    writer: WriterId,
    /// The entry of the intent being reversed.
    intent: EntryHash,
    /// The clock reading of the intent being reversed.
    at: Hlc,
    finals: &'a BTreeSet<EntryHash>,
    folded: &'a Folded,
    view: &'a View,
    writers: &'a BTreeMap<WriterId, Placed>,
    before: &'a Before,
    displaced: &'a [Displaced],
    names: &'a dyn Names,
    /// The entries of other writers since the intent, each with its ops' paths as
    /// keys, built at the first check that needs them.
    later: OnceCell<Vec<Later>>,
}

/// An entry of another writer since the intent being reversed.
struct Later {
    by: WriterId,
    at: Hlc,
    entry: EntryHash,
    ops: Vec<Keyed>,
}

/// What an op changes, its paths as keys [`Names`] gives.
struct Keyed {
    entity: Option<EntityId>,
    /// The paths a file effect moves a file from or leaves it at, or binds it at.
    files: Vec<String>,
    /// The tree a `tree` op moves.
    tree: Option<String>,
    /// The directory a `dir` op makes or removes.
    dir: Option<String>,
}

impl Keyed {
    fn of(op: &Op, names: &dyn Names) -> Self {
        let key = |path: &RelPath| names.key(path.as_str());
        let (files, tree, dir) = match op {
            Op::Path { from, file } => {
                let files = from.iter().chain(file.iter().map(|file| &file.path));
                (files.map(key).collect(), None, None)
            }
            Op::File { file, .. } => (
                file.iter().map(|file| key(&file.path)).collect(),
                None,
                None,
            ),
            Op::Pin { file, .. } => (vec![key(&file.path)], None, None),
            Op::Tree { from, .. } => (Vec::new(), Some(key(from)), None),
            Op::Dir { path, .. } => (Vec::new(), None, Some(key(path))),
            Op::Create { .. }
            | Op::Delete { .. }
            | Op::Write { .. }
            | Op::Add { .. }
            | Op::Remove { .. }
            | Op::Dismiss { .. }
            | Op::Unknown(_) => (Vec::new(), None, None),
        };
        Self {
            entity: op.entity(),
            files,
            tree,
            dir,
        }
    }

    /// Whether the op changes what is at the path whose key is `path`: a file effect
    /// from or to it, a binding of a file there, or a move of a tree holding it.
    fn names(&self, path: &str) -> bool {
        self.files.iter().any(|file| file == path)
            || self.tree.as_ref().is_some_and(|tree| within(path, tree))
    }

    /// Whether the op changes the tree whose key is `tree`: a file effect from or to
    /// a path in it, a binding of a file in it, a move of it, of a tree in it or of
    /// one holding it, or a directory made or removed in it.
    fn touches(&self, tree: &str) -> bool {
        let moved = self.tree.as_ref();
        self.files.iter().any(|file| within(file, tree))
            || moved.is_some_and(|from| within(from, tree) || within(tree, from))
            || self.dir.as_ref().is_some_and(|dir| within(dir, tree))
    }
}

/// The entity the binding gives a file a `path` op left.
struct Bound {
    entity: EntityId,
    /// The latest live write of its file register that says where its file is.
    write: Write<Option<FileFact>>,
    /// What `write` says.
    fact: FileFact,
    /// Whether the entity is in sync with the file the op left.
    same: bool,
    /// Whether `write` names the path the op left the file at.
    named: bool,
}

fn changed<V>(write: &Write<V>) -> Refusal {
    Refusal::ChangedSince {
        by: write.by,
        entry: write.entry,
    }
}

impl Reversing<'_> {
    /// Whether another writer made `write`, or this one made it in a final intent.
    fn foreign<V>(&self, write: &Write<V>) -> bool {
        write.by != self.writer || self.finals.contains(&write.entry)
    }

    /// Refuses unless every surviving existence write of `entity` is as `left`.
    fn existence(&self, entity: EntityId, left: Exists) -> Result<(), Refusal> {
        let survivors = self.folded.existence(entity);
        match survivors.iter().rev().find(|write| write.value != left) {
            Some(write) => Err(changed(write)),
            None => Ok(()),
        }
    }

    /// Refuses when a [`Reversing::foreign`] field write on `entity` is live.
    fn only_own_writes(&self, entity: EntityId) -> Result<(), Refusal> {
        let live = self.folded.live_writes(entity);
        match live.iter().rev().find(|(_, write)| self.foreign(write)) {
            Some((_, write)) => Err(changed(write)),
            None => Ok(()),
        }
    }

    /// Refuses unless the merged state shows `entity`, with or without a live
    /// existence write, and no live delete of it saw the intent.
    fn shown(&self, entity: EntityId) -> Result<(), Refusal> {
        self.undeleted(entity)?;
        match self.folded.present(entity) {
            true => Ok(()),
            false => Err(Refusal::Invalid(Invalid::NoEntity(entity))),
        }
    }

    /// Refuses where a live delete of `entity` saw the intent, naming the latest live
    /// delete: one that saw it ended the intent's writes on the entity, while one
    /// concurrent with it, or from before it, left them live, or left live this
    /// writer's later writes that replaced them.
    fn undeleted(&self, entity: EntityId) -> Result<(), Refusal> {
        let existence = self.folded.existence(entity);
        let Some(delete) = existence.iter().rev().find(|w| w.value == Exists::Deleted) else {
            return Ok(());
        };
        let live = self.folded.live_writes(entity);
        let since = |write: &Write<()>| {
            write.entry == self.intent || write.by == self.writer && write.at >= self.at
        };
        match live.iter().any(|(_, write)| since(write)) {
            true => Ok(()),
            false => Err(changed(delete)),
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
        if let Some(tag) = tags.iter().rev().find(|tag| self.foreign(tag)) {
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
        if let Some(tag) = tags.iter().rev().find(|tag| self.foreign(tag)) {
            return Err(changed(tag));
        }
        Ok(FactChange::Add {
            entity,
            key: key.to_owned(),
            value: value.clone(),
        })
    }

    /// Whether `write` is a binding's pin, in a `bind` entry or one a snapshot folded:
    /// where a file is, which no user changed.
    fn pinned<V>(&self, write: &Write<V>) -> bool {
        let Some(placed) = self.writers.get(&write.by) else {
            return false;
        };
        match placed.merged.iter().rev().find(|e| e.hash() == write.entry) {
            Some(entry) => matches!(entry.kind(), EntryKind::Bind(_)),
            None => placed
                .tips
                .iter()
                .any(|tip| tip.binds.contains(&write.entry)),
        }
    }

    /// Refuses where an op of another writer's entry since the intent is `changes`,
    /// naming the latest such entry.
    fn unchanged(&self, changes: impl Fn(&Keyed) -> bool) -> Result<(), Refusal> {
        let later = self.later.get_or_init(|| self.later_entries());
        let naming = later.iter().filter(|later| later.ops.iter().any(&changes));
        match naming.max_by_key(|later| later.at) {
            Some(later) => Err(Refusal::ChangedSince {
                by: later.by,
                entry: later.entry,
            }),
            None => Ok(()),
        }
    }

    fn later_entries(&self) -> Vec<Later> {
        let others = self.writers.iter().filter(|(by, _)| **by != self.writer);
        let since = others.flat_map(|(by, placed)| {
            let since = placed.merged.iter().rev();
            since
                .take_while(|entry| entry.at() > self.at)
                .map(move |entry| (*by, entry))
        });
        let since = since.filter(|(_, entry)| !matches!(entry.kind(), EntryKind::Bind(_)));
        let later = since.map(|(by, entry)| {
            let ops = contribution::ops(entry.kind()).iter();
            Later {
                by,
                at: entry.at(),
                entry: entry.hash(),
                ops: ops.map(|op| Keyed::of(op, self.names)).collect(),
            }
        });
        later.collect()
    }

    /// Refuses where an op of another writer's entry since the intent changes the tree
    /// at `tree`.
    fn unchanged_under(&self, tree: &RelPath) -> Result<(), Refusal> {
        let tree = self.names.key(tree.as_str());
        self.unchanged(|op| op.touches(&tree))
    }

    /// Refuses where the file at `path` is an entity's by another writer's file write,
    /// other than a binding's pin, that the intent did not see, made concurrently
    /// with it.
    fn unclaimed(&self, path: &RelPath) -> Result<(), Refusal> {
        let Some(entity) = self.view.at(path) else {
            return Ok(());
        };
        let unseen = |write: &Write<Option<FileFact>>| {
            let Some(placed) = self.writers.get(&write.by) else {
                return false;
            };
            let seen = self.before.seen.get(&write.by).copied();
            let unseen = placed.merged.iter().rev();
            let mut unseen = unseen.take_while(|entry| seen.is_none_or(|seen| entry.n() > seen));
            unseen.any(|entry| entry.hash() == write.entry)
        };
        let writes = self.folded.file_writes(entity.id());
        let claim = writes.iter().find(|write| {
            let at = write.value.as_ref().is_some_and(|file| file.path == *path);
            write.by != self.writer && at && !self.pinned(write) && unseen(write)
        });
        match claim {
            Some(write) => Err(changed(write)),
            None => Ok(()),
        }
    }

    /// The file effect that puts back what an effect on a file no entity is bound
    /// to changed: the displaced bytes from the trash, the old name, or the trash
    /// for a file the effect added.
    fn path(
        &self,
        from: Option<&RelPath>,
        file: Option<&FileFact>,
    ) -> Result<Option<FileChange>, Refusal> {
        if let Some(left) = file.map(|file| &file.path).or(from) {
            let key = self.names.key(left.as_str());
            self.unchanged(|op| op.names(&key))?;
            self.unclaimed(left)?;
        }
        // A file since an entity's is renamed back or trashed as that entity's file, and
        // has bytes brought back over it as its file where its file fact names it. Else
        // the binding found the entity in it by its identity alone: the bytes come back as
        // no entity's and the entity is missing again.
        let bound = file.and_then(|file| self.bound(file));
        let renames = matches!((from, file), (Some(from), Some(file)) if *from != file.path);
        if let Some(bound) = &bound {
            let claim = Some(&bound.write).filter(|write| !self.pinned(write));
            let taken =
                |write: &&Write<Option<FileFact>>| !bound.same || !renames && self.foreign(write);
            if let Some(write) = claim.filter(taken) {
                return Err(changed(write));
            }
        }
        let entity = bound.as_ref().map(|bound| bound.entity);
        let named = bound.as_ref().filter(|bound| bound.named);
        let named = named.map(|bound| bound.entity);
        let found = entity.filter(|_| named.is_none());
        let restore = |path: &RelPath, expect| {
            let item = self.displaced.iter().find(|item| item.from == *path)?.item;
            let to = path.clone();
            Some(match named {
                Some(entity) => FileChange::Restore {
                    entity,
                    item,
                    to,
                    expect,
                },
                None => FileChange::RestoreFile {
                    item,
                    to,
                    expect,
                    found,
                },
            })
        };
        Ok(match (from, file) {
            (Some(from), Some(file)) if *from == file.path => {
                restore(from, Expect::Holds(file.identity))
            }
            (Some(from), Some(file)) => {
                let (to, expect) = (from.clone(), Expect::Holds(file.identity));
                Some(match entity {
                    Some(entity) => FileChange::Rename { entity, to, expect },
                    None => FileChange::RenameFile {
                        from: file.path.clone(),
                        to,
                        expect,
                    },
                })
            }
            (None, Some(file)) => {
                let expect = Expect::Holds(file.identity);
                Some(match bound {
                    Some(bound) => FileChange::TrashLeaving {
                        entity: bound.entity,
                        expect,
                        leaves: bound.fact,
                    },
                    None => FileChange::TrashFile {
                        path: file.path.clone(),
                        expect,
                    },
                })
            }
            (Some(from), None) => restore(from, Expect::Absent),
            (None, None) => None,
        })
    }

    /// The entity bound now to the file at the path `file` says an effect left where no
    /// entity's file was.
    fn bound(&self, file: &FileFact) -> Option<Bound> {
        let entity = self.view.at(&file.path)?;
        let bound = entity.file()?;
        let mut writes = self.folded.file_writes(entity.id()).into_iter();
        let write = writes.rfind(|write| write.value.is_some())?;
        let fact = write.value.clone()?;
        let key = |path: &RelPath| self.names.key(path.as_str());
        Some(Bound {
            entity: entity.id(),
            same: bound.state == FileState::InSync && fact.identity == file.identity,
            named: key(&fact.path) == key(&file.path),
            write,
            fact,
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
        // Where moves copy, putting a file back gives it a new modification time: only
        // its path and identity say whether it is still the file this intent wrote.
        let held = |fact: Option<&FileFact>| fact.map(|fact| (fact.path.clone(), fact.identity));
        let wrote = held(file);
        let survivors = self.folded.file_writes(entity);
        if let Some(write) = survivors
            .iter()
            .rev()
            .find(|write| held(write.value.as_ref()) != wrote && !self.pinned(write))
        {
            return Err(changed(write));
        }
        let old = self.before.files.get(&entity).cloned().flatten();
        for path in file.into_iter().chain(&old).map(|fact| &fact.path) {
            let path = self.names.key(path.as_str());
            self.unchanged(|op| op.entity != Some(entity) && op.names(&path))?;
        }
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
                // An undo that trashed an added file put it in the trash from where it
                // was, which its redo puts it back at.
                let missing = self.view.parts().bindings.bound.get(&entity);
                let missing = missing.is_some_and(|file| file.state == FileState::Missing);
                let trashed = old.as_ref().filter(|_| missing);
                let trashed = trashed.and_then(|old| restore(&old.path, Expect::Absent));
                let back = match old {
                    Some(leaves) if self.before.missing.contains(&entity) => {
                        Some(FileChange::TrashLeaving {
                            entity,
                            expect: Expect::Holds(new.identity),
                            leaves,
                        })
                    }
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
                // A file register the intent left saying a file nowhere, as an undo
                // that trashed an added file leaves it, holds nothing there now.
                let expect = match missing {
                    true => Expect::Absent,
                    false => Expect::Holds(new.identity),
                };
                restore(&new.path, expect).or(trashed).or(back)
            }
            (None, Some(old)) => restore(&old.path, Expect::Absent),
            (None, None) => None,
        })
    }
}

/// Whether the path whose key is `path` is the one whose key is `tree` or under it.
fn within(path: &str, tree: &str) -> bool {
    tree.is_empty()
        || path
            .strip_prefix(tree)
            .is_some_and(|rest| rest.is_empty() || rest.starts_with('/'))
}
