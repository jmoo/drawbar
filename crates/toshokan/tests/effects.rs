//! File effects on every backend: what a committed intent leaves in the folder,
//! and that a refused one leaves nothing.

mod common;

use std::collections::BTreeMap;

use common::{bound, commit, env, files, identity, layout, path, put, Driven, WRITER};
use toshokan::effects::{self, EffectStep};
use toshokan::error::{Invalid, Mismatch};
use toshokan::io::Capabilities;
use toshokan::log::{Displaced, FileFact};
use toshokan::plan::{Expect, FileChange, Target};
use toshokan::{EntityId, MemDisk, Nonce, Outcome, Refusal, RelPath, Root};

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

mod suite {
    use super::*;

    pub fn a_save_places_new_bytes_and_moves_the_old_into_the_trash(d: &mut impl Driven) {
        put(d, &[("a.syx", b"old bytes")]);
        let save = FileChange::Save {
            entity: Target::Existing(E),
            path: path("a.syx"),
            bytes: b"new bytes".to_vec(),
            expect: Expect::Holds(identity(b"old bytes")),
        };
        let applied = commit(d, &[save], &bound(&[]), &mut env(1)).unwrap();
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
            entity: Target::Existing(E),
            path: path("x/y/new.syx"),
            bytes: b"new".to_vec(),
            expect: Expect::Absent,
        };
        let applied = commit(d, &[save], &bound(&[]), &mut env(1)).unwrap();
        fact("x/y/new.syx", b"new", &applied.files);
        assert!(applied.displaced.is_empty());
        assert_eq!(sorted(d).user, user(&[("x/y/new.syx", b"new")]));
    }

    pub fn a_failed_precondition_refuses_and_leaves_nothing_behind(d: &mut impl Driven) {
        put(d, &[("a.syx", b"theirs")]);
        for expect in [Expect::Absent, Expect::Holds(identity(b"mine"))] {
            let save = FileChange::Save {
                entity: Target::Existing(E),
                path: path("a.syx"),
                bytes: b"new".to_vec(),
                expect,
            };
            let refused = commit(d, &[save], &bound(&[]), &mut env(1)).unwrap_err();
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
        let applied = commit(d, &[trash], &bound(&[(E, "d/a.syx")]), &mut env(1)).unwrap();
        assert_eq!(applied.files, [(E, None)]);
        let item = applied.displaced[0].item;
        assert_eq!(sorted(d).user, user(&[]));
        let restore = FileChange::Restore {
            entity: E,
            item,
            to: path("d/a.syx"),
            expect: Expect::Absent,
        };
        let applied = commit(d, &[restore], &bound(&[]), &mut env(2)).unwrap();
        fact("d/a.syx", b"bytes", &applied.files);
        assert_eq!(sorted(d).user, user(&[("d/a.syx", b"bytes")]));
        let gone = FileChange::Restore {
            entity: E,
            item,
            to: path("elsewhere"),
            expect: Expect::Absent,
        };
        assert_eq!(
            commit(d, &[gone], &bound(&[]), &mut env(3)).unwrap_err(),
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
        let refused = commit(d, &[rename("b")], &bindings, &mut env(1)).unwrap_err();
        assert!(
            matches!(&refused, Refusal::Changed(m) if m.path == path("b")),
            "{refused:?}"
        );
        let applied = commit(d, &[rename("c/a")], &bindings, &mut env(2)).unwrap();
        fact("c/a", b"a", &applied.files);
        assert_eq!(sorted(d).user, user(&[("b", b"b"), ("c/a", b"a")]));
    }

    pub fn a_tree_moves_with_every_file_in_it(d: &mut impl Driven) {
        put(d, &[("x/y/1", b"1"), ("x/2", b"2"), ("xx", b"3")]);
        let mut bindings = bound(&[(E, "x/y/1")]);
        bindings.unbound = vec![path("x/2"), path("xx")];
        let tree = FileChange::MoveTree {
            from: path("x"),
            to: path("z/x"),
        };
        let applied = commit(d, &[tree], &bindings, &mut env(1)).unwrap();
        fact("z/x/y/1", b"1", &applied.files);
        assert_eq!(
            sorted(d).user,
            user(&[("xx", b"3"), ("z/x/2", b"2"), ("z/x/y/1", b"1")])
        );
    }
}

for_every_backend!(suite:
    a_save_places_new_bytes_and_moves_the_old_into_the_trash,
    a_save_where_nothing_is_makes_its_directories,
    a_failed_precondition_refuses_and_leaves_nothing_behind,
    trashing_then_restoring_brings_the_same_bytes_back,
    a_rename_is_refused_over_an_existing_file,
    a_tree_moves_with_every_file_in_it,
);

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
        &[],
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
    let applied = commit(d, &[tree], &bindings, &mut env(1)).unwrap();
    assert_eq!(applied.outcome, Outcome::Complete);
    assert_eq!(sorted(d).user, user(&[("z/2", b"2"), ("z/y/1", b"1")]));
    assert!(!d.0.directories(Root::Folder).contains(&path("x")));
}

#[test]
fn an_intent_is_refused_for_overlapping_or_reserved_paths() {
    let bindings = bound(&[(E, "a"), (EntityId::from_u128(2), "x/f")]);
    let save = |text: &str, entity: u128| FileChange::Save {
        entity: Target::Existing(EntityId::from_u128(entity)),
        path: path(text),
        bytes: Vec::new(),
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
            &[],
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
        &[],
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
