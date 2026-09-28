//! A library on disk, opened, edited, changed behind the app's back and opened again.
//!
//! Every test works in its own directory under the system's temp folder, never in the
//! default library.

use std::collections::BTreeMap;
use std::fs;

use super::diff::{match_files, Known};
use super::sidecar::{self, Read};
use super::*;
use crate::testing::{Bench, Temp};
use crate::workspace::{Fresh, Origin};
use nord_usb::{Location, ObjectClass};

/// One run of the app over a library: the store, and the state it mirrors.
struct Session {
    store: Store,
    bench: Bench,
}

impl Session {
    fn open(root: &Temp) -> Session {
        let bench = Bench::new();
        let store = Store::start(Backend::start(&bench.ctx, root.0.clone()));
        let mut session = Session { store, bench };
        assert!(session.next(), "opening answered");
        session
    }

    fn next(&mut self) -> bool {
        let Bench {
            workspace,
            browser,
            queue,
            log,
            ..
        } = &mut self.bench;
        self.store.next(workspace, browser, queue, log)
    }

    /// Wait for the answer to everything sent so far: a rescan runs after all of it.
    fn settle(&mut self) {
        self.store.rescan();
        while self.store.scanning() {
            assert!(self.next(), "the rescan answered");
        }
    }

    /// Write everything, and wait until the disk has it.
    fn sync(&mut self) {
        let Bench {
            workspace,
            browser,
            queue,
            ..
        } = &mut self.bench;
        assert!(self.store.sync(workspace, browser, queue, Pass::Last));
        self.settle();
    }

    /// The window loses focus and gets it back.
    fn refocus(&mut self) {
        self.store.focus(false);
        self.store.focus(true);
        while self.store.scanning() {
            assert!(self.next(), "the rescan answered");
        }
    }

    /// Quit: everything is written first.
    fn close(mut self) {
        let Bench {
            workspace,
            browser,
            queue,
            log,
            ..
        } = &mut self.bench;
        self.store.close(workspace, browser, queue, log);
    }

    fn create(&mut self) -> u64 {
        self.bench
            .workspace
            .create(Fresh::Program, &mut self.bench.log)
            .expect("a new program")
    }

    fn bytes(&self, id: u64) -> Vec<u8> {
        self.bench.workspace.get(id).expect("held").bytes.clone()
    }

    fn path(&self, id: u64) -> Option<String> {
        let entity = self.bench.workspace.get(id)?;
        Some(entity.path.as_ref()?.to_string())
    }

    fn said(&self, words: &str) -> usize {
        self.bench
            .log
            .iter()
            .filter(|entry| entry.text.contains(words))
            .count()
    }
}

/// A program with its gain set, so two edits of one program are two different files.
fn with_gain(bytes: &[u8], gain: &str) -> Vec<u8> {
    crate::fields::apply(bytes, &[("center_panel.gain".into(), gain.into())])
        .expect("the gain is a field")
        .1
}

#[test]
fn a_new_asset_is_a_file_that_comes_back_with_its_id_and_tags() {
    let root = Temp::new();
    let mut first = Session::open(&root);
    let id = first.create();
    let tag = first.bench.browser.tags.make("Sunday").unwrap();
    first.bench.browser.tags.set(id, tag, true);
    let bytes = first.bytes(id);
    first.close();
    assert_eq!(root.read("untitled.ne5p"), bytes, "the file is the asset");

    let second = Session::open(&root);
    let entity = second.bench.workspace.get(id).expect("the same id");
    assert_eq!(entity.name, "untitled.ne5p");
    assert_eq!(entity.bytes, bytes);
    assert!(!entity.is_unsaved());
    assert!(second.bench.browser.tags.worn(id).contains(&tag));
    assert_eq!(second.bench.browser.tags.name_of(tag), Some("Sunday"));
}

#[test]
fn an_unsaved_edit_survives_a_restart_and_the_file_stays_as_last_saved() {
    let root = Temp::new();
    let mut first = Session::open(&root);
    let id = first.create();
    first.sync();
    let saved = first.bytes(id);
    let edited = with_gain(&saved, "96");
    first
        .bench
        .workspace
        .replace_bytes(id, edited.clone(), &mut first.bench.log);
    first.close();
    assert_eq!(root.read("untitled.ne5p"), saved, "nothing was saved");
    assert_eq!(root.names(".drawbar/working").len(), 1, "one working copy");

    let mut second = Session::open(&root);
    let entity = second.bench.workspace.get(id).expect("the same id");
    assert_eq!(entity.bytes, edited, "the edit");
    assert_eq!(entity.saved.bytes, saved, "and what it is an edit of");
    assert!(entity.is_unsaved());

    second.bench.workspace.mark_saved(id);
    second.sync();
    assert_eq!(
        root.read("untitled.ne5p"),
        edited,
        "the save wrote the file in place"
    );
    second.sync();
    assert!(
        root.names(".drawbar/working").is_empty(),
        "the working copy goes once the save has landed"
    );
}

