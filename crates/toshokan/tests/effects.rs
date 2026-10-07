//! File effects on every backend: what a committed intent leaves in the folder,
//! and that a refused one leaves nothing.

mod common;

use std::cell::Cell;
use std::collections::BTreeMap;
use std::rc::Rc;

use common::{
    bound, commit, copying, entry, env, files, identify, identity, layout, path, put, Driven, Fill,
    HEAD, WRITER,
};
use toshokan::blocking::{self, Backend};
use toshokan::effects::{self, EffectStep, Moves};
use toshokan::error::{Invalid, Mismatch};
use toshokan::io::{Capabilities, IoError, Range, CHUNK};
use toshokan::log::{Displaced, FileFact};
use toshokan::pending::PendingRecord;
use toshokan::plan::{Content, Expect, FileChange, Piece, Splice};
use toshokan::recovery;
use toshokan::{
    EntityId, Error, Io, IoResult, MemDisk, Nonce, Operation, Outcome, Refusal, RelPath, Root, Step,
};

const E: EntityId = EntityId::from_u128(0xe);

/// The folder's files outside toshokan's root, and the contents of this writer's
/// trash, staging and pending records.
fn sorted(d: &mut impl Driven) -> Folder {
    let mut folder = Folder::default();
    let writer = layout().writer(WRITER);
    for (path, bytes) in files(d, Root::Folder) {
        let place = |dir: &str| path.starts_with(&writer.join(dir).unwrap());
        match () {
            () if place("trash") => folder.trash.push(bytes),
            () if place("tmp") => folder.staged.push(bytes),
            () if place("pending") => folder.pending += 1,
            () if layout().owns(&path) => {}
            () => {
                folder.user.insert(path, bytes);
            }
        }
    }
    folder.trash.sort();
    folder
}

#[derive(Default, Debug, PartialEq)]
struct Folder {
    user: BTreeMap<RelPath, Vec<u8>>,
    trash: Vec<Vec<u8>>,
    staged: Vec<Vec<u8>>,
    pending: usize,
}

fn user(entries: &[(&str, &[u8])]) -> BTreeMap<RelPath, Vec<u8>> {
    entries.iter().map(|(p, b)| (path(p), b.to_vec())).collect()
}

fn fact(text: &str, bytes: &[u8], applied: &[(EntityId, Option<FileFact>)]) -> FileFact {
    let fact = applied[0].1.clone().expect("the entity has a file");
    assert_eq!(
        (fact.path.as_str(), fact.identity, fact.len),
        (text, identity(bytes), bytes.len() as u64)
    );
    fact
}

fn kept(offset: u64, len: u64) -> Piece {
    Piece::Kept(Range { offset, len })
}

/// A save of `E` over the file at `at`, which holds `old`, from the first content.
fn save_over(at: &str, old: &[u8]) -> FileChange {
    FileChange::Save {
        entity: E,
        path: path(at),
        content: Content(0),
        expect: Expect::Holds(identity(old)),
    }
}

mod suite {
    use super::*;

    pub fn a_save_places_new_bytes_and_moves_the_old_into_the_trash(d: &mut impl Driven) {
        put(d, &[("a.syx", b"old bytes")]);
        let save = FileChange::Save {
            entity: E,
            path: path("a.syx"),
            content: Content(0),
            expect: Expect::Holds(identity(b"old bytes")),
        };
        let new = vec![Fill::Bytes(b"new bytes".to_vec())];
        let applied = commit(d, &[save], new, &bound(&[]), &mut env(1)).unwrap();
        assert_eq!(applied.outcome, Outcome::Complete);
        fact("a.syx", b"new bytes", &applied.files);
        let [Displaced {
            from,
            identity: displaced,
            len,
            ..
        }] = &applied.displaced[..]
        else {
            panic!("{:?}", applied.displaced);
        };
        assert_eq!(
            (from.as_str(), *displaced, *len),
            ("a.syx", identity(b"old bytes"), 9)
        );
        assert_eq!(
            sorted(d),
            Folder {
                user: user(&[("a.syx", b"new bytes")]),
                trash: vec![b"old bytes".to_vec()],
                ..Folder::default()
            }
        );
    }

