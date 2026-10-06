//! Every file effect crashed after every operation, under torn and zero-filled
//! tails, renames done by copy and names made durable early.
//!
//! Before recovery, no bytes have left the folder, and each user path holds its
//! old bytes, its new bytes or nothing, and nothing only while a pending record
//! names it. Recovery finishes every recorded effect, removes every record and
//! staged file, changes nothing when run again, and ends the same however often
//! it is itself cut short. After the local root is lost, an unfinished effect is
//! reported for consent, and settling it never writes in its writer's directory.

mod common;

use std::collections::{BTreeMap, BTreeSet};
use std::rc::Rc;

use common::{
    bound, close, entry, env, identify, identity, layout, path, BlockingMem, Driven, HEAD, WRITER,
};
use toshokan::binding::Bindings;
use toshokan::crash::{self, Fault};
use toshokan::disk::{Renames, Tail};
use toshokan::effects;
use toshokan::io::Capabilities;
use toshokan::log::Settlement;
use toshokan::pending::{self, PendingRecord};
use toshokan::plan::{Expect, FileChange, Target};
use toshokan::recovery::{self, Chain};
use toshokan::{EntityId, EntryHash, Error, MemDisk, Nonce, Outcome, RelPath, Root, WriterId};

const E: EntityId = EntityId::from_u128(0xe);
const F: EntityId = EntityId::from_u128(0xf);
const ITEM: Nonce = Nonce::from_u128(0x1);
const OLD: &[u8] = b"old bytes";
const NEW: &[u8] = b"new bytes!";
const OTHER: &[u8] = b"other";