/// ⚠️ A save sent before the file's first write answered would carry no fingerprint to
/// check, and be refused as a write over someone else's file.
#[test]
fn a_save_made_while_the_first_write_is_in_flight_lands_after_it() {
    let root = Temp::new();
    let mut session = Session::open(&root);
    let id = session.create();
    let Bench {
        workspace,
        browser,
        queue,
        log,
        ..
    } = &mut session.bench;
    assert!(session.store.sync(workspace, browser, queue, Pass::Files));
    let edited = with_gain(&workspace.get(id).unwrap().bytes, "96");
    workspace.replace_bytes(id, edited.clone(), log);
    workspace.mark_saved(id);
    assert!(
        !session.store.sync(workspace, browser, queue, Pass::Files),
        "the save waits"
    );
    session.close();
    assert_eq!(root.read("untitled.ne5p"), edited);
}

#[test]
fn a_file_renamed_outside_keeps_its_id_and_tags() {
    let root = Temp::new();
    let mut session = Session::open(&root);
    let id = session.create();
    let tag = session.bench.browser.tags.make("Sunday").unwrap();
    session.bench.browser.tags.set(id, tag, true);
    session.sync();

    fs::rename(root.at("untitled.ne5p"), root.at("Grand.ne5p")).unwrap();
    session.refocus();
    assert_eq!(session.path(id).as_deref(), Some("Grand.ne5p"));
    assert_eq!(session.bench.workspace.get(id).unwrap().name, "Grand.ne5p");
    assert!(session.bench.browser.tags.worn(id).contains(&tag));
    assert_eq!(
        session.bench.workspace.listed().count(),
        1,
        "not a new asset"
    );
    session.close();

    // And while drawbar is not running at all.
    fs::create_dir(root.at("Pianos")).unwrap();
    fs::rename(root.at("Grand.ne5p"), root.at("Pianos/Grand.ne5p")).unwrap();
    let again = Session::open(&root);
    assert_eq!(again.path(id).as_deref(), Some("Pianos/Grand.ne5p"));
    assert!(again.bench.browser.tags.worn(id).contains(&tag));
    assert_eq!(again.bench.workspace.listed().count(), 1);
}

#[test]
fn a_file_changed_outside_under_an_unsaved_edit_asks_whose_to_keep() {
    let root = Temp::new();
    let mut session = Session::open(&root);
    let id = session.create();
    session.sync();
    let saved = session.bytes(id);
    let (mine, theirs) = (with_gain(&saved, "96"), with_gain(&saved, "12"));
    session
        .bench
        .workspace
        .replace_bytes(id, mine.clone(), &mut session.bench.log);
    fs::write(root.at("untitled.ne5p"), &theirs).unwrap();

    session.refocus();
    let (title, answers) = session.bench.browser.asking().expect("a question");
    assert_eq!(title, "“untitled.ne5p” changed on disk");
    assert_eq!(answers, ["Keep mine", "Keep both", "Take theirs"]);
    let entity = session.bench.workspace.get(id).unwrap();
    assert_eq!(entity.bytes, mine, "nothing was taken without asking");
    assert_eq!(entity.saved.bytes, theirs, "a save would write over theirs");

    let acts = session.bench.browser.answer("Keep both");
    session.bench.act(acts);
    session.sync();
    assert_eq!(root.read("untitled.ne5p"), theirs);
    assert_eq!(root.read("untitled 2.ne5p"), mine);
    let entity = session.bench.workspace.get(id).unwrap();
    assert_eq!(entity.bytes, theirs);
    assert!(!entity.is_unsaved());
}

