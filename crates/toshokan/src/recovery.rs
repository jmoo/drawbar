//! Recovery of interrupted effects, after a crash and after loss of the local root.
//!
//! Opening assesses and writes nothing in the folder. This writer's own records
//! are settled before its next write: the steps resume where they stopped, and
//! the intent's entry is appended with what they did. The record is then spent,
//! as is one whose entry the log already holds, and is removed only when the
//! library tidies or closes. A record of a writer this install does not write as
//! is reported as possibly still in progress elsewhere, and settled only with the
//! user's consent, by this writer and in its own log; only its owner ever removes
//! it. Settling twice equals settling once.
//!
//! A record is open while its writer's log ends at the head the record was
//! written after: the entry that closes it is the next one the writer appends.

use std::collections::{BTreeMap, BTreeSet};
use std::rc::Rc;

use crate::derive::Placed;
use crate::effects::{
    self, Applied, Cut, EffectPlan, EffectStep, FileEnd, Moves, Progress, Staged,
};
use crate::env::{Env, Identify};
use crate::error::{Refusal, Result};
use crate::flow::{self, each, fold, Fallible};
use crate::ids::{EntityId, Nonce, WriterId};
use crate::io::{Capabilities, Capability, Kind, Root, Task};
use crate::layout::Layout;
use crate::log::{EntryKind, Logged, Op, Own, Settlement};
use crate::path::RelPath;
use crate::pending::{self, PendingRecord, Records, MAX_RECORD};
use crate::report::{Orphan, Outcome};

/// What recovery needs of a writer's placed history.
pub trait Chain {
    /// Whether the entry `at` is placed or folded.
    fn holds(&self, at: Own) -> bool;

    /// Whether an entry after `at` is placed or folded.
    fn continues(&self, at: Own) -> bool;

    /// Whether the entry after `at` is an intent whose continuation the chain
    /// holds only part of, as an append a crash tore leaves it.
    fn unfinished(&self, at: Own) -> bool;
}

/// A tip folds every entry its head descends from, which it certifies by
/// number: an entry numbered below a tip's head is taken as folded even where
/// no certificate names its hash, unless it is a run, on another branch.
impl Chain for Placed {
    fn holds(&self, at: Own) -> bool {
        Placed::holds(self, at.hash) || self.tips.iter().any(|tip| tip.head.n > at.n)
    }

    fn continues(&self, at: Own) -> bool {
        let run = self.runs.iter().any(|entry| entry.hash() == at.hash);
        let passed = !run && self.tips.iter().any(|tip| tip.head.n > at.n);
        let entries = self.merged.iter().chain(&self.runs).chain(&self.gaps);
        let followed = entries.into_iter().any(|entry| entry.prev() == at.hash);
        Chain::holds(self, at) && (passed || followed)
    }

    fn unfinished(&self, at: Own) -> bool {
        matches!(after(self, at), After::Intent(c) if c.held < c.logged.more)
    }
}

/// Where the chain stands after an entry a record names.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum After {
    /// Nothing follows it.
    Nothing,
    /// The intent merged right after it, and what of its continuation the
    /// entries merged after the intent hold.
    Intent(Continued),
    /// A tip folds what follows it.
    Folded,
}

/// What an intent's continuation holds so far.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Continued {
    pub logged: Logged,
    /// How many ops and displaced items its `more` entries hold.
    pub held: u64,
    /// The entities a file op of the intent or its continuation names.
    pub files: BTreeSet<EntityId>,
    /// The trash items the intent or its continuation names.
    pub displaced: BTreeSet<Nonce>,
}

