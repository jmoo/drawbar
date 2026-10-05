//! The journal of effects in progress, and recovery from it at open.
//!
//! A writer records each intent's file effects under `journal/<writer>/` before the
//! first step and clears the record after the last, so a crash leaves a record of
//! exactly the intents it interrupted. The record carries the intent's log entries,
//! stamped before the first step, so recovery appends the same entries the intent
//! would have.

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};

use crate::blobs;
use crate::effects::{Ran, Report, Step, Stored};
use crate::error::{Conflict, Error, Result};
use crate::fs::{ensure_dir, hash_file, FileKind, Fs, RelPath};
use crate::ids::{canonical_u64, IntentId, Version, WriterId};
use crate::layout::Layout;
use crate::log::{json_texts, read_log, Entry, Kind, LogWriter, WriterLog};
use crate::value::{BlobId, Value};
use crate::PATH_FIELD;

/// How recovery settled an interrupted intent.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Outcome {
    /// Every file changed as the intent meant, and the log has all its entries.
    Finished,
    /// Some files changed and others were not as the intent expected. The log has the
    /// entries of the changes made, under an intent that reverses nothing.
    Partial,
    /// No file changed. The log has only the bytes recovery kept.
    RolledBack,
    /// This writer is read-only, so the intent is left as the crash left it, for a
    /// build that can finish it.
    Pending,
    /// As `Pending`, and this build cannot read the record, so its paths are unknown.
    Unreadable,
}

/// An intent a crash interrupted, and how recovery settled it.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Recovered {
    pub intent: IntentId,
    /// The library paths the intent's effects touched.
    pub paths: Vec<RelPath>,
    pub outcome: Outcome,
    /// Bytes the intent had written or displaced that recovery moved into the blob
    /// store and logged, so a rolled-back save keeps its new contents.
    pub kept: Vec<BlobId>,
    /// Files the intent meant to change or move that were left where and as they were.
    pub stayed: Vec<RelPath>,
    /// Why the intent did not finish: the first file that was not as it expected.
    pub conflict: Option<Conflict>,
}

/// Finish or roll back each of this writer's journaled intents, then clear the
/// journal. Bytes a crash left in the writer's `tmp/` before journaling them are
/// moved into the blob store and reported under a new intent, and any other file
/// there is removed. Recovery is itself safe to interrupt and repeat.
///
/// A read-only writer changes nothing and reports each journaled intent as pending. A
/// record this build cannot read makes the writer read-only.
pub async fn recover<F: Fs>(
    fs: &F,
    layout: &Layout,
    log: &mut LogWriter,
) -> Result<Vec<Recovered>> {
    let records = read(fs, layout, log.writer()).await?;
    if records.iter().any(|(_, record)| record.is_err()) {
        log.unreadable("its journal holds a record this build cannot read");
    }
    if log.read_only().is_some() {
        return Ok(records.into_iter().map(pending).collect());
    }
    let records: Vec<Record> = records
        .into_iter()
        .map(|(_, record)| record)
        .collect::<Result<_>>()?;
    remove_unfinished(fs, layout, log.writer()).await?;
    let logged = match records.is_empty() {
        true => Logged::default(),
        false => Logged::of(&read_log(fs, layout, log.writer()).await?, &records),
    };
    let mut recovered = Vec::new();
    for record in records {
        let settled = settle(fs, layout, log, &record, &logged).await?;
        recovered.push(Recovered {
            intent: record.intent,
            paths: record.paths(),
            outcome: settled.outcome(),
            kept: settled.kept.iter().map(|stored| stored.blob).collect(),
            stayed: settled.stayed,
            conflict: settled.error,
        });
    }
    recovered.extend(keep_staged(fs, layout, log).await?);
    Ok(recovered)
}

fn pending((intent, record): (IntentId, Result<Record>)) -> Recovered {
    let (paths, outcome) = match record {
        Ok(record) => (record.paths(), Outcome::Pending),
        Err(_) => (Vec::new(), Outcome::Unreadable),
    };
    Recovered {
        intent,
        paths,
        outcome,
        kept: Vec::new(),
        stayed: Vec::new(),
        conflict: None,
    }
}

/// What this writer's log held before recovery appended to it.
#[derive(Default)]
pub(crate) struct Logged {
    versions: BTreeSet<Version>,
    /// The kinds of the entries of each intent a record names.
    kinds: BTreeMap<IntentId, Vec<Kind>>,
}