#[test]
fn a_file_changed_outside_with_nothing_unsaved_is_shown_as_it_is_now() {
    let root = Temp::new();
    let mut session = Session::open(&root);
    let id = session.create();
    session.sync();
    let theirs = with_gain(&session.bytes(id), "12");
    fs::write(root.at("untitled.ne5p"), &theirs).unwrap();

    session.refocus();
    assert!(session.bench.browser.asking().is_none(), "nothing to ask");
    assert_eq!(session.bytes(id), theirs);
    assert!(!session.bench.workspace.get(id).unwrap().is_unsaved());
}

#[test]
fn a_save_over_a_file_changed_since_it_was_read_is_refused() {
    let root = Temp::new();
    let mut session = Session::open(&root);
    let id = session.create();
    session.sync();
    let saved = session.bytes(id);
    let theirs = with_gain(&saved, "12");
    fs::write(root.at("untitled.ne5p"), &theirs).unwrap();

    // Saved before any rescan could see theirs.
    let mine = with_gain(&saved, "96");
    let log = &mut session.bench.log;
    session.bench.workspace.replace_bytes(id, mine.clone(), log);
    session.bench.workspace.mark_saved(id);
    session.sync();

    assert_eq!(
        root.read("untitled.ne5p"),
        theirs,
        "theirs was not written over"
    );
    assert_eq!(session.said("was not saved, because it changed on disk"), 1);
    let entity = session.bench.workspace.get(id).unwrap();
    assert_eq!(entity.bytes, mine, "mine is still held");
    assert!(entity.is_unsaved());
}

#[test]
fn leftovers_of_interrupted_writes_are_swept_and_the_last_index_reads() {
    let root = Temp::new();
    let mut first = Session::open(&root);
    let id = first.create();
    let bytes = first.bytes(id);
    first.close();
    let index = root.read(exec::INDEX);

    // What a crash leaves: an index and a save written halfway to their temporaries,
    // and a working copy the index never came to name.
    fs::write(
        root.at(".drawbar/tmp/library.ron"),
        &index[..index.len() / 2],
    )
    .unwrap();
    fs::write(
        root.at(".untitled.ne5p.drawbar-tmp"),
        &bytes[..bytes.len() / 2],
    )
    .unwrap();
    fs::write(root.at(".drawbar/working/99-7"), b"half an edit").unwrap();

    let second = Session::open(&root);
    assert!(root.names(".drawbar/tmp").is_empty());
    assert!(root.names(".drawbar/working").is_empty());
    assert_eq!(root.names(""), [".drawbar", "untitled.ne5p"]);
    assert_eq!(second.said("removed 3 leftovers"), 1);
    assert_eq!(second.bytes(id), bytes, "the asset is whole");
    assert_eq!(
        root.read(exec::INDEX),
        index,
        "the index is the last one written"
    );
}

#[test]
fn a_write_that_fails_leaves_the_file_and_the_index_as_they_were() {
    let root = Temp::new();
    let mut session = Session::open(&root);
    let id = session.create();
    session.sync();
    let (index, file) = (root.read(exec::INDEX), root.read("untitled.ne5p"));

    // The index's temporary cannot be made where a file stands in for its folder.
    fs::remove_dir_all(root.at(".drawbar/tmp")).unwrap();
    fs::write(root.at(".drawbar/tmp"), b"not a folder").unwrap();
    let tag = session.bench.browser.tags.make("Sunday").unwrap();
    session.bench.browser.tags.set(id, tag, true);
    session.sync();
    assert_eq!(root.read(exec::INDEX), index, "the index was not touched");
    assert_eq!(
        session.said("did not change as asked: keeping the library's index"),
        1
    );

    // Nor can a save's temporary, where a folder has its name.
    fs::create_dir(root.at(".untitled.ne5p.drawbar-tmp")).unwrap();
    let edited = with_gain(&file, "96");
    let log = &mut session.bench.log;
    session.bench.workspace.replace_bytes(id, edited, log);
    session.bench.workspace.mark_saved(id);
    session.sync();
    assert_eq!(root.read("untitled.ne5p"), file, "the file was not touched");
    assert!(
        session.bench.workspace.get(id).unwrap().is_unsaved(),
        "the edit is still unsaved"
    );
    assert_eq!(session.said("was not saved"), 1);
}