/// Where the chain `placed` stands after `at`.
pub fn after(placed: &Placed, at: Own) -> After {
    let Some(start) = placed
        .merged
        .iter()
        .position(|entry| entry.prev() == at.hash && Some(entry.n()) == at.n.checked_add(1))
    else {
        return match Chain::continues(placed, at) {
            true => After::Folded,
            false => After::Nothing,
        };
    };
    let EntryKind::Intent(logged) = placed.merged[start].kind() else {
        return After::Folded;
    };
    let named = |ops: &[Op]| ops.iter().filter_map(op_file).collect::<Vec<_>>();
    let mut continued = Continued {
        files: named(&logged.ops).into_iter().collect(),
        displaced: logged.displaced.iter().map(|item| item.item).collect(),
        held: 0,
        logged,
    };
    for entry in &placed.merged[start + 1..] {
        let EntryKind::More(more) = entry.kind() else {
            break;
        };
        continued.held += more.len();
        continued.files.extend(named(&more.ops));
        continued
            .displaced
            .extend(more.displaced.iter().map(|item| item.item));
    }
    After::Intent(continued)
}

/// The entity whose file `op` writes.
fn op_file(op: &Op) -> Option<EntityId> {
    match op {
        Op::File { entity, .. } | Op::Pin { entity, .. } => Some(*entity),
        _ => None,
    }
}

/// What opening found to recover.
#[derive(Clone, PartialEq, Eq, Debug, Default)]
pub struct Recovery {
    /// This writer's open records, with how each will be settled.
    pub own: Vec<Settling>,
    /// This writer's records closed by an entry in its log, left to remove.
    pub spent: Vec<(Nonce, PendingRecord)>,
    pub orphaned: Vec<Orphan>,
    /// Records ignored as unreadable, unchained or unconfined.
    pub ignored: Vec<RelPath>,
}

#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Settling {
    pub record: Nonce,
    pub pending: PendingRecord,
    /// How settling will end if nothing else changes the folder first.
    pub outcome: Outcome,
}

/// Reads every writer's pending records and sorts them. `own` is this writer and
/// the head its local root recorded; `settled` the records some writer settled.
/// Every other writer's open records are orphans: after the local root is lost,
/// an install cannot tell its former writer's records from a live writer's.
/// Requests only [`crate::Io::List`], [`crate::Io::Stat`] and [`crate::Io::Read`].
///
/// [`read`], [`classify`] and [`predict`] in turn, for a caller that cannot lend
/// `logs` while the reads run.
pub fn assess<'a, C: Chain>(
    layout: &'a Layout,
    own: Option<(WriterId, Own)>,
    logs: &'a BTreeMap<WriterId, C>,
    settled: &'a BTreeSet<(WriterId, Nonce)>,
    capabilities: Capabilities,
) -> Task<'a, Result<Recovery>> {
    flow::run(read(layout, logs.keys().copied().collect()))
        .and_then(move |found| {
            let (recovery, open) = classify(layout, own, logs, settled, found);
            flow::run(predict(layout, recovery, open, capabilities))
        })
        .task()
}

/// Every pending record of each of `writers`.
pub fn read(
    layout: &Layout,
    writers: Vec<WriterId>,
) -> Task<'static, Result<Vec<(WriterId, Records)>>> {
    let layout = layout.clone();
    fold(writers.into_iter(), Vec::new(), move |mut all, writer| {
        flow::run(pending::read_all(&layout, writer)).map_ok(move |found| {
            all.push((writer, found));
            all
        })
    })
    .task()
}

/// Sorts what [`read`] found by where each record stands. Returns this writer's
/// open records apart, for [`predict`].
pub fn classify<C: Chain>(
    layout: &Layout,
    own: Option<(WriterId, Own)>,
    logs: &BTreeMap<WriterId, C>,
    settled: &BTreeSet<(WriterId, Nonce)>,
    found: Vec<(WriterId, Records)>,
) -> (Recovery, Vec<(Nonce, PendingRecord)>) {
    let mut recovery = Recovery::default();
    let mut open = Vec::new();
    for (writer, found) in found {
        recovery.ignored.extend(found.unreadable);
        let Some(chain) = logs.get(&writer) else {
            continue;
        };
        for (name, record) in found.records {
            match standing(layout, own, chain, settled, name, &record) {
                Standing::Ignored => recovery.ignored.push(layout.pending(writer, name)),
                Standing::Closed => {}
                Standing::Spent if chain.unfinished(record.after()) => open.push((name, record)),
                Standing::Spent => recovery.spent.push((name, record)),
                Standing::Open => open.push((name, record)),
                Standing::Orphan => recovery.orphaned.push(Orphan {
                    writer,
                    record: name,
                    label: record.label.clone(),
                    paths: record.paths(),
                }),
            }
        }
    }
    (recovery, open)
}