impl Logged {
    fn of(own: &WriterLog, records: &[Record]) -> Self {
        let mut logged = Self::default();
        for entry in own.all_entries() {
            logged.versions.insert(entry.version);
            if records.iter().any(|record| record.intent == entry.intent) {
                let kinds = logged.kinds.entry(entry.intent).or_default();
                kinds.push(entry.kind.clone());
            }
        }
        logged
    }

    /// Whether the log holds `entry`: an entry of its version, or, as an earlier
    /// recovery of its record stamped it, one of its intent and kind. Any `Intent`
    /// entry of its intent stands for its `Intent` entry.
    fn holds(&self, entry: &Entry) -> bool {
        let same = |kind: &Kind| match (kind, &entry.kind) {
            (Kind::Intent { .. }, Kind::Intent { .. }) => true,
            (kind, other) => kind == other,
        };
        self.versions.contains(&entry.version)
            || self
                .kinds
                .get(&entry.intent)
                .is_some_and(|kinds| kinds.iter().any(same))
    }
}

/// What running a record's steps did.
#[derive(Default)]
pub(crate) struct Settled {
    pub report: Report,
    /// The steps' bytes now in the store that the log names because a step did not
    /// finish.
    pub kept: Vec<Stored>,
    pub stayed: Vec<RelPath>,
    /// Why a step did not finish.
    pub error: Option<Conflict>,
    /// Whether any step changed a file.
    pub changed: bool,
}

impl Settled {
    pub(crate) fn outcome(&self) -> Outcome {
        match (&self.error, self.changed) {
            (None, _) => Outcome::Finished,
            (Some(_), true) => Outcome::Partial,
            (Some(_), false) => Outcome::RolledBack,
        }
    }

    /// The report of a finished intent, or why it did not finish.
    pub(crate) fn into_result(self) -> Result<Report> {
        match (self.error, self.changed) {
            (None, _) => Ok(self.report),
            (Some(error), true) => Err(Error::Partial {
                error: Box::new(error.into()),
                stayed: self.stayed,
            }),
            (Some(error), false) => Err(error.into()),
        }
    }
}

/// Run `record`'s steps in order, append the entries they leave that `logged` does
/// not hold, and clear the record. The clock first passes every version the record
/// holds, so no entry recovery stamps shares one with a recorded entry. When every step finishes the log gains all the
/// record's entries. A step the files no longer allow ends the run: the steps after
/// it are given up, and the log gains only the entries of the steps that changed
/// files, and the bytes kept, under a new `Intent` entry that reverses nothing.
pub(crate) async fn settle<F: Fs>(
    fs: &F,
    layout: &Layout,
    log: &mut LogWriter,
    record: &Record,
    logged: &Logged,
) -> Result<Settled> {
    let writer = log.writer();
    if let Some(latest) = record.versions().max() {
        log.observe(latest);
    }
    let mut settled = Settled::default();
    let mut facts = Vec::new();
    for StepRecord { step, entries } in &record.steps {
        if settled.error.is_some() {
            settled.kept.extend(step.abandon(fs, layout, writer).await?);
            settled.stayed.push(step.subject().clone());
            continue;
        }
        match step.run(fs, layout, writer).await? {
            Ran::Finished(report) => {
                settled.changed = true;
                settled.report.extend(report);
                facts.extend(entries.iter().cloned());
            }
            Ran::Partly {
                report,
                stayed,
                error,
            } => {
                let arrived = |entry: &&Entry| !stayed.iter().any(|(_, to)| moves_to(entry, to));
                facts.extend(entries.iter().filter(arrived).cloned());
                settled.changed |= !report.moved.is_empty();
                settled.report.extend(report);
                settled
                    .stayed
                    .extend(stayed.into_iter().map(|(from, _)| from));
                settled.error = Some(error);
            }
            Ran::Conflict { error, kept } => {
                settled.kept.extend(kept);
                settled.stayed.push(step.subject().clone());
                settled.error = Some(error);
            }
        }
    }
    let entries: Vec<Entry> = match (&settled.error, settled.changed) {
        (None, _) => record.entries.iter().chain(&facts).cloned().collect(),
        (Some(_), false) if settled.kept.is_empty() => Vec::new(),
        (Some(_), changed) => {
            let label = changed.then(|| record.label()).flatten();
            let head = log.stamp(
                record.intent,
                Kind::Intent {
                    label,
                    reverses: None,
                },
            );
            let mut entries = vec![head];
            entries.extend(facts);
            for stored in &settled.kept {
                entries.push(log.stamp(record.intent, stored.added()));
            }
            entries
        }
    };
    let entries: Vec<Entry> = entries
        .into_iter()
        .filter(|entry| !logged.holds(entry))
        .collect();
    log.append(fs, layout, &entries).await?;
    clear(fs, layout, writer, record.intent).await?;
    Ok(settled)
}