    pub fn a_save_where_nothing_is_makes_its_directories(d: &mut impl Driven) {
        let save = FileChange::Save {
            entity: E,
            path: path("x/y/new.syx"),
            content: Content(0),
            expect: Expect::Absent,
        };
        let new = vec![Fill::Bytes(b"new".to_vec())];
        let applied = commit(d, &[save], new, &bound(&[]), &mut env(1)).unwrap();
        fact("x/y/new.syx", b"new", &applied.files);
        assert!(applied.displaced.is_empty());
        assert_eq!(sorted(d).user, user(&[("x/y/new.syx", b"new")]));
    }

    pub fn a_failed_precondition_refuses_and_leaves_nothing_behind(d: &mut impl Driven) {
        put(d, &[("a.syx", b"theirs")]);
        for expect in [Expect::Absent, Expect::Holds(identity(b"mine"))] {
            let save = FileChange::Save {
                entity: E,
                path: path("a.syx"),
                content: Content(0),
                expect,
            };
            let new = vec![Fill::Bytes(b"new".to_vec())];
            let refused = commit(d, &[save], new, &bound(&[]), &mut env(1)).unwrap_err();
            let expected = Refusal::Changed(Box::new(Mismatch {
                path: path("a.syx"),
                expected: expect,
                found: Some(identity(b"theirs")),
            }));
            assert_eq!(refused, expected);
            assert_eq!(
                sorted(d),
                Folder {
                    user: user(&[("a.syx", b"theirs")]),
                    ..Folder::default()
                }
            );
        }
    }

    pub fn trashing_then_restoring_brings_the_same_bytes_back(d: &mut impl Driven) {
        put(d, &[("d/a.syx", b"bytes")]);
        let trash = FileChange::Trash {
            entity: E,
            expect: Expect::Holds(identity(b"bytes")),
        };
        let bindings = bound(&[(E, "d/a.syx")]);
        let applied = commit(d, &[trash], vec![], &bindings, &mut env(1)).unwrap();
        assert_eq!(applied.files, [(E, None)]);
        let item = applied.displaced[0].item;
        assert_eq!(sorted(d).user, user(&[]));
        let restore = FileChange::Restore {
            entity: E,
            item,
            to: path("d/a.syx"),
            expect: Expect::Absent,
        };
        let applied = commit(d, &[restore], vec![], &bound(&[]), &mut env(2)).unwrap();
        fact("d/a.syx", b"bytes", &applied.files);
        assert_eq!(sorted(d).user, user(&[("d/a.syx", b"bytes")]));
        let gone = FileChange::Restore {
            entity: E,
            item,
            to: path("elsewhere"),
            expect: Expect::Absent,
        };
        assert_eq!(
            commit(d, &[gone], vec![], &bound(&[]), &mut env(3)).unwrap_err(),
            Refusal::Emptied
        );
    }

    pub fn a_rename_is_refused_over_an_existing_file(d: &mut impl Driven) {
        put(d, &[("a", b"a"), ("b", b"b")]);
        let rename = |to: &str| FileChange::Rename {
            entity: E,
            to: path(to),
            expect: Expect::Holds(identity(b"a")),
        };
        let bindings = bound(&[(E, "a")]);
        let refused = commit(d, &[rename("b")], vec![], &bindings, &mut env(1)).unwrap_err();
        assert!(
            matches!(&refused, Refusal::Changed(m) if m.path == path("b")),
            "{refused:?}"
        );
        let applied = commit(d, &[rename("c/a")], vec![], &bindings, &mut env(2)).unwrap();
        fact("c/a", b"a", &applied.files);
        assert_eq!(sorted(d).user, user(&[("b", b"b"), ("c/a", b"a")]));
    }

