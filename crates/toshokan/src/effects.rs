//! File effects without atomic replace.
//!
//! Nothing in a user path is overwritten or deleted in place. A save stages its
//! bytes in `tmp/` and syncs them; every precondition is checked; a pending record
//! is written; displaced bytes are renamed into this writer's trash and the staged
//! file into place; the intent's entry is appended; the pending record is removed.
//! Each user path holds its old bytes, nothing or its new bytes, and while it holds
//! nothing a pending record names it. A source is never removed before its
//! destination is durable.
//!
//! A commit runs [`resolve`], [`prepare`], [`apply`], the writer's append, then
//! [`finish`]. Recovery resumes [`apply`] where an interrupted run stopped.

use std::collections::{BTreeMap, BTreeSet};
use std::rc::Rc;

use serde::{Deserialize, Serialize};

use crate::binding::Bindings;
use crate::env::{Env, Identify};
use crate::error::{Error, Invalid, Mismatch, Refusal, Result};
use crate::flow::{self, each, fold, ok, Fallible, Flow};
use crate::ids::{EntityId, Identity, Nonce, WriterId};
use crate::io::{Capabilities, Io, IoError, Kind, Root, Task};
use crate::layout::Layout;
use crate::log::{Displaced, FileFact};
use crate::path::RelPath;
use crate::pending::PendingRecord;
use crate::plan::{Expect, FileChange, Target};
use crate::report::{Outcome, PartialReport};
use crate::view::FileState;

/// One step of an intent's file effects, as its pending record lists it. Paths
/// are library paths; items and staged files are this writer's.
#[derive(Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
#[serde(tag = "step", rename_all = "snake_case")]
pub enum EffectStep {
    /// Moves the file at `path` into this writer's trash as `item`.
    ToTrash {
        path: RelPath,
        item: Nonce,
    },
    /// Moves the staged file `staged` to `path`.
    Place {
        staged: Nonce,
        path: RelPath,
    },
    Rename {
        from: RelPath,
        to: RelPath,
    },
    /// Moves trash item `item` back to `path`.
    FromTrash {
        item: Nonce,
        path: RelPath,
    },
    MakeDir {
        path: RelPath,
    },
    /// Removes a directory a move emptied. A directory that is not empty, or not
    /// there, is left as it is.
    RemoveDir {
        path: RelPath,
    },
}

impl EffectStep {
    /// The library path the step changes; for a rename, its destination.
    pub fn path(&self) -> &RelPath {
        match self {
            Self::ToTrash { path, .. }
            | Self::Place { path, .. }
            | Self::Rename { to: path, .. }
            | Self::FromTrash { path, .. }
            | Self::MakeDir { path }
            | Self::RemoveDir { path } => path,
        }
    }

    /// Every library path the step names.
    pub fn library_paths(&self) -> Vec<&RelPath> {
        match self {
            Self::Rename { from, to } => vec![from, to],
            _ => vec![self.path()],
        }
    }

    /// Where the step moves a file from and to, in the folder; `None` for a
    /// directory step.
    pub(crate) fn ends(&self, layout: &Layout, writer: WriterId) -> Option<(RelPath, RelPath)> {
        match self {
            Self::ToTrash { path, item } => Some((path.clone(), layout.trash(writer, *item))),
            Self::Place { staged, path } => Some((layout.staged(writer, *staged), path.clone())),
            Self::Rename { from, to } => Some((from.clone(), to.clone())),
            Self::FromTrash { item, path } => Some((layout.trash(writer, *item), path.clone())),
            Self::MakeDir { .. } | Self::RemoveDir { .. } => None,
        }
    }
}

/// What an intent requires at a library path before anything is written.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Precondition {
    pub path: RelPath,
    pub expect: Expect,
}

/// The bytes of a staged file.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Staged {
    Bytes(Vec<u8>),
    /// A copy of the file at this path in the folder.
    Copy(RelPath),
}

/// Where an entity's file is once the first `done_after` steps are done: at
/// `path`, or nowhere.
#[derive(Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub struct FileEnd {
    pub entity: EntityId,
    pub path: Option<RelPath>,
    pub done_after: usize,
    /// The file is given to the entity as it is, and logged as a `pin` op, which
    /// undo leaves alone.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub pin: bool,
}