/// Whether `entry` binds an entity to `path`.
fn moves_to(entry: &Entry, path: &RelPath) -> bool {
    matches!(
        &entry.kind,
        Kind::Field { name, value: Some(Value::Text(to)), .. }
            if name == PATH_FIELD && to == path.as_str()
    )
}

/// Remove every file in this writer's `tmp/` that is not whole staged bytes: a
/// journal record or snapshot not yet renamed into place, or staged bytes cut short.
/// No record names any of them.
async fn remove_unfinished<F: Fs>(fs: &F, layout: &Layout, writer: WriterId) -> Result<()> {
    let mut removed = false;
    for path in tmp_files(fs, layout, writer).await? {
        if whole_blob(fs, &path).await?.is_none() {
            fs.remove_file(&path).await?;
            removed = true;
        }
    }
    if removed {
        fs.sync(&layout.tmp(writer)).await?;
    }
    Ok(())
}

/// Move the staged bytes a crash left in this writer's `tmp/` into the store, logging
/// each before moving it so that no blob enters the store unlogged.
async fn keep_staged<F: Fs>(
    fs: &F,
    layout: &Layout,
    log: &mut LogWriter,
) -> Result<Option<Recovered>> {
    let mut staged = Vec::new();
    for path in tmp_files(fs, layout, log.writer()).await? {
        if let Some((blob, len)) = whole_blob(fs, &path).await? {
            staged.push((path, blob, len));
        }
    }
    if staged.is_empty() {
        return Ok(None);
    }
    let intent = log.new_intent();
    let added = staged
        .iter()
        .map(|&(_, blob, len)| Kind::BlobAdded { blob, len });
    let entries: Vec<Entry> = std::iter::once(Kind::INTENT)
        .chain(added)
        .map(|kind| log.stamp(intent, kind))
        .collect();
    log.append(fs, layout, &entries).await?;
    for (path, blob, _) in &staged {
        blobs::displace(fs, layout, path, *blob).await?;
    }
    fs.sync(&layout.tmp(log.writer())).await?;
    Ok(Some(Recovered {
        intent,
        paths: Vec::new(),
        outcome: Outcome::RolledBack,
        kept: staged.into_iter().map(|(_, blob, _)| blob).collect(),
        stayed: Vec::new(),
        conflict: None,
    }))
}

async fn tmp_files<F: Fs>(fs: &F, layout: &Layout, writer: WriterId) -> Result<Vec<RelPath>> {
    let dir = layout.tmp(writer);
    if fs.metadata(&dir).await?.is_none() {
        return Ok(Vec::new());
    }
    let mut files = Vec::new();
    for entry in fs.list(&dir).await? {
        if entry.kind == FileKind::File {
            files.push(dir.join(&entry.name)?);
        }
    }
    Ok(files)
}

/// The blob and length of a staged file whose bytes hash to its name.
async fn whole_blob<F: Fs>(fs: &F, path: &RelPath) -> Result<Option<(BlobId, u64)>> {
    let Some(Ok(named)) = path.name().map(str::parse::<BlobId>) else {
        return Ok(None);
    };
    let (blob, len) = hash_file(fs, path).await?;
    Ok((blob == named).then_some((blob, len)))
}

/// One intent's effects in progress: `journal/<writer>/<intent counter>.json`.
#[derive(Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Record {
    pub intent: IntentId,
    /// The intent's own entries, its `Intent` entry first, which the log gains once
    /// every step finishes.
    #[serde(with = "json_texts")]
    pub entries: Vec<Entry>,
    pub steps: Vec<StepRecord>,
}

impl Record {
    fn paths(&self) -> Vec<RelPath> {
        self.steps.iter().flat_map(|s| s.step.paths()).collect()
    }

