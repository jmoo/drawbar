//! Changing the user's files, with preconditions, so that nothing is overwritten.
//!
//! Every effect checks its precondition first and refuses with
//! [`crate::Error::Changed`] rather than overwrite. Displaced bytes enter the blob
//! store. An intent's effects are journaled together before the first step, so
//! recovery at the next open can finish them or say what it could not finish.

use std::borrow::Cow;
use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};

use crate::blobs::{self, displace};
use crate::error::{Error, Mismatch, Result};
use crate::fs::{
    ensure_dir, fingerprint, hash_file, sync_parent, Capability, FileKind, Fingerprint, Fs,
    RelPath, Sameness,
};
use crate::ids::{EntityId, IntentId, WriterId};
use crate::journal::{self, Record, StepRecord};
use crate::layout::Layout;
use crate::log::{Entry, Kind, LogWriter};
use crate::merge::State;
use crate::value::{BlobId, Value};
use crate::{CONTENT_FIELD, LENGTH_FIELD, MODIFIED_FIELD, PATH_FIELD};

/// What the writer expects to find at a path before changing it.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Precondition {
    Absent,
    /// The file still matches what the writer last read.
    Matches(Fingerprint),
    /// The file holds exactly these bytes.
    Holds(BlobId),
}

impl Precondition {
    fn holds(&self, found: Option<&Fingerprint>) -> bool {
        match (self, found) {
            (Self::Absent, None) => true,
            (Self::Matches(expected), Some(found)) => expected.compare(found) == Sameness::Same,
            (Self::Holds(blob), Some(found)) => found.hash == Some(*blob),
            (Self::Absent, Some(_)) | (Self::Matches(_) | Self::Holds(_), None) => false,
        }
    }
}

/// Where new contents come from.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Source {
    Bytes(Vec<u8>),
    Blob(BlobId),
}

/// A change to the user's files.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Effect {
    /// Write the entity's file at `path`, displacing any old contents into the blob
    /// store.
    Save {
        entity: EntityId,
        path: RelPath,
        contents: Source,
        expect: Precondition,
    },
    /// Move the entity's file into the blob store.
    Delete {
        entity: EntityId,
        path: RelPath,
        expect: Precondition,
    },
    /// Move the entity's file to `to`, where nothing may be.
    Rename {
        entity: EntityId,
        from: RelPath,
        to: RelPath,
    },
    /// Move a directory and everything in it to `to`, where nothing may be.
    MoveTree { from: RelPath, to: RelPath },
}

/// What an effect did to the files.
#[derive(Clone, PartialEq, Eq, Debug, Default)]
pub struct Report {
    /// Files moved into the blob store, by the path they had.
    pub displaced: Vec<(RelPath, BlobId)>,
    /// Files moved, from and to.
    pub moved: Vec<(RelPath, RelPath)>,
}

impl Report {
    pub(crate) fn extend(&mut self, other: Report) {
        self.displaced.extend(other.displaced);
        self.moved.extend(other.moved);
    }
}

/// Apply `effects` as one intent, and append `entries` and the effects' own entries to
/// the log under `intent` once the files are changed. Every effect is checked before
/// any is run, so when one is refused nothing is written.
///
/// An intent the disk has no room for first collects this writer's blobs that nothing
/// needs, then gives up this writer's undo history and collects again, and only then
/// refuses with [`Error::NoSpace`]. One that saves bytes from the blob store, as undo
/// and redo do, keeps both those blobs and the history.
pub async fn apply<F: Fs>(
    fs: &F,
    layout: &Layout,
    log: &mut LogWriter,
    state: &State,
    intent: IntentId,
    entries: Vec<Kind>,
    effects: &[Effect],
) -> Result<Report> {
    log.writable()?;
    let known = Known::of(state, effects);
    let sources: Vec<BlobId> = effects.iter().filter_map(Effect::source_blob).collect();
    let rounds: &[bool] = match sources.is_empty() {
        true => &[false, true],
        false => &[false],
    };
    let mut planned = plan(fs, layout, log.writer(), effects, &known).await;
    for &evict_undo in rounds {
        if !matches!(planned, Err(Error::NoSpace { .. })) {
            break;
        }
        blobs::make_room(fs, layout, log, &sources, evict_undo).await?;
        planned = plan(fs, layout, log.writer(), effects, &known).await;
    }
    perform(fs, layout, log, intent, entries, planned?).await
}

impl Effect {
    fn source_blob(&self) -> Option<BlobId> {
        match self {
            Self::Save {
                contents: Source::Blob(blob),
                ..
            } => Some(*blob),
            Self::Save { .. }
            | Self::Delete { .. }
            | Self::Rename { .. }
            | Self::MoveTree { .. } => None,
        }
    }
}

/// Stamp `entries` and the planned steps' entries, journal them as one record, and run
/// the steps.
pub(crate) async fn perform<F: Fs>(
    fs: &F,
    layout: &Layout,
    log: &mut LogWriter,
    intent: IntentId,
    entries: Vec<Kind>,
    planned: Vec<Planned>,
) -> Result<Report> {
    let record = stamped(log, intent, entries, planned);
    journal::write(fs, layout, log.writer(), &record).await?;
    journal::settle(fs, layout, log, &record, &journal::Logged::default())
        .await?
        .into_result()
}

/// The journal record of `entries` and `planned`, stamped in that order.
pub(crate) fn stamped(
    log: &mut LogWriter,
    intent: IntentId,
    entries: Vec<Kind>,
    planned: Vec<Planned>,
) -> Record {
    let mut stamp = |kinds: Vec<Kind>| -> Vec<Entry> {
        kinds
            .into_iter()
            .map(|kind| log.stamp(intent, kind))
            .collect()
    };
    let entries = stamp(entries);
    let steps = planned
        .into_iter()
        .map(|planned| StepRecord {
            step: planned.step,
            entries: stamp(planned.facts),
        })
        .collect();
    Record {
        intent,
        entries,
        steps,
    }
}

/// The fields of their files the merged state holds for the entities effects touch.
#[derive(Clone, Default, Debug)]
pub(crate) struct Known(pub BTreeMap<EntityId, Fields>);

#[derive(Clone, Default, Debug)]
pub(crate) struct Fields {
    pub path: Option<Value>,
    pub content: Option<Value>,
    pub length: Option<Value>,
    pub modified: Option<Value>,
}

impl Known {
    fn of(state: &State, effects: &[Effect]) -> Self {
        let mut entities = BTreeSet::new();
        for effect in effects {
            match effect {
                Effect::Save { entity, .. }
                | Effect::Delete { entity, .. }
                | Effect::Rename { entity, .. } => {
                    entities.insert(*entity);
                }
                Effect::MoveTree { from, .. } => {
                    entities.extend(state.entities().into_iter().filter(|entity| {
                        bound_path(state.field(*entity, PATH_FIELD))
                            .is_some_and(|p| p.starts_with(from))
                    }));
                }
            }
        }
        let fields = |entity| Fields {
            path: state.field(entity, PATH_FIELD).cloned(),
            content: state.field(entity, CONTENT_FIELD).cloned(),
            length: state.field(entity, LENGTH_FIELD).cloned(),
            modified: state.field(entity, MODIFIED_FIELD).cloned(),
        };
        Self(entities.into_iter().map(|e| (e, fields(e))).collect())
    }

    fn fields(&self, entity: EntityId) -> Fields {
        self.0.get(&entity).cloned().unwrap_or_default()
    }
}