/// The file effects of one intent, resolved against the bindings.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct EffectPlan {
    /// The name of the pending record.
    pub record: Nonce,
    pub checks: Vec<Precondition>,
    /// Staged before anything else, by staged name.
    pub staged: Vec<(Nonce, Staged)>,
    pub steps: Vec<EffectStep>,
    pub files: Vec<FileEnd>,
}

impl EffectPlan {
    /// No effects yet, under the record name `record`.
    pub fn new(record: Nonce) -> Self {
        Self {
            record,
            checks: Vec::new(),
            staged: Vec::new(),
            steps: Vec::new(),
            files: Vec::new(),
        }
    }

    /// Whether the intent has no file effects at all.
    pub fn is_empty(&self) -> bool {
        self.steps.is_empty() && self.checks.is_empty() && self.files.is_empty()
    }

    /// Whether the effects change the folder, and so need a pending record.
    pub fn moves_files(&self) -> bool {
        !self.steps.is_empty()
    }
}

/// What a run of the steps did, as the folder shows it afterwards.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Applied {
    /// Bytes now in this writer's trash because of the intent: what its steps
    /// displaced, and staged bytes that could not be placed.
    pub displaced: Vec<Displaced>,
    /// What each entity's file register should say, for the effects that are done.
    pub files: Vec<(EntityId, Option<FileFact>)>,
    pub outcome: Outcome,
}

/// Resolves `changes` into checks and steps. `created` gives the ids of the plan's
/// new entities; `bindings` where existing entities' files are. Directory renames
/// are used only where `capabilities` declares them.
///
/// A tree moved by per-file renames moves the files `bindings` knows of; a
/// directory left behind holding anything else stays.
pub fn resolve(
    changes: &[FileChange],
    created: &[EntityId],
    bindings: &Bindings,
    layout: &Layout,
    capabilities: Capabilities,
    env: &mut Env,
) -> std::result::Result<EffectPlan, Refusal> {
    let mut resolver = Resolver {
        plan: EffectPlan::new(env.nonce()),
        touched: Vec::new(),
        claimed: BTreeSet::new(),
        layout,
        bindings,
    };
    for change in changes {
        resolver.change(change, created, capabilities, env)?;
    }
    Ok(resolver.plan)
}

struct Resolver<'r> {
    plan: EffectPlan,
    touched: Vec<RelPath>,
    claimed: BTreeSet<EntityId>,
    layout: &'r Layout,
    bindings: &'r Bindings,
}

fn invalid(invalid: Invalid) -> Refusal {
    Refusal::Invalid(invalid)
}