    fn versions(&self) -> impl Iterator<Item = Version> + '_ {
        let steps = self.steps.iter().flat_map(|step| &step.entries);
        self.entries.iter().chain(steps).map(|entry| entry.version)
    }

    fn label(&self) -> Option<String> {
        self.entries.iter().find_map(|entry| match &entry.kind {
            Kind::Intent { label, .. } => label.clone(),
            _ => None,
        })
    }
}

/// One step of an intent, with the entries the log gains once it finishes.
#[derive(Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct StepRecord {
    pub step: Step,
    #[serde(with = "json_texts")]
    pub entries: Vec<Entry>,
}

fn record_path(layout: &Layout, writer: WriterId, intent: IntentId) -> RelPath {
    layout
        .journal(writer)
        .join(&format!("{}.json", intent.counter))
        .expect("a record name is one path component")
}

/// Record an intent's effects durably before the first step. The record is written
/// under the writer's `tmp/` directory and renamed into the journal, so the journal
/// never holds part of a record.
pub(crate) async fn write<F: Fs>(
    fs: &F,
    layout: &Layout,
    writer: WriterId,
    record: &Record,
) -> Result<()> {
    let bytes = serde_json::to_vec(record).expect("a record has only string map keys");
    let tmp = layout.tmp(writer);
    let unfinished = tmp.join(&format!("journal-{}.json", record.intent.counter))?;
    ensure_dir(fs, &tmp).await?;
    fs.create(&unfinished, &bytes).await?;
    fs.sync(&unfinished).await?;
    let dir = layout.journal(writer);
    ensure_dir(fs, &dir).await?;
    fs.rename(&unfinished, &record_path(layout, writer, record.intent))
        .await?;
    fs.sync(&dir).await
}

async fn clear<F: Fs>(fs: &F, layout: &Layout, writer: WriterId, intent: IntentId) -> Result<()> {
    fs.remove_file(&record_path(layout, writer, intent)).await?;
    fs.sync(&layout.journal(writer)).await
}

/// The writer's records by intent, in the order of their intents, each decoded or
/// why it does not decode.
async fn read<F: Fs>(
    fs: &F,
    layout: &Layout,
    writer: WriterId,
) -> Result<Vec<(IntentId, Result<Record>)>> {
    let dir = layout.journal(writer);
    if fs.metadata(&dir).await?.is_none() {
        return Ok(Vec::new());
    }
    let mut records = Vec::new();
    for entry in fs.list(&dir).await? {
        let Some(counter) = entry.name.strip_suffix(".json").and_then(canonical_u64) else {
            continue;
        };
        let path = dir.join(&entry.name)?;
        let bytes = fs.read(&path).await?;
        let record = serde_json::from_slice(&bytes).map_err(|error| Error::Corrupt {
            path,
            reason: error.to_string(),
        });
        records.push((IntentId::new(writer, counter), record));
    }
    records.sort_by_key(|(intent, _)| intent.counter);
    Ok(records)
}

#[cfg(test)]
mod tests {
    use pollster::block_on;

    use super::*;
    use crate::effects::{Precondition, Stored};
    use crate::error::Mismatch;
    use crate::fs::{fingerprint, MemFs};
    use crate::ids::{EntityId, Version};
    use crate::log::testing::{append_unknown, logged, reopen, well_formed, Session};
    use crate::value::Value;

    const WRITER: WriterId = WriterId::from_u128(0xc);
    const INTENT: IntentId = IntentId::new(WRITER, 4);

    fn path(text: &str) -> RelPath {
        RelPath::new(text).unwrap()
    }

    fn library(files: &[(&str, &[u8])]) -> MemFs {
        let fs = MemFs::new();
        for (name, bytes) in files {
            block_on(fs.create(&path(name), bytes)).unwrap();
        }
        fs
    }

    fn contents(fs: &MemFs, path: &RelPath) -> Vec<u8> {
        fs.files()[path].clone()
    }

    fn save_record(layout: &Layout, fs: &MemFs, new: &[u8], old: &[u8]) -> Record {
        block_on(blobs::stage(fs, layout, WRITER, new)).unwrap();
        let entity = EntityId::new(WRITER, 1);
        Record {
            intent: INTENT,
            entries: vec![Entry {
                version: Version::new(8, WRITER),
                intent: INTENT,
                kind: Kind::INTENT,
            }],
            steps: vec![StepRecord {
                step: Step::Save {
                    path: path("song"),
                    new: Stored::of(new),
                    old: Some(Stored::of(old)),
                },
                entries: vec![Entry {
                    version: Version::new(9, WRITER),
                    intent: INTENT,
                    kind: Kind::Field {
                        entity,
                        name: "content".into(),
                        value: Some(Value::Blob(BlobId::of(new))),
                        prior: None,
                    },
                }],
            }],
        }
    }