fn bound_path(value: Option<&Value>) -> Option<RelPath> {
    match value {
        Some(Value::Text(text)) => RelPath::new(text).ok(),
        _ => None,
    }
}

/// An effect whose precondition held and whose new bytes are staged: the step to
/// journal and the facts to log once it is done.
pub(crate) struct Planned {
    pub step: Step,
    pub facts: Vec<Kind>,
}

/// An effect whose precondition held, with the bytes it stages before it is journaled.
struct Checked<'a> {
    step: Step,
    facts: Vec<Kind>,
    stage: Option<Cow<'a, [u8]>>,
    /// The entity whose `length` and `modified` describe the file the step leaves at
    /// its path, with the file there now, which stays when nothing is staged.
    describes: Option<(EntityId, Option<Fingerprint>)>,
}

/// Check every effect against the files, then stage what they write. Nothing in the
/// library changes, and a refusal leaves nothing staged.
pub(crate) async fn plan<F: Fs>(
    fs: &F,
    layout: &Layout,
    writer: WriterId,
    effects: &[Effect],
    known: &Known,
) -> Result<Vec<Planned>> {
    if !fs.capabilities().rename_file {
        return Err(Error::Unsupported(Capability::RenameFile));
    }
    let planner = Planner { fs, layout, known };
    let mut checked = Vec::with_capacity(effects.len());
    for effect in effects {
        checked.push(planner.check(effect).await?);
    }
    let mut planned = Vec::with_capacity(checked.len());
    for effect in checked {
        let staged = match &effect.stage {
            None => None,
            Some(bytes) => match blobs::stage(fs, layout, writer, bytes).await {
                Ok(staged) => Some(staged),
                Err(error) => {
                    unstage(fs, layout, writer, &planned).await?;
                    return Err(error);
                }
            },
        };
        let mut facts = effect.facts;
        if let Some((entity, present)) = effect.describes {
            let print = match staged {
                Some(staged) => fingerprint(fs, &staged, false).await?,
                None => present,
            };
            facts.extend(described(entity, print, &known.fields(entity)));
        }
        planned.push(Planned {
            step: effect.step,
            facts,
        });
    }
    Ok(planned)
}

/// The `length` and `modified` entries for an entity whose file has `print`, or has
/// none. A rename keeps a file's modification time, so a staged file's is the one it
/// has once placed.
fn described(entity: EntityId, print: Option<Fingerprint>, fields: &Fields) -> Vec<Kind> {
    let length = print.and_then(|print| int(print.len));
    let modified = print.and_then(|print| print.modified).and_then(int);
    field(entity, LENGTH_FIELD, length, fields.length.clone())
        .into_iter()
        .chain(field(
            entity,
            MODIFIED_FIELD,
            modified,
            fields.modified.clone(),
        ))
        .collect()
}

/// `n` as an `Int`; `None` past `i64::MAX`, which leaves a scan to hash the file.
pub(crate) fn int(n: u64) -> Option<Value> {
    i64::try_from(n).ok().map(Value::Int)
}

/// Remove the bytes `planned` staged.
async fn unstage<F: Fs>(
    fs: &F,
    layout: &Layout,
    writer: WriterId,
    planned: &[Planned],
) -> Result<()> {
    for planned in planned {
        if let Step::Save { new, .. } = &planned.step {
            let staged = blobs::staged(layout, writer, new.blob);
            if exists(fs, &staged).await? {
                fs.remove_file(&staged).await?;
            }
        }
    }
    fs.sync(&layout.tmp(writer)).await
}

struct Planner<'a, F> {
    fs: &'a F,
    layout: &'a Layout,
    known: &'a Known,
}

impl<F: Fs> Planner<'_, F> {
    async fn check<'e>(&self, effect: &'e Effect) -> Result<Checked<'e>> {
        match effect {
            Effect::Save {
                entity,
                path,
                contents,
                expect,
            } => self.save(*entity, path, contents, expect).await,
            Effect::Delete {
                entity,
                path,
                expect,
            } => self.delete(*entity, path, expect).await,
            Effect::Rename { entity, from, to } => self.rename(*entity, from, to).await,
            Effect::MoveTree { from, to } => self.move_tree(from, to).await,
        }
    }

    async fn save<'e>(
        &self,
        entity: EntityId,
        path: &RelPath,
        contents: &'e Source,
        expect: &Precondition,
    ) -> Result<Checked<'e>> {
        self.library_path(path)?;
        let found = fingerprint(self.fs, path, true).await?;
        let old = check(path, expect, found)?;
        let bytes = match contents {
            Source::Bytes(bytes) => Cow::Borrowed(bytes.as_slice()),
            Source::Blob(blob) => Cow::Owned(blobs::get(self.fs, self.layout, *blob).await?),
        };
        let new = Stored::of(&bytes);
        let displaced = old.filter(|old| old.blob != new.blob);
        let fields = self.known.fields(entity);
        let facts = displaced
            .map(Stored::added)
            .into_iter()
            .chain(field(entity, PATH_FIELD, Some(text(path)), fields.path))
            .chain(field(
                entity,
                CONTENT_FIELD,
                Some(Value::Blob(new.blob)),
                fields.content,
            ))
            .collect();
        Ok(Checked {
            step: Step::Save {
                path: path.clone(),
                new,
                old,
            },
            facts,
            stage: (old != Some(new)).then_some(bytes),
            describes: Some((entity, found)),
        })
    }

    async fn delete(
        &self,
        entity: EntityId,
        path: &RelPath,
        expect: &Precondition,
    ) -> Result<Checked<'static>> {
        self.library_path(path)?;
        let found = fingerprint(self.fs, path, true).await?;
        let old =
            check(path, expect, found)?.ok_or_else(|| Error::NotFound { path: path.clone() })?;
        let fields = self.known.fields(entity);
        let facts = std::iter::once(old.added())
            .chain(field(entity, PATH_FIELD, None, fields.path))
            .chain(field(entity, CONTENT_FIELD, None, fields.content))
            .collect();
        Ok(Checked {
            step: Step::Delete {
                path: path.clone(),
                old,
            },
            facts,
            stage: None,
            describes: Some((entity, None)),
        })
    }

    async fn rename(
        &self,
        entity: EntityId,
        from: &RelPath,
        to: &RelPath,
    ) -> Result<Checked<'static>> {
        self.free_destination(from, to).await?;
        match self.fs.metadata(from).await? {
            None => return Err(Error::NotFound { path: from.clone() }),
            Some(found) if found.kind == FileKind::Directory => {
                return Err(Error::IsDirectory { path: from.clone() })
            }
            Some(_) => {}
        }
        let prior = self.known.fields(entity).path;
        Ok(Checked {
            step: Step::Move {
                from: from.clone(),
                to: to.clone(),
            },
            facts: field(entity, PATH_FIELD, Some(text(to)), prior)
                .into_iter()
                .collect(),
            stage: None,
            describes: None,
        })
    }

    async fn move_tree(&self, from: &RelPath, to: &RelPath) -> Result<Checked<'static>> {
        self.free_destination(from, to).await?;
        if to.starts_with(from) {
            return Err(Error::InvalidPath {
                path: to.as_str().to_owned(),
                reason: "a directory cannot move inside itself",
            });
        }
        match self.fs.metadata(from).await? {
            None => return Err(Error::NotFound { path: from.clone() }),
            Some(found) if found.kind == FileKind::File => {
                return Err(Error::NotDirectory { path: from.clone() })
            }
            Some(_) => {}
        }
        let step = match self.fs.capabilities().rename_dir {
            true => Step::Move {
                from: from.clone(),
                to: to.clone(),
            },
            false => {
                let (files, dirs) = tree(self.fs, from).await?;
                Step::MoveFiles {
                    from: from.clone(),
                    to: to.clone(),
                    files,
                    dirs,
                }
            }
        };
        let facts = self
            .known
            .0
            .iter()
            .filter_map(|(entity, fields)| {
                let moved = bound_path(fields.path.as_ref())?.rebase(from, to)?;
                field(*entity, PATH_FIELD, Some(text(&moved)), fields.path.clone())
            })
            .collect();
        Ok(Checked {
            step,
            facts,
            stage: None,
            describes: None,
        })
    }

    /// Refuse paths that are toshokan's own, and a destination something occupies.
    async fn free_destination(&self, from: &RelPath, to: &RelPath) -> Result<()> {
        self.library_path(from)?;
        self.library_path(to)?;
        match self.fs.metadata(to).await? {
            Some(_) => Err(Error::AlreadyExists { path: to.clone() }),
            None => Ok(()),
        }
    }

    fn library_path(&self, path: &RelPath) -> Result<()> {
        self.layout.check_library_path(path)
    }
}

