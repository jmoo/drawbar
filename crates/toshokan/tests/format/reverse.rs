//! What this build makes of files a newer build writes: the reverse cases under
//! `tests/fixtures/reverse`, each laid over the state its README names.

use std::cell::RefCell;
use std::collections::BTreeSet;
use std::rc::Rc;

use serde_json::{json, Value};
use toshokan::binding::Scan;
use toshokan::blocking::{self, Backend};
use toshokan::env::TestClock;
use toshokan::io::Capabilities;
use toshokan::log::{Entry, EntryKind, Op, Settlement};
use toshokan::report::{Mode, Opened, Start};
use toshokan::simulator::Machine;
use toshokan::snapshot::Snapshot;
use toshokan::{EntryHash, Io, IoResult, RelPath, Root, Why, WriterId};

use crate::fixture::{
    fixtures, layout, load, load_case, open, path, read_tree, schema, Faulty, Library, REVERSE,
    TAGS,
};
use crate::oracle::{assert_facts, observed, oracle};

const W: &str = "10e2c46865e9874614d7973c5c2a449c";
const ONE: &str = "910a2dec89025cc1beeb8da1658eec67";
const FOUR: &str = "e099ec6cd7363ca585e7bb0f12278575";
const SETTLER: &str = "e1b9ae67265f78875330404c13ee7598";

fn writer(text: &str) -> WriterId {
    text.parse().unwrap()
}

/// The case's file at `file` of its tree `tree`.
fn case_file(case: &str, tree: &str, file: &str) -> Vec<u8> {
    std::fs::read(fixtures().join(REVERSE).join(case).join(tree).join(file)).unwrap()
}

/// The lines of the case's segment `newer.txt` of writer `w`.
fn newer_lines(case: &str, w: &str) -> Vec<Vec<u8>> {
    let bytes = case_file(case, "folder", &format!(".t/writers.2/{w}/newer.txt"));
    let lines = bytes.split_inclusive(|&b| b == b'\n');
    lines.map(<[u8]>::to_vec).collect()
}

fn hash_of(line: &[u8]) -> EntryHash {
    Entry::parse(line).unwrap().hash()
}

fn json_of(line: &[u8]) -> Value {
    let text = std::str::from_utf8(line).unwrap();
    serde_json::from_str(text.split('\t').next().unwrap()).unwrap()
}

/// Opens `machine` and scans every file. The library holds its writer's lock until
/// it closes, so each test opens a machine once.
fn opened(machine: &Machine) -> (Library, Opened) {
    let (mut library, opened) = open(&Faulty::new(machine), "reader", 99, &TestClock::at(100_000));
    library.rescan().unwrap();
    (library, opened)
}

/// The facts a reader shows of `machine`.
fn facts(machine: &Machine) -> Value {
    let (library, opened) = opened(machine);
    observed(&library, &opened, &machine.folder)
}

/// [`load_case`] with the local root left empty: a reader for whom every writer is
/// another's.
fn load_case_fresh(base: &str, case: &str) -> Machine {
    let machine = load_case(base, case);
    machine.local.lose_local();
    machine
}

/// The oracle of `base`, as a fresh install reads it.
fn fresh_oracle(base: &str) -> Value {
    let mut expected = oracle(base, "view.oracle.json");
    expected["start"] = "new".into();
    expected
}

fn newer_own_history(entry: EntryHash) -> Mode {
    Mode::ReadOnly(Why::NewerOwnHistory { entry })
}

/// The pairs a reader keeps unknown, as JSON.
fn unknown(library: &Library) -> Vec<(EntryHash, Value)> {
    let view = library.view();
    let kept = view.folded().unknown();
    let kept = kept.map(|(entry, raw)| (*entry, serde_json::from_str(raw.as_str()).unwrap()));
    kept.collect()
}

#[test]
fn entries_and_ops_this_build_does_not_know_are_kept_and_stop_their_own_writer() {
    let lines = newer_lines("unknown-kind-and-op", W);
    let (kind, op) = (hash_of(&lines[0]), hash_of(&lines[1]));
    let mut kept = vec![
        (kind, json_of(&lines[0])),
        (op, json_of(&lines[1])["ops"][0].clone()),
    ];
    kept.sort_by_key(|(entry, _)| *entry);

    let machine = load_case("one-writer-open", "unknown-kind-and-op");
    let (library, _) = opened(&machine);
    assert_eq!(library.mode(), &newer_own_history(kind));
    assert_eq!(unknown(&library), kept);

    let mut expected = fresh_oracle("one-writer-open");
    expected["entities"][ONE]["tags"] = json!(["b", "newer"]);
    expected["unknown"] = json!(2);
    let machine = load_case_fresh("one-writer-open", "unknown-kind-and-op");
    assert_facts("another writer's history", &expected, &facts(&machine));
    let (library, _) = opened(&machine);
    assert_eq!(unknown(&library), kept);
}