    fn all_entries(record: &Record) -> Vec<Entry> {
        let steps = record.steps.iter().flat_map(|step| &step.entries);
        record.entries.iter().chain(steps).cloned().collect()
    }

    #[test]
    fn an_interrupted_save_whose_file_is_unchanged_is_finished() {
        let layout = Layout::default();
        let fs = library(&[("song", b"old")]);
        let record = save_record(&layout, &fs, b"new", b"old");
        block_on(write(&fs, &layout, WRITER, &record)).unwrap();
        let mut log = reopen(&fs, WRITER);
        let recovered = block_on(recover(&fs, &layout, &mut log)).unwrap();
        assert_eq!(
            recovered,
            [Recovered {
                intent: INTENT,
                paths: vec![path("song")],
                outcome: Outcome::Finished,
                kept: vec![],
                stayed: vec![],
                conflict: None,
            }]
        );
        assert_eq!(contents(&fs, &path("song")), b"new");
        assert_eq!(contents(&fs, &layout.blob(BlobId::of(b"old"))), b"old");
        assert_eq!(logged(&fs, WRITER), all_entries(&record));
        assert!(block_on(read(&fs, &layout, WRITER)).unwrap().is_empty());
    }

    #[test]
    fn an_interrupted_save_whose_file_changed_since_is_rolled_back_and_keeps_its_bytes() {
        let layout = Layout::default();
        let fs = library(&[("song", b"old")]);
        let record = save_record(&layout, &fs, b"new", b"old");
        block_on(write(&fs, &layout, WRITER, &record)).unwrap();
        block_on(fs.remove_file(&path("song"))).unwrap();
        block_on(fs.create(&path("song"), b"theirs")).unwrap();
        let mut log = reopen(&fs, WRITER);
        let found = block_on(fingerprint(&fs, &path("song"), false)).unwrap();
        let recovered = block_on(recover(&fs, &layout, &mut log)).unwrap();
        let new = BlobId::of(b"new");
        let mismatch = Mismatch {
            path: path("song"),
            expected: Precondition::Holds(BlobId::of(b"old")),
            found,
        };
        assert_eq!(
            recovered,
            [Recovered {
                intent: INTENT,
                paths: vec![path("song")],
                outcome: Outcome::RolledBack,
                kept: vec![new],
                stayed: vec![path("song")],
                conflict: Some(Conflict::Changed(Box::new(mismatch))),
            }]
        );
        assert_eq!(contents(&fs, &path("song")), b"theirs");
        assert_eq!(contents(&fs, &layout.blob(new)), b"new");
        let kinds: Vec<Kind> = logged(&fs, WRITER).into_iter().map(|e| e.kind).collect();
        assert_eq!(kinds, [Kind::INTENT, Kind::BlobAdded { blob: new, len: 3 }]);
    }

    #[test]
    fn recovery_stamps_its_entries_past_every_version_the_record_holds() {
        let layout = Layout::default();
        let fs = library(&[("a", b"a"), ("song", b"theirs")]);
        let mut session = Session::open(&fs, WRITER);
        let entity = session.log.new_entity();
        session.act(vec![Kind::Create { entity }]);
        block_on(blobs::stage(&fs, &layout, WRITER, b"new")).unwrap();
        let entry = |lamport, kind| Entry {
            version: Version::new(lamport, WRITER),
            intent: INTENT,
            kind,
        };
        let field = |name: &str, value: Value| Kind::Field {
            entity,
            name: name.into(),
            value: Some(value),
            prior: None,
        };
        let record = Record {
            intent: INTENT,
            entries: vec![entry(3, Kind::INTENT)],
            steps: vec![
                StepRecord {
                    step: Step::Move {
                        from: path("a"),
                        to: path("b"),
                    },
                    entries: vec![entry(4, field("path", Value::Text("b".into())))],
                },
                StepRecord {
                    step: Step::Save {
                        path: path("song"),
                        new: Stored::of(b"new"),
                        old: Some(Stored::of(b"old")),
                    },
                    entries: vec![entry(5, field("content", Value::Blob(BlobId::of(b"new"))))],
                },
            ],
        };
        block_on(write(&fs, &layout, WRITER, &record)).unwrap();

        let recovered = block_on(recover(&fs, &layout, &mut reopen(&fs, WRITER))).unwrap();
        assert_eq!(recovered[0].outcome, Outcome::Partial);
        well_formed(&logged(&fs, WRITER)).unwrap();
    }

