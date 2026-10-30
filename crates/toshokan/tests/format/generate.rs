//! Writes the fixture states, once. A state already in `tests/fixtures` is never
//! written again: a format change adds a state instead.
//!
//! ```sh
//! cargo test -p toshokan --test format -- --ignored --exact generate::fixtures
//! ```

use std::collections::BTreeMap;

use toshokan::env::TestClock;
use toshokan::log::Settlement;
use toshokan::report::Orphan;
use toshokan::simulator::Machine;
use toshokan::{EntityId, Expect, Io, MemDisk, Root};

use crate::fixture::{
    identity, layout, open, path, put, Fault, Faulty, Library, COPYING, EMPTY_DIRS, NOTE, ORIGIN,
    TAGS,
};

const VERSION: &str = env!("CARGO_PKG_VERSION");

/// A fresh machine on its own folder.
fn machine(folder: &MemDisk) -> Machine {
    Machine {
        folder: folder.clone(),
        local: MemDisk::new(),
    }
}

/// A copy of what `disk` holds in `root`, on a new disk whose folder can do what
/// `disk`'s can.
fn copied(disk: &MemDisk, root: Root) -> MemDisk {
    let copy = MemDisk::with_capabilities(disk.capabilities(Root::Folder), disk.capabilities(root));
    put(&copy, root, &disk.files(root));
    copy
}

/// Puts each file of `from`'s folder that `into`'s lacks into `into`, as a sync
/// client delivers another machine's files.
fn deliver(from: &MemDisk, into: &MemDisk) {
    let have = into.files(Root::Folder);
    let mut missing = from.files(Root::Folder);
    missing.retain(|path, _| !have.contains_key(path));
    put(into, Root::Folder, &missing);
}

/// Puts `bytes` at `text` in the folder, as another program would.
fn outside(folder: &MemDisk, text: &str, bytes: &[u8]) {
    put(
        folder,
        Root::Folder,
        &BTreeMap::from([(path(text), bytes.to_vec())]),
    );
}

/// Moves the file at `from` to `to`, as another program would.
fn move_outside(folder: &MemDisk, from: &str, to: &str) {
    let to = path(to);
    folder
        .perform(Io::MakeDir {
            root: Root::Folder,
            path: to.parent().unwrap(),
        })
        .unwrap();
    folder
        .perform(Io::Rename {
            root: Root::Folder,
            from: path(from),
            to,
        })
        .unwrap();
}

fn holds(bytes: &[u8]) -> Expect {
    Expect::Holds(identity(bytes))
}

/// The instance stops without closing: what it wrote stays, and its locks go.
fn stop(library: Library, machine: &mut Machine) {
    drop(library);
    machine.crash();
}

/// The instance stops, and its install loses its local root: it never writes as
/// its writer again.
fn vanish(library: Library, machine: &mut Machine) {
    drop(library);
    machine.lose_local();
}

/// The directories of `root` that hold nothing, which a tree of files in git cannot
/// keep.
fn empty_dirs(disk: &MemDisk, root: Root) -> Vec<String> {
    let dirs = disk.directories(root);
    let files = disk.files(root);
    let holds = |dir: &toshokan::RelPath| {
        let under = |path: &toshokan::RelPath| path != dir && path.starts_with(dir);
        files.keys().any(under) || dirs.iter().any(under)
    };
    let empty = dirs.iter().filter(|dir| !holds(dir));
    empty.map(|dir| dir.as_str().to_owned()).collect()
}

/// Writes the trees of `machine` as state `name`, with its README, unless the
/// state is there already.
fn write_state(name: &str, machine: &Machine, readme: &str) {
    let dir = crate::fixture::fixtures().join(name);
    if dir.exists() {
        println!("{name}: kept");
        return;
    }
    let trees = [
        ("folder", &machine.folder, Root::Folder),
        ("local", &machine.local, Root::Local),
    ];
    let mut empty = BTreeMap::new();
    for (tree, disk, root) in trees {
        for (file, bytes) in disk.files(root) {
            let at = dir.join(tree).join(file.as_str());
            std::fs::create_dir_all(at.parent().unwrap()).unwrap();
            std::fs::write(at, bytes).unwrap();
        }
        empty.insert(tree, empty_dirs(disk, root));
    }
    let empty = serde_json::to_string_pretty(&empty).unwrap();
    std::fs::write(dir.join(EMPTY_DIRS), empty + "\n").unwrap();
    std::fs::write(dir.join("README.md"), readme).unwrap();
    println!("{name}: written");
}