/// Adds this writer's `open` records to `recovery`, each with how settling it
/// will end, in a folder that can do what `capabilities` says.
pub fn predict(
    layout: &Layout,
    recovery: Recovery,
    open: Vec<(Nonce, PendingRecord)>,
    capabilities: Capabilities,
) -> Task<'static, Result<Recovery>> {
    let layout = layout.clone();
    fold(
        open.into_iter(),
        recovery,
        move |mut recovery, (name, record)| {
            let layout = layout.clone();
            effects::observe(&layout, name, &record).map_ok(move |seen| {
                let progress = effects::progress(&layout, &record, &seen, capabilities.fsync);
                let outcome = effects::predict(&layout, name, &record, &seen, &progress);
                recovery.own.push(Settling {
                    record: name,
                    pending: record,
                    outcome,
                });
                recovery
            })
        },
    )
    .task()
}

/// Where a record stands for a reader writing as `own`.
enum Standing {
    /// Unconfined, or not chained to its writer's log.
    Ignored,
    /// Closed, waiting for its writer to remove it.
    Closed,
    /// This writer's, closed by an entry in its log.
    Spent,
    /// This writer's, open after the head its local root recorded.
    Open,
    /// Another writer's, open.
    Orphan,
}

fn standing(
    layout: &Layout,
    own: Option<(WriterId, Own)>,
    chain: &impl Chain,
    settled: &BTreeSet<(WriterId, Nonce)>,
    name: Nonce,
    record: &PendingRecord,
) -> Standing {
    let after = record.after();
    if !record.is_confined(layout) || !chain.holds(after) {
        return Standing::Ignored;
    }
    let closed = chain.continues(after);
    match own {
        Some((own, head)) if own == record.writer => match (closed, head == after) {
            (true, _) => Standing::Spent,
            (false, true) => Standing::Open,
            (false, false) => Standing::Ignored,
        },
        _ if closed || settled.contains(&(record.writer, name)) => Standing::Closed,
        _ => Standing::Orphan,
    }
}

/// Resumes this writer's open record `name` where its steps stopped, in a folder
/// that can do what `capabilities` says, after removing a source the last done
/// step left beside its destination and the start of a copy the next step left,
/// and setting aside in the trash any other copy cut short there. The caller then
/// appends the record's entry with what [`Applied`] says, after which
/// [`effects::finish`] may remove the record.
pub fn settle(
    layout: &Layout,
    name: Nonce,
    record: Rc<PendingRecord>,
    identify: Rc<dyn Identify>,
    capabilities: Capabilities,
) -> Task<'static, Result<Applied>> {
    let layout = layout.clone();
    effects::observe(&layout, name, &record)
        .and_then(move |seen| {
            let Progress {
                done,
                leftover,
                partial,
            } = effects::progress(&layout, &record, &seen, capabilities.fsync);
            let (start, aside): (Vec<Cut>, Vec<Cut>) =
                partial.into_iter().partition(|cut| cut.start);
            let stale = leftover
                .into_iter()
                .chain(start.into_iter().map(|cut| cut.path));
            let set_aside = {
                let (layout, record) = (layout.clone(), Rc::clone(&record));
                move |()| {
                    each(aside.into_iter(), move |cut| {
                        let at = Aside {
                            record: name,
                            step: done,
                            writer: record.writer,
                        };
                        set_aside(layout.clone(), at, cut.path, record.moves, 0)
                    })
                }
            };
            each(stale, |path| flow::remove(Root::Folder, &path))
                .and_then(set_aside)
                .and_then(move |()| {
                    flow::run(effects::apply(&layout, name, record, done, identify))
                })
        })
        .task()
}