    #[test]
    fn a_recovery_repeated_before_its_record_is_cleared_logs_nothing_again() {
        let layout = Layout::default();
        let fs = library(&[("song", b"theirs")]);
        let record = save_record(&layout, &fs, b"new", b"old");
        block_on(write(&fs, &layout, WRITER, &record)).unwrap();
        block_on(recover(&fs, &layout, &mut reopen(&fs, WRITER))).unwrap();
        let once = logged(&fs, WRITER);
        block_on(write(&fs, &layout, WRITER, &record)).unwrap();

        let recovered = block_on(recover(&fs, &layout, &mut reopen(&fs, WRITER))).unwrap();
        assert_eq!(recovered[0].outcome, Outcome::RolledBack);
        assert_eq!(recovered[0].kept, [BlobId::of(b"new")]);
        assert!(
            matches!(&recovered[0].conflict, Some(Conflict::Changed(m)) if m.path == path("song")),
            "{:?}",
            recovered[0].conflict
        );
        assert_eq!(logged(&fs, WRITER), once);
        well_formed(&once).unwrap();
    }

    #[test]
    fn bytes_staged_but_never_journaled_are_kept_as_blobs() {
        let layout = Layout::default();
        let fs = library(&[]);
        block_on(blobs::stage(&fs, &layout, WRITER, b"unsaved")).unwrap();
        let mut log = reopen(&fs, WRITER);
        let recovered = block_on(recover(&fs, &layout, &mut log)).unwrap();
        let blob = BlobId::of(b"unsaved");
        assert_eq!(recovered.len(), 1);
        assert_eq!(
            (recovered[0].outcome, &recovered[0].kept),
            (Outcome::RolledBack, &vec![blob])
        );
        assert_eq!(contents(&fs, &layout.blob(blob)), b"unsaved");
        assert!(logged(&fs, WRITER)
            .iter()
            .any(|e| e.kind == Kind::BlobAdded { blob, len: 7 }));
    }

    #[test]
    fn recovery_removes_what_a_crash_left_unfinished_in_tmp() {
        let layout = Layout::default();
        let fs = library(&[]);
        let tmp = layout.tmp(WRITER);
        block_on(ensure_dir(&fs, &tmp)).unwrap();
        let snapshot = format!("snapshot-{}.json", BlobId::of(b"{}"));
        let cut_short = BlobId::of(b"whole").to_string();
        for (name, bytes) in [
            (snapshot.as_str(), &b"{}"[..]),
            ("journal-3.json", b"{"),
            (&cut_short, b"who"),
        ] {
            block_on(fs.create(&tmp.join(name).unwrap(), bytes)).unwrap();
        }
        block_on(blobs::stage(&fs, &layout, WRITER, b"kept")).unwrap();

        let recovered = block_on(recover(&fs, &layout, &mut reopen(&fs, WRITER))).unwrap();
        let kept: Vec<BlobId> = recovered.iter().flat_map(|r| r.kept.clone()).collect();
        assert_eq!(kept, [BlobId::of(b"kept")]);
        let left: Vec<RelPath> = fs
            .files()
            .into_keys()
            .filter(|p| p.starts_with(&tmp))
            .collect();
        assert_eq!(left, []);
        let added: Vec<Kind> = logged(&fs, WRITER)
            .into_iter()
            .map(|entry| entry.kind)
            .filter(|kind| matches!(kind, Kind::BlobAdded { .. }))
            .collect();
        let blob = BlobId::of(b"kept");
        assert_eq!(added, [Kind::BlobAdded { blob, len: 4 }]);
    }

    #[test]
    fn recovery_with_nothing_to_do_writes_nothing() {
        let layout = Layout::default();
        let fs = library(&[("song", b"old")]);
        let mut log = reopen(&fs, WRITER);
        assert_eq!(block_on(recover(&fs, &layout, &mut log)).unwrap(), []);
        assert_eq!(fs.mutations(), 1);
    }