#[test]
fn known_ops_holding_members_this_build_does_not_know_merge_and_survive_compaction() {
    let lines = newer_lines("novel-members", W);
    let entry = Entry::parse(&lines[0]).unwrap();
    assert!(entry.unknown_members());
    let EntryKind::Intent(logged) = entry.kind() else {
        panic!("{entry:?}");
    };
    assert!(logged.ops.iter().all(|op| !matches!(op, Op::Unknown(_))));
    assert_eq!((logged.ops.len(), logged.displaced.len()), (3, 1));

    let mut expected = oracle("one-writer-open", "view.oracle.json");
    expected["entities"][ONE]["tags"] = json!(["b", "novel"]);
    expected["entities"][FOUR]["origin"] = json!("four");
    expected["extended"] = json!(1);
    let trash = expected["trash"].as_array_mut().unwrap();
    trash.push(
        json!({"writer": W, "item": "00000000000000000000000000000123",
                      "from": "novel.txt", "len": 1, "bytes": null}),
    );
    let machine = load_case("one-writer-open", "novel-members");
    assert_facts("novel-members", &expected, &facts(&machine));

    let machine = load_case("one-writer-open", "novel-members");
    let (mut library, _) = opened(&machine);
    let compacted = library.compact().unwrap();
    let written = machine.folder.files(Root::Folder);
    let written = &written[&layout().snapshot(writer(W), compacted.snapshot)];
    let snapshot = Snapshot::decode(written).unwrap();
    let extended: Vec<(EntryHash, Value)> = snapshot
        .state
        .extended
        .iter()
        .map(|(entry, raw)| (*entry, serde_json::from_str(raw.as_str()).unwrap()))
        .collect();
    assert_eq!(extended, [(entry.hash(), json_of(&lines[0]))]);
    let text = String::from_utf8_lossy(written);
    for member in [
        r#""novel-op":{"a":[1,"b"]}"#,
        r#""novel-run":true"#,
        r#""novel-file":"x""#,
        r#""novel-item":null"#,
        r#""novel-entry":[{"deep":{}}]"#,
    ] {
        assert!(text.contains(member), "{member} lost from {text}");
    }
}

#[test]
fn a_delete_whose_observed_write_holds_a_member_this_build_does_not_know_is_an_unknown_op() {
    let lines = newer_lines("novel-member-in-observed", W);
    let EntryKind::Intent(logged) = Entry::parse(&lines[0]).unwrap().kind() else {
        panic!("not an intent");
    };
    assert!(matches!(logged.ops[..], [Op::Unknown(_)]), "{logged:?}");

    let machine = load_case("one-writer-open", "novel-member-in-observed");
    let (library, _) = opened(&machine);
    assert_eq!(library.mode(), &newer_own_history(hash_of(&lines[0])));
    let mut expected = fresh_oracle("one-writer-open");
    expected["unknown"] = json!(1);
    let machine = load_case_fresh("one-writer-open", "novel-member-in-observed");
    assert_facts("the delete of five", &expected, &facts(&machine));
}

#[test]
fn a_clock_reading_with_an_extra_element_leaves_its_line_unplaced_and_its_writer_read_only() {
    let lines = newer_lines("at-extra-element", W);
    assert_eq!(Entry::parse(&lines[0]).unwrap().n(), Entry::UNNUMBERED);
    let after = hash_of(&lines[1]);

    let mut expected = fresh_oracle("one-writer-open");
    expected["gaps"] = json!([{ "writer": W, "held": 1 }]);
    expected["items"] = json!([{
        "kind": "gap",
        "run": { "writer": W, "from": 12, "first": after.to_string(), "last": [12, after.to_string()] },
        "by": "fixture",
        "entries": 1,
        "changes": [format!("{ONE} tags")],
    }]);
    let machine = load_case_fresh("one-writer-open", "at-extra-element");
    assert_facts("at-extra-element", &expected, &facts(&machine));

    let machine = load_case("one-writer-open", "at-extra-element");
    let before = machine.folder.files(Root::Folder);
    let (library, first) = opened(&machine);
    assert_eq!(first.mode, newer_own_history(hash_of(&lines[1])));
    library.close().unwrap();
    let after = machine.folder.files(Root::Folder);
    let gone: Vec<&RelPath> = before.keys().filter(|p| !after.contains_key(*p)).collect();
    assert_eq!(gone, Vec::<&RelPath>::new(), "closing deleted files");
}