/// The step of a record whose cut-short copy is set aside.
#[derive(Clone, Copy)]
struct Aside {
    writer: WriterId,
    record: Nonce,
    step: usize,
}

impl Aside {
    /// The name of the `n`th file set aside from this step in the trash: derived,
    /// so that settling again finds the names used before, and named by no entry,
    /// so that emptying the trash never removes it.
    fn item(self, n: u32) -> Nonce {
        let named = format!("aside {} {} {} {n}", self.writer, self.record, self.step);
        let hash = blake3::hash(named.as_bytes());
        let mut first = [0; 16];
        first.copy_from_slice(&hash.as_bytes()[..16]);
        Nonce::from_u128(u128::from_be_bytes(first))
    }
}

/// Moves `path` to the first trash item of `at` from the `n`th on that is free,
/// so that nothing set aside before is replaced.
fn set_aside<'a>(
    layout: Layout,
    at: Aside,
    path: RelPath,
    moves: Moves,
    n: u32,
) -> Fallible<'a, ()> {
    let item = layout.trash(at.writer, at.item(n));
    flow::stat(Root::Folder, &item).and_then(move |found| match found {
        Some(_) => set_aside(layout, at, path, moves, n + 1),
        None => effects::relocate(moves, &path, &item),
    })
}

/// Removes every file in this writer's staging, once no record of its is open
/// to place one: what a run cut short before its record was written left behind.
pub fn tidy(layout: &Layout, writer: WriterId) -> Task<'static, Result<()>> {
    let dir = layout.tmp_dir(writer);
    flow::list(Root::Folder, &dir)
        .and_then(move |entries| {
            let stale = entries.into_iter().filter(|entry| entry.kind == Kind::File);
            each(stale, move |entry| {
                flow::remove(
                    Root::Folder,
                    &dir.join(&entry.name)
                        .expect("a listed name is one component"),
                )
            })
        })
        .task()
}

/// Removes those of `ignored` that are records of this writer cut short before
/// their bytes landed, and returns them. A record whose moves copy is created in
/// place and synced before any of its steps runs, so a crash before then leaves
/// it empty or, in a folder that syncs, whose `capabilities` declare `fsync`,
/// started or zeroed ([`pending::cut_short`]); no version writes such a record.
/// Any other record that does not decode stays, and so does one longer than
/// [`MAX_RECORD`].
pub fn remove_empty(
    layout: &Layout,
    writer: WriterId,
    ignored: &[RelPath],
    capabilities: Capabilities,
) -> Task<'static, Result<Vec<RelPath>>> {
    let dir = layout.pending_dir(writer);
    let records = ignored.iter().filter(|path| {
        let name = path.name().and_then(|name| name.strip_suffix(".json"));
        path.parent().as_ref() == Some(&dir) && name.is_some_and(|n| n.parse::<Nonce>().is_ok())
    });
    let records: Vec<RelPath> = records.cloned().collect();
    let cut_short = move |bytes: &[u8]| match capabilities.fsync {
        true => pending::cut_short(bytes),
        false => bytes.is_empty(),
    };
    fold(records.into_iter(), Vec::new(), move |mut removed, path| {
        flow::stat(Root::Folder, &path)
            .and_then({
                let path = path.clone();
                move |meta| match meta {
                    Some(meta) if meta.kind == Kind::File && meta.len <= MAX_RECORD => {
                        flow::read_file(Root::Folder, &path, MAX_RECORD)
                    }
                    _ => flow::ok(None),
                }
            })
            .and_then(
                move |bytes| match bytes.is_some_and(|bytes| cut_short(&bytes)) {
                    true => flow::remove(Root::Folder, &path).map_ok(move |()| {
                        removed.push(path);
                        removed
                    }),
                    false => flow::ok(removed),
                },
            )
    })
    .task()
}

/// How far another writer's record got, as [`orphan_plan`] needs it.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Reached {
    progress: Progress,
    /// The library paths its `to_trash` steps move that hold nothing.
    vacant: BTreeSet<RelPath>,
    /// The library paths its steps name that are directories.
    directories: BTreeSet<RelPath>,
}