    pub fn a_save_spliced_from_the_file_it_replaces_copies_its_kept_ranges(d: &mut impl Driven) {
        put(d, &[("a.syx", b"old bytes")]);
        let splice = Splice {
            from: path("a.syx"),
            pieces: vec![kept(0, 4), Piece::Bytes(b"new ".to_vec()), kept(4, 5)],
        };
        let applied = commit(
            d,
            &[save_over("a.syx", b"old bytes")],
            vec![Fill::Splice(splice)],
            &bound(&[]),
            &mut env(1),
        )
        .unwrap();
        fact("a.syx", b"old new bytes", &applied.files);
        assert_eq!(
            sorted(d),
            Folder {
                user: user(&[("a.syx", b"old new bytes")]),
                trash: vec![b"old bytes".to_vec()],
                ..Folder::default()
            }
        );
    }

    pub fn a_splice_past_the_end_of_its_source_fails_before_any_user_path_changes(
        d: &mut impl Driven,
    ) {
        put(d, &[("a.syx", b"short")]);
        let splice = Splice {
            from: path("a.syx"),
            pieces: vec![kept(0, 6)],
        };
        let failed = common::prepare(
            d,
            WRITER,
            &[save_over("a.syx", b"short")],
            vec![Fill::Splice(splice)],
            &bound(&[]),
            &mut env(1),
        );
        assert!(
            matches!(
                failed,
                Err(Error::Io {
                    error: IoError::SpliceRange,
                    ..
                })
            ),
            "{:?}",
            failed.map(|run| run.map(|_| ()))
        );
        let folder = sorted(d);
        assert_eq!(
            (folder.user, folder.pending),
            (user(&[("a.syx", b"short")]), 0)
        );
    }

    pub fn a_splice_keeping_more_than_any_file_holds_fails_at_its_first_read(d: &mut impl Driven) {
        put(d, &[("a.syx", b"short")]);
        let splice = Splice {
            from: path("a.syx"),
            pieces: vec![kept(0, u64::MAX / 2)],
        };
        let failed = common::prepare(
            d,
            WRITER,
            &[save_over("a.syx", b"short")],
            vec![Fill::Splice(splice)],
            &bound(&[]),
            &mut env(1),
        );
        assert!(
            matches!(
                failed,
                Err(Error::Io {
                    error: IoError::SpliceRange,
                    ..
                })
            ),
            "{:?}",
            failed.map(|run| run.map(|_| ()))
        );
        assert_eq!(sorted(d).user, user(&[("a.syx", b"short")]));
    }

    pub fn a_tree_moves_with_every_file_in_it(d: &mut impl Driven) {
        put(d, &[("x/y/1", b"1"), ("x/2", b"2"), ("xx", b"3")]);
        let mut bindings = bound(&[(E, "x/y/1")]);
        bindings.unbound = vec![path("x/2"), path("xx")];
        let tree = FileChange::MoveTree {
            from: path("x"),
            to: path("z/x"),
        };
        let applied = commit(d, &[tree], vec![], &bindings, &mut env(1)).unwrap();
        fact("z/x/y/1", b"1", &applied.files);
        assert_eq!(
            sorted(d).user,
            user(&[("xx", b"3"), ("z/x/2", b"2"), ("z/x/y/1", b"1")])
        );
    }
}