#[test]
fn an_index_from_a_newer_drawbar_opens_read_only_and_is_never_written() {
    let root = Temp::new();
    fs::create_dir(root.at(".drawbar")).unwrap();
    let future = "(version: 99, next_id: 3, something_new: [1, 2])";
    fs::write(root.at(exec::INDEX), future).unwrap();
    let program = Fresh::Program.bytes().unwrap();
    fs::write(root.at("Grand.ne5p"), &program).unwrap();

    let mut session = Session::open(&root);
    let why = session.store.read_only().expect("read-only");
    assert!(why.contains("newer drawbar"), "{why}");
    let names: Vec<&str> = session
        .bench
        .workspace
        .listed()
        .map(|entity| entity.name.as_str())
        .collect();
    assert_eq!(names, ["Grand.ne5p"], "the files still show");

    session.create();
    session.close();
    assert_eq!(root.read(exec::INDEX), future.as_bytes());
    assert_eq!(
        root.names(""),
        [".drawbar", "Grand.ne5p"],
        "nothing was added"
    );
}

#[test]
fn a_second_drawbar_on_one_library_only_reads_it() {
    let root = Temp::new();
    let first = Session::open(&root);
    let second = Session::open(&root);
    assert_eq!(first.store.read_only(), None);
    let why = second.store.read_only().expect("read-only");
    assert!(why.contains("another drawbar"), "{why}");
}

#[test]
fn a_file_deleted_outside_goes_unless_it_holds_what_the_file_did_not() {
    let root = Temp::new();
    let mut session = Session::open(&root);
    let (tagged, plain) = (session.create(), session.create());
    let tag = session.bench.browser.tags.make("Sunday").unwrap();
    session.bench.browser.tags.set(tagged, tag, true);
    session.sync();
    for name in ["untitled.ne5p", "untitled 2.ne5p"] {
        fs::remove_file(root.at(name)).unwrap();
    }

    session.refocus();
    assert!(
        session.bench.workspace.get(plain).is_none(),
        "gone with its file"
    );
    assert!(
        session.bench.workspace.get(tagged).is_some(),
        "its tag is kept"
    );
    assert!(session.bench.browser.folders.missing.contains(&tagged));
    session.close();
    assert_eq!(
        root.names(""),
        [".drawbar"],
        "nothing was written back unasked"
    );

    let again = Session::open(&root);
    let lost: Vec<u64> = again
        .bench
        .browser
        .folders
        .lost()
        .iter()
        .map(|lost| lost.id)
        .collect();
    assert_eq!(lost, [tagged], "a row for what only the index had");
    assert!(again.bench.browser.tags.worn(tagged).contains(&tag));
}

#[test]
fn a_view_holding_an_edit_comes_back_as_a_file() {
    let root = Temp::new();
    let mut first = Session::open(&root);
    let at = Location { bank: 6, slot: 3 };
    let Bench { workspace, log, .. } = &mut first.bench;
    let id = workspace.view(
        "Africa Split.ne5p".into(),
        Origin::Device {
            class: ObjectClass::Program,
            at,
        },
        Fresh::Program.bytes().unwrap(),
        log,
    );
    let edited = with_gain(&workspace.get(id).unwrap().bytes, "96");
    workspace.replace_bytes(id, edited.clone(), log);
    first.close();
    assert_eq!(root.names(""), [".drawbar"], "a view has no file");

    let mut second = Session::open(&root);
    let entity = second.bench.workspace.get(id).expect("kept");
    assert!(entity.kept);
    assert_eq!(entity.bytes, edited);
    assert_eq!(entity.origin.slot(), Some((ObjectClass::Program, at)));
    second.sync();
    assert_eq!(root.read("Africa Split.ne5p"), edited);
}

#[test]
fn two_assets_one_name_apart_in_case_are_flagged_both() {
    let mut bench = Bench::new();
    let mut place = |name: &str| {
        let id = bench
            .workspace
            .create(Fresh::Program, &mut bench.log)
            .unwrap();
        bench.workspace.place(id, LibPath::root().join(name));
        id
    };
    let (lower, upper, other) = (place("c3.ne5p"), place("C3.ne5p"), place("d3.ne5p"));
    let (flagged, groups) = mirror::duplicates(&bench.workspace, &bench.browser.folders);
    assert_eq!(flagged.into_iter().collect::<Vec<_>>(), [lower, upper]);
    assert!(!groups
        .iter()
        .any(|(_, group)| group.iter().any(|(id, _)| *id == Some(other))));
    let names: Vec<&str> = groups[0].1.iter().map(|(_, name)| name.as_str()).collect();
    assert_eq!(names, ["c3.ne5p", "C3.ne5p"]);
}

