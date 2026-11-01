//! What this build writes is what the fixtures hold: every file of every state
//! decodes to members this build knows and encodes back to the same bytes.

use std::collections::BTreeMap;

use toshokan::binding::Scan;
use toshokan::env::TestClock;
use toshokan::log::{Entry, EntryKind, Op};
use toshokan::pending::{cut_short, PendingRecord};
use toshokan::report::Start;
use toshokan::snapshot::Snapshot;
use toshokan::{EntryHash, RelPath, Root, WriterId};

use crate::fixture::{fixtures, layout, load, open, path, read_tree, Faulty, STATES, TAGS};

/// Every file of a state's tree `tree`, by its path in that tree.
fn tree(state: &str, tree: &str) -> BTreeMap<RelPath, Vec<u8>> {
    read_tree(&fixtures().join(state).join(tree))
}

/// The files directly in a writer's directory, of every state.
fn writer_files() -> Vec<(String, Vec<u8>)> {
    let writers = layout().writers();
    let mut found = Vec::new();
    for state in STATES {
        for (path, bytes) in tree(state, "folder") {
            let Some(under) = path.strip_prefix(&writers) else {
                continue;
            };
            if under.split('/').count() == 2 {
                found.push((format!("{state}/{path}"), bytes));
            }
        }
    }
    found
}

trait Under {
    fn strip_prefix(&self, prefix: &RelPath) -> Option<&str>;
}

impl Under for RelPath {
    fn strip_prefix(&self, prefix: &RelPath) -> Option<&str> {
        self.as_str()
            .strip_prefix(prefix.as_str())?
            .strip_prefix('/')
    }
}

/// The lines of a segment, each with its newline, without its seal marker.
fn lines(bytes: &[u8]) -> Vec<&[u8]> {
    let mut lines: Vec<&[u8]> = bytes.split_inclusive(|&b| b == b'\n').collect();
    if lines
        .last()
        .is_some_and(|line| line.starts_with(b"sealed\t"))
    {
        lines.pop();
    }
    lines
}

#[test]
fn every_line_decodes_to_members_this_build_knows_and_encodes_back_byte_for_byte() {
    let segments = writer_files().into_iter();
    let segments = segments.filter(|(name, _)| name.ends_with(".txt"));
    let mut count = 0;
    for (name, bytes) in segments {
        for line in lines(&bytes) {
            let shown = String::from_utf8_lossy(line);
            let entry = Entry::parse(line).unwrap_or_else(|e| panic!("{name}: {e}: {shown}"));
            assert!(
                !entry.unknown_members(),
                "{name}: unknown members in {shown}"
            );
            let kind = entry.kind();
            let ops = match kind {
                EntryKind::Unknown(_) => panic!("{name}: an unknown kind: {shown}"),
                EntryKind::Intent(logged) => &logged.ops[..],
                EntryKind::Bind(bound) => &bound.ops[..],
                EntryKind::Genesis(_) | EntryKind::Settle(_) => &[],
            };
            let unknown = ops.iter().find(|op| matches!(op, Op::Unknown(_)));
            assert_eq!(unknown, None, "{name}: an unknown op in {shown}");
            let written = Entry::encode(entry.prev(), entry.n(), entry.at(), kind.clone()).unwrap();
            assert_eq!(
                String::from_utf8_lossy(&written.to_bytes()),
                shown,
                "{name}: this build writes the entry otherwise"
            );
            count += 1;
        }
    }
    assert!(count > 30, "only {count} lines");
}

#[test]
fn every_segment_ends_with_a_whole_line_or_its_seal_marker() {
    for (name, bytes) in writer_files() {
        if !name.ends_with(".txt") {
            continue;
        }
        let lines: Vec<&[u8]> = bytes.split_inclusive(|&b| b == b'\n').collect();
        let last = lines.last().unwrap_or_else(|| panic!("{name} is empty"));
        assert!(last.ends_with(b"\n"), "{name} ends inside a line");
        if let Some(hash) = last.strip_prefix(b"sealed\t") {
            let before = lines[lines.len() - 2];
            assert!(
                before.ends_with(hash),
                "{name}: a seal marker names its last line"
            );
        }
    }
}

#[test]
fn every_snapshot_decodes_to_members_this_build_knows_and_encodes_back_byte_for_byte() {
    let snapshots = writer_files().into_iter();
    let snapshots = snapshots.filter(|(name, _)| name.contains("/snapshot-"));
    let mut count = 0;
    for (name, bytes) in snapshots {
        let snapshot = Snapshot::decode(&bytes).unwrap_or_else(|e| panic!("{name}: {e}"));
        assert!(
            snapshot.unknown.is_empty(),
            "{name}: {:?}",
            snapshot.unknown
        );
        assert!(snapshot.state.later.is_empty(), "{name}");
        assert!(snapshot.state.unknown.is_empty(), "{name}");
        assert!(snapshot.state.extended.is_empty(), "{name}");
        assert_eq!(
            String::from_utf8_lossy(&snapshot.encode()),
            String::from_utf8_lossy(&bytes),
            "{name}: this build writes the snapshot otherwise"
        );
        count += 1;
    }
    assert!(count > 0);
}