impl Resolver<'_> {
    fn change(
        &mut self,
        change: &FileChange,
        created: &[EntityId],
        capabilities: Capabilities,
        env: &mut Env,
    ) -> std::result::Result<(), Refusal> {
        match change {
            FileChange::Save {
                entity,
                path,
                bytes,
                expect,
            } => {
                let entity = match *entity {
                    Target::Existing(entity) => entity,
                    Target::New(n) => *created.get(n).expect("the plan creates what it names"),
                };
                let path = self.library(path)?;
                self.claim(entity, &path)?;
                self.check(&path, *expect);
                if let Expect::Holds(_) = expect {
                    self.step(EffectStep::ToTrash {
                        path: path.clone(),
                        item: env.nonce(),
                    });
                }
                let staged = env.nonce();
                self.plan
                    .staged
                    .push((staged, Staged::Bytes(bytes.clone())));
                self.step(EffectStep::Place {
                    staged,
                    path: path.clone(),
                });
                self.end(entity, Some(path));
            }
            FileChange::Trash { entity, expect } => {
                let path = self.current(*entity)?;
                self.claim(*entity, &path)?;
                self.check(&path, *expect);
                self.step(EffectStep::ToTrash {
                    path,
                    item: env.nonce(),
                });
                self.end(*entity, None);
            }
            FileChange::Rename { entity, to, expect } => {
                let from = self.current(*entity)?;
                let to = self.library(to)?;
                self.claim(*entity, &from)?;
                self.touch(&to)?;
                self.check(&from, *expect);
                self.check(&to, Expect::Absent);
                self.step(EffectStep::Rename {
                    from,
                    to: to.clone(),
                });
                self.end(*entity, Some(to));
            }
            FileChange::MoveTree { from, to } => {
                let (from, to) = (self.library(from)?, self.library(to)?);
                self.touch(&from)?;
                self.touch(&to)?;
                self.check(&to, Expect::Absent);
                self.move_tree(&from, &to, capabilities.rename_dir)?;
            }
            FileChange::Adopt {
                entity,
                path,
                expect,
            } => {
                let entity = match *entity {
                    Target::Existing(entity) => entity,
                    Target::New(n) => *created.get(n).expect("the plan creates what it names"),
                };
                let path = self.library(path)?;
                self.claim(entity, &path)?;
                self.check(&path, *expect);
                self.plan.files.push(FileEnd {
                    entity,
                    path: Some(path),
                    done_after: 0,
                    pin: true,
                });
            }
            FileChange::Restore {
                entity,
                item,
                to,
                expect,
            } => {
                let to = self.library(to)?;
                self.claim(*entity, &to)?;
                self.check(&to, *expect);
                if let Expect::Holds(_) = expect {
                    self.step(EffectStep::ToTrash {
                        path: to.clone(),
                        item: env.nonce(),
                    });
                }
                self.step(EffectStep::FromTrash {
                    item: *item,
                    path: to.clone(),
                });
                self.end(*entity, Some(to));
            }
        }
        Ok(())
    }

    fn move_tree(
        &mut self,
        from: &RelPath,
        to: &RelPath,
        rename_dir: bool,
    ) -> std::result::Result<(), Refusal> {
        let bound: BTreeMap<&RelPath, EntityId> = self
            .bindings
            .bound
            .iter()
            .filter(|(_, file)| file.state != FileState::Missing)
            .map(|(&entity, file)| (&file.path, entity))
            .collect();
        let files: BTreeSet<&RelPath> = bound
            .keys()
            .copied()
            .chain(&self.bindings.unbound)
            .filter(|path| path.starts_with(from))
            .collect();
        for &entity in bound
            .iter()
            .filter(|(path, _)| files.contains(*path))
            .map(|(_, e)| e)
        {
            if !self.claimed.insert(entity) {
                return Err(invalid(Invalid::Overlapping(from.clone())));
            }
        }
        let rebased = |path: &RelPath| path.rebase(from, to).expect("the file is under the tree");
        if rename_dir {
            self.step(EffectStep::Rename {
                from: from.clone(),
                to: to.clone(),
            });
        }
        for path in &files {
            if !rename_dir {
                self.step(EffectStep::Rename {
                    from: (*path).clone(),
                    to: rebased(path),
                });
            }
            if let Some(&entity) = bound.get(path) {
                self.end(entity, Some(rebased(path)));
            }
        }
        if !rename_dir {
            let mut dirs: Vec<RelPath> = files
                .iter()
                .flat_map(|path| std::iter::successors(path.parent(), RelPath::parent))
                .filter(|dir| dir.starts_with(from))
                .collect::<BTreeSet<_>>()
                .into_iter()
                .collect();
            dirs.sort_by_key(|dir| std::cmp::Reverse(dir.components().count()));
            if !files.contains(from) && !dirs.contains(from) {
                dirs.push(from.clone());
            }
            for path in dirs {
                self.step(EffectStep::RemoveDir { path });
            }
        }
        Ok(())
    }

    fn library(&self, path: &RelPath) -> std::result::Result<RelPath, Refusal> {
        match self.layout.check_library_path(path) {
            Ok(()) => Ok(path.clone()),
            Err(_) => Err(invalid(Invalid::ReservedPath(path.clone()))),
        }
    }

    /// Refuses a path one effect of the intent already touches, or contains.
    fn touch(&mut self, path: &RelPath) -> std::result::Result<(), Refusal> {
        if self
            .touched
            .iter()
            .any(|other| other.starts_with(path) || path.starts_with(other))
        {
            return Err(invalid(Invalid::Overlapping(path.clone())));
        }
        self.touched.push(path.clone());
        Ok(())
    }

    /// Takes the entity's file for one effect, and the path it touches.
    fn claim(&mut self, entity: EntityId, path: &RelPath) -> std::result::Result<(), Refusal> {
        self.touch(path)?;
        match self.claimed.insert(entity) {
            true => Ok(()),
            false => Err(invalid(Invalid::Overlapping(path.clone()))),
        }
    }

    fn current(&self, entity: EntityId) -> std::result::Result<RelPath, Refusal> {
        match self.bindings.bound.get(&entity) {
            Some(file) if file.state != FileState::Missing => Ok(file.path.clone()),
            _ => Err(invalid(Invalid::NoFile(entity))),
        }
    }

    fn check(&mut self, path: &RelPath, expect: Expect) {
        self.plan.checks.push(Precondition {
            path: path.clone(),
            expect,
        });
    }

    fn step(&mut self, step: EffectStep) {
        self.plan.steps.push(step);
    }

    fn end(&mut self, entity: EntityId, path: Option<RelPath>) {
        let done_after = self.plan.steps.len();
        self.plan.files.push(FileEnd {
            entity,
            path,
            done_after,
            pin: false,
        });
    }
}