/// How far another writer's record `name`, `theirs`, got, for [`orphan_plan`].
pub fn progress(
    layout: &Layout,
    name: Nonce,
    theirs: &PendingRecord,
    capabilities: Capabilities,
) -> Task<'static, Result<Reached>> {
    let layout = layout.clone();
    let theirs = theirs.clone();
    effects::observe(&layout, name, &theirs)
        .map_ok(move |seen| {
            let vacant = theirs
                .steps
                .iter()
                .filter_map(|step| match step {
                    EffectStep::ToTrash { path, .. } if !seen.holds(path) => Some(path.clone()),
                    _ => None,
                })
                .collect();
            let directories = theirs
                .steps
                .iter()
                .flat_map(EffectStep::library_paths)
                .filter(|path| seen.is_directory(path))
                .cloned()
                .collect();
            Reached {
                progress: effects::reached(&layout, &theirs, &seen, capabilities.fsync),
                vacant,
                directories,
            }
        })
        .task()
}

/// The effects that settle another writer's record `theirs` as `how`, from how
/// far it got, carried out by this writer like an intent's, in a folder that can
/// do what `capabilities` says. Nothing is written in the other writer's
/// directory: its staged and trashed bytes are copied, and library files the
/// settlement displaces go to this writer's trash, the start of a copy their next
/// step left included. Finishing skips moving to the trash a path that holds
/// nothing. Dismissing changes no file. Finishing or rolling back a step that
/// renames a directory is refused where `capabilities` cannot rename one.
///
/// Planned again after a settlement stopped partway, it plans what remains. The
/// caller commits the plan, then, once its effects complete, appends a
/// [`crate::log::Settle`] entry.
pub fn orphan_plan(
    layout: &Layout,
    theirs: &PendingRecord,
    reached: &Reached,
    how: Settlement,
    capabilities: Capabilities,
    env: &mut Env,
) -> std::result::Result<EffectPlan, Refusal> {
    let moves = Moves::of(capabilities);
    Orphaned {
        layout,
        theirs,
        vacant: &reached.vacant,
        directories: &reached.directories,
        rename_dir: capabilities.rename_dir && moves.renames(),
        plan: EffectPlan {
            moves,
            ..EffectPlan::new(env.nonce())
        },
        env,
    }
    .plan(&reached.progress, how)
}

struct Orphaned<'a> {
    layout: &'a Layout,
    theirs: &'a PendingRecord,
    vacant: &'a BTreeSet<RelPath>,
    directories: &'a BTreeSet<RelPath>,
    rename_dir: bool,
    plan: EffectPlan,
    env: &'a mut Env,
}