/// Commits one intent and compacts, as the writer of the local root of `case`
/// laid over `one-writer-compacted`. Returns the snapshot and the folder.
fn compact_after_a_write(case: &str) -> (Vec<u8>, Machine) {
    let machine = load_case("one-writer-compacted", case);
    let (mut library, opened) = opened(&machine);
    assert_eq!(opened.start, Start::Resumed(writer(W)));
    let one = ONE.parse().unwrap();
    library
        .intent("Tag")
        .add(one, TAGS, "later".into())
        .commit()
        .unwrap();
    let compacted = library.compact().unwrap();
    let written = machine.folder.files(Root::Folder);
    let written = written[&layout().snapshot(writer(W), compacted.snapshot)].clone();
    (written, machine)
}

#[test]
fn a_snapshots_members_this_build_does_not_know_survive_its_compaction_but_certified_ones() {
    let expected = oracle("one-writer-compacted", "view.oracle.json");
    let machine = load_case("one-writer-compacted", "snapshot-novel-members");
    assert_facts("snapshot-novel-members", &expected, &facts(&machine));

    let (written, _) = compact_after_a_write("snapshot-novel-members");
    let text = String::from_utf8_lossy(&written);
    assert!(text.contains(r#""novel-top":{"since":2}"#), "{text}");
    assert!(text.contains(r#""novel-state":[1,2]"#), "{text}");
    assert!(!text.contains("novel-certified"), "{text}");
}

#[test]
fn a_snapshot_with_a_record_this_build_cannot_read_is_reported_and_never_deleted() {
    let newer = RelPath::new(&format!(".t/writers.2/{W}/snapshot-newer.json")).unwrap();
    let mut expected = oracle("one-writer-compacted", "view.oracle.json");
    expected["unreadable"] = json!([newer.as_str()]);
    let machine = load_case("one-writer-compacted", "snapshot-refused-record");
    assert_facts("snapshot-refused-record", &expected, &facts(&machine));

    let held = machine.folder.files(Root::Folder)[&newer].clone();
    let (_, machine) = compact_after_a_write("snapshot-refused-record");
    let after = machine.folder.files(Root::Folder);
    assert_eq!(after.get(&newer), Some(&held), "the compaction kept it");
}

#[test]
fn a_snapshot_holding_an_op_this_build_does_not_know_stops_its_own_writer() {
    let head: EntryHash = "8ba7d4f5c6e616f69f641f70c1492f38".parse().unwrap();
    let machine = load_case("one-writer-compacted", "snapshot-unknown-entries");
    let (library, _) = opened(&machine);
    assert_eq!(library.mode(), &newer_own_history(head));

    let mut expected = fresh_oracle("one-writer-compacted");
    expected["unknown"] = json!(1);
    let machine = load_case_fresh("one-writer-compacted", "snapshot-unknown-entries");
    assert_facts("snapshot-unknown-entries", &expected, &facts(&machine));
}

#[test]
fn a_record_holding_a_member_this_build_does_not_know_is_reported_and_finishes() {
    let expected = oracle("pending", "view.oracle.json");
    let machine = load_case("pending", "pending-novel-member");
    assert_facts("pending-novel-member", &expected, &facts(&machine));

    let machine = load_case("pending", "pending-novel-member");
    let (mut library, opened) = opened(&machine);
    let everything = opened.orphaned.iter().find(|o| o.label == "Everything");
    let everything = everything.expect("reported").clone();
    library.settle(everything, Settlement::Finished).unwrap();
    let files = machine.folder.files(Root::Folder);
    assert_eq!(
        files.get(&path("v.txt")).map(Vec::as_slice),
        Some(&b"unbound"[..])
    );
    assert_eq!(files.get(&path("u.txt")), None);
}

#[test]
fn records_with_a_step_or_moves_this_build_does_not_know_are_ignored_and_hold_the_trash() {
    let records: Vec<RelPath> = ["a01", "a02"]
        .iter()
        .map(|n| {
            let name = format!("{:0>32}", n);
            RelPath::new(&format!(".t/writers.2/{SETTLER}/pending/{name}.json")).unwrap()
        })
        .collect();
    let mut expected = oracle("pending", "view.oracle.json");
    let ignored = expected["ignored"].as_array_mut().unwrap();
    ignored.extend(records.iter().map(|p| json!(p.as_str())));
    let machine = load_case("pending", "pending-unknown-step-and-moves");
    assert_facts(
        "pending-unknown-step-and-moves",
        &expected,
        &facts(&machine),
    );

    let machine = load_case("pending", "pending-unknown-step-and-moves");
    let (mut library, _) = opened(&machine);
    let emptied = library.empty_trash(BTreeSet::new()).unwrap();
    assert_eq!(emptied.held, records);
    assert_eq!(emptied.removed, []);
    let files = machine.folder.files(Root::Folder);
    for record in &records {
        assert!(files.contains_key(record), "{record} was removed");
    }
}

#[test]
fn a_settlement_with_an_outcome_this_build_does_not_know_leaves_the_record_reported() {
    let lines = newer_lines("settle-unknown-outcome", SETTLER);
    let machine = load_case("pending", "settle-unknown-outcome");
    let (library, _) = opened(&machine);
    assert_eq!(library.mode(), &newer_own_history(hash_of(&lines[0])));

    let mut expected = fresh_oracle("pending");
    expected["unknown"] = json!(1);
    let machine = load_case_fresh("pending", "settle-unknown-outcome");
    assert_facts("settle-unknown-outcome", &expected, &facts(&machine));
}

#[test]
fn a_head_record_with_a_member_this_build_does_not_know_still_resumes_its_writer() {
    let machine = load_case("local", "head-extra-member");
    let expected = oracle("local", "view.oracle.json");
    assert_facts("head-extra-member", &expected, &facts(&machine));

    let machine = load_case("local", "head-extra-member");
    let (mut library, first) = opened(&machine);
    let Start::Resumed(own) = first.start else {
        panic!("resumed: {:?}", first.start);
    };
    let (intent, _) = library.intent("Make").create(|_| {});
    intent.commit().unwrap();
    library.close().unwrap();
    let (_, again) = opened(&machine);
    assert_eq!(
        again.start,
        Start::Resumed(own),
        "the shorter record reads after a write"
    );
}

/// A machine that keeps the library files whose bytes it is asked for.
#[derive(Clone)]
struct Reads {
    machine: Machine,
    read: Rc<RefCell<BTreeSet<RelPath>>>,
}

impl Backend for Reads {
    fn capabilities(&self, root: Root) -> Capabilities {
        Backend::capabilities(&self.machine, root)
    }

    fn perform(&mut self, io: Io) -> IoResult {
        let paths: Vec<RelPath> = match &io {
            Io::Read { root, path, .. } if *root == Root::Folder => vec![path.clone()],
            Io::ReadMany { root, reads } if *root == Root::Folder => {
                reads.iter().map(|(path, _)| path.clone()).collect()
            }
            _ => Vec::new(),
        };
        let library = paths.into_iter().filter(|p| !layout().owns(p));
        self.read.borrow_mut().extend(library);
        self.machine.perform(io)
    }
}

/// The library files a scan of every file reads the bytes of, with `other.syx` at
/// the time `identities.json` keeps for it.
fn read_by_a_scan(machine: Machine) -> BTreeSet<RelPath> {
    let other = path("other.syx");
    machine
        .folder
        .set_modified(Root::Folder, &other, 2)
        .unwrap();
    let reads = Reads {
        machine,
        read: Rc::default(),
    };
    let env = crate::fixture::env("reader", 99, &TestClock::at(100_000));
    let (mut library, _) =
        blocking::Library::open(reads.clone(), layout(), &schema(), env).unwrap();
    library.rescan().unwrap();
    let read = reads.read.borrow().clone();
    read
}

#[test]
fn identities_rows_with_an_element_this_build_does_not_know_are_ignored_and_read_again() {
    let local = read_tree(&fixtures().join("local/local"));
    let rows = &local[&path("identities.json")];
    assert!(Scan::of_identities(rows).is_some());
    let newer = case_file("identities-extra-element", "local", "identities.json");
    assert!(Scan::of_identities(&newer).is_none());

    assert_eq!(read_by_a_scan(load("local")), BTreeSet::new());
    let read = read_by_a_scan(load_case("local", "identities-extra-element"));
    assert_eq!(read, BTreeSet::from([path("other.syx")]));
}

#[test]
fn a_later_layouts_writers_directory_opens_the_library_read_only() {
    let mut expected = oracle("local", "view.oracle.json");
    expected["mode"] = "read-only: a newer version keeps this library in writers.3".into();
    let machine = load_case("local", "writers-3");
    assert_facts("writers-3", &expected, &facts(&machine));
}

#[test]
fn every_reverse_case_is_tested() {
    let source = include_str!("reverse.rs");
    let dir = fixtures().join(REVERSE);
    for case in std::fs::read_dir(dir).unwrap() {
        let name = case.unwrap().file_name().into_string().unwrap();
        let quoted = format!("\"{name}\"");
        assert!(
            source.contains(&quoted),
            "no test loads reverse case {name}"
        );
    }
}