/// Stages and syncs the plan's bytes, checks every precondition, then writes
/// `record` as the plan's pending record, whole or not at all. A failed
/// precondition removes what was staged and refuses; nothing in a user path has
/// changed. A plan that moves no file only checks. A staged copy is read whole
/// into memory.
pub fn prepare(
    layout: &Layout,
    plan: Rc<EffectPlan>,
    record: Rc<PendingRecord>,
    identify: Rc<dyn Identify>,
) -> Task<'static, Result<std::result::Result<(), Refusal>>> {
    if plan.is_empty() {
        return Task::ready(Ok(Ok(())));
    }
    let layout = layout.clone();
    let writer = record.writer;
    if !plan.moves_files() {
        return refusal(&layout, writer, &plan, &identify)
            .map_ok(|refused| refused.map_or(Ok(()), Err))
            .task();
    }
    stage(&layout, writer, Rc::clone(&plan))
        .and_then({
            let (layout, plan) = (layout.clone(), Rc::clone(&plan));
            move |()| refusal(&layout, writer, &plan, &identify)
        })
        .and_then(move |refused| match refused {
            Some(refusal) => {
                let staged: Vec<RelPath> = plan
                    .staged
                    .iter()
                    .map(|(name, _)| layout.staged(writer, *name))
                    .collect();
                each(staged.into_iter(), |path| flow::remove(Root::Folder, &path))
                    .map_ok(|()| Err(refusal))
            }
            None => write_record(&layout, writer, plan.record, &record).map_ok(|()| Ok(())),
        })
        .task()
}

/// The refusal [`prepare`] would give, found without writing anything.
pub fn check(
    layout: &Layout,
    writer: WriterId,
    plan: &EffectPlan,
    identify: &Rc<dyn Identify>,
) -> Task<'static, Result<Option<Refusal>>> {
    refusal(layout, writer, plan, identify).task()
}

/// Writes the record whole: staged and synced first, then renamed into place, so
/// a crash never leaves part of one.
fn write_record<'a>(
    layout: &Layout,
    writer: WriterId,
    name: Nonce,
    record: &PendingRecord,
) -> Fallible<'a, ()> {
    let staged = layout.staged(writer, name);
    let path = layout.pending(writer, name);
    flow::ensure_dir(Root::Folder, &layout.tmp_dir(writer))
        .and_then({
            let staged = staged.clone();
            let bytes = record.encode();
            move |()| flow::create(Root::Folder, &staged, bytes)
        })
        .and_then(move |()| flow::rename(Root::Folder, &staged, &path))
}

fn stage<'a>(layout: &Layout, writer: WriterId, plan: Rc<EffectPlan>) -> Fallible<'a, ()> {
    if plan.staged.is_empty() {
        return ok(());
    }
    let layout = layout.clone();
    let dir = layout.tmp_dir(writer);
    let synced = dir.clone();
    flow::ensure_dir(Root::Folder, &dir)
        .and_then(move |()| {
            each(0..plan.staged.len(), move |i| {
                let (name, staged) = &plan.staged[i];
                let path = layout.staged(writer, *name);
                let bytes = match staged {
                    Staged::Bytes(bytes) => ok(bytes.clone()),
                    Staged::Copy(source) => flow::read_all(Root::Folder, source, u64::MAX),
                };
                bytes
                    .and_then({
                        let path = path.clone();
                        move |bytes| {
                            flow::act(Io::Create {
                                root: Root::Folder,
                                path,
                                bytes,
                            })
                        }
                    })
                    .and_then(move |()| flow::sync(Root::Folder, &path))
            })
        })
        .and_then(move |()| flow::sync(Root::Folder, &synced))
}

