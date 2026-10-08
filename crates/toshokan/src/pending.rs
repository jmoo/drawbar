//! Pending records: the journal of multi-step effects, in `pending/<nonce>.json`.
//!
//! A record is written before an intent's first file effect and removed after its
//! entry is durable, only by its owner. The folder may be adversarial: a record is
//! acted on only when it is chained to the head the owner's local root recorded
//! and every path it names is a library path.

use serde::{Deserialize, Serialize};
use thiserror::Error as ThisError;

use crate::effects::{EffectPlan, EffectStep, FileEnd, Moves};
use crate::error::Result;
use crate::flow::{self, fold, ok, Flow};
use crate::ids::{EntryHash, Nonce, WriterId};
use crate::io::{Kind, Root, Task};
use crate::layout::{is_swap_file, Layout};
use crate::line::Line;
use crate::path::RelPath;
use crate::schema::Raw;

/// The longest record a reader reads.
pub const MAX_RECORD: u64 = 16 << 20;

#[derive(Clone, PartialEq, Eq, Debug)]
pub struct PendingRecord {
    pub writer: WriterId,
    /// The intent's entry as planned, chained after the writer's head when the
    /// record was written. Its file ops and displaced bytes are replaced by what
    /// the steps did.
    pub entry: Line,
    /// The intent's label, shown when another writer reports the record.
    pub label: String,
    pub steps: Vec<EffectStep>,
    pub files: Vec<FileEnd>,
    /// How the steps move files, whoever carries them out.
    pub moves: Moves,
}

#[derive(Serialize, Deserialize)]
struct Wire {
    writer: WriterId,
    entry: Raw,
    label: String,
    steps: Vec<EffectStep>,
    files: Vec<FileEnd>,
    #[serde(default, skip_serializing_if = "Moves::renames")]
    moves: Moves,
}

#[derive(ThisError, Clone, PartialEq, Eq, Debug)]
#[error("not a pending record: {reason}")]
pub struct NotRecord {
    pub reason: String,
}

impl PendingRecord {
    pub fn new(writer: WriterId, label: &str, entry: Line, plan: &EffectPlan) -> Self {
        Self {
            writer,
            entry,
            label: label.to_owned(),
            steps: plan.steps.clone(),
            files: plan.files.clone(),
            moves: plan.moves,
        }
    }

    /// The writer's head when the record was written.
    pub fn after(&self) -> EntryHash {
        self.entry.prev()
    }

    /// Refuses bytes that are not a record, or longer than [`MAX_RECORD`].
    pub fn decode(bytes: &[u8]) -> std::result::Result<Self, NotRecord> {
        let not = |reason: String| NotRecord { reason };
        if bytes.len() as u64 > MAX_RECORD {
            return Err(not(format!("longer than {MAX_RECORD} bytes")));
        }
        let wire: Wire = serde_json::from_slice(bytes).map_err(|e| not(e.to_string()))?;
        let entry = Line::seal(wire.entry.as_str().to_owned()).map_err(|e| not(e.to_string()))?;
        Ok(Self {
            writer: wire.writer,
            entry,
            label: wire.label,
            steps: wire.steps,
            files: wire.files,
            moves: wire.moves,
        })
    }

    pub fn encode(&self) -> Vec<u8> {
        let wire = Wire {
            writer: self.writer,
            entry: Raw::new(self.entry.json()).expect("a line holds a JSON object"),
            label: self.label.clone(),
            steps: self.steps.clone(),
            files: self.files.clone(),
            moves: self.moves,
        };
        serde_json::to_vec(&wire).expect("a record serializes")
    }

    /// Whether every path the record names is a library path. A record that is
    /// not is ignored and reported.
    pub fn is_confined(&self, layout: &Layout) -> bool {
        let steps = self.steps.iter().flat_map(EffectStep::library_paths);
        let ends = self.files.iter().filter_map(|end| end.path.as_ref());
        steps
            .chain(ends)
            .all(|path| layout.check_library_path(path).is_ok())
    }