/// A user path, what it holds before the intent and what after.
type Change = (&'static str, Option<&'static [u8]>, Option<&'static [u8]>);

/// One intent from a known folder.
struct Case {
    name: &'static str,
    rename_dir: bool,
    files: Vec<(String, &'static [u8])>,
    bound: Vec<(EntityId, &'static str)>,
    changes: Vec<FileChange>,
    /// What each user path the intent touches holds before and after it.
    paths: Vec<Change>,
}

fn cases() -> Vec<Case> {
    let item = layout().trash(WRITER, ITEM).as_str().to_owned();
    let save = |at: &str, expect| FileChange::Save {
        entity: Target::Existing(E),
        path: path(at),
        bytes: NEW.to_vec(),
        expect,
    };
    let tree = FileChange::MoveTree {
        from: path("x"),
        to: path("z/x"),
    };
    let tree_paths = vec![
        ("x/1", Some(OLD), None),
        ("x/y/2", Some(OTHER), None),
        ("z/x/1", None, Some(OLD)),
        ("z/x/y/2", None, Some(OTHER)),
    ];
    vec![
        Case {
            name: "save over a file",
            rename_dir: true,
            files: vec![("a".into(), OLD)],
            bound: vec![],
            changes: vec![save("a", Expect::Holds(identity(OLD)))],
            paths: vec![("a", Some(OLD), Some(NEW))],
        },
        Case {
            name: "save a new file",
            rename_dir: true,
            files: vec![],
            bound: vec![],
            changes: vec![save("d/n", Expect::Absent)],
            paths: vec![("d/n", None, Some(NEW))],
        },
        Case {
            name: "trash",
            rename_dir: true,
            files: vec![("a".into(), OLD)],
            bound: vec![(E, "a")],
            changes: vec![FileChange::Trash {
                entity: E,
                expect: Expect::Holds(identity(OLD)),
            }],
            paths: vec![("a", Some(OLD), None)],
        },
        Case {
            name: "rename",
            rename_dir: true,
            files: vec![("a".into(), OLD)],
            bound: vec![(E, "a")],
            changes: vec![FileChange::Rename {
                entity: E,
                to: path("b/c"),
                expect: Expect::Holds(identity(OLD)),
            }],
            paths: vec![("a", Some(OLD), None), ("b/c", None, Some(OLD))],
        },
        Case {
            name: "move a tree",
            rename_dir: true,
            files: vec![("x/1".into(), OLD), ("x/y/2".into(), OTHER)],
            bound: vec![(E, "x/1")],
            changes: vec![tree.clone()],
            paths: tree_paths.clone(),
        },
        Case {
            name: "move a tree file by file",
            rename_dir: false,
            files: vec![("x/1".into(), OLD), ("x/y/2".into(), OTHER)],
            bound: vec![(E, "x/1")],
            changes: vec![tree],
            paths: tree_paths,
        },
        Case {
            name: "restore over a file",
            rename_dir: true,
            files: vec![("a".into(), NEW), (item, OLD)],
            bound: vec![],
            changes: vec![FileChange::Restore {
                entity: E,
                item: ITEM,
                to: path("a"),
                expect: Expect::Holds(identity(NEW)),
            }],
            paths: vec![("a", Some(NEW), Some(OLD))],
        },
        Case {
            name: "save and rename in one intent",
            rename_dir: true,
            files: vec![("a".into(), OLD), ("b".into(), OTHER)],
            bound: vec![(F, "b")],
            changes: vec![
                save("a", Expect::Holds(identity(OLD))),
                FileChange::Rename {
                    entity: F,
                    to: path("c"),
                    expect: Expect::Holds(identity(OTHER)),
                },
            ],
            paths: vec![
                ("a", Some(OLD), Some(NEW)),
                ("b", Some(OTHER), None),
                ("c", None, Some(OTHER)),
            ],
        },
    ]
}

impl Case {
    fn disk(&self) -> MemDisk {
        let caps = Capabilities {
            rename_dir: self.rename_dir,
            ..Capabilities::ALL
        };
        let disk = MemDisk::with_capabilities(caps, Capabilities::ALL);
        let files: Vec<(&str, &[u8])> = self.files.iter().map(|(p, b)| (p.as_str(), *b)).collect();
        common::put(&mut BlockingMem(disk.clone()), &files);
        disk.restart()
    }

    fn bindings(&self) -> Bindings {
        let mut bindings = bound(&self.bound);
        let bound: BTreeSet<&str> = self.bound.iter().map(|(_, p)| *p).collect();
        bindings.unbound = self
            .files
            .iter()
            .filter(|(p, _)| !bound.contains(p.as_str()) && !layout().owns(&path(p)))
            .map(|(p, _)| path(p))
            .collect();
        bindings
    }

    /// Commits the intent, stopping at the first error.
    fn commit(&self, disk: &MemDisk) {
        let d = &mut BlockingMem(disk.clone());
        let Ok(Ok(run)) = common::prepare(d, WRITER, &self.changes, &self.bindings(), &mut env(7))
        else {
            return;
        };
        let _ = common::complete(d, &run);
    }

    fn state(&self, after: bool) -> Vec<Option<Vec<u8>>> {
        self.paths
            .iter()
            .map(|&(_, before, then)| if after { then } else { before }.map(<[u8]>::to_vec))
            .collect()
    }

    fn holds(&self, files: &BTreeMap<RelPath, Vec<u8>>) -> Vec<Option<Vec<u8>>> {
        self.paths
            .iter()
            .map(|(p, _, _)| files.get(&path(p)).cloned())
            .collect()
    }

    /// No bytes the folder held have left it.
    fn kept(&self, case: &str, files: &BTreeMap<RelPath, Vec<u8>>) {
        let present: BTreeSet<&[u8]> = files.values().map(Vec::as_slice).collect();
        for (_, bytes) in &self.files {
            assert!(
                present.contains(bytes),
                "{case}: {bytes:?} left the folder: {files:?}"
            );
        }
    }
}

/// A writer's history as recovery sees it: its head, and whether the entry
/// closing its record follows.
struct Fake {
    closed: bool,
}

impl Chain for Fake {
    fn holds(&self, hash: EntryHash) -> bool {
        hash == HEAD
    }

    fn continues(&self, hash: EntryHash) -> bool {
        hash == HEAD && self.closed
    }
}

fn logs(disk: &MemDisk, writer: WriterId) -> BTreeMap<WriterId, Fake> {
    let dir = layout().writer(writer);
    let closed = disk.files(Root::Folder).keys().any(|p| {
        p.parent().as_ref() == Some(&dir) && p.name().is_some_and(|n| n.starts_with("closed-"))
    });
    BTreeMap::from([(writer, Fake { closed })])
}

fn records(disk: &MemDisk, writer: WriterId) -> Vec<(Nonce, PendingRecord)> {
    BlockingMem(disk.clone())
        .run(pending::read_all(&layout(), writer))
        .unwrap()
        .records
}

/// Opens as `WRITER` and settles its records, as the library does before its next
/// write.
fn recover(disk: &MemDisk) -> Result<Vec<Outcome>, Error> {
    let layout = layout();
    let d = &mut BlockingMem(disk.clone());
    let logs = logs(disk, WRITER);
    let found = d.run(recovery::assess(
        &layout,
        Some((WRITER, HEAD)),
        &logs,
        &BTreeSet::new(),
    ))?;
    assert!(
        found.orphaned.is_empty() && found.ignored.is_empty(),
        "{found:?}"
    );
    let mut outcomes = Vec::new();
    for settling in &found.own {
        if !settling.logged {
            let applied = d.run(recovery::settle(
                &layout,
                settling.record,
                Rc::new(settling.pending.clone()),
                identify(),
            ))?;
            assert_eq!(applied.outcome, settling.outcome, "predicted");
            outcomes.push(applied.outcome);
            close(d, WRITER, settling.record)?;
        }
        d.run(effects::finish(&layout, WRITER, settling.record))?;
    }
    d.run(recovery::tidy(&layout, WRITER, &[]))?;
    Ok(outcomes)
}

fn staged(files: &BTreeMap<RelPath, Vec<u8>>) -> Vec<&RelPath> {
    let tmp = layout().tmp_dir(WRITER);
    files.keys().filter(|p| p.starts_with(&tmp)).collect()
}

#[test]
fn every_effect_crashed_anywhere_keeps_its_bytes_and_recovers_whole() {
    for case in cases() {
        crash::sweep(
            || case.disk(),
            |disk| case.commit(disk),
            |fault, disk| {
                let shown = format!("{} at {fault:?}", case.name);
                let files = disk.files(Root::Folder);
                case.kept(&shown, &files);
                let open = records(&disk, WRITER);
                let named: BTreeSet<RelPath> = open.iter().flat_map(|(_, r)| r.paths()).collect();
                let holds = case.holds(&files);
                let midway = holds != case.state(false) && holds != case.state(true);
                for (holds, &(p, before, after)) in holds.iter().zip(&case.paths) {
                    let allowed = [before, after, None].map(|b| b.map(<[u8]>::to_vec));
                    assert!(allowed.contains(holds), "{shown}: {p} holds {holds:?}");
                    if midway && holds.is_none() && before.is_some() {
                        assert!(
                            named.contains(&path(p)),
                            "{shown}: {p} is empty and no record names it"
                        );
                    }
                }

                let outcomes = recover(&disk).unwrap_or_else(|e| panic!("{shown}: {e}"));
                assert!(
                    outcomes.iter().all(|o| *o == Outcome::Complete),
                    "{shown}: {outcomes:?}"
                );
                let recovered = disk.files(Root::Folder);
                case.kept(&shown, &recovered);
                assert!(
                    records(&disk, WRITER).is_empty(),
                    "{shown}: a record is left"
                );
                assert!(staged(&recovered).is_empty(), "{shown}: staging is left");
                let holds = case.holds(&recovered);
                let finished = !open.is_empty() || logs(&disk, WRITER)[&WRITER].closed;
                let expected = case.state(finished);
                assert_eq!(holds, expected, "{shown}: recovered");

                let operations = disk.mutations();
                recover(&disk).unwrap();
                assert_eq!(
                    disk.mutations(),
                    operations,
                    "{shown}: recovering again wrote"
                );
                assert_eq!(
                    disk.files(Root::Folder),
                    recovered,
                    "{shown}: recovering again"
                );
            },
        );
    }
}

/// The disk `case` leaves when it crashes as `fault` says.
fn crashed(case: &Case, fault: Fault) -> MemDisk {
    let disk = case.disk();
    disk.set_tail(fault.tail);
    disk.set_renames(fault.renames);
    disk.set_eager_names(fault.eager_names);
    disk.crash_after(fault.after);
    case.commit(&disk);
    disk.restart()
}

#[test]
fn recovery_cut_short_anywhere_then_run_again_ends_as_one_run_does() {
    for case in cases() {
        for renames in [Renames::Atomic, Renames::CopyThenRemove] {
            for eager_names in [false, true] {
                let fault = |after| Fault {
                    after,
                    tail: Tail::Lost,
                    renames,
                    eager_names,
                };
                let count = case.disk();
                count.set_renames(renames);
                case.commit(&count);
                for after in 0..=count.mutations() {
                    let once = crashed(&case, fault(after));
                    recover(&once).unwrap();
                    let expected = once.files(Root::Folder);
                    let operations = once.mutations();
                    for cut in 0..operations {
                        let disk = crashed(&case, fault(after));
                        disk.crash_after(cut);
                        assert!(recover(&disk).is_err());
                        let disk = disk.restart();
                        recover(&disk).unwrap();
                        assert_eq!(
                            disk.files(Root::Folder),
                            expected,
                            "{} at {:?}, recovery cut after {cut}",
                            case.name,
                            fault(after)
                        );
                    }
                }
            }
        }
    }
}

const HEIR: WriterId = WriterId::from_u128(0x88);

/// Settles every orphan of `WRITER` as `HEIR` would with the user's consent.
fn settle_orphans(disk: &MemDisk, how: Settlement) -> usize {
    let layout = layout();
    let d = &mut BlockingMem(disk.clone());
    let found = d
        .run(recovery::assess(
            &layout,
            None,
            &logs(disk, WRITER),
            &BTreeSet::new(),
        ))
        .unwrap();
    assert!(
        found.own.is_empty() && found.ignored.is_empty(),
        "{found:?}"
    );
    let mut env = env(9);
    for orphan in &found.orphaned {
        let theirs = d
            .run(pending::read_one(&layout, orphan.writer, orphan.record))
            .unwrap()
            .expect("the orphan's record is there");
        let progress = d.run(recovery::progress(&layout, &theirs)).unwrap();
        let plan = Rc::new(recovery::orphan_plan(
            &layout, &theirs, &progress, how, &mut env,
        ));
        let record = Rc::new(PendingRecord::new(HEIR, "settle", entry(HEAD), &plan));
        let prepared = d
            .run(effects::prepare(
                &layout,
                Rc::clone(&plan),
                Rc::clone(&record),
                identify(),
            ))
            .unwrap();
        assert_eq!(prepared, Ok(()));
        if !plan.is_empty() {
            let applied = d
                .run(effects::apply(&layout, plan.record, record, 0, identify()))
                .unwrap();
            assert_eq!(applied.outcome, Outcome::Complete, "{how:?}");
            d.run(effects::finish(&layout, HEIR, plan.record)).unwrap();
        }
    }
    found.orphaned.len()
}

#[test]
fn after_losing_the_local_root_an_unfinished_effect_is_reported_and_settled_only_from_outside() {
    for case in cases() {
        for how in [
            Settlement::Finished,
            Settlement::RolledBack,
            Settlement::Dismissed,
        ] {
            crash::sweep(
                || case.disk(),
                |disk| case.commit(disk),
                |fault, disk| {
                    let shown = format!("{} at {fault:?}, {how:?}", case.name);
                    disk.lose_local();
                    let open =
                        !records(&disk, WRITER).is_empty() && !logs(&disk, WRITER)[&WRITER].closed;
                    let theirs = layout().writer(WRITER);
                    let before = disk.files(Root::Folder);
                    let holds = case.holds(&before);
                    let reported = settle_orphans(&disk, how);
                    assert_eq!(reported, usize::from(open), "{shown}: reported");

                    let after = disk.files(Root::Folder);
                    let in_theirs = |files: &BTreeMap<RelPath, Vec<u8>>| {
                        files
                            .iter()
                            .filter(|(p, _)| p.starts_with(&theirs))
                            .map(|(p, b)| (p.clone(), b.clone()))
                            .collect::<Vec<_>>()
                    };
                    assert_eq!(
                        in_theirs(&after),
                        in_theirs(&before),
                        "{shown}: wrote in their directory"
                    );
                    case.kept(&shown, &after);
                    let expected = match (open, how) {
                        (false, _) | (true, Settlement::Dismissed) => holds,
                        (true, Settlement::Finished) => case.state(true),
                        (true, Settlement::RolledBack) => case.state(false),
                    };
                    assert_eq!(case.holds(&after), expected, "{shown}: settled");
                },
            );
        }
    }
}

#[test]
fn records_that_are_unchained_unconfined_or_settled_are_not_acted_on() {
    let layout = layout();
    let disk = MemDisk::new();
    let d = &mut BlockingMem(disk.clone());
    let write = |d: &mut BlockingMem, writer: WriterId, name: u128, after: EntryHash, to: &str| {
        let plan = effects::EffectPlan {
            steps: vec![effects::EffectStep::Rename {
                from: path("a"),
                to: path(to),
            }],
            ..effects::EffectPlan::new(Nonce::from_u128(name))
        };
        let record = PendingRecord::new(writer, "label", entry(after), &plan);
        let dir = layout.pending_dir(writer);
        d.ok(toshokan::Io::MakeDir {
            root: Root::Folder,
            path: dir,
        });
        d.ok(toshokan::Io::Create {
            root: Root::Folder,
            path: layout.pending(writer, Nonce::from_u128(name)),
            bytes: record.encode(),
        });
    };
    let other = WriterId::from_u128(0x99);
    write(d, WRITER, 1, HEAD, "b");
    write(d, WRITER, 2, EntryHash::from_u128(0x12), "b");
    write(d, WRITER, 3, HEAD, ".t/writers/x");
    write(d, other, 4, HEAD, "b");
    write(d, other, 5, HEAD, "c");

    struct Settled;
    impl Chain for Settled {
        fn holds(&self, hash: EntryHash) -> bool {
            hash == HEAD
        }
        fn continues(&self, _: EntryHash) -> bool {
            false
        }
    }
    let logs: BTreeMap<WriterId, Settled> = [(WRITER, Settled), (other, Settled)].into();
    let settled = BTreeSet::from([(other, Nonce::from_u128(5))]);
    let written = disk.mutations();
    let found = d
        .run(recovery::assess(
            &layout,
            Some((WRITER, HEAD)),
            &logs,
            &settled,
        ))
        .unwrap();
    assert_eq!(disk.mutations(), written, "assessing wrote");
    let names = |s: &[recovery::Settling]| s.iter().map(|s| s.record.to_u128()).collect::<Vec<_>>();
    assert_eq!(names(&found.own), [1]);
    assert_eq!(
        found.own[0].outcome,
        Outcome::Partial(toshokan::report::PartialReport {
            applied: 0,
            stopped: path("b"),
            error: toshokan::io::IoError::NotFound,
            record: Some(Nonce::from_u128(1)),
        }),
        "the source is not there"
    );
    assert_eq!(
        found
            .orphaned
            .iter()
            .map(|o| (o.writer, o.record.to_u128()))
            .collect::<Vec<_>>(),
        [(other, 4)]
    );
    assert_eq!(found.orphaned[0].paths, [path("a"), path("b")]);
    assert_eq!(
        found.ignored,
        [
            layout.pending(WRITER, Nonce::from_u128(2)),
            layout.pending(WRITER, Nonce::from_u128(3))
        ]
    );
}