/// Refuse with [`Error::Changed`] unless `found`, the hashed fingerprint of the file at
/// `path`, meets `expect`, and return what is there.
fn check(
    path: &RelPath,
    expect: &Precondition,
    found: Option<Fingerprint>,
) -> Result<Option<Stored>> {
    if !expect.holds(found.as_ref()) {
        return Err(Error::Changed(Box::new(Mismatch {
            path: path.clone(),
            expected: *expect,
            found,
        })));
    }
    Ok(found.map(|found| Stored {
        blob: found.hash.expect("the file was hashed"),
        len: found.len,
    }))
}

fn field(entity: EntityId, name: &str, value: Option<Value>, prior: Option<Value>) -> Option<Kind> {
    (value != prior).then(|| Kind::Field {
        entity,
        name: name.to_owned(),
        value,
        prior,
    })
}

fn text(path: &RelPath) -> Value {
    Value::Text(path.as_str().to_owned())
}

fn parent(path: &RelPath) -> RelPath {
    path.parent()
        .expect("a library path is not the library folder")
}

fn under(base: &RelPath, relative: &RelPath) -> RelPath {
    relative
        .rebase(&RelPath::ROOT, base)
        .expect("every path is under the library folder")
}

/// Every file and directory under `root`, relative to it and sorted; the directories
/// include `root` itself as the empty path.
async fn tree<F: Fs>(fs: &F, root: &RelPath) -> Result<(Vec<RelPath>, Vec<RelPath>)> {
    let mut files = Vec::new();
    let mut dirs = vec![RelPath::ROOT];
    let mut next = 0;
    while let Some(dir) = dirs.get(next).cloned() {
        next += 1;
        for entry in fs.list(&under(root, &dir)).await? {
            let child = dir.join(&entry.name)?;
            match entry.kind {
                FileKind::File => files.push(child),
                FileKind::Directory => dirs.push(child),
            }
        }
    }
    files.sort();
    dirs.sort();
    Ok((files, dirs))
}

/// Bytes in the blob store, or bound for it, with their length.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Stored {
    pub blob: BlobId,
    pub len: u64,
}

impl Stored {
    pub(crate) fn of(bytes: &[u8]) -> Self {
        Self {
            blob: BlobId::of(bytes),
            len: bytes.len() as u64,
        }
    }

    pub(crate) fn added(self) -> Kind {
        Kind::BlobAdded {
            blob: self.blob,
            len: self.len,
        }
    }
}

/// The file operations of one journaled effect. Running a step again from any state
/// a crash during it leaves brings the files to the same end.
#[derive(Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub(crate) enum Step {
    /// Put the bytes staged as `new` at `path`, which held `old`, moving `old` into
    /// the store.
    Save {
        path: RelPath,
        new: Stored,
        old: Option<Stored>,
    },
    /// Move the file at `path`, which holds `old`, into the store.
    Delete { path: RelPath, old: Stored },
    /// Rename a file, or a directory with everything in it.
    Move { from: RelPath, to: RelPath },
    /// Move a directory's files one at a time: create `dirs` under `to`, move `files`,
    /// then remove `dirs` under `from`. Both lists are relative to the directory.
    MoveFiles {
        from: RelPath,
        to: RelPath,
        files: Vec<RelPath>,
        dirs: Vec<RelPath>,
    },
}

/// How a step ended.
#[derive(Debug)]
pub(crate) enum Ran {
    Finished(Report),
    /// Some of a tree's files moved, and the rest stayed where they were for the
    /// reason in `error`. `stayed` holds each one's source and destination.
    Partly {
        report: Report,
        stayed: Vec<(RelPath, RelPath)>,
        error: Error,
    },
    /// The files no longer allow the step, for the reason in `error`, and it changed
    /// nothing. `kept` are the step's bytes now in the store, which the log must name.
    Conflict {
        error: Error,
        kept: Vec<Stored>,
    },
}

impl Step {
    pub(crate) fn paths(&self) -> Vec<RelPath> {
        match self {
            Self::Save { path, .. } | Self::Delete { path, .. } => vec![path.clone()],
            Self::Move { from, to } | Self::MoveFiles { from, to, .. } => {
                vec![from.clone(), to.clone()]
            }
        }
    }

    /// The path whose file the step changes or moves.
    pub(crate) fn subject(&self) -> &RelPath {
        match self {
            Self::Save { path, .. } | Self::Delete { path, .. } => path,
            Self::Move { from, .. } | Self::MoveFiles { from, .. } => from,
        }
    }

    pub(crate) async fn run<F: Fs>(
        &self,
        fs: &F,
        layout: &Layout,
        writer: WriterId,
    ) -> Result<Ran> {
        match self {
            Self::Save { path, new, old } => save(fs, layout, writer, path, *new, *old).await,
            Self::Delete { path, old } => delete(fs, layout, path, *old).await,
            Self::Move { from, to } => move_entry(fs, from, to).await,
            Self::MoveFiles {
                from,
                to,
                files,
                dirs,
            } => move_files(fs, from, to, files, dirs).await,
        }
    }

    /// Give up a step that has not run, because an earlier step of its intent could not
    /// finish: its staged bytes move into the store. Returns the step's bytes the store
    /// holds.
    pub(crate) async fn abandon<F: Fs>(
        &self,
        fs: &F,
        layout: &Layout,
        writer: WriterId,
    ) -> Result<Vec<Stored>> {
        let Self::Save { new, .. } = self else {
            return Ok(Vec::new());
        };
        let staged = blobs::staged(layout, writer, new.blob);
        if exists(fs, &staged).await? {
            displace(fs, layout, &staged, new.blob).await?;
            sync_parent(fs, &staged).await?;
        }
        in_store(fs, layout, vec![*new]).await
    }
}

enum At {
    Nothing,
    File(BlobId),
    Directory,
}

async fn at<F: Fs>(fs: &F, path: &RelPath) -> Result<At> {
    Ok(match fs.metadata(path).await? {
        None => At::Nothing,
        Some(found) if found.kind == FileKind::Directory => At::Directory,
        Some(_) => At::File(hash_file(fs, path).await?.0),
    })
}