/// The first precondition that does not hold, or a trash item a step needs that
/// is gone.
fn refusal<'a>(
    layout: &Layout,
    writer: WriterId,
    plan: &EffectPlan,
    identify: &Rc<dyn Identify>,
) -> Fallible<'a, Option<Refusal>> {
    let identify = Rc::clone(identify);
    let checked = fold(
        plan.checks.clone().into_iter(),
        None,
        move |refused, check| match refused {
            Some(refused) => ok(Some(refused)),
            None => found(&check.path, &identify).map_ok(move |found| {
                let holds = match (check.expect, found) {
                    (Expect::Absent, None) => true,
                    (Expect::Holds(expected), Some(found)) => expected == found,
                    _ => false,
                };
                (!holds).then(|| {
                    Refusal::Changed(Box::new(Mismatch {
                        path: check.path,
                        expected: check.expect,
                        found,
                    }))
                })
            }),
        },
    );
    let items: Vec<RelPath> = plan
        .steps
        .iter()
        .filter_map(|step| match step {
            EffectStep::FromTrash { item, .. } => Some(layout.trash(writer, *item)),
            _ => None,
        })
        .collect();
    checked.and_then(move |refused| {
        fold(items.into_iter(), refused, |refused, item| match refused {
            Some(refused) => ok(Some(refused)),
            None => flow::stat(Root::Folder, &item)
                .map_ok(|meta| meta.is_none().then_some(Refusal::Emptied)),
        })
    })
}

/// The identity of the file at a library path; a directory there is something
/// already there.
fn found<'a>(path: &RelPath, identify: &Rc<dyn Identify>) -> Fallible<'a, Option<Identity>> {
    let path = path.clone();
    let identify = Rc::clone(identify);
    flow::stat(Root::Folder, &path).and_then(move |meta| match meta {
        None => ok(None),
        Some(meta) if meta.kind == Kind::Directory => Flow::Done(Err(Error::Io {
            root: Root::Folder,
            path,
            error: IoError::AlreadyExists,
        })),
        Some(meta) => flow::identity(Root::Folder, path, meta.len, &identify).map_ok(Some),
    })
}

/// Carries out `record`'s steps from `start`, each destination durable before its
/// source is gone, then reports what the folder shows. A step that fails stops the
/// run with [`Outcome::Partial`]; staged bytes that were not placed then go to this
/// writer's trash. The pending record `name` stays until [`finish`].
pub fn apply(
    layout: &Layout,
    name: Nonce,
    record: Rc<PendingRecord>,
    start: usize,
    identify: Rc<dyn Identify>,
) -> Task<'static, Result<Applied>> {
    let layout = layout.clone();
    let writer = record.writer;
    let steps = start..record.steps.len();
    let running = (layout.clone(), Rc::clone(&record));
    fold(
        steps,
        None,
        move |failed: Option<IoError>, i| match failed {
            Some(failed) => ok(Some(failed)),
            None => {
                run_step(&running.0, writer, &running.1.steps[i]).then(move |result| match result {
                    Ok(()) => ok(None),
                    Err(Error::Io { error, .. }) => ok(Some(error)),
                    Err(other) => ok(Some(IoError::Other(format!("step {i}: {other}")))),
                })
            }
        },
    )
    .and_then(move |failed| account(layout, name, record, failed, identify))
    .task()
}

