//! Recovery of interrupted effects, after a crash and after loss of the local root.
//!
//! Opening assesses and writes nothing in the folder. This writer's own records
//! are settled before its next write: the steps resume where they stopped, the
//! intent's entry is appended with what they did, and the record is removed. A
//! record of a writer this install does not write as is reported as possibly
//! still in progress elsewhere, and settled only with the user's consent, by this
//! writer and in its own log; only its owner ever removes it. Settling twice
//! equals settling once.
//!
//! A record is open while its writer's log ends at the head the record was
//! written after: the entry that closes it is the next one the writer appends.

use std::collections::{BTreeMap, BTreeSet};
use std::rc::Rc;

use crate::effects::{self, Applied, EffectPlan, EffectStep, FileEnd, Progress, Staged};
use crate::env::{Env, Identify};
use crate::error::Result;
use crate::flow::{self, each, fold, ok};
use crate::ids::{EntryHash, Nonce, WriterId};
use crate::io::{Kind, Root, Task};
use crate::layout::Layout;
use crate::log::Settlement;
use crate::path::RelPath;
use crate::pending::{self, PendingRecord, Records};
use crate::reader::WriterLog;
use crate::report::{Orphan, Outcome};

/// What recovery needs of a writer's placed history.
pub trait Chain {
    /// Whether the entry `hash` is placed or folded.
    fn holds(&self, hash: EntryHash) -> bool;

    /// Whether an entry after `hash` is placed or folded.
    fn continues(&self, hash: EntryHash) -> bool;
}

impl Chain for WriterLog {
    fn holds(&self, hash: EntryHash) -> bool {
        WriterLog::holds(self, hash)
    }

    fn continues(&self, hash: EntryHash) -> bool {
        WriterLog::holds(self, hash) && !self.heads().contains(&hash)
    }
}

/// What opening found to recover.
#[derive(Clone, PartialEq, Eq, Debug, Default)]
pub struct Recovery {
    /// This writer's records, with how each will be settled.
    pub own: Vec<Settling>,
    pub orphaned: Vec<Orphan>,
    /// Records ignored as unreadable, unchained or unconfined.
    pub ignored: Vec<RelPath>,
}

