//! The journal of effects in progress, and recovery from it at open.
//!
//! A writer records each file effect under `journal/<writer>/` before its first step
//! and clears the record after its last, so a crash leaves a record of exactly the
//! effects it interrupted. The record carries the intent's log entries, stamped
//! before the first step, so recovery appends the same entries the intent would have.

use serde::{Deserialize, Serialize};

use crate::blobs;
use crate::effects::{Ran, Step};
use crate::error::{Error, Result};
use crate::fs::{ensure_dir, hash_file, FileKind, Fs, RelPath};
use crate::ids::{canonical_u64, IntentId, WriterId};
use crate::layout::Layout;
use crate::log::{json_texts, Entry, Kind, LogWriter};
use crate::value::BlobId;

/// How recovery settled an interrupted effect.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Outcome {
    Finished,
    RolledBack,
}

/// An effect a crash interrupted, and how recovery settled it.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Recovered {
    pub intent: IntentId,
    /// The library paths the effect touched.
    pub paths: Vec<RelPath>,
    pub outcome: Outcome,
    /// Bytes the effect had written or displaced that recovery moved into the blob
    /// store and logged, so a rolled-back save keeps its new contents.
    pub kept: Vec<BlobId>,
}

/// Finish or roll back each of this writer's journaled effects, then clear the
/// journal. Bytes a crash left in the writer's `tmp/` before journaling them are
/// moved into the blob store and reported under a new intent. Recovery is itself
/// safe to interrupt and repeat.
pub async fn recover<F: Fs>(
    fs: &F,
    layout: &Layout,
    log: &mut LogWriter,
) -> Result<Vec<Recovered>> {
    let records = read(fs, layout, log.writer()).await?;
    if records.is_empty() && log.read_only().is_some() {
        return Ok(Vec::new());
    }
    log.writable()?;
    let mut recovered = Vec::new();
    for record in records {
        let ran = settle(fs, layout, log, &record).await?;
        recovered.push(Recovered {
            intent: record.intent,
            paths: record.step.paths(),
            outcome: ran.outcome(),
            kept: ran.kept(),
        });
    }
    recovered.extend(keep_staged(fs, layout, log).await?);
    Ok(recovered)
}

/// Run `record`'s step to its end, append the entries it leaves, and clear the record.
/// A step that finished appends the record's entries; one the files no longer allow
/// appends only the bytes it kept, under a bare `Intent` entry, so the intent rolls
/// back whole.
pub(crate) async fn settle<F: Fs>(
    fs: &F,
    layout: &Layout,
    log: &mut LogWriter,
    record: &Record,
) -> Result<Ran> {
    let ran = record.step.run(fs, layout, log.writer()).await?;
    let entries: Vec<Entry> = match &ran {
        Ran::Finished(_) => record.entries.clone(),
        Ran::Conflict { kept, .. } if kept.is_empty() => Vec::new(),
        Ran::Conflict { kept, .. } => std::iter::once(Kind::INTENT)
            .chain(kept.iter().map(|stored| stored.added()))
            .map(|kind| log.stamp(record.intent, kind))
            .collect(),
    };
    log.append(fs, layout, &entries).await?;
    clear(fs, layout, log.writer(), record.intent).await?;
    Ok(ran)
}

/// Move the blobs a crash left in this writer's `tmp/` into the store, logging each
/// before moving it so that no blob enters the store unlogged. Other files there are
/// a compaction's, which rewrites them.
async fn keep_staged<F: Fs>(
    fs: &F,
    layout: &Layout,
    log: &mut LogWriter,
) -> Result<Option<Recovered>> {
    let dir = layout.tmp(log.writer());
    if fs.metadata(&dir).await?.is_none() {
        return Ok(None);
    }
    let mut staged = Vec::new();
    for entry in fs.list(&dir).await? {
        if entry.kind == FileKind::File && entry.name.parse::<BlobId>().is_ok() {
            let path = dir.join(&entry.name)?;
            let (blob, len) = hash_file(fs, &path).await?;
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
    fs.sync(&dir).await?;
    Ok(Some(Recovered {
        intent,
        paths: Vec::new(),
        outcome: Outcome::RolledBack,
        kept: staged.into_iter().map(|(_, blob, _)| blob).collect(),
    }))
}

/// One effect in progress: `journal/<writer>/<intent counter>.json`.
#[derive(Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Record {
    pub intent: IntentId,
    pub step: Step,
    /// What the log gains when the step finishes.
    #[serde(with = "json_texts")]
    pub entries: Vec<Entry>,
}

fn record_path(layout: &Layout, writer: WriterId, intent: IntentId) -> RelPath {
    layout
        .journal(writer)
        .join(&format!("{}.json", intent.counter))
        .expect("a record name is one path component")
}

/// Record an effect durably before its first step.
pub(crate) async fn write<F: Fs>(
    fs: &F,
    layout: &Layout,
    writer: WriterId,
    record: &Record,
) -> Result<()> {
    let dir = layout.journal(writer);
    ensure_dir(fs, &dir).await?;
    let path = record_path(layout, writer, record.intent);
    let bytes = serde_json::to_vec(record).expect("a record has only string map keys");
    fs.create(&path, &bytes).await?;
    fs.sync(&path).await?;
    fs.sync(&dir).await
}

async fn clear<F: Fs>(fs: &F, layout: &Layout, writer: WriterId, intent: IntentId) -> Result<()> {
    fs.remove_file(&record_path(layout, writer, intent)).await?;
    fs.sync(&layout.journal(writer)).await
}

/// The writer's records, in the order of their intents. A record that does not
/// decode is corrupt; one from a newer build is refused the same way.
async fn read<F: Fs>(fs: &F, layout: &Layout, writer: WriterId) -> Result<Vec<Record>> {
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
        let record: Record = serde_json::from_slice(&bytes).map_err(|error| Error::Corrupt {
            path,
            reason: error.to_string(),
        })?;
        records.push((counter, record));
    }
    records.sort_by_key(|(counter, _)| *counter);
    Ok(records.into_iter().map(|(_, record)| record).collect())
}