fn readme(name: &str, seeds: &str, what: &str) -> String {
    format!(
        "# {name}\n\n{what}\n\nWritten by toshokan {VERSION} with \
         `cargo test -p toshokan --test format -- --ignored --exact generate::fixtures`, \
         from `tests/format/generate.rs`: {seeds}, a `TestClock` from 1000 ms, \
         `PrefixIdentity` over 64 KiB and the layout `.t`.\n"
    )
}

#[test]
#[ignore = "writes the fixture states; run once per new state"]
fn fixtures() {
    one_writer();
    two_writers_fork();
    pending();
    local();
}

/// The library files one writer starts from.
fn library_files(folder: &MemDisk) {
    outside(folder, "loose/adopt.syx", b"adopted");
    outside(folder, "plain.txt", b"plain");
    outside(folder, "junk.txt", b"junk");
    outside(folder, "tree/a.syx", b"tree a");
    folder
        .perform(Io::MakeDir {
            root: Root::Folder,
            path: path("empty"),
        })
        .unwrap();
}

/// What one writer, seed 1, writes: every op an intent logs but those that need
/// another writer or a fork. Returns the library, still open, and entity `one`.
fn one_writer_history(machine: &Machine, clock: &TestClock) -> (Library, Faulty, EntityId) {
    library_files(&machine.folder);
    let backend = Faulty::new(machine);
    let (mut lib, _) = open(&backend, "fixture", 1, clock);
    lib.rescan().unwrap();
    let tick = || clock.advance(1000);

    tick();
    let (intent, one) = lib.intent("Create").create(|e| {
        e.set(ORIGIN, "one".into())
            .set(NOTE, Some("first".into()))
            .add(TAGS, "a".into())
            .add(TAGS, "b".into())
            .save(&path("one.syx"), b"one v1".to_vec(), Expect::Absent);
    });
    let (intent, two) = intent.create(|e| {
        e.set(ORIGIN, "two".into()).add(TAGS, "t".into()).save(
            &path("two.syx"),
            b"two".to_vec(),
            Expect::Absent,
        );
    });
    let (intent, three) = intent.create(|e| {
        e.adopt(&path("loose/adopt.syx"), holds(b"adopted"));
    });
    let (intent, four) = intent.create(|e| {
        e.add(TAGS, "tree".into()).save(
            &path("tree/sub/four.syx"),
            b"four".to_vec(),
            Expect::Absent,
        );
    });
    let (intent, five) = intent.create(|e| {
        e.save(&path("five.syx"), b"five".to_vec(), Expect::Absent);
    });
    intent.commit().unwrap();

    tick();
    lib.intent("Edit")
        .set(one, ORIGIN, "uno".into())
        .set(one, NOTE, None)
        .remove(one, TAGS, &"a".into())
        .clear(two, ORIGIN)
        .add(two, TAGS, "u".into())
        .commit()
        .unwrap();

    tick();
    lib.intent("Save over")
        .save(one, &path("one.syx"), b"one v2".to_vec(), holds(b"one v1"))
        .commit()
        .unwrap();

    tick();
    lib.intent("Trash")
        .trash(two, holds(b"two"))
        .commit()
        .unwrap();

    tick();
    lib.intent("Delete").delete(two).commit().unwrap();

    tick();
    lib.intent("Files")
        .save_file(&path("new.txt"), b"new".to_vec(), Expect::Absent)
        .rename_file(&path("plain.txt"), &path("renamed.txt"), holds(b"plain"))
        .trash_file(&path("junk.txt"), holds(b"junk"))
        .commit()
        .unwrap();

    tick();
    lib.intent("Tree")
        .move_tree(&path("tree"), &path("forest"))
        .commit()
        .unwrap();

    tick();
    lib.intent("Dirs")
        .make_dir(&path("made"))
        .remove_dir(&path("empty"))
        .commit()
        .unwrap();

    move_outside(&machine.folder, "five.syx", "moved/five.syx");
    lib.rescan().unwrap();
    tick();
    lib.intent("Into made")
        .save_file(&path("made/inside.txt"), b"inside".to_vec(), Expect::Absent)
        .commit()
        .unwrap();

    println!("one writer: one {one}, two {two}, three {three}, four {four}, five {five}");
    (lib, backend, one)
}