    /// The library paths the effect touches, sorted.
    pub fn paths(&self) -> Vec<RelPath> {
        let mut paths: Vec<RelPath> = self
            .steps
            .iter()
            .flat_map(EffectStep::library_paths)
            .cloned()
            .collect();
        paths.sort();
        paths.dedup();
        paths
    }
}

/// The pending records found in one writer's directory.
#[derive(Clone, PartialEq, Eq, Debug, Default)]
pub struct Records {
    /// Records that decode and name the directory's writer, by name.
    pub records: Vec<(Nonce, PendingRecord)>,
    /// Everything else in the directory but markers.
    pub unreadable: Vec<RelPath>,
}

/// The record and step a file of a pending directory marks as started, when its
/// name is a marker's.
pub(crate) fn marker(name: &str) -> Option<(Nonce, usize)> {
    let (record, step) = name.split_once('.')?;
    let marked = (record.parse().ok()?, step.parse().ok()?);
    (format!("{}.{}", marked.0, marked.1) == name).then_some(marked)
}

/// Every pending record of `writer`. Reads only, each read bounded.
pub fn read_all(layout: &Layout, writer: WriterId) -> Task<'static, Result<Records>> {
    let layout = layout.clone();
    let dir = layout.pending_dir(writer);
    flow::list(Root::Folder, &dir)
        .and_then(move |entries| {
            fold(
                entries.into_iter(),
                Records::default(),
                move |mut found, entry| {
                    let path = dir
                        .join(&entry.name)
                        .expect("a listed name is one component");
                    let name = entry
                        .name
                        .strip_suffix(".json")
                        .and_then(|n| n.parse().ok());
                    let skipped = marker(&entry.name).is_some() || is_swap_file(&entry.name);
                    match (entry.kind, name) {
                        (Kind::File, _) if skipped => ok(found),
                        (Kind::File, Some(name)) => {
                            read(path.clone(), writer).map_ok(move |record| {
                                match record {
                                    Some(record) => found.records.push((name, record)),
                                    None => found.unreadable.push(path),
                                }
                                found
                            })
                        }
                        _ => {
                            found.unreadable.push(path);
                            ok(found)
                        }
                    }
                },
            )
        })
        .task()
}

/// `writer`'s record `name`, when it is there and decodes.
pub fn read_one(
    layout: &Layout,
    writer: WriterId,
    name: Nonce,
) -> Task<'static, Result<Option<PendingRecord>>> {
    read(layout.pending(writer, name), writer).task()
}