#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Settling {
    pub record: Nonce,
    pub pending: PendingRecord,
    /// The log already holds the entry closing the record, so settling only
    /// removes it.
    pub logged: bool,
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
    own: Option<(WriterId, EntryHash)>,
    logs: &'a BTreeMap<WriterId, C>,
    settled: &'a BTreeSet<(WriterId, Nonce)>,
) -> Task<'a, Result<Recovery>> {
    flow::run(read(layout, logs.keys().copied().collect()))
        .and_then(move |found| {
            let (recovery, open) = classify(layout, own, logs, settled, found);
            flow::run(predict(layout, recovery, open))
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
    own: Option<(WriterId, EntryHash)>,
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
                Standing::Logged => recovery.own.push(Settling {
                    record: name,
                    pending: record,
                    logged: true,
                    outcome: Outcome::Complete,
                }),
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
/// will end.
pub fn predict(
    layout: &Layout,
    recovery: Recovery,
    open: Vec<(Nonce, PendingRecord)>,
) -> Task<'static, Result<Recovery>> {
    let layout = layout.clone();
    fold(
        open.into_iter(),
        recovery,
        move |mut recovery, (name, record)| {
            let layout = layout.clone();
            effects::observe(&layout, &record).map_ok(move |seen| {
                let progress = effects::progress(&layout, &record, &seen);
                let outcome = effects::predict(&layout, name, &record, &seen, &progress);
                recovery.own.push(Settling {
                    record: name,
                    pending: record,
                    logged: false,
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
    Logged,
    /// This writer's, open after the head its local root recorded.
    Open,
    /// Another writer's, open.
    Orphan,
}

fn standing(
    layout: &Layout,
    own: Option<(WriterId, EntryHash)>,
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
            (true, _) => Standing::Logged,
            (false, true) => Standing::Open,
            (false, false) => Standing::Ignored,
        },
        _ if closed || settled.contains(&(record.writer, name)) => Standing::Closed,
        _ => Standing::Orphan,
    }
}

/// Resumes this writer's open record `name` where its steps stopped, after
/// removing a source the last done step left beside its destination. The caller
/// then appends the record's entry with what [`Applied`] says and removes the
/// record with [`effects::finish`].
pub fn settle(
    layout: &Layout,
    name: Nonce,
    record: Rc<PendingRecord>,
    identify: Rc<dyn Identify>,
) -> Task<'static, Result<Applied>> {
    let layout = layout.clone();
    effects::observe(&layout, &record)
        .and_then(move |seen| {
            let Progress { done, leftover } = effects::progress(&layout, &record, &seen);
            let removed = match leftover {
                Some(leftover) => flow::remove(Root::Folder, &leftover),
                None => ok(()),
            };
            removed.and_then(move |()| {
                flow::run(effects::apply(&layout, name, record, done, identify))
            })
        })
        .task()
}

/// Removes this writer's staged files that no record in `open` will place: what
/// a run cut short before its record was written left behind.
pub fn tidy(
    layout: &Layout,
    writer: WriterId,
    open: &[PendingRecord],
) -> Task<'static, Result<()>> {
    let keep: BTreeSet<String> = open
        .iter()
        .flat_map(|record| &record.steps)
        .filter_map(|step| match step {
            EffectStep::Place { staged, .. } => Some(staged.to_string()),
            _ => None,
        })
        .collect();
    let dir = layout.tmp_dir(writer);
    flow::list(Root::Folder, &dir)
        .and_then(move |entries| {
            let stale = entries
                .into_iter()
                .filter(move |entry| entry.kind == Kind::File && !keep.contains(&entry.name));
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

/// How far another writer's record `theirs` got, for [`orphan_plan`].
pub fn progress(layout: &Layout, theirs: &PendingRecord) -> Task<'static, Result<Progress>> {
    let layout = layout.clone();
    let theirs = theirs.clone();
    effects::observe(&layout, &theirs)
        .map_ok(move |seen| effects::progress(&layout, &theirs, &seen))
        .task()
}

/// The effects that settle another writer's record `theirs` as `how`, from
/// `progress`, carried out by this writer like an intent's. Nothing is written in
/// the other writer's directory: its staged and trashed bytes are copied, and
/// library files the settlement displaces go to this writer's trash. Dismissing
/// changes no file.
///
/// The caller commits the plan, then appends a [`crate::log::Settle`] entry.
pub fn orphan_plan(
    layout: &Layout,
    theirs: &PendingRecord,
    progress: &Progress,
    how: Settlement,
    env: &mut Env,
) -> EffectPlan {
    Orphaned {
        layout,
        theirs,
        plan: EffectPlan::new(env.nonce()),
        env,
    }
    .plan(progress, how)
}

struct Orphaned<'a> {
    layout: &'a Layout,
    theirs: &'a PendingRecord,
    plan: EffectPlan,
    env: &'a mut Env,
}

impl Orphaned<'_> {
    fn plan(mut self, progress: &Progress, how: Settlement) -> EffectPlan {
        if how == Settlement::Dismissed {
            return self.plan;
        }
        if let Some(leftover) = &progress.leftover {
            if self.layout.check_library_path(leftover).is_ok() {
                self.trash(leftover);
            }
        }
        let steps = &self.theirs.steps;
        match how {
            Settlement::Finished => {
                for step in &steps[progress.done..] {
                    self.finish(step);
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
                    self.undo(step);
                }
            }
            Settlement::Dismissed => {}
        }
        self.plan
    }

    fn finish(&mut self, step: &EffectStep) {
        let writer = self.theirs.writer;
        match step {
            EffectStep::ToTrash { path, .. } => self.trash(path),
            EffectStep::Place { staged, path } => {
                self.copy(self.layout.staged(writer, *staged), path)
            }
            EffectStep::FromTrash { item, path } => {
                self.copy(self.layout.trash(writer, *item), path)
            }
            EffectStep::Rename { .. }
            | EffectStep::MakeDir { .. }
            | EffectStep::RemoveDir { .. } => {
                self.plan.steps.push(step.clone());
            }
        }
    }

    fn undo(&mut self, step: &EffectStep) {
        let writer = self.theirs.writer;
        match step {
            EffectStep::ToTrash { path, item } => self.copy(self.layout.trash(writer, *item), path),
            EffectStep::Place { path, .. } | EffectStep::FromTrash { path, .. } => self.trash(path),
            EffectStep::Rename { from, to } => self.plan.steps.push(EffectStep::Rename {
                from: to.clone(),
                to: from.clone(),
            }),
            EffectStep::MakeDir { .. } => {}
            EffectStep::RemoveDir { path } => self
                .plan
                .steps
                .push(EffectStep::MakeDir { path: path.clone() }),
        }
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
        self.plan.staged.push((staged, Staged::Copy(source)));
        self.plan.steps.push(EffectStep::Place {
            staged,
            path: path.clone(),
        });
    }
}