impl Orphaned<'_> {
    fn plan(
        mut self,
        progress: &Progress,
        how: Settlement,
    ) -> std::result::Result<EffectPlan, Refusal> {
        if how == Settlement::Dismissed {
            return Ok(self.plan);
        }
        let partial = progress.partial.iter().map(|cut| &cut.path);
        for stale in progress.leftover.iter().chain(partial) {
            if self.layout.check_library_path(stale).is_ok() {
                self.trash(stale);
            }
        }
        let steps = &self.theirs.steps;
        match how {
            Settlement::Finished => {
                for step in &steps[progress.done..] {
                    self.finish(step)?;
                }
                let done_after = self.plan.steps.len();
                self.plan.files = self
                    .theirs
                    .files
                    .iter()
                    .map(|end| FileEnd {
                        done_after,
                        ..end.clone()
                    })
                    .collect();
            }
            Settlement::RolledBack => {
                for step in steps[..progress.done].iter().rev() {
                    self.undo(step)?;
                }
            }
            Settlement::Dismissed => {}
        }
        Ok(self.plan)
    }

    fn finish(&mut self, step: &EffectStep) -> std::result::Result<(), Refusal> {
        let writer = self.theirs.writer;
        match step {
            EffectStep::ToTrash { path, .. } if self.vacant.contains(path) => {}
            EffectStep::ToTrash { path, .. } => self.trash(path),
            EffectStep::Place { staged, path } => {
                self.copy(self.layout.staged(writer, *staged, path), path)
            }
            EffectStep::FromTrash { item, path } => {
                self.copy(self.layout.trash(writer, *item), path)
            }
            EffectStep::Rename { from, to } => return self.rename(from, to),
            EffectStep::MakeDir { .. } | EffectStep::RemoveDir { .. } => {
                self.plan.steps.push(step.clone());
            }
        }
        Ok(())
    }

    fn undo(&mut self, step: &EffectStep) -> std::result::Result<(), Refusal> {
        let writer = self.theirs.writer;
        match step {
            EffectStep::ToTrash { path, item } => self.copy(self.layout.trash(writer, *item), path),
            EffectStep::Place { path, .. } | EffectStep::FromTrash { path, .. } => self.trash(path),
            EffectStep::Rename { from, to } => return self.rename(to, from),
            EffectStep::MakeDir { .. } => {}
            EffectStep::RemoveDir { path } => self
                .plan
                .steps
                .push(EffectStep::MakeDir { path: path.clone() }),
        }
        Ok(())
    }

    fn rename(&mut self, from: &RelPath, to: &RelPath) -> std::result::Result<(), Refusal> {
        if self.directories.contains(from) && !self.rename_dir {
            return Err(Refusal::Unsupported(Capability::RenameDir));
        }
        self.plan.steps.push(EffectStep::Rename {
            from: from.clone(),
            to: to.clone(),
        });
        Ok(())
    }

    fn trash(&mut self, path: &RelPath) {
        let item = self.env.nonce();
        self.plan.steps.push(EffectStep::ToTrash {
            path: path.clone(),
            item,
        });
    }

    fn copy(&mut self, source: RelPath, path: &RelPath) {
        let staged = self.env.nonce();
        self.plan.place(staged, Staged::Copy(source), path.clone());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::blocking::run;
    use crate::disk::MemDisk;
    use crate::io::Io;

    /// Removes what [`remove_empty`] takes of records holding `made`, numbered
    /// from 1, in a folder that can do what `capabilities` says, and returns their
    /// numbers.
    fn removed(made: &[Vec<u8>], capabilities: Capabilities) -> Vec<u128> {
        let layout = Layout::new(".t").unwrap();
        let writer = WriterId::from_u128(7);
        let mut disk = MemDisk::new();
        let record = |n: usize| layout.pending(writer, Nonce::from_u128(n as u128 + 1));
        disk.perform(Io::MakeDir {
            root: Root::Folder,
            path: layout.pending_dir(writer),
        })
        .unwrap();
        for (n, bytes) in made.iter().enumerate() {
            let create = Io::Create {
                root: Root::Folder,
                path: record(n),
                bytes: bytes.clone(),
            };
            disk.perform(create).unwrap();
        }
        let ignored: Vec<RelPath> = (0..made.len()).map(record).collect();
        let removed = run(
            &mut disk,
            remove_empty(&layout, writer, &ignored, capabilities),
        );
        let names = removed.unwrap().into_iter().map(|path| {
            let name = path.name().unwrap().strip_suffix(".json").unwrap();
            name.parse::<Nonce>().unwrap().to_u128()
        });
        names.collect()
    }

    fn made() -> Vec<Vec<u8>> {
        let mut long = b"{\"writer\":\"".to_vec();
        long.resize(usize::try_from(MAX_RECORD).unwrap() + 1, b'x');
        vec![
            b"".to_vec(),
            vec![0; 40],
            b"{\"writer\":".to_vec(),
            b"{\"from\":\"a newer version\"}".to_vec(),
            long,
        ]
    }

    #[test]
    fn a_record_cut_short_is_removed_and_any_other_unreadable_one_stays() {
        assert_eq!(removed(&made(), Capabilities::ALL), [1, 2, 3]);
    }

    #[test]
    fn where_nothing_syncs_only_an_empty_record_is_cut_short() {
        let unsynced = Capabilities {
            fsync: false,
            ..Capabilities::ALL
        };
        assert_eq!(removed(&made(), unsynced), [1]);
    }
}