fn run_step<'a>(layout: &Layout, writer: WriterId, step: &EffectStep) -> Fallible<'a, ()> {
    match (step, step.ends(layout, writer)) {
        (_, Some((from, to))) => flow::rename(Root::Folder, &from, &to),
        (EffectStep::MakeDir { path }, None) => flow::ensure_dir(Root::Folder, path),
        (EffectStep::RemoveDir { path }, None) => {
            let io = Io::RemoveDir {
                root: Root::Folder,
                path: path.clone(),
            };
            let parent = path.clone();
            flow::attempt(io.clone()).then(move |result| match result {
                Ok(_) => flow::sync_parent(Root::Folder, &parent),
                Err(IoError::NotEmpty | IoError::NotFound | IoError::NotDirectory) => ok(()),
                Err(error) => Flow::Done(Err(io.failed(error))),
            })
        }
        (_, None) => unreachable!("only directory steps have no ends"),
    }
}

/// What the steps did: every step when none failed, else what the folder shows.
fn account<'a>(
    layout: Layout,
    name: Nonce,
    record: Rc<PendingRecord>,
    failed: Option<IoError>,
    identify: Rc<dyn Identify>,
) -> Fallible<'a, Applied> {
    let writer = record.writer;
    let done: Fallible<'a, Progress> = match failed {
        None => ok(Progress {
            done: record.steps.len(),
            leftover: None,
        }),
        Some(_) => {
            let (layout, record) = (layout.clone(), Rc::clone(&record));
            observe(&layout, &record).map_ok(move |seen| progress(&layout, &record, &seen))
        }
    };
    done.and_then(move |progress| {
        let unplaced: Vec<(Nonce, RelPath)> = record.steps[progress.done..]
            .iter()
            .filter_map(|step| match step {
                EffectStep::Place { staged, path } => Some((*staged, path.clone())),
                _ => None,
            })
            .collect();
        let trashed: Vec<(Nonce, RelPath)> = record.steps[..progress.done]
            .iter()
            .filter_map(|step| match step {
                EffectStep::ToTrash { path, item } => Some((*item, path.clone())),
                _ => None,
            })
            .collect();
        let ends: Vec<FileEnd> = record
            .files
            .iter()
            .filter(|end| end.done_after <= progress.done)
            .cloned()
            .collect();
        let outcome = match record.steps.get(progress.done) {
            None => Outcome::Complete,
            Some(step) => Outcome::Partial(PartialReport {
                applied: progress.done,
                stopped: step.path().clone(),
                error: failed.unwrap_or(IoError::NotFound),
                record: Some(name),
            }),
        };
        let sweeping = layout.clone();
        fold(
            unplaced.into_iter(),
            Vec::new(),
            move |mut swept, (staged, path)| {
                let from = sweeping.staged(writer, staged);
                let to = sweeping.trash(writer, staged);
                flow::stat(Root::Folder, &from).and_then(move |meta| match meta {
                    None => ok(swept),
                    Some(_) => flow::rename(Root::Folder, &from, &to).map_ok(move |()| {
                        swept.push((staged, path));
                        swept
                    }),
                })
            },
        )
        .and_then({
            let identify = Rc::clone(&identify);
            move |swept| {
                let items: Vec<(Nonce, RelPath)> = trashed.into_iter().chain(swept).collect();
                fold(
                    items.into_iter(),
                    Vec::new(),
                    move |mut displaced, (item, from)| {
                        flow::observe(Root::Folder, &layout.trash(writer, item), &identify).map_ok(
                            move |seen| {
                                displaced.extend(seen.map(|seen| Displaced {
                                    item,
                                    from,
                                    identity: seen.identity,
                                    len: seen.len,
                                }));
                                displaced
                            },
                        )
                    },
                )
            }
        })
        .and_then(move |displaced| {
            fold(
                ends.into_iter(),
                Vec::new(),
                move |mut files, end| match end.path {
                    None => {
                        files.push((end.entity, None));
                        ok(files)
                    }
                    Some(path) => {
                        flow::observe(Root::Folder, &path, &identify).map_ok(move |seen| {
                            files.extend(seen.map(|seen| {
                                let fact = FileFact {
                                    path,
                                    identity: seen.identity,
                                    len: seen.len,
                                    modified: seen.modified,
                                };
                                (end.entity, Some(fact))
                            }));
                            files
                        })
                    }
                },
            )
            .map_ok(move |files| Applied {
                displaced,
                files,
                outcome,
            })
        })
    })
}

/// Removes the pending record `name` once the intent's entry is durable. A record
/// already gone is success.
pub fn finish(layout: &Layout, writer: WriterId, name: Nonce) -> Task<'static, Result<()>> {
    flow::remove(Root::Folder, &layout.pending(writer, name)).task()
}