fn one_writer() {
    let clock = TestClock::at(1000);
    let mut open_machine = machine(&MemDisk::new());
    let (lib, _, _) = one_writer_history(&open_machine, &clock);
    stop(lib, &mut open_machine);
    write_state(
        "one-writer-open",
        &open_machine,
        &readme(
            "one-writer-open",
            "seed 1",
            "One writer's log, its last segment left open by an instance that stopped \
             without closing, and its local root.",
        ),
    );

    let clock = TestClock::at(1000);
    let mut compacted = machine(&MemDisk::new());
    let (mut lib, backend, one) = one_writer_history(&compacted, &clock);
    lib.compact().unwrap();
    clock.advance(1000);
    lib.intent("After")
        .add(one, TAGS, "after".into())
        .commit()
        .unwrap();
    let tmp = layout().tmp_dir(lib.writer_id().unwrap());
    backend.fault(move |io| match io {
        Io::Create { path, .. } if path.starts_with(&tmp) => Some(Fault::Fail),
        _ => None,
    });
    lib.close().unwrap();
    backend.heal();
    compacted.crash();
    write_state(
        "one-writer-compacted",
        &compacted,
        &readme(
            "one-writer-compacted",
            "seed 1",
            "The history of `one-writer-open`, compacted into a snapshot, then one more \
             intent in a segment sealed at close, whose compaction failed to stage its \
             snapshot.",
        ),
    );
}

fn two_writers_fork() {
    let clock = TestClock::at(1000);
    let folder = MemDisk::new();
    let mut a_machine = machine(&folder);
    let mut b_machine = machine(&folder);
    let a = Faulty::new(&a_machine);
    let b = Faulty::new(&b_machine);

    let (mut lib, _) = open(&a, "a", 11, &clock);
    lib.rescan().unwrap();
    clock.advance(1000);
    let (intent, song) = lib.intent("Song").create(|e| {
        e.set(ORIGIN, "a".into()).add(TAGS, "x".into()).save(
            &path("song.syx"),
            b"song".to_vec(),
            Expect::Absent,
        );
    });
    intent.commit().unwrap();
    lib.close().unwrap();

    let (mut lib, _) = open(&b, "b", 12, &clock);
    lib.rescan().unwrap();
    clock.advance(1000);
    let (intent, other) = lib
        .intent("Edit")
        .set(song, ORIGIN, "b".into())
        .add(song, TAGS, "y".into())
        .create(|e| {
            e.set(ORIGIN, "b2".into())
                .save(&path("other.syx"), b"other".to_vec(), Expect::Absent);
        });
    intent.commit().unwrap();
    lib.close().unwrap();

    let (mut lib, _) = open(&a, "a", 13, &clock);
    lib.rescan().unwrap();
    clock.advance(1000);
    lib.intent("Reply")
        .remove(song, TAGS, &"y".into())
        .set(other, ORIGIN, "a2".into())
        .commit()
        .unwrap();
    lib.close().unwrap();

    let branch = |seed: u64, tag: &str, local: &MemDisk| {
        let branch_machine = Machine {
            folder: copied(&folder, Root::Folder),
            local: copied(local, Root::Local),
        };
        let backend = Faulty::new(&branch_machine);
        let (mut lib, _) = open(&backend, "a", seed, &clock);
        clock.advance(1000);
        lib.intent(&format!("Branch {tag}"))
            .add(song, TAGS, tag.into())
            .commit()
            .unwrap();
        (lib, branch_machine)
    };
    let (here, here_machine) = branch(14, "here", &a_machine.local);
    let (there, there_machine) = branch(15, "there", &a_machine.local);
    let (lines, mut lines_machine) = branch(16, "lines", &a_machine.local);
    here.close().unwrap();
    there.close().unwrap();
    stop(lines, &mut lines_machine);
    for branch in [&here_machine, &there_machine, &lines_machine] {
        deliver(&branch.folder, &folder);
    }
    a_machine.lose_local();

    let (mut lib, _) = open(&b, "b", 17, &clock);
    lib.rescan().unwrap();
    let items = lib.view().items().to_vec();
    let run_making = |text: &str| {
        let item = items.iter().find(|item| {
            item.changes.iter().any(|c| {
                matches!(&c.change, Some(toshokan::plan::FactChange::Add { value, .. })
                    if value.as_str() == format!("\"{text}\""))
            })
        });
        let item = item.unwrap_or_else(|| panic!("no item adds {text}: {items:?}"));
        (item.run, item.changes[0].change.clone().unwrap())
    };
    let (here_run, here_change) = run_making("here");
    let (lines_run, lines_change) = run_making("lines");
    let (there_run, _) = run_making("there");
    clock.advance(1000);
    lib.intent("Keep lines")
        .take(lines_run, lines_change)
        .dismiss(there_run)
        .commit()
        .unwrap();
    lib.close().unwrap();

    let (mut lib, _) = open(&b, "b", 18, &clock);
    lib.rescan().unwrap();
    clock.advance(1000);
    lib.intent("Keep here")
        .take(here_run, here_change)
        .dismiss(lines_run)
        .commit()
        .unwrap();
    clock.advance(1000);
    lib.intent("Later")
        .set(other, ORIGIN, "b3".into())
        .commit()
        .unwrap();
    stop(lib, &mut b_machine);

    println!("two writers: song {song}, other {other}");
    println!("runs: here {here_run:?}, lines {lines_run:?}, there {there_run:?}");
    write_state(
        "two-writers-fork",
        &b_machine,
        &readme(
            "two-writers-fork",
            "seeds 11 to 18",
            "Writers `a` and `b` naming each other's writes. `a`'s history forks three \
             ways after a copy of its local root: two branches compacted into tips, one \
             left as a line. `b` takes from the line's run and dismisses one tip's run, \
             compacts, then takes from the other tip's run, dismisses the line's run \
             and writes once more, its segment left open. The local root is `b`'s.",
        ),
    );
}