    #[test]
    fn a_read_only_writer_reports_its_journal_as_pending_and_changes_nothing() {
        let layout = Layout::default();
        let fs = library(&[("song", b"old")]);
        let record = save_record(&layout, &fs, b"new", b"old");
        block_on(write(&fs, &layout, WRITER, &record)).unwrap();
        let unreadable = layout.journal(WRITER).join("9.json").unwrap();
        block_on(fs.create(&unreadable, b"{\"intent\"")).unwrap();
        append_unknown(&fs, WRITER, 1);
        let (files, mutations) = (fs.files(), fs.mutations());
        let mut log = reopen(&fs, WRITER);
        let pending = |intent, paths, outcome| Recovered {
            intent,
            paths,
            outcome,
            kept: vec![],
            stayed: vec![],
            conflict: None,
        };
        assert_eq!(
            block_on(recover(&fs, &layout, &mut log)).unwrap(),
            [
                pending(INTENT, vec![path("song")], Outcome::Pending),
                pending(IntentId::new(WRITER, 9), vec![], Outcome::Unreadable),
            ]
        );
        assert_eq!((fs.files(), fs.mutations()), (files, mutations));
    }

    #[test]
    fn a_record_that_does_not_decode_makes_the_writer_read_only() {
        let layout = Layout::default();
        let fs = library(&[("song", b"old")]);
        let record = save_record(&layout, &fs, b"new", b"old");
        block_on(write(&fs, &layout, WRITER, &record)).unwrap();
        let unreadable = layout.journal(WRITER).join("9.json").unwrap();
        let unknown_step = format!(
            r#"{{"intent":"{INTENT}","entries":[],"steps":[{{"step":{{"copy":{{}}}},"entries":[]}}]}}"#
        );
        for bytes in [&b"{\"intent\""[..], unknown_step.as_bytes()] {
            block_on(fs.create(&unreadable, bytes)).unwrap();
            let (files, mutations) = (fs.files(), fs.mutations());
            let mut log = reopen(&fs, WRITER);
            let outcomes: Vec<Outcome> = block_on(recover(&fs, &layout, &mut log))
                .unwrap()
                .into_iter()
                .map(|recovered| recovered.outcome)
                .collect();
            assert_eq!(outcomes, [Outcome::Pending, Outcome::Unreadable]);
            assert!(log.read_only().is_some());
            assert_eq!((fs.files(), fs.mutations()), (files, mutations));
            block_on(fs.remove_file(&unreadable)).unwrap();
        }
    }

    #[test]
    fn a_record_is_json_naming_its_intent_entries_and_steps() {
        let writer = WriterId::from_u128(1);
        let intent = IntentId::new(writer, 2);
        let record = Record {
            intent,
            entries: vec![Entry {
                version: Version::new(2, writer),
                intent,
                kind: Kind::INTENT,
            }],
            steps: vec![StepRecord {
                step: Step::Move {
                    from: path("a"),
                    to: path("b"),
                },
                entries: vec![Entry {
                    version: Version::new(3, writer),
                    intent,
                    kind: Kind::Field {
                        entity: EntityId::new(writer, 4),
                        name: "path".into(),
                        value: Some(Value::Text("b".into())),
                        prior: Some(Value::Text("a".into())),
                    },
                }],
            }],
        };
        let w = "00000000000000000000000000000001";
        let json = format!(
            concat!(
                r#"{{"intent":"{w}:2","#,
                r#""entries":["{{\"version\":\"2@{w}\",\"intent\":\"{w}:2\",\"kind\":\"intent\"}}"],"#,
                r#""steps":[{{"step":{{"move":{{"from":"a","to":"b"}}}},"#,
                r#""entries":["{{\"version\":\"3@{w}\",\"intent\":\"{w}:2\","#,
                r#"\"kind\":\"field\",\"entity\":\"{w}:4\",\"name\":\"path\","#,
                r#"\"value\":{{\"text\":\"b\"}},\"prior\":{{\"text\":\"a\"}}}}"]}}]}}"#
            ),
            w = w
        );
        assert_eq!(serde_json::to_string(&record).unwrap(), json);
        assert_eq!(serde_json::from_str::<Record>(&json).unwrap(), record);
    }
}