/// Which of a record's paths the folder holds, and which pairs of a step's source
/// and destination both hold the same bytes.
#[derive(Clone, PartialEq, Eq, Debug, Default)]
pub(crate) struct Observation {
    present: BTreeSet<RelPath>,
    equal: BTreeSet<(RelPath, RelPath)>,
}

/// How far a record's steps got: the first `done`, and a source the last of them
/// left behind beside its destination with the same bytes.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Progress {
    pub done: usize,
    pub leftover: Option<RelPath>,
}

/// Reads what [`progress`] needs. Requests only [`Io::Stat`] and [`Io::Read`].
pub(crate) fn observe<'a>(layout: &Layout, record: &PendingRecord) -> Fallible<'a, Observation> {
    let writer = record.writer;
    let mut paths = BTreeSet::new();
    let mut pairs = Vec::new();
    for step in &record.steps {
        if let EffectStep::Place { staged, .. } = step {
            paths.insert(layout.trash(writer, *staged));
        }
        if let Some((from, to)) = step.ends(layout, writer) {
            paths.insert(from.clone());
            paths.insert(to.clone());
            pairs.push((from, to));
        }
    }
    fold(paths.into_iter(), BTreeSet::new(), |mut present, path| {
        flow::stat(Root::Folder, &path).map_ok(move |meta| {
            if meta.is_some() {
                present.insert(path);
            }
            present
        })
    })
    .and_then(move |present| {
        let both: Vec<(RelPath, RelPath)> = pairs
            .into_iter()
            .filter(|(from, to)| present.contains(from) && present.contains(to))
            .collect();
        fold(
            both.into_iter(),
            BTreeSet::new(),
            |mut equal, (from, to)| {
                flow::same_bytes(Root::Folder, &from, &to).map_ok(move |same| {
                    if same {
                        equal.insert((from, to));
                    }
                    equal
                })
            },
        )
        .map_ok(move |equal| Observation { present, equal })
    })
}

/// How far a record's steps got. Steps run in order, each finished before the
/// next starts, so the last step that shows done ends the done prefix. Directory
/// steps are idempotent and do not count.
pub(crate) fn progress(layout: &Layout, record: &PendingRecord, seen: &Observation) -> Progress {
    let writer = record.writer;
    let has = |path: &RelPath| seen.present.contains(path);
    for (i, step) in record.steps.iter().enumerate().rev() {
        let Some((from, to)) = step.ends(layout, writer) else {
            continue;
        };
        let equal = seen.equal.contains(&(from.clone(), to.clone()));
        // A trash item's name is new, so its presence is proof; a staged file's
        // absence is, unless it was swept into the trash instead.
        let done = match step {
            EffectStep::ToTrash { .. } => has(&to),
            EffectStep::Place { staged, .. } => {
                has(&to) && (equal || !has(&from) && !has(&layout.trash(writer, *staged)))
            }
            _ => has(&to) && (equal || !has(&from)),
        };
        if done {
            return Progress {
                done: i + 1,
                leftover: (has(&from) && equal).then_some(from),
            };
        }
    }
    Progress {
        done: 0,
        leftover: None,
    }
}

/// How resuming from `progress` will end, if nothing else changes the folder.
pub(crate) fn predict(
    layout: &Layout,
    name: Nonce,
    record: &PendingRecord,
    seen: &Observation,
    progress: &Progress,
) -> Outcome {
    let mut present = seen.present.clone();
    if let Some(leftover) = &progress.leftover {
        present.remove(leftover);
    }
    for (i, step) in record.steps.iter().enumerate().skip(progress.done) {
        let Some((from, to)) = step.ends(layout, record.writer) else {
            continue;
        };
        let error = match (present.contains(&from), present.contains(&to)) {
            (false, _) => IoError::NotFound,
            (true, true) => IoError::AlreadyExists,
            (true, false) => {
                present.remove(&from);
                present.insert(to);
                continue;
            }
        };
        return Outcome::Partial(PartialReport {
            applied: i,
            stopped: step.path().clone(),
            error,
            record: Some(name),
        });
    }
    Outcome::Complete
}