fn pending() {
    let clock = TestClock::at(1000);
    let folder = MemDisk::new();
    outside(&folder, "loose.txt", b"loose");
    outside(&folder, "u.txt", b"unbound");
    outside(&folder, "t/in.txt", b"in tree");
    let mut seed = 20;
    let mut next = |label: &str, machine: &Machine| {
        seed += 1;
        let backend = Faulty::new(machine);
        let (mut lib, opened) = open(&backend, label, seed, &clock);
        lib.rescan().unwrap();
        clock.advance(1000);
        (lib, backend, opened)
    };

    let mut m = machine(&folder);
    let (mut lib, _, _) = next("setup", &m);
    let (intent, one) = lib.intent("Setup").create(|e| {
        e.save(&path("one.syx"), b"one".to_vec(), Expect::Absent);
    });
    let (intent, two) = intent.create(|e| {
        e.save(&path("two.syx"), b"two".to_vec(), Expect::Absent);
    });
    let (intent, three) = intent.create(|e| {
        e.save(&path("three.syx"), b"three".to_vec(), Expect::Absent);
    });
    let (intent, four) = intent.create(|e| {
        e.save(&path("four.syx"), b"four".to_vec(), Expect::Absent);
    });
    let (intent, spent) = intent.create(|e| {
        e.save(&path("spent.syx"), b"spent".to_vec(), Expect::Absent);
    });
    intent.commit().unwrap();
    vanish(lib, &mut m);

    // An open record naming every kind of end, stopped at the rename of `u.txt`.
    let (mut lib, backend, _) = next("open", &m);
    backend.fault(|io| match io {
        Io::Rename { to, .. } if to.as_str() == "v.txt" => Some(Fault::Fail),
        _ => None,
    });
    let (intent, adopted) = lib
        .intent("Everything")
        .save(one, &path("one.syx"), b"one v2".to_vec(), holds(b"one"))
        .make_dir(&path("nd"))
        .move_tree(&path("t"), &path("t2"))
        .rename_file(&path("u.txt"), &path("v.txt"), holds(b"unbound"))
        .create(|e| {
            e.adopt(&path("loose.txt"), holds(b"loose"));
        });
    assert!(intent.commit().is_err(), "the rename is faulted");
    backend.heal();
    vanish(lib, &mut m);

    // Records another writer settles, one per outcome.
    for (label, entity, at, bytes) in [
        ("Finish me", two, "two-moved.syx", &b"two"[..]),
        ("Roll me back", three, "three-moved.syx", b"three"),
        ("Dismiss me", four, "four-moved.syx", b"four"),
    ] {
        let (mut lib, backend, _) = next("settled", &m);
        backend.fault(move |io| match io {
            Io::Rename { to, .. } if to.as_str() == at => Some(Fault::Fail),
            _ => None,
        });
        let moved = lib
            .intent(label)
            .rename(entity, &path(at), holds(bytes))
            .commit();
        assert!(moved.is_err(), "the rename is faulted");
        backend.heal();
        vanish(lib, &mut m);
    }

    // A record its own entry closed, left by a removal that failed.
    let (mut lib, backend, _) = next("spent", &m);
    backend.fault(|io| match io {
        Io::Remove { path, .. } if path.as_str().contains("/pending/") => Some(Fault::Fail),
        _ => None,
    });
    lib.intent("Spent")
        .save(
            spent,
            &path("spent.syx"),
            b"spent v2".to_vec(),
            holds(b"spent"),
        )
        .commit()
        .unwrap();
    backend.heal();
    vanish(lib, &mut m);

    // A folder whose moves copy, delivered into this one.
    let copying = MemDisk::with_capabilities(COPYING, toshokan::io::Capabilities::ALL);
    put(&copying, Root::Folder, &folder.files(Root::Folder));
    let mut c = machine(&copying);
    let (mut lib, backend, _) = next("copy", &c);
    let (intent, copied_entity) = lib.intent("Copy setup").create(|e| {
        e.save(&path("c/x.syx"), b"copied".to_vec(), Expect::Absent);
    });
    intent.commit().unwrap();
    backend.fault(|io| match io {
        Io::Remove { path, .. } if path.as_str() == "c/x.syx" => Some(Fault::Fail),
        _ => None,
    });
    let stopped = lib
        .intent("Copy move")
        .rename(copied_entity, &path("c/y.syx"), holds(b"copied"))
        .commit();
    assert!(stopped.is_err(), "the copy's removal is faulted");
    backend.heal();
    vanish(lib, &mut c);

    let (mut lib, backend, _) = next("cut", &c);
    backend.fault(|io| match io {
        Io::Create { path, .. } if path.as_str().contains("/pending/") => Some(Fault::Empty),
        _ => None,
    });
    let cut = lib
        .intent("Cut short")
        .save_file(&path("c/cut.txt"), b"cut".to_vec(), Expect::Absent)
        .commit();
    assert!(cut.is_err(), "the record is cut short");
    backend.heal();
    vanish(lib, &mut c);
    deliver(&copying, &folder);

    let (mut lib, _, opened) = next("settler", &m);
    let orphans: Vec<Orphan> = opened.orphaned.clone();
    for (label, how) in [
        ("Finish me", Settlement::Finished),
        ("Roll me back", Settlement::RolledBack),
        ("Dismiss me", Settlement::Dismissed),
    ] {
        let orphan = orphans.iter().find(|o| o.label == label);
        let orphan = orphan
            .unwrap_or_else(|| panic!("{label} in {orphans:?}"))
            .clone();
        clock.advance(1000);
        lib.settle(orphan, how).unwrap();
    }
    stop(lib, &mut m);

    println!(
        "pending: one {one}, two {two}, three {three}, four {four}, spent {spent}, \
         adopted {adopted}, copied {copied_entity}"
    );
    write_state(
        "pending",
        &m,
        &readme(
            "pending",
            "seeds 21 to 29",
            "Pending records of writers whose installs lost their local roots: one open \
             record naming every kind of end, stopped partway; three that writer \
             `settler` finished, rolled back and dismissed; one its own entry closed; \
             from a folder whose moves copy, one stopped with its markers and one cut \
             short before its bytes landed. The local root is `settler`'s.",
        ),
    );
}