/// Runs each behavior of the suite on every backend, then where the folder cannot
/// rename, through both drivers.
macro_rules! everywhere {
    ($suite:ident: $($behavior:ident),* $(,)?) => {
        for_every_backend!($suite: $($behavior),*);
        mod blocking_copying {
            $(#[test] fn $behavior() { super::$suite::$behavior(&mut $crate::common::BlockingMem($crate::common::copying())); })*
        }
        mod async_copying {
            $(#[test] fn $behavior() { super::$suite::$behavior(&mut $crate::common::AsyncMem($crate::common::copying())); })*
        }
    };
}

everywhere!(suite:
    a_save_places_new_bytes_and_moves_the_old_into_the_trash,
    a_save_spliced_from_the_file_it_replaces_copies_its_kept_ranges,
    a_splice_past_the_end_of_its_source_fails_before_any_user_path_changes,
    a_splice_keeping_more_than_any_file_holds_fails_at_its_first_read,
    a_save_where_nothing_is_makes_its_directories,
    a_failed_precondition_refuses_and_leaves_nothing_behind,
    trashing_then_restoring_brings_the_same_bytes_back,
    a_rename_is_refused_over_an_existing_file,
    a_tree_moves_with_every_file_in_it,
);

/// A backend that keeps each request it is asked, and refuses every create at
/// `refused`, as a backend might.
struct Refusing {
    disk: MemDisk,
    refused: Option<RelPath>,
    asked: Vec<Io>,
}

impl Refusing {
    fn new(disk: MemDisk, refused: Option<&str>) -> Self {
        Self {
            disk,
            refused: refused.map(path),
            asked: Vec::new(),
        }
    }
}

impl Backend for Refusing {
    fn capabilities(&self, root: Root) -> Capabilities {
        self.disk.capabilities(root)
    }

    fn perform(&mut self, io: Io) -> IoResult {
        self.asked.push(io.clone());
        match &io {
            Io::Create { path, .. } if Some(path) == self.refused.as_ref() => {
                Err(IoError::Other("refused by the test".into()))
            }
            _ => self.disk.perform(io),
        }
    }
}

/// Stages, journals and carries out `changes` on `backend`, saving `saved`.
fn run_effects(
    backend: &mut Refusing,
    changes: &[FileChange],
    saved: &[u8],
    bindings: &toshokan::binding::Bindings,
) -> Result<(Rc<PendingRecord>, effects::Applied), Error> {
    let caps = backend.capabilities(Root::Folder);
    let plan = Rc::new(effects::resolve(changes, bindings, &layout(), caps, &mut env(1)).unwrap());
    let record = Rc::new(PendingRecord::new(WRITER, "Save", entry(HEAD), &plan));
    let prepare = effects::prepare(&layout(), Rc::clone(&plan), Rc::clone(&record), identify());
    let sources: Vec<Box<dyn blocking::Source<Refusing>>> = vec![Box::new(saved.to_vec())];
    blocking::run_with(backend, sources, prepare)?.unwrap();
    let apply = effects::apply(&layout(), plan.record, Rc::clone(&record), 0, identify());
    let applied = blocking::run(backend, apply)?;
    Ok((record, applied))
}

#[test]
fn where_the_folder_cannot_rename_every_move_copies_and_then_removes_its_source() {
    let mut backend = Refusing::new(copying(), None);
    let d = &mut common::BlockingMem(backend.disk.clone());
    put(d, &[("a", b"old"), ("b", b"other")]);
    let changes = [
        save_over("a", b"old"),
        FileChange::Rename {
            entity: EntityId::from_u128(2),
            to: path("c/b"),
            expect: Expect::Holds(identity(b"other")),
        },
    ];
    let bindings = bound(&[(EntityId::from_u128(2), "b")]);
    let (record, applied) = run_effects(&mut backend, &changes, b"new", &bindings).unwrap();
    assert_eq!(record.moves, Moves::Copy);
    assert_eq!(applied.outcome, Outcome::Complete);
    let renames: Vec<&Io> = backend
        .asked
        .iter()
        .filter(|io| io.root() == Root::Folder && matches!(io, Io::Rename { .. }))
        .collect();
    assert_eq!(renames, Vec::<&Io>::new());
    let name = effects_record(&backend);
    let created: Vec<&RelPath> = backend
        .asked
        .iter()
        .filter_map(|io| match io {
            Io::Create { path, .. } => Some(path),
            _ => None,
        })
        .collect();
    for step in 0..3 {
        let marker = layout().marker(WRITER, name, step);
        assert!(
            created.contains(&&marker),
            "step {step} is marked: {created:?}"
        );
    }
    let folder = sorted(d);
    assert_eq!(
        (folder.user, folder.trash, folder.pending),
        (
            user(&[("a", b"new"), ("c/b", b"other")]),
            vec![b"old".to_vec()],
            4
        ),
        "every move made, the record and a marker per move left to remove"
    );
    d.run(effects::finish(&layout(), name, &record)).unwrap();
    assert_eq!(sorted(d).pending, 0, "the record and its markers removed");
}

/// The name of the record the run on `backend` wrote.
fn effects_record(backend: &Refusing) -> Nonce {
    let dir = layout().pending_dir(WRITER);
    backend
        .asked
        .iter()
        .find_map(|io| match io {
            Io::Create { path, .. } if path.parent() == Some(dir.clone()) => path
                .name()
                .and_then(|name| name.strip_suffix(".json"))
                .and_then(|name| name.parse().ok()),
            _ => None,
        })
        .expect("a record was written")
}

#[test]
fn where_moves_copy_a_long_file_moves_a_chunk_at_a_time() {
    let disk = copying();
    let long: Vec<u8> = (0..2 * CHUNK + 3).map(|i| (i % 251) as u8).collect();
    put(&mut common::BlockingMem(disk.clone()), &[("a", &long)]);
    let mut backend = Measuring {
        disk: disk.clone(),
        largest: 0,
    };
    let rename = FileChange::Rename {
        entity: E,
        to: path("b"),
        expect: Expect::Holds(identity(&long)),
    };
    let plan = Rc::new(
        effects::resolve(
            &[rename],
            &bound(&[(E, "a")]),
            &layout(),
            common::COPYING,
            &mut env(1),
        )
        .unwrap(),
    );
    let record = Rc::new(PendingRecord::new(WRITER, "Rename", entry(HEAD), &plan));
    let prepare = effects::prepare(&layout(), Rc::clone(&plan), Rc::clone(&record), identify());
    blocking::run(&mut backend, prepare).unwrap().unwrap();
    let apply = effects::apply(&layout(), plan.record, record, 0, identify());
    let applied = blocking::run(&mut backend, apply).unwrap();
    assert_eq!(applied.outcome, Outcome::Complete);
    assert_eq!(backend.largest as u64, CHUNK);
    let d = &mut common::BlockingMem(disk);
    assert_eq!(sorted(d).user, user(&[("b", &long)]));
}

#[test]
fn a_copy_cut_short_where_another_program_then_made_a_file_leaves_that_file() {
    let save = FileChange::Save {
        entity: E,
        path: path("d/n"),
        content: Content(0),
        expect: Expect::Absent,
    };
    let mut counting = Refusing::new(copying(), None);
    run_effects(
        &mut counting,
        std::slice::from_ref(&save),
        b"mine",
        &bound(&[]),
    )
    .unwrap();
    let mutations: Vec<&Io> = counting.asked.iter().filter(|io| io.mutates()).collect();
    let marked = mutations
        .iter()
        .position(|io| matches!(io, Io::Create { path, .. } if pending_marker(path)))
        .expect("the place is marked");

    let disk = copying();
    disk.crash_after(marked as u64 + 1);
    let mut crashing = Refusing::new(disk.clone(), None);
    assert!(run_effects(
        &mut crashing,
        std::slice::from_ref(&save),
        b"mine",
        &bound(&[])
    )
    .is_err());
    let disk = disk.restart();
    put(
        &mut common::BlockingMem(disk.clone()),
        &[("d/n", b"theirs")],
    );
    let d = &mut common::BlockingMem(disk.clone());
    let record = effects_record(&crashing);
    let pending = d
        .run(toshokan::pending::read_one(&layout(), WRITER, record))
        .unwrap()
        .unwrap();
    let applied = d
        .run(recovery::settle(
            &layout(),
            record,
            Rc::new(pending),
            identify(),
        ))
        .unwrap();
    assert!(
        matches!(&applied.outcome, Outcome::Partial(report) if report.error == IoError::AlreadyExists),
        "{:?}",
        applied.outcome
    );
    let folder = sorted(d);
    assert_eq!(
        (folder.user, folder.trash),
        (user(&[("d/n", b"theirs")]), vec![b"mine".to_vec()])
    );
}

fn pending_marker(path: &RelPath) -> bool {
    path.parent() == Some(layout().pending_dir(WRITER))
        && path.name().is_some_and(|name| !name.ends_with(".json"))
}

#[test]
fn staged_bytes_a_sweep_into_the_trash_left_half_copied_are_swept_again() {
    let save = FileChange::Save {
        entity: E,
        path: path("d/n"),
        content: Content(0),
        expect: Expect::Absent,
    };
    let mut counting = Refusing::new(copying(), Some("d/n"));
    let (_, applied) = run_effects(
        &mut counting,
        std::slice::from_ref(&save),
        b"mine",
        &bound(&[]),
    )
    .unwrap();
    assert!(matches!(applied.outcome, Outcome::Partial(_)));
    let trash = layout().trash_dir(WRITER);
    let mutations: Vec<&Io> = counting.asked.iter().filter(|io| io.mutates()).collect();
    let swept = mutations
        .iter()
        .position(|io| matches!(io, Io::Create { path, .. } if path.starts_with(&trash)))
        .expect("the staged file is swept into the trash");

    let disk = copying();
    disk.crash_after(swept as u64 + 1);
    let mut crashing = Refusing::new(disk.clone(), Some("d/n"));
    assert!(run_effects(
        &mut crashing,
        std::slice::from_ref(&save),
        b"mine",
        &bound(&[])
    )
    .is_err());
    let disk = disk.restart();
    let record = effects_record(&crashing);
    let mut again = Refusing::new(disk.clone(), Some("d/n"));
    let pending = blocking::run(
        &mut again,
        toshokan::pending::read_one(&layout(), WRITER, record),
    )
    .unwrap()
    .unwrap();
    let applied = blocking::run(
        &mut again,
        recovery::settle(&layout(), record, Rc::new(pending), identify()),
    )
    .unwrap();
    assert!(
        matches!(applied.outcome, Outcome::Partial(_)),
        "{:?}",
        applied.outcome
    );
    let folder = sorted(&mut common::BlockingMem(disk));
    assert_eq!(
        (folder.trash, folder.staged),
        (vec![b"mine".to_vec()], Vec::<Vec<u8>>::new())
    );
}

#[test]
fn without_directory_renames_a_tree_moves_file_by_file_and_removes_its_directories() {
    let caps = Capabilities {
        rename_dir: false,
        ..Capabilities::ALL
    };
    let d = &mut common::BlockingMem(MemDisk::with_capabilities(caps, caps));
    put(d, &[("x/y/1", b"1"), ("x/2", b"2")]);
    let mut bindings = bound(&[(E, "x/y/1")]);
    bindings.unbound = vec![path("x/2")];
    let tree = FileChange::MoveTree {
        from: path("x"),
        to: path("z"),
    };
    let plan = effects::resolve(
        std::slice::from_ref(&tree),
        &bindings,
        &layout(),
        caps,
        &mut env(1),
    )
    .unwrap();
    let rename = |from: &str, to: &str| EffectStep::Rename {
        from: path(from),
        to: path(to),
    };
    let remove = |dir: &str| EffectStep::RemoveDir { path: path(dir) };
    assert_eq!(
        plan.steps,
        [
            rename("x/2", "z/2"),
            rename("x/y/1", "z/y/1"),
            remove("x/y"),
            remove("x")
        ]
    );
    let applied = commit(d, &[tree], vec![], &bindings, &mut env(1)).unwrap();
    assert_eq!(applied.outcome, Outcome::Complete);
    assert_eq!(sorted(d).user, user(&[("z/2", b"2"), ("z/y/1", b"1")]));
    assert!(!d.0.directories(Root::Folder).contains(&path("x")));
}

#[test]
fn an_intent_is_refused_for_overlapping_or_reserved_paths() {
    let bindings = bound(&[(E, "a"), (EntityId::from_u128(2), "x/f")]);
    let save = |text: &str, entity: u128| FileChange::Save {
        entity: EntityId::from_u128(entity),
        path: path(text),
        content: Content(0),
        expect: Expect::Absent,
    };
    let tree = FileChange::MoveTree {
        from: path("x"),
        to: path("y"),
    };
    let refusals = [
        (
            vec![save(".t/writers/w", 1)],
            Invalid::ReservedPath(path(".t/writers/w")),
        ),
        (
            vec![save("b", 1), save("b", 3)],
            Invalid::Overlapping(path("b")),
        ),
        (
            vec![save("b", 1), save("c", 1)],
            Invalid::Overlapping(path("c")),
        ),
        (
            vec![tree.clone(), save("y/z", 1)],
            Invalid::Overlapping(path("y/z")),
        ),
        (vec![save("q", 2), tree], Invalid::Overlapping(path("x"))),
        (
            vec![FileChange::Trash {
                entity: EntityId::from_u128(9),
                expect: Expect::Absent,
            }],
            Invalid::NoFile(EntityId::from_u128(9)),
        ),
    ];
    for (changes, invalid) in refusals {
        let resolved = effects::resolve(
            &changes,
            &bindings,
            &layout(),
            Capabilities::ALL,
            &mut env(1),
        );
        assert_eq!(resolved, Err(Refusal::Invalid(invalid)), "{changes:?}");
    }
}

#[test]
fn a_restore_over_a_file_trashes_it_first() {
    let restore = FileChange::Restore {
        entity: E,
        item: Nonce::from_u128(5),
        to: path("a"),
        expect: Expect::Holds(identity(b"now")),
    };
    let plan = effects::resolve(
        &[restore],
        &bound(&[]),
        &layout(),
        Capabilities::ALL,
        &mut env(1),
    )
    .unwrap();
    assert!(matches!(
        &plan.steps[..],
        [EffectStep::ToTrash { path: p, .. }, EffectStep::FromTrash { item, path: q }]
            if *p == path("a") && *q == path("a") && *item == Nonce::from_u128(5)
    ));
}

/// The largest bytes one request carries: those an operation asks for, or those
/// a backend is asked to write.
fn carried(io: &Io) -> usize {
    match io {
        Io::Create { bytes, .. } | Io::Append { bytes, .. } | Io::Write { bytes, .. } => {
            bytes.len()
        }
        _ => 0,
    }
}

/// An operation whose requests are measured as it makes them.
struct Measured<O> {
    operation: O,
    largest: Rc<Cell<usize>>,
}

impl<O: Operation> Operation for Measured<O> {
    type Output = O::Output;

    fn resume(&mut self, result: Option<IoResult>) -> Step<O::Output> {
        let step = self.operation.resume(result);
        if let Step::Io(io) = &step {
            self.largest.set(self.largest.get().max(carried(io)));
        }
        step
    }
}

/// A disk that measures the writes it is asked for.
struct Measuring {
    disk: MemDisk,
    largest: usize,
}

impl Backend for Measuring {
    fn capabilities(&self, root: Root) -> Capabilities {
        self.disk.capabilities(root)
    }

    fn perform(&mut self, io: Io) -> IoResult {
        self.largest = self.largest.max(carried(&io));
        self.disk.perform(io)
    }
}

#[test]
fn a_saved_file_streams_into_staging_without_passing_through_the_core() {
    let disk = MemDisk::new();
    let big: Vec<u8> = (0..3 * CHUNK + 7).map(|i| (i % 251) as u8).collect();
    put(
        &mut common::BlockingMem(disk.clone()),
        &[("big.npno", &big)],
    );
    let splice = Splice {
        from: path("big.npno"),
        pieces: vec![kept(0, big.len() as u64), Piece::Bytes(b"!".to_vec())],
    };
    let plan = Rc::new(
        effects::resolve(
            &[save_over("big.npno", &big)],
            &bound(&[]),
            &layout(),
            Capabilities::ALL,
            &mut env(1),
        )
        .unwrap(),
    );
    let record = Rc::new(PendingRecord::new(WRITER, "Save", entry(HEAD), &plan));
    let mut backend = Measuring {
        disk: disk.clone(),
        largest: 0,
    };
    let largest = Rc::new(Cell::new(0));
    let core = Measured {
        operation: effects::prepare(&layout(), Rc::clone(&plan), record, identify()),
        largest: Rc::clone(&largest),
    };
    let sources: Vec<Box<dyn blocking::Source<Measuring>>> = vec![Box::new(splice)];
    blocking::run_with(&mut backend, sources, core)
        .unwrap()
        .unwrap();
    let largest = largest.get();
    assert!(largest < 4096, "the core asked to write {largest} bytes");
    assert_eq!(
        backend.largest as u64, CHUNK,
        "the driver writes a chunk at a time"
    );
    let mut expected = big;
    expected.push(b'!');
    let staged = disk
        .files(Root::Folder)
        .into_iter()
        .find(|(p, _)| p.starts_with(&layout().tmp_dir(WRITER)))
        .map(|(_, bytes)| bytes);
    assert!(staged == Some(expected), "the staged file holds the splice");
}

/// A folder another program writes in: once the pending record of an intent is
/// written, after its preconditions are checked, it makes a file at `at`.
struct Intruder {
    disk: MemDisk,
    at: RelPath,
}

impl Backend for Intruder {
    fn capabilities(&self, root: Root) -> Capabilities {
        self.disk.capabilities(root)
    }

    fn perform(&mut self, io: Io) -> IoResult {
        let journaled =
            matches!(&io, Io::Rename { to, .. } if to.starts_with(&layout().pending_dir(WRITER)));
        let result = self.disk.perform(io);
        if journaled {
            put(
                &mut common::BlockingMem(self.disk.clone()),
                &[(self.at.as_str(), b"theirs")],
            );
        }
        result
    }
}

#[test]
fn where_renames_may_replace_a_file_made_at_a_destination_after_the_checks_is_kept() {
    let racy = Capabilities {
        no_replace: false,
        ..Capabilities::ALL
    };
    let disk = MemDisk::with_capabilities(racy, Capabilities::ALL);
    let save = FileChange::Save {
        entity: E,
        path: path("d/n"),
        content: Content(0),
        expect: Expect::Absent,
    };
    let plan =
        Rc::new(effects::resolve(&[save], &bound(&[]), &layout(), racy, &mut env(1)).unwrap());
    let record = Rc::new(PendingRecord::new(WRITER, "Save", entry(HEAD), &plan));
    let mut intruder = Intruder {
        disk: disk.clone(),
        at: path("d/n"),
    };
    let prepare = effects::prepare(&layout(), Rc::clone(&plan), Rc::clone(&record), identify());
    let sources: Vec<Box<dyn blocking::Source<Intruder>>> = vec![Box::new(b"mine".to_vec())];
    blocking::run_with(&mut intruder, sources, prepare)
        .unwrap()
        .unwrap();
    let apply = effects::apply(&layout(), plan.record, record, 0, identify());
    let applied = blocking::run(&mut intruder, apply).unwrap();
    assert!(
        matches!(&applied.outcome, Outcome::Partial(report) if report.error == IoError::AlreadyExists),
        "{:?}",
        applied.outcome
    );
    let d = &mut common::BlockingMem(disk);
    assert_eq!(sorted(d).user, user(&[("d/n", b"theirs")]));
    assert_eq!(sorted(d).trash, [b"mine".to_vec()]);
}