#[test]
fn every_pending_record_encodes_back_byte_for_byte_but_the_one_cut_short() {
    let mut records = Vec::new();
    for (path, bytes) in tree("pending", "folder") {
        let name = path.name().unwrap();
        if path.as_str().contains("/pending/") && name.ends_with(".json") {
            records.push((path, bytes));
        }
    }
    let mut cut = Vec::new();
    for (path, bytes) in &records {
        let Ok(record) = PendingRecord::decode(bytes) else {
            assert!(cut_short(bytes), "{path} does not decode");
            cut.push(path.as_str());
            continue;
        };
        assert_eq!(
            String::from_utf8_lossy(&record.encode()),
            String::from_utf8_lossy(bytes),
            "{path}: this build writes the record otherwise"
        );
    }
    assert_eq!(records.len(), 7);
    assert_eq!(
        cut,
        [".t/writers.2/895787111aaa1d835a18dbf36e14dada/pending/905c768ad49f146cb579d288df624bb6.json"]
    );
}

/// The genesis hash a writer's directory holds: its first line's, or its
/// snapshots'.
fn genesis(state: &str, writer: WriterId) -> Vec<EntryHash> {
    let dir = layout().writer(writer);
    let mut found = Vec::new();
    for (path, bytes) in tree(state, "folder") {
        if path.parent() != Some(dir.clone()) {
            continue;
        }
        if let Ok(snapshot) = Snapshot::decode(&bytes) {
            found.push(snapshot.genesis);
        }
        if let Some(entry) = lines(&bytes).first().and_then(|l| Entry::parse(l).ok()) {
            if entry.n() == 0 {
                found.push(entry.hash());
            }
        }
    }
    found.sort();
    found.dedup();
    found
}

#[test]
fn every_head_record_is_written_as_this_build_writes_it_under_its_writers_genesis() {
    let mut retired = 0;
    for state in STATES {
        for (path, bytes) in tree(state, "local") {
            if path.name() != Some("head.json") {
                continue;
            }
            let shown = String::from_utf8_lossy(&bytes);
            let record: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
            let writer: WriterId = record["writer"].as_str().unwrap().parse().unwrap();
            let head = record["head"].as_str().unwrap();
            let written = match record.get("retired") {
                None => format!(r#"{{"writer":"{writer}","head":"{head}"}}"#),
                Some(_) => {
                    retired += 1;
                    format!(r#"{{"writer":"{writer}","head":"{head}","retired":true}}"#)
                }
            };
            assert_eq!(shown, written, "{state}/{path}");
            let dir: EntryHash = path.parent().unwrap().as_str().parse().unwrap();
            assert_eq!(genesis(state, writer), [dir], "{state}/{path}");
        }
    }
    assert!(retired > 0, "the local state's forked writer is retired");
}

#[test]
fn every_identities_file_encodes_back_byte_for_byte() {
    let mut count = 0;
    for state in STATES {
        let local = tree(state, "local");
        let Some(bytes) = local.get(&path("identities.json")) else {
            continue;
        };
        count += 1;
        let scan = Scan::of_identities(bytes).unwrap_or_else(|| panic!("{state}"));
        assert!(!scan.files.is_empty(), "{state}");
        assert_eq!(
            String::from_utf8_lossy(&scan.identities()),
            String::from_utf8_lossy(bytes),
            "{state}"
        );
    }
    assert!(count > 0);
}

#[test]
fn compacting_one_writer_open_writes_the_snapshot_one_writer_compacted_holds() {
    let machine = load("one-writer-open");
    let backend = Faulty::new(&machine);
    let (mut library, _) = open(&backend, "fixture", 99, &TestClock::at(100_000));
    let compacted = library.compact().unwrap();
    let writer = library.writer_id().unwrap();
    let written = machine.folder.files(Root::Folder);
    let written = &written[&layout().snapshot(writer, compacted.snapshot)];
    let held = tree("one-writer-compacted", "folder");
    let held = held
        .iter()
        .find(|(path, _)| path.as_str().contains("/snapshot-"));
    assert_eq!(
        String::from_utf8_lossy(written),
        String::from_utf8_lossy(held.unwrap().1)
    );
}

#[test]
fn opening_the_local_state_continues_its_live_writer() {
    let local = tree("local", "local");
    let live: Vec<(WriterId, EntryHash)> = local
        .iter()
        .filter(|(path, _)| path.name() == Some("head.json"))
        .filter_map(|(_, bytes)| {
            let record: serde_json::Value = serde_json::from_slice(bytes).unwrap();
            let field = |name: &str| record[name].as_str().unwrap().to_owned();
            let live = record.get("retired").is_none();
            live.then(|| {
                (
                    field("writer").parse().unwrap(),
                    field("head").parse().unwrap(),
                )
            })
        })
        .collect();
    let [(writer, head)] = live[..] else {
        panic!("{live:?}");
    };

    let machine = load("local");
    let backend = Faulty::new(&machine);
    let (mut library, opened) = open(&backend, "local", 99, &TestClock::at(100_000));
    assert_eq!(opened.start, Start::Resumed(writer));
    let gone = library.view().entities()[0].id();
    let committed = library
        .intent("Continue")
        .add(gone, TAGS, "continued".into())
        .commit()
        .unwrap();
    assert_eq!(library.writer_id(), Some(writer), "no rekey");
    let segments = machine.folder.files(Root::Folder);
    let line = segments
        .iter()
        .filter(|(path, _)| path.starts_with(&layout().writer(writer)))
        .flat_map(|(_, bytes)| {
            lines(bytes)
                .into_iter()
                .map(<[u8]>::to_vec)
                .collect::<Vec<_>>()
        })
        .map(|line| Entry::parse(&line).unwrap())
        .find(|entry| entry.hash() == committed.intent)
        .expect("the intent is logged in the live writer's directory");
    assert_eq!(
        line.prev(),
        head,
        "it follows the head the local root recorded"
    );
}