async fn exists<F: Fs>(fs: &F, path: &RelPath) -> Result<bool> {
    Ok(fs.metadata(path).await?.is_some())
}

async fn same_bytes<F: Fs>(fs: &F, a: &RelPath, b: &RelPath) -> Result<bool> {
    Ok(hash_file(fs, a).await?.0 == hash_file(fs, b).await?.0)
}

/// Rename `from` to `to` durably, creating `to`'s directory if it is missing. The
/// destination's directory is synced first, so a crash between the syncs leaves the
/// entry at both names rather than at neither.
async fn place<F: Fs>(fs: &F, from: &RelPath, to: &RelPath) -> Result<()> {
    ensure_dir(fs, &parent(to)).await?;
    fs.rename(from, to).await?;
    sync_parent(fs, to).await?;
    if from.parent() != to.parent() {
        sync_parent(fs, from).await?;
    }
    Ok(())
}

async fn save<F: Fs>(
    fs: &F,
    layout: &Layout,
    writer: WriterId,
    path: &RelPath,
    new: Stored,
    old: Option<Stored>,
) -> Result<Ran> {
    let staged = blobs::staged(layout, writer, new.blob);
    let has_staged = exists(fs, &staged).await?;
    let finished = match at(fs, path).await? {
        At::File(blob) if blob == new.blob => {
            if has_staged {
                fs.remove_file(&staged).await?;
                sync_parent(fs, &staged).await?;
            }
            true
        }
        _ if !has_staged => false,
        At::Nothing => {
            place(fs, &staged, path).await?;
            true
        }
        At::File(blob) if Some(blob) == old.map(|old| old.blob) => {
            displace(fs, layout, path, blob).await?;
            place(fs, &staged, path).await?;
            true
        }
        At::File(_) | At::Directory => false,
    };
    if finished {
        let displaced = old.filter(|old| old.blob != new.blob);
        return Ok(Ran::Finished(Report {
            displaced: displaced
                .map(|old| (path.clone(), old.blob))
                .into_iter()
                .collect(),
            moved: Vec::new(),
        }));
    }
    let error = changed(fs, path, old).await?;
    if has_staged {
        displace(fs, layout, &staged, new.blob).await?;
        sync_parent(fs, &staged).await?;
    }
    let mut kept = vec![new];
    kept.extend(old.filter(|old| old.blob != new.blob));
    Ok(Ran::Conflict {
        error,
        kept: in_store(fs, layout, kept).await?,
    })
}

async fn delete<F: Fs>(fs: &F, layout: &Layout, path: &RelPath, old: Stored) -> Result<Ran> {
    let finished = match at(fs, path).await? {
        At::File(blob) if blob == old.blob => {
            displace(fs, layout, path, blob).await?;
            sync_parent(fs, path).await?;
            true
        }
        At::Nothing => exists(fs, &layout.blob(old.blob)).await?,
        At::File(_) | At::Directory => false,
    };
    if finished {
        return Ok(Ran::Finished(Report {
            displaced: vec![(path.clone(), old.blob)],
            moved: Vec::new(),
        }));
    }
    Ok(Ran::Conflict {
        error: changed(fs, path, Some(old)).await?,
        kept: in_store(fs, layout, vec![old]).await?,
    })
}

async fn move_entry<F: Fs>(fs: &F, from: &RelPath, to: &RelPath) -> Result<Ran> {
    let error = match (fs.metadata(from).await?, fs.metadata(to).await?) {
        (Some(_), None) => {
            place(fs, from, to).await?;
            None
        }
        (None, Some(_)) => None,
        (Some(a), Some(b)) if a.kind == FileKind::File && b.kind == FileKind::File => {
            match same_bytes(fs, from, to).await? {
                true => {
                    fs.remove_file(from).await?;
                    sync_parent(fs, from).await?;
                    None
                }
                false => Some(Error::AlreadyExists { path: to.clone() }),
            }
        }
        (Some(_), Some(_)) => Some(Error::AlreadyExists { path: to.clone() }),
        (None, None) => Some(Error::NotFound { path: from.clone() }),
    };
    Ok(match error {
        Some(error) => Ran::Conflict {
            error,
            kept: Vec::new(),
        },
        None => Ran::Finished(Report {
            displaced: Vec::new(),
            moved: vec![(from.clone(), to.clone())],
        }),
    })
}

/// Moves every file whose destination is free or already holds its bytes, and leaves
/// any other where it is.
async fn move_files<F: Fs>(
    fs: &F,
    from: &RelPath,
    to: &RelPath,
    files: &[RelPath],
    dirs: &[RelPath],
) -> Result<Ran> {
    ensure_dir(fs, &parent(to)).await?;
    for dir in dirs {
        fs.create_dir_all(&under(to, dir)).await?;
    }
    let mut report = Report::default();
    let mut stayed = Vec::new();
    let mut error = None;
    for file in files {
        let (source, target) = (under(from, file), under(to, file));
        let refused = match (exists(fs, &source).await?, exists(fs, &target).await?) {
            (true, false) => {
                fs.rename(&source, &target).await?;
                None
            }
            (true, true) if same_bytes(fs, &source, &target).await? => {
                fs.remove_file(&source).await?;
                None
            }
            (true, true) => Some(Error::AlreadyExists {
                path: target.clone(),
            }),
            (false, true) => None,
            (false, false) => Some(Error::NotFound {
                path: source.clone(),
            }),
        };
        match refused {
            None => report.moved.push((source, target)),
            Some(refused) => {
                error.get_or_insert(refused);
                stayed.push((source, target));
            }
        }
    }
    sync_parent(fs, to).await?;
    for dir in dirs {
        fs.sync(&under(to, dir)).await?;
    }
    for dir in dirs {
        let source = under(from, dir);
        if exists(fs, &source).await? {
            fs.sync(&source).await?;
        }
    }
    for dir in dirs.iter().rev() {
        match fs.remove_dir(&under(from, dir)).await {
            Ok(()) | Err(Error::NotFound { .. } | Error::DirectoryNotEmpty { .. }) => {}
            Err(error) => return Err(error),
        }
    }
    sync_parent(fs, from).await?;
    Ok(match error {
        None => Ran::Finished(report),
        Some(error) => Ran::Partly {
            report,
            stayed,
            error,
        },
    })
}

async fn changed<F: Fs>(fs: &F, path: &RelPath, expected: Option<Stored>) -> Result<Error> {
    let expected = match expected {
        Some(stored) => Precondition::Holds(stored.blob),
        None => Precondition::Absent,
    };
    match fingerprint(fs, path, false).await {
        Ok(found) => Ok(Error::Changed(Box::new(Mismatch {
            path: path.clone(),
            expected,
            found,
        }))),
        Err(Error::IsDirectory { path }) => Ok(Error::IsDirectory { path }),
        Err(error) => Err(error),
    }
}