#[cfg(test)]
mod tests {
    use pollster::block_on;

    use super::*;
    use crate::effects::Stored;
    use crate::fs::MemFs;
    use crate::ids::{EntityId, Version};
    use crate::log::testing::{append_unknown, logged, reopen};
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
        }
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
            }]
        );
        assert_eq!(contents(&fs, &path("song")), b"new");
        assert_eq!(contents(&fs, &layout.blob(BlobId::of(b"old"))), b"old");
        assert_eq!(logged(&fs, WRITER), record.entries);
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
        let recovered = block_on(recover(&fs, &layout, &mut log)).unwrap();
        let new = BlobId::of(b"new");
        assert_eq!(
            recovered,
            [Recovered {
                intent: INTENT,
                paths: vec![path("song")],
                outcome: Outcome::RolledBack,
                kept: vec![new],
            }]
        );
        assert_eq!(contents(&fs, &path("song")), b"theirs");
        assert_eq!(contents(&fs, &layout.blob(new)), b"new");
        let kinds: Vec<Kind> = logged(&fs, WRITER).into_iter().map(|e| e.kind).collect();
        assert_eq!(kinds, [Kind::INTENT, Kind::BlobAdded { blob: new, len: 3 }]);
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
    fn recovery_with_nothing_to_do_writes_nothing() {
        let layout = Layout::default();
        let fs = library(&[("song", b"old")]);
        let mut log = reopen(&fs, WRITER);
        assert_eq!(block_on(recover(&fs, &layout, &mut log)).unwrap(), []);
        assert_eq!(fs.mutations(), 1);
    }

    #[test]
    fn a_read_only_writer_does_not_recover_its_journal() {
        let layout = Layout::default();
        let fs = library(&[("song", b"old")]);
        let record = save_record(&layout, &fs, b"new", b"old");
        block_on(write(&fs, &layout, WRITER, &record)).unwrap();
        append_unknown(&fs, WRITER, 1);
        let files = fs.files();
        let mut log = reopen(&fs, WRITER);
        let result = block_on(recover(&fs, &layout, &mut log));
        assert!(matches!(result, Err(Error::ReadOnly { .. })), "{result:?}");
        assert_eq!(fs.files(), files);
    }

    #[test]
    fn a_record_that_does_not_decode_is_corrupt() {
        let layout = Layout::default();
        let fs = library(&[]);
        let dir = layout.journal(WRITER);
        block_on(ensure_dir(&fs, &dir)).unwrap();
        let record = dir.join("1.json").unwrap();
        let unknown_step =
            format!(r#"{{"intent":"{INTENT}","step":{{"copy":{{}}}},"entries":[]}}"#);
        for bytes in [&b"{\"intent\""[..], unknown_step.as_bytes()] {
            block_on(fs.create(&record, bytes)).unwrap();
            let result = block_on(recover(&fs, &layout, &mut reopen(&fs, WRITER)));
            assert!(
                matches!(&result, Err(Error::Corrupt { path, .. }) if *path == record),
                "{result:?}"
            );
            block_on(fs.remove_file(&record)).unwrap();
        }
    }

    #[test]
    fn a_record_is_json_naming_its_intent_step_and_stamped_entries() {
        let writer = WriterId::from_u128(1);
        let intent = IntentId::new(writer, 2);
        let record = Record {
            intent,
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
        };
        let w = "00000000000000000000000000000001";
        let json = format!(
            concat!(
                r#"{{"intent":"{w}:2","step":{{"move":{{"from":"a","to":"b"}}}},"#,
                r#""entries":["{{\"version\":\"3@{w}\",\"intent\":\"{w}:2\","#,
                r#"\"kind\":\"field\",\"entity\":\"{w}:4\",\"name\":\"path\","#,
                r#"\"value\":{{\"text\":\"b\"}},\"prior\":{{\"text\":\"a\"}}}}"]}}"#
            ),
            w = w
        );
        assert_eq!(serde_json::to_string(&record).unwrap(), json);
        assert_eq!(serde_json::from_str::<Record>(&json).unwrap(), record);
    }
}