fn read<'a>(path: RelPath, writer: WriterId) -> Flow<'a, Result<Option<PendingRecord>>> {
    flow::stat(Root::Folder, &path).and_then(move |meta| match meta {
        Some(meta) if meta.kind == Kind::File && meta.len <= MAX_RECORD => {
            flow::read_all(Root::Folder, &path, MAX_RECORD).map_ok(move |bytes| {
                PendingRecord::decode(&bytes)
                    .ok()
                    .filter(|record| record.writer == writer)
            })
        }
        _ => ok(None),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ids::EntityId;

    fn path(text: &str) -> RelPath {
        RelPath::new(text).unwrap()
    }

    fn record(steps: Vec<EffectStep>) -> PendingRecord {
        let entry = Line::seal(format!(
            r#"{{"prev":"{}","kind":"x"}}"#,
            EntryHash::from_u128(9)
        ))
        .unwrap();
        PendingRecord {
            writer: WriterId::from_u128(1),
            entry,
            label: "Save".into(),
            steps,
            files: vec![FileEnd {
                entity: EntityId::from_u128(2),
                path: Some(path("a/f")),
                done_after: 2,
                pin: false,
            }],
            moves: Moves::Rename,
        }
    }

    #[test]
    fn a_record_round_trips_and_keeps_its_entry_line() {
        let record = record(vec![
            EffectStep::ToTrash {
                path: path("a/f"),
                item: Nonce::from_u128(3),
            },
            EffectStep::Place {
                staged: Nonce::from_u128(4),
                path: path("a/f"),
            },
        ]);
        let decoded = PendingRecord::decode(&record.encode()).unwrap();
        assert_eq!(decoded, record);
        assert_eq!(decoded.after(), EntryHash::from_u128(9));
        assert_eq!(decoded.paths(), [path("a/f")]);
    }

    #[test]
    fn a_record_is_written_with_its_steps_tagged() {
        let text =
            String::from_utf8(record(vec![EffectStep::RemoveDir { path: path("d") }]).encode())
                .unwrap();
        assert!(
            text.contains(r#"{"step":"remove_dir","path":"d"}"#),
            "{text}"
        );
    }

    #[test]
    fn a_record_that_moves_by_copying_says_so_and_one_that_renames_says_nothing() {
        let renaming = record(vec![]);
        let copying = PendingRecord {
            moves: Moves::Copy,
            ..record(vec![])
        };
        let text = |record: &PendingRecord| String::from_utf8(record.encode()).unwrap();
        assert!(!text(&renaming).contains("moves"), "{}", text(&renaming));
        assert!(
            text(&copying).ends_with(r#","moves":"copy"}"#),
            "{}",
            text(&copying)
        );
        assert_eq!(PendingRecord::decode(&copying.encode()).unwrap(), copying);
    }

    #[test]
    fn only_a_nonce_and_a_step_number_name_a_marker() {
        let nonce = Nonce::from_u128(3);
        assert_eq!(marker(&format!("{nonce}.12")), Some((nonce, 12)));
        for name in [
            format!("{nonce}.json"),
            format!("{nonce}.+1"),
            format!("{nonce}.01"),
            format!("{nonce}."),
            "x.1".to_owned(),
            nonce.to_string(),
        ] {
            assert_eq!(marker(&name), None, "{name}");
        }
    }

    #[test]
    fn a_record_naming_toshokan_files_or_the_folder_is_not_confined() {
        let layout = Layout::new(".t").unwrap();
        assert!(record(vec![]).is_confined(&layout));
        for bad in [".t/writers/x", ".t", ""] {
            let rename = EffectStep::Rename {
                from: path("a"),
                to: path(bad),
            };
            assert!(!record(vec![rename]).is_confined(&layout), "{bad:?}");
        }
        let mut escaping = record(vec![]);
        escaping.files[0].path = Some(path(".t/x"));
        assert!(!escaping.is_confined(&layout));
    }

    #[test]
    fn every_truncation_or_flipped_byte_of_a_record_decodes_or_is_refused() {
        let layout = Layout::new(".t").unwrap();
        let good = record(vec![EffectStep::Rename {
            from: path("a"),
            to: path("b"),
        }])
        .encode();
        for i in 0..good.len() {
            let mut flipped = good.clone();
            flipped[i] ^= 0x20;
            for bytes in [&good[..i], &flipped[..]] {
                if let Ok(record) = PendingRecord::decode(bytes) {
                    record.is_confined(&layout);
                    record.paths();
                }
            }
        }
    }

    #[test]
    fn malformed_records_are_refused() {
        let good = String::from_utf8(record(vec![]).encode()).unwrap();
        let unchained = good.replace(r#""prev""#, r#""before""#);
        let bad_path = good.replace(r#""a/f""#, r#""../f""#);
        for bytes in [
            b"".to_vec(),
            b"[]".to_vec(),
            unchained.into_bytes(),
            bad_path.into_bytes(),
            vec![b' '; MAX_RECORD as usize + 1],
        ] {
            assert!(
                PendingRecord::decode(&bytes).is_err(),
                "{:?}",
                String::from_utf8_lossy(&bytes[..bytes.len().min(80)])
            );
        }
    }
}