async fn in_store<F: Fs>(fs: &F, layout: &Layout, candidates: Vec<Stored>) -> Result<Vec<Stored>> {
    let mut kept = Vec::new();
    for stored in candidates {
        if exists(fs, &layout.blob(stored.blob)).await? {
            kept.push(stored);
        }
    }
    Ok(kept)
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use pollster::block_on;

    use super::*;
    use crate::fs::{Capabilities, MemFs};
    use crate::journal::{recover, Outcome, Recovered};
    use crate::log::testing::{logged, reopen, well_formed};
    use crate::log::{Entry, Kind};

    const WRITER: WriterId = WriterId::from_u128(0xe);
    const INTENT: IntentId = IntentId::new(WRITER, 40);

    fn path(text: &str) -> RelPath {
        RelPath::new(text).unwrap()
    }

    fn entity(counter: u64) -> EntityId {
        EntityId::new(WRITER, counter)
    }

    fn hashed(bytes: &[u8]) -> Precondition {
        Precondition::Holds(BlobId::of(bytes))
    }

    fn bound(entities: &[(u64, &str, Option<&[u8]>)]) -> Known {
        Known(
            entities
                .iter()
                .map(|&(counter, at, content)| {
                    let fields = Fields {
                        path: Some(Value::Text(at.into())),
                        content: content.map(|bytes| Value::Blob(BlobId::of(bytes))),
                        ..Fields::default()
                    };
                    (entity(counter), fields)
                })
                .collect(),
        )
    }

    fn set_field(counter: u64, name: &str, value: Option<Value>, prior: Option<Value>) -> Kind {
        Kind::Field {
            entity: entity(counter),
            name: name.into(),
            value,
            prior,
        }
    }

    fn text_value(text: &str) -> Option<Value> {
        Some(Value::Text(text.into()))
    }

    fn blob_value(bytes: &[u8]) -> Option<Value> {
        Some(Value::Blob(BlobId::of(bytes)))
    }

    fn added(bytes: &[u8]) -> Kind {
        Kind::BlobAdded {
            blob: BlobId::of(bytes),
            len: bytes.len() as u64,
        }
    }

    /// A library folder whose files are all durable, with a log and what the merged
    /// state says about its entities.
    struct Library {
        fs: MemFs,
        layout: Layout,
        log: LogWriter,
        known: Known,
    }

    impl Library {
        fn new(capabilities: Capabilities, files: &[(&str, &[u8])], known: Known) -> Self {
            let fs = MemFs::with_capabilities(capabilities);
            for (name, bytes) in files {
                let file = path(name);
                block_on(fs.create_dir_all(&parent(&file))).unwrap();
                block_on(fs.create(&file, bytes)).unwrap();
            }
            let synced: Vec<RelPath> = fs.files().into_keys().chain(fs.directories()).collect();
            for durable in synced.iter().chain([&RelPath::ROOT]) {
                block_on(fs.sync(durable)).unwrap();
            }
            Self {
                log: reopen(&fs, WRITER),
                fs,
                layout: Layout::default(),
                known,
            }
        }

        fn with(files: &[(&str, &[u8])], known: Known) -> Self {
            Self::new(Capabilities::ALL, files, known)
        }

        fn plan(&self, effects: &[Effect]) -> Result<Vec<Planned>> {
            block_on(plan(&self.fs, &self.layout, WRITER, effects, &self.known))
        }

        fn apply_all(&mut self, effects: &[Effect]) -> Result<Report> {
            let planned = self.plan(effects)?;
            block_on(perform(
                &self.fs,
                &self.layout,
                &mut self.log,
                INTENT,
                vec![Kind::INTENT],
                planned,
            ))
        }

        fn apply(&mut self, effect: &Effect) -> Result<Report> {
            self.apply_all(std::slice::from_ref(effect))
        }

        fn files(&self) -> BTreeMap<RelPath, Vec<u8>> {
            library_files(&self.fs, &self.layout)
        }

        fn store(&self) -> BTreeMap<BlobId, Vec<u8>> {
            store(&self.fs, &self.layout)
        }

        /// The logged entries' kinds, without `Intent` entries and the fields that
        /// describe a file's length and time.
        fn kinds(&self) -> Vec<Kind> {
            let described = [LENGTH_FIELD, MODIFIED_FIELD];
            logged(&self.fs, WRITER)
                .into_iter()
                .map(|entry| entry.kind)
                .filter(|kind| match kind {
                    Kind::Intent { .. } => false,
                    Kind::Field { name, .. } => !described.contains(&name.as_str()),
                    _ => true,
                })
                .collect()
        }

        /// The entity's `length` and `modified` as logged last.
        fn described(&self, counter: u64) -> (Option<Value>, Option<Value>) {
            let mut described = (None, None);
            for entry in logged(&self.fs, WRITER) {
                if let Kind::Field {
                    entity: of,
                    name,
                    value,
                    ..
                } = entry.kind
                {
                    match name.as_str() {
                        LENGTH_FIELD if of == entity(counter) => described.0 = value,
                        MODIFIED_FIELD if of == entity(counter) => described.1 = value,
                        _ => {}
                    }
                }
            }
            described
        }
    }

    fn library_files(fs: &MemFs, layout: &Layout) -> BTreeMap<RelPath, Vec<u8>> {
        fs.files()
            .into_iter()
            .filter(|(path, _)| !layout.owns(path))
            .collect()
    }

    fn store(fs: &MemFs, layout: &Layout) -> BTreeMap<BlobId, Vec<u8>> {
        fs.files()
            .into_iter()
            .filter(|(path, _)| path.parent() == Some(layout.blobs()))
            .map(|(path, bytes)| (path.name().unwrap().parse().unwrap(), bytes))
            .collect()
    }

    fn files(entries: &[(&str, &[u8])]) -> BTreeMap<RelPath, Vec<u8>> {
        entries
            .iter()
            .map(|(name, bytes)| (path(name), bytes.to_vec()))
            .collect()
    }

    fn blobs(entries: &[&[u8]]) -> BTreeMap<BlobId, Vec<u8>> {
        entries
            .iter()
            .map(|bytes| (BlobId::of(bytes), bytes.to_vec()))
            .collect()
    }

    macro_rules! assert_refused {
        ($library:expr, $effects:expr, $pattern:pat) => {
            let library = $library;
            let fs = &library.fs;
            let before = (fs.files(), fs.directories(), fs.mutations());
            let result = library.apply_all(&$effects);
            assert!(matches!(result, Err($pattern)), "{result:?}");
            let fs = &library.fs;
            assert_eq!(
                (fs.files(), fs.directories(), fs.mutations()),
                before,
                "a refused effect wrote"
            );
            assert_eq!(logged(&library.fs, WRITER), Vec::<Entry>::new());
        };
    }

    #[test]
    fn saving_a_new_file_writes_it_and_binds_the_entity() {
        let mut library = Library::with(&[], Known::default());
        let report = library
            .apply(&Effect::Save {
                entity: entity(1),
                path: path("Organ/new.npno"),
                contents: Source::Bytes(b"new".to_vec()),
                expect: Precondition::Absent,
            })
            .unwrap();
        assert_eq!(report, Report::default());
        assert_eq!(library.files(), files(&[("Organ/new.npno", b"new")]));
        assert_eq!(library.store(), BTreeMap::new());
        assert_eq!(
            library.kinds(),
            [
                set_field(1, PATH_FIELD, text_value("Organ/new.npno"), None),
                set_field(1, CONTENT_FIELD, blob_value(b"new"), None),
            ]
        );
    }

    #[test]
    fn saving_over_a_file_moves_its_old_bytes_into_the_store() {
        let known = bound(&[(1, "song", Some(b"old"))]);
        let mut library = Library::with(&[("song", b"old")], known);
        let report = library
            .apply(&Effect::Save {
                entity: entity(1),
                path: path("song"),
                contents: Source::Bytes(b"new".to_vec()),
                expect: hashed(b"old"),
            })
            .unwrap();
        assert_eq!(report.displaced, [(path("song"), BlobId::of(b"old"))]);
        assert_eq!(library.files(), files(&[("song", b"new")]));
        assert_eq!(library.store(), blobs(&[b"old"]));
        assert_eq!(
            library.kinds(),
            [
                added(b"old"),
                set_field(1, CONTENT_FIELD, blob_value(b"new"), blob_value(b"old")),
            ]
        );
    }

    #[test]
    fn saving_records_the_length_and_time_of_the_placed_file_and_deleting_clears_them() {
        let mut library = Library::with(&[], Known::default());
        let save = Effect::Save {
            entity: entity(1),
            path: path("song"),
            contents: Source::Bytes(b"new".to_vec()),
            expect: Precondition::Absent,
        };
        library.apply(&save).unwrap();
        let placed = block_on(library.fs.metadata(&path("song")))
            .unwrap()
            .unwrap();
        let time = i64::try_from(placed.modified.unwrap()).unwrap();
        assert_eq!(
            library.described(1),
            (Some(Value::Int(3)), Some(Value::Int(time)))
        );
        library.known = bound(&[(1, "song", Some(b"new"))]);
        let described = library.described(1);
        let fields = library.known.0.get_mut(&entity(1)).unwrap();
        (fields.length, fields.modified) = described;
        library
            .apply(&Effect::Delete {
                entity: entity(1),
                path: path("song"),
                expect: hashed(b"new"),
            })
            .unwrap();
        assert_eq!(library.described(1), (None, None));
    }

    #[test]
    fn saving_accepts_a_file_whose_length_and_time_are_unchanged() {
        let mut library = Library::with(&[("song", b"old")], Known::default());
        let print = block_on(fingerprint(&library.fs, &path("song"), false)).unwrap();
        let save = Effect::Save {
            entity: entity(1),
            path: path("song"),
            contents: Source::Bytes(b"new".to_vec()),
            expect: Precondition::Matches(print.unwrap()),
        };
        library.apply(&save).unwrap();
        assert_eq!(library.files(), files(&[("song", b"new")]));
    }

    #[test]
    fn saving_refuses_a_file_changed_since_it_was_read() {
        let mut library = Library::with(&[("song", b"theirs")], Known::default());
        let save = Effect::Save {
            entity: entity(1),
            path: path("song"),
            contents: Source::Bytes(b"new".to_vec()),
            expect: hashed(b"mine!!"),
        };
        assert_refused!(&mut library, [save], Error::Changed(_));
    }

    #[test]
    fn saving_a_new_file_refuses_one_that_appeared() {
        let mut library = Library::with(&[("song", b"theirs")], Known::default());
        let save = Effect::Save {
            entity: entity(1),
            path: path("song"),
            contents: Source::Bytes(b"new".to_vec()),
            expect: Precondition::Absent,
        };
        assert_refused!(&mut library, [save], Error::Changed(_));
    }

    #[test]
    fn saving_the_bytes_already_there_displaces_nothing() {
        let known = bound(&[(1, "old/song", Some(b"same"))]);
        let mut library = Library::with(&[("song", b"same")], known);
        let report = library
            .apply(&Effect::Save {
                entity: entity(1),
                path: path("song"),
                contents: Source::Bytes(b"same".to_vec()),
                expect: hashed(b"same"),
            })
            .unwrap();
        assert_eq!(report, Report::default());
        assert_eq!(library.store(), BTreeMap::new());
        assert_eq!(
            library.kinds(),
            [set_field(
                1,
                PATH_FIELD,
                text_value("song"),
                text_value("old/song")
            )]
        );
    }

    #[test]
    fn saving_from_a_blob_copies_it_and_keeps_the_blob() {
        let mut library = Library::with(&[("song", b"new")], Known::default());
        let old = block_on(blobs::put(&library.fs, &library.layout, WRITER, b"old")).unwrap();
        library
            .apply(&Effect::Save {
                entity: entity(1),
                path: path("song"),
                contents: Source::Blob(old),
                expect: hashed(b"new"),
            })
            .unwrap();
        assert_eq!(library.files(), files(&[("song", b"old")]));
        assert_eq!(library.store(), blobs(&[b"old", b"new"]));
    }

    #[test]
    fn deleting_moves_the_file_into_the_store_and_unbinds_the_entity() {
        let known = bound(&[(1, "song", Some(b"old"))]);
        let mut library = Library::with(&[("song", b"old"), ("other", b"o")], known);
        let report = library
            .apply(&Effect::Delete {
                entity: entity(1),
                path: path("song"),
                expect: hashed(b"old"),
            })
            .unwrap();
        assert_eq!(report.displaced, [(path("song"), BlobId::of(b"old"))]);
        assert_eq!(library.files(), files(&[("other", b"o")]));
        assert_eq!(library.store(), blobs(&[b"old"]));
        assert_eq!(
            library.kinds(),
            [
                added(b"old"),
                set_field(1, PATH_FIELD, None, text_value("song")),
                set_field(1, CONTENT_FIELD, None, blob_value(b"old")),
            ]
        );
    }

    #[test]
    fn deleting_refuses_a_changed_or_missing_file() {
        let mut library = Library::with(&[("song", b"theirs")], Known::default());
        let delete = |at: &str| Effect::Delete {
            entity: entity(1),
            path: path(at),
            expect: hashed(b"mine!!"),
        };
        assert_refused!(&mut library, [delete("song")], Error::Changed(_));
        assert_refused!(&mut library, [delete("gone")], Error::Changed(_));
    }

    #[test]
    fn renaming_moves_the_file_and_rebinds_the_entity() {
        let known = bound(&[(1, "a/song", None)]);
        let mut library = Library::with(&[("a/song", b"s")], known);
        let report = library
            .apply(&Effect::Rename {
                entity: entity(1),
                from: path("a/song"),
                to: path("b/song"),
            })
            .unwrap();
        assert_eq!(report.moved, [(path("a/song"), path("b/song"))]);
        assert_eq!(library.files(), files(&[("b/song", b"s")]));
        assert_eq!(
            library.kinds(),
            [set_field(
                1,
                PATH_FIELD,
                text_value("b/song"),
                text_value("a/song")
            )]
        );
    }

    #[test]
    fn renaming_refuses_a_destination_that_exists() {
        let mut library = Library::with(&[("a", b"1"), ("b", b"2")], Known::default());
        let rename = Effect::Rename {
            entity: entity(1),
            from: path("a"),
            to: path("b"),
        };
        assert_refused!(&mut library, [rename], Error::AlreadyExists { .. });
    }

    #[test]
    fn an_intent_whose_later_effect_is_refused_changes_nothing() {
        let known = bound(&[(1, "a", Some(b"1")), (2, "c", None)]);
        let mut library = Library::with(&[("a", b"1"), ("c", b"2"), ("d", b"3")], known);
        let save = Effect::Save {
            entity: entity(1),
            path: path("a"),
            contents: Source::Bytes(b"new".to_vec()),
            expect: hashed(b"1"),
        };
        let rename = |from: &str, to: &str| Effect::Rename {
            entity: entity(2),
            from: path(from),
            to: path(to),
        };
        assert_refused!(
            &mut library,
            [save.clone(), rename("c", "d")],
            Error::AlreadyExists { .. }
        );
        assert_refused!(
            &mut library,
            [rename("c", "e"), rename("c", "d")],
            Error::AlreadyExists { .. }
        );
    }

    #[test]
    fn a_save_refused_for_space_leaves_no_directory_behind() {
        let mut library = Library::with(&[("keep", b"k")], Known::default());
        library.fs.set_capacity(Some(2));
        let save = Effect::Save {
            entity: entity(1),
            path: path("d/new"),
            contents: Source::Bytes(b"new".to_vec()),
            expect: Precondition::Absent,
        };
        let result = library.apply(&save);
        assert!(matches!(result, Err(Error::NoSpace { .. })), "{result:?}");
        let directories: Vec<RelPath> = library
            .fs
            .directories()
            .into_iter()
            .filter(|dir| !library.layout.owns(dir))
            .collect();
        assert_eq!(directories, []);
        assert_eq!(library.files(), files(&[("keep", b"k")]));
    }

    fn tree_library(capabilities: Capabilities) -> Library {
        let known = bound(&[(1, "a/x/1", None), (2, "a/x/y/2", None), (3, "a/xy", None)]);
        let files = [("a/x/1", &b"1"[..]), ("a/x/y/2", b"2"), ("a/xy", b"3")];
        Library::new(capabilities, &files, known)
    }

    const NO_DIRECTORY_RENAME: Capabilities = Capabilities {
        rename_dir: false,
        ..Capabilities::ALL
    };

    #[test]
    fn moving_a_tree_moves_every_file_and_rebinds_every_entity_under_it() {
        for capabilities in [Capabilities::ALL, NO_DIRECTORY_RENAME] {
            let mut library = tree_library(capabilities);
            library
                .apply(&Effect::MoveTree {
                    from: path("a/x"),
                    to: path("b/x"),
                })
                .unwrap();
            assert_eq!(
                library.files(),
                files(&[("a/xy", b"3"), ("b/x/1", b"1"), ("b/x/y/2", b"2")]),
                "{capabilities:?}"
            );
            let directories: BTreeSet<RelPath> = library
                .fs
                .directories()
                .into_iter()
                .filter(|dir| !library.layout.owns(dir))
                .collect();
            let expected = ["a", "b", "b/x", "b/x/y"].map(path);
            assert_eq!(directories, expected.into(), "{capabilities:?}");
            assert_eq!(
                library.kinds(),
                [
                    set_field(1, PATH_FIELD, text_value("b/x/1"), text_value("a/x/1")),
                    set_field(2, PATH_FIELD, text_value("b/x/y/2"), text_value("a/x/y/2")),
                ],
                "{capabilities:?}"
            );
        }
    }

    #[test]
    fn moving_a_tree_renames_the_directory_only_where_the_backend_declares_it() {
        let move_tree = Effect::MoveTree {
            from: path("a/x"),
            to: path("b/x"),
        };
        let step = tree_library(Capabilities::ALL)
            .plan(std::slice::from_ref(&move_tree))
            .unwrap()
            .remove(0)
            .step;
        assert!(matches!(step, Step::Move { .. }), "{step:?}");
        let step = tree_library(NO_DIRECTORY_RENAME)
            .plan(std::slice::from_ref(&move_tree))
            .unwrap()
            .remove(0)
            .step;
        let Step::MoveFiles { files, dirs, .. } = step else {
            panic!("{step:?}");
        };
        assert_eq!(files, [path("1"), path("y/2")]);
        assert_eq!(dirs, [RelPath::ROOT, path("y")]);
    }

    #[test]
    fn moving_a_tree_refuses_a_destination_inside_it_or_taken() {
        let mut library = tree_library(Capabilities::ALL);
        let into = |to: &str| Effect::MoveTree {
            from: path("a/x"),
            to: path(to),
        };
        assert_refused!(&mut library, [into("a/x/y/z")], Error::InvalidPath { .. });
        assert_refused!(&mut library, [into("a/xy")], Error::AlreadyExists { .. });
    }

    #[test]
    fn recovering_a_tree_move_whose_destination_was_taken_logs_only_the_files_that_moved() {
        let mut library = tree_library(NO_DIRECTORY_RENAME);
        let move_tree = Effect::MoveTree {
            from: path("a/x"),
            to: path("b/x"),
        };
        let planned = library.plan(std::slice::from_ref(&move_tree)).unwrap();
        let record = stamped(&mut library.log, INTENT, vec![Kind::INTENT], planned);
        let (fs, layout) = (&library.fs, &library.layout);
        block_on(journal::write(fs, layout, WRITER, &record)).unwrap();
        block_on(fs.create_dir_all(&path("b/x/y"))).unwrap();
        block_on(fs.rename(&path("a/x/1"), &path("b/x/1"))).unwrap();
        block_on(fs.create(&path("b/x/y/2"), b"theirs")).unwrap();

        let recovered = block_on(recover(fs, layout, &mut reopen(fs, WRITER))).unwrap();
        assert_eq!(
            recovered,
            [Recovered {
                intent: INTENT,
                paths: vec![path("a/x"), path("b/x")],
                outcome: Outcome::Partial,
                kept: vec![],
                stayed: vec![path("a/x/y/2")],
            }]
        );
        assert_eq!(
            library.files(),
            files(&[
                ("a/x/y/2", b"2"),
                ("a/xy", b"3"),
                ("b/x/1", b"1"),
                ("b/x/y/2", b"theirs")
            ])
        );
        assert_eq!(
            library.kinds(),
            [set_field(
                1,
                PATH_FIELD,
                text_value("b/x/1"),
                text_value("a/x/1")
            )]
        );
    }

    #[test]
    fn toshokans_own_files_are_not_library_paths() {
        let mut library = Library::with(&[("lib/song", b"s")], Known::default());
        library.layout = Layout::new(path("lib/.toshokan")).unwrap();
        let save = Effect::Save {
            entity: entity(1),
            path: path("lib/.toshokan/blobs/x"),
            contents: Source::Bytes(b"x".to_vec()),
            expect: Precondition::Absent,
        };
        assert_refused!(&mut library, [save], Error::InvalidPath { .. });
        let move_holder = Effect::MoveTree {
            from: path("lib"),
            to: path("elsewhere"),
        };
        assert_refused!(&mut library, [move_holder], Error::InvalidPath { .. });
    }

    #[test]
    fn a_backend_that_cannot_rename_files_is_refused_before_any_change() {
        let capabilities = Capabilities {
            fsync: true,
            ..Capabilities::NONE
        };
        let mut library = Library::new(capabilities, &[("song", b"old")], Known::default());
        let delete = Effect::Delete {
            entity: entity(1),
            path: path("song"),
            expect: hashed(b"old"),
        };
        assert_refused!(
            &mut library,
            [delete],
            Error::Unsupported(Capability::RenameFile)
        );
    }

    /// One effect on one library, run with a crash at every mutating operation.
    struct Case {
        files: &'static [(&'static str, &'static [u8])],
        known: fn() -> Known,
        effect: Effect,
        /// Bytes the effect writes, which must survive once staged.
        new: Option<&'static [u8]>,
    }

    /// What the uncrashed effect leaves.
    struct Expected {
        before: BTreeMap<RelPath, Vec<u8>>,
        after: BTreeMap<RelPath, Vec<u8>>,
        entries: Vec<Entry>,
    }

    impl Case {
        fn library(&self, capabilities: Capabilities) -> Library {
            Library::new(capabilities, self.files, (self.known)())
        }

        /// The disk and log a writer opens after a crash `crash` operations into
        /// the effect.
        fn crashed(&self, capabilities: Capabilities, crash: u64) -> (MemFs, LogWriter) {
            let mut library = self.library(capabilities);
            library.fs.crash_after(crash);
            let result = library.apply(&self.effect);
            assert!(
                matches!(result, Err(Error::Crashed)),
                "crash {crash}: {result:?}"
            );
            let disk = library.fs.restart();
            let log = reopen(&disk, WRITER);
            (disk, log)
        }

        fn run(&self, capabilities: Capabilities) {
            let layout = Layout::default();
            let mut clean = self.library(capabilities);
            let start = clean.fs.mutations();
            let before = clean.files();
            clean.apply(&self.effect).unwrap();
            let operations = clean.fs.mutations() - start;
            let expected = Expected {
                before,
                after: clean.files(),
                entries: logged(&clean.fs, WRITER),
            };
            let finished = clean.fs.restart();
            check(&finished, &layout, &expected, self.new)
                .unwrap_or_else(|failure| panic!("{capabilities:?}, uncrashed: {failure}"));
            let staging = {
                let library = self.library(capabilities);
                let start = library.fs.mutations();
                library.plan(std::slice::from_ref(&self.effect)).unwrap();
                library.fs.mutations() - start
            };
            for crash in 0..operations {
                let new = self.new.filter(|_| crash >= staging);
                let (disk, mut log) = self.crashed(capabilities, crash);
                let start = disk.mutations();
                block_on(recover(&disk, &layout, &mut log)).unwrap();
                let recovery = disk.mutations() - start;
                for interrupt in (0..recovery).map(Some).chain([None]) {
                    let (mut disk, mut log) = self.crashed(capabilities, crash);
                    if let Some(interrupt) = interrupt {
                        disk.crash_after(interrupt);
                        let result = block_on(recover(&disk, &layout, &mut log));
                        assert!(matches!(result, Err(Error::Crashed)), "{result:?}");
                        disk = disk.restart();
                        log = reopen(&disk, WRITER);
                    }
                    block_on(recover(&disk, &layout, &mut log)).unwrap();
                    let recovered = disk.restart();
                    let at =
                        format!("{capabilities:?}, crash {crash}, recovery crash {interrupt:?}");
                    check(&recovered, &layout, &expected, new)
                        .unwrap_or_else(|failure| panic!("{at}: {failure}"));
                }
            }
        }
    }

    /// The invariants every recovered library holds.
    fn check(
        disk: &MemFs,
        layout: &Layout,
        expected: &Expected,
        new: Option<&[u8]>,
    ) -> std::result::Result<(), String> {
        let all = disk.files();
        let leftovers: Vec<_> = all
            .keys()
            .filter(|path| {
                path.starts_with(&layout.tmp(WRITER)) || path.starts_with(&layout.journal(WRITER))
            })
            .collect();
        if !leftovers.is_empty() {
            return Err(format!("recovery left {leftovers:?}"));
        }
        let files = library_files(disk, layout);
        let entries = logged(disk, WRITER);
        well_formed(&entries)?;
        if files == expected.after {
            if let Some(missing) = expected.entries.iter().find(|e| !entries.contains(e)) {
                return Err(format!("the effect finished without logging {missing:?}"));
            }
        } else {
            for (path, bytes) in &expected.before {
                if files.get(path) != Some(bytes) {
                    return Err(format!("{path} is not as it was: {files:?}"));
                }
            }
            for (path, bytes) in &files {
                let known = [&expected.before, &expected.after]
                    .iter()
                    .any(|f| f.get(path) == Some(bytes));
                if !known {
                    return Err(format!("{path} holds bytes neither before nor after"));
                }
            }
            if let Some(logged) = expected.entries.iter().find(|e| entries.contains(e)) {
                return Err(format!("the effect rolled back but logged {logged:?}"));
            }
        }
        let store = store(disk, layout);
        for (blob, bytes) in &store {
            if BlobId::of(bytes) != *blob {
                return Err(format!("blob {blob} holds other bytes"));
            }
            let logged = Kind::BlobAdded {
                blob: *blob,
                len: bytes.len() as u64,
            };
            if !entries.iter().any(|e| e.kind == logged) {
                return Err(format!("blob {blob} is in the store but not in the log"));
            }
        }
        let present: BTreeSet<&Vec<u8>> = files.values().chain(store.values()).collect();
        let kept = expected.before.values().map(Vec::as_slice).chain(new);
        for bytes in kept {
            if !present.contains(&bytes.to_vec()) {
                return Err(format!("{:?} were lost", String::from_utf8_lossy(bytes)));
            }
        }
        Ok(())
    }

    const WITHOUT_FSYNC: Capabilities = Capabilities {
        fsync: false,
        ..Capabilities::ALL
    };

    fn crash_everywhere(case: Case, capabilities: &[Capabilities]) {
        for capabilities in capabilities {
            case.run(*capabilities);
        }
    }

    #[test]
    fn every_crash_while_saving_a_new_file_recovers() {
        let case = Case {
            files: &[("keep", b"k")],
            known: Known::default,
            effect: Effect::Save {
                entity: entity(1),
                path: path("d/new"),
                contents: Source::Bytes(b"new".to_vec()),
                expect: Precondition::Absent,
            },
            new: Some(b"new"),
        };
        crash_everywhere(case, &[Capabilities::ALL, WITHOUT_FSYNC]);
    }

    #[test]
    fn every_crash_while_saving_over_a_file_recovers() {
        let case = Case {
            files: &[("song", b"old")],
            known: || bound(&[(1, "song", Some(b"old"))]),
            effect: Effect::Save {
                entity: entity(1),
                path: path("song"),
                contents: Source::Bytes(b"new".to_vec()),
                expect: hashed(b"old"),
            },
            new: Some(b"new"),
        };
        crash_everywhere(case, &[Capabilities::ALL, WITHOUT_FSYNC]);
    }

    #[test]
    fn every_crash_while_deleting_recovers() {
        let case = Case {
            files: &[("song", b"old"), ("other", b"o")],
            known: || bound(&[(1, "song", Some(b"old"))]),
            effect: Effect::Delete {
                entity: entity(1),
                path: path("song"),
                expect: hashed(b"old"),
            },
            new: None,
        };
        crash_everywhere(case, &[Capabilities::ALL, WITHOUT_FSYNC]);
    }

    #[test]
    fn every_crash_while_renaming_recovers() {
        let case = Case {
            files: &[("a/song", b"s")],
            known: || bound(&[(1, "a/song", None)]),
            effect: Effect::Rename {
                entity: entity(1),
                from: path("a/song"),
                to: path("b/song"),
            },
            new: None,
        };
        crash_everywhere(case, &[Capabilities::ALL, WITHOUT_FSYNC]);
    }

    #[test]
    fn every_crash_while_moving_a_tree_recovers() {
        let case = Case {
            files: &[("a/x/1", b"1"), ("a/x/y/2", b"2"), ("a/xy", b"3")],
            known: || bound(&[(1, "a/x/1", None), (2, "a/x/y/2", None)]),
            effect: Effect::MoveTree {
                from: path("a/x"),
                to: path("b/x"),
            },
            new: None,
        };
        let without_both = Capabilities {
            fsync: false,
            ..NO_DIRECTORY_RENAME
        };
        crash_everywhere(
            case,
            &[
                Capabilities::ALL,
                WITHOUT_FSYNC,
                NO_DIRECTORY_RENAME,
                without_both,
            ],
        );
    }
}