#[test]
fn a_disk_holding_both_spellings_shows_both_and_renames_neither() {
    let root = Temp::new();
    let program = Fresh::Program.bytes().unwrap();
    fs::write(root.at("c3.ne5p"), &program).unwrap();
    fs::write(root.at("C3.ne5p"), with_gain(&program, "12")).unwrap();
    if root.names("").len() < 2 {
        // A disk that ignores case cannot hold the pair at all.
        return;
    }

    let mut session = Session::open(&root);
    assert_eq!(session.bench.workspace.listed().count(), 2);
    assert_eq!(session.bench.browser.folders.duplicates.len(), 2);
    assert_eq!(session.said("are one name on a disk that ignores case"), 1);
    session.refocus();
    assert_eq!(session.said("are one name"), 1, "said once");
    session.close();
    assert_eq!(root.names(""), [".drawbar", "C3.ne5p", "c3.ne5p"]);
}

#[test]
fn a_path_from_the_index_cannot_leave_the_library() {
    for bad in ["..", "a/../b", "/etc", "a//b", "a\\b", "./a", "a/"] {
        assert_eq!(LibPath::parse(bad), None, "{bad:?}");
    }
    let row: Result<Row, _> = ron::from_str(r#"(path: Some("../outside.ne5p"))"#);
    assert!(row.is_err(), "{row:?}");
    assert_eq!(
        LibPath::parse("Cello/c3.wav").map(|path| path.parent()),
        LibPath::parse("Cello")
    );
}

#[test]
fn a_move_carries_the_folder_and_what_is_in_it_and_nothing_beside_it() {
    let path = |text| LibPath::parse(text).unwrap();
    let (from, to) = (path("Cello"), path("Strings/Cello"));
    assert_eq!(
        path("Cello/c3.wav").moved(&from, &to),
        Some(path("Strings/Cello/c3.wav"))
    );
    assert_eq!(path("Cello").moved(&from, &to), Some(to.clone()));
    assert_eq!(path("Cellos/c3.wav").moved(&from, &to), None);
    assert_eq!(path("c3.wav").moved(&from, &to), None);
}

#[test]
fn the_index_reads_back_what_was_written_and_a_newer_one_is_known_as_that() {
    let mut index = Sidecar::default();
    index.tags.insert(4, "Sunday".into());
    index.assets.insert(
        9,
        Row {
            path: LibPath::parse("Pianos/Grand.npno"),
            name: "Grand.npno".into(),
            fingerprint: Some(Fingerprint {
                len: 10,
                modified: Some(7),
                crc: 0xdead_beef,
            }),
            tags: [4].into(),
            origin: Stored::Device {
                class: ObjectClass::Piano.to_raw(),
                bank: 0,
                slot: 2,
            },
            working: Some(3),
        },
    );
    let text = sidecar::write(&index).unwrap();
    assert_eq!(sidecar::read(&text), Read::Known(index));

    assert_eq!(sidecar::read("(version: 2, assets: 7)"), Read::Newer(2));
    assert!(matches!(sidecar::read("not an index"), Read::Unreadable(_)));
}

#[test]
fn a_file_at_a_new_path_is_an_asset_moved_only_when_one_matches_one() {
    let found = |path: &str, bytes: &[u8]| Found {
        path: LibPath::parse(path).unwrap(),
        stat: Stat {
            len: bytes.len() as u64,
            modified: Some(1),
        },
        bytes: Some(bytes.to_vec()),
    };
    let known = |path: &str, bytes: &[u8]| Known {
        path: LibPath::parse(path).unwrap(),
        fingerprint: Some(Fingerprint::of(
            Stat {
                len: bytes.len() as u64,
                modified: Some(0),
            },
            bytes,
        )),
    };
    let known = BTreeMap::from([
        (1, known("a.ne5p", b"one")),
        (2, known("b.ne5p", b"two")),
        (3, known("c.ne5p", b"two")),
    ]);
    let matched = match_files(
        &known,
        vec![found("moved.ne5p", b"one"), found("copy.ne5p", b"two")],
    );
    let renamed: Vec<(u64, &str)> = matched
        .renamed
        .iter()
        .map(|(id, found)| (*id, found.path.as_str()))
        .collect();
    assert_eq!(renamed, [(1, "moved.ne5p")]);
    assert_eq!(
        matched.vanished,
        [2, 3],
        "two could have moved, so neither did"
    );
    assert_eq!(matched.arrived.len(), 1);
}