fn local() {
    let clock = TestClock::at(1000);
    let folder = MemDisk::new();
    outside(&folder, "other.syx", b"othr");
    let mut here = machine(&folder);
    let backend = Faulty::new(&here);
    let (mut lib, _) = open(&backend, "local", 41, &clock);
    lib.rescan().unwrap();
    clock.advance(1000);
    let (intent, gone) = lib.intent("Gone").create(|e| {
        e.save(&path("gone.syx"), b"gone".to_vec(), Expect::Absent);
    });
    intent.commit().unwrap();
    lib.close().unwrap();

    let there = here.cloned();
    let (mut original, _) = open(&Faulty::new(&here), "local", 42, &clock);
    let (mut copy, _) = open(&Faulty::new(&there), "local", 43, &clock);
    clock.advance(1000);
    original
        .intent("Here")
        .add(gone, TAGS, "here".into())
        .commit()
        .unwrap();
    copy.intent("There")
        .add(gone, TAGS, "there".into())
        .commit()
        .unwrap();
    drop(copy);
    let refreshed = original.refresh().unwrap();
    assert!(refreshed.retired.is_some(), "the fork retires the writer");
    folder
        .perform(Io::Remove {
            root: Root::Folder,
            path: path("gone.syx"),
        })
        .unwrap();
    original.rescan().unwrap();
    clock.advance(1000);
    original
        .intent("New writer")
        .add(gone, TAGS, "after".into())
        .commit()
        .unwrap();
    original.close().unwrap();
    here.crash();

    println!("local: gone {gone}");
    write_state(
        "local",
        &here,
        &readme(
            "local",
            "seeds 41 to 43",
            "A local root holding a writer retired when a copy of the local root forked \
             it, the writer that took over, and the identities its scans read: of \
             `other.syx`, which is as long as the file of an entity whose file is gone.",
        ),
    );
}
