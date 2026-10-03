//! A library on disk, opened, edited, changed behind the app's back and opened again.
//!
//! Every test works in its own directory under the system's temp folder, never in the
//! default library.

use std::collections::BTreeMap;
use std::fs;

use super::diff::{match_files, Known};
use super::exec::MOST_BYTES;
use super::sidecar::{self, Read};
use super::*;
use crate::browser::Kind;
use crate::testing::{Bench, Temp};
use crate::workspace::{Fresh, Origin, VerifyState};
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
        session.opened();
        session
    }

    /// Wait for the open's answer, and for every part of its listing.
    fn opened(&mut self) {
        assert!(self.next(), "opening answered");
        while self.store.scanning() {
            assert!(self.next(), "the listing answered");
        }
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

    /// Whether the library may be let go without waiting, as a browser asks each frame.
    fn settled(&mut self) -> bool {
        let Bench {
            workspace,
            browser,
            queue,
            ..
        } = &mut self.bench;
        self.store.settled(workspace, browser, queue)
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

    /// One frame of the document open on `id`, and every word it painted.
    fn document(&mut self, id: u64) -> Vec<String> {
        let mut document = crate::document::Document::default();
        let Bench {
            ctx,
            workspace,
            device,
            log,
            queue,
            browser,
            ..
        } = &mut self.bench;
        let input = crate::testing::screen(eframe::egui::vec2(1280.0, 720.0), Vec::new());
        let output = crate::testing::run(&ctx.clone(), input, |ctx| {
            eframe::egui::CentralPanel::default().show(ctx, |ui| {
                document.ui(
                    ui,
                    id,
                    workspace,
                    device,
                    log,
                    &crate::document::Around {
                        queue,
                        tags: &browser.tags,
                        played: &crate::midi::Played::default(),
                    },
                );
            });
        });
        crate::testing::words(&output)
    }

    /// The one asset the library lists.
    fn only(&self) -> u64 {
        let mut listed = self.bench.workspace.listed();
        let id = listed.next().expect("an asset is listed").id;
        assert!(listed.next().is_none(), "one asset is listed");
        id
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

/// A working copy that does not read is the only copy of its edit, so the library opens
/// read-only rather than drop it, and the edit comes back once the copy reads again.
#[cfg(unix)]
#[test]
fn an_unsaved_edit_that_does_not_read_is_left_for_the_next_open() {
    use std::os::unix::fs::PermissionsExt;

    let root = Temp::new();
    let mut first = Session::open(&root);
    let id = first.create();
    first.sync();
    let edited = with_gain(&first.bytes(id), "96");
    let log = &mut first.bench.log;
    first.bench.workspace.replace_bytes(id, edited.clone(), log);
    first.close();
    let working = root.names(".drawbar/working");
    assert_eq!(working.len(), 1, "{working:?}");
    let copy = root.at(&format!(".drawbar/working/{}", working[0]));
    fs::set_permissions(&copy, fs::Permissions::from_mode(0o000)).unwrap();
    if fs::read(&copy).is_ok() {
        // Permissions do not bind this user.
        return;
    }

    let second = Session::open(&root);
    let why = second.store.read_only().expect("read-only");
    assert!(why.contains("could not be read"), "{why}");
    second.close();
    assert_eq!(root.names(".drawbar/working"), working, "the copy is left");

    fs::set_permissions(&copy, fs::Permissions::from_mode(0o644)).unwrap();
    let third = Session::open(&root);
    assert_eq!(third.store.read_only(), None);
    let entity = third.bench.workspace.get(id).expect("the same id");
    assert_eq!(entity.bytes, edited, "the edit came back");
    assert!(entity.is_unsaved());
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

/// A browser cannot wait for its library's answers, so it lets a library go only once it
/// is settled, after one last pass whose answers nothing reads. That pass sends no save,
/// so the index it writes holds what each save answered.
#[test]
fn a_settled_library_let_go_without_waiting_keeps_its_save_and_its_edit() {
    let root = Temp::new();
    let mut session = Session::open(&root);
    let id = session.create();
    assert!(!session.settled(), "the first write is in flight");
    while !session.settled() {
        assert!(session.next(), "the first write answered");
    }
    let saved = with_gain(&session.bytes(id), "96");
    let edited = with_gain(&saved, "12");
    let Bench { workspace, log, .. } = &mut session.bench;
    workspace.replace_bytes(id, saved.clone(), log);
    workspace.mark_saved(id);
    workspace.replace_bytes(id, edited.clone(), log);
    assert!(!session.settled(), "the save is in flight");
    while !session.settled() {
        assert!(session.next(), "the save answered");
    }
    let Bench {
        workspace,
        browser,
        queue,
        ..
    } = &mut session.bench;
    assert!(
        session.store.sync(workspace, browser, queue, Pass::Last),
        "nothing waits"
    );
    drop(session);
    assert_eq!(root.read("untitled.ne5p"), saved, "the save landed");

    let again = Session::open(&root);
    let entity = again.bench.workspace.get(id).expect("the same id");
    assert_eq!(entity.bytes, edited, "the edit came back");
    assert!(entity.is_unsaved());
    assert_eq!(again.bench.browser.asking(), None, "nothing to ask");
}

/// Quitting right after a save writes the index only once the save has answered, so the
/// next run knows the file as the save left it, not as changed outside drawbar.
#[test]
fn quitting_right_after_a_save_leaves_no_conflict_for_the_next_run() {
    let root = Temp::new();
    let mut first = Session::open(&root);
    let id = first.create();
    first.sync();
    let saved = with_gain(&first.bytes(id), "96");
    let edited = with_gain(&saved, "12");
    let Bench { workspace, log, .. } = &mut first.bench;
    workspace.replace_bytes(id, saved.clone(), log);
    workspace.mark_saved(id);
    workspace.replace_bytes(id, edited.clone(), log);
    first.close();
    assert_eq!(root.read("untitled.ne5p"), saved, "the save landed");

    let second = Session::open(&root);
    assert_eq!(second.bench.browser.asking(), None, "nothing to ask");
    let entity = second.bench.workspace.get(id).expect("the same id");
    assert_eq!(entity.saved.bytes, saved, "the file as saved");
    assert_eq!(entity.bytes, edited, "and the edit over it");
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

/// A file written again elsewhere, its time new and its length the same, is the asset
/// moved when its contents are what the asset's file held.
#[test]
fn a_file_copied_to_a_new_path_and_deleted_is_matched_by_its_contents() {
    let root = Temp::new();
    let mut session = Session::open(&root);
    let id = session.create();
    let tag = session.bench.browser.tags.make("Sunday").unwrap();
    session.bench.browser.tags.set(id, tag, true);
    session.sync();

    let bytes = root.read("untitled.ne5p");
    fs::remove_file(root.at("untitled.ne5p")).unwrap();
    fs::write(root.at("Grand.ne5p"), &bytes).unwrap();
    session.refocus();
    assert_eq!(session.path(id).as_deref(), Some("Grand.ne5p"));
    assert!(session.bench.browser.tags.worn(id).contains(&tag));
    assert_eq!(session.bench.workspace.listed().count(), 1);
}

/// A file drawbar listed but never wrote is fingerprinted by its length and time until
/// its contents are read, and then by its CRC as well. A save over it after it changed
/// on disk, its length the same, is refused; one after it was only written again with
/// the same contents goes through.
#[test]
fn a_save_over_a_listed_file_is_refused_only_where_its_contents_changed() {
    let root = Temp::new();
    let program = Fresh::Program.bytes().unwrap();
    fs::write(root.at("Grand.ne5p"), &program).unwrap();
    let mut session = Session::open(&root);
    let id = session.only();
    session.sync();

    let theirs = with_gain(&program, "12");
    assert_eq!(theirs.len(), program.len());
    fs::write(root.at("Grand.ne5p"), &theirs).unwrap();
    let mine = with_gain(&program, "96");
    let log = &mut session.bench.log;
    session.bench.workspace.replace_bytes(id, mine.clone(), log);
    session.bench.workspace.mark_saved(id);
    session.sync();
    assert_eq!(
        root.read("Grand.ne5p"),
        theirs,
        "theirs was not written over"
    );
    assert_eq!(session.said("was not saved, because it changed on disk"), 1);

    let again = Temp::new();
    fs::write(again.at("Grand.ne5p"), &program).unwrap();
    let mut session = Session::open(&again);
    let id = session.only();
    session.sync();
    fs::write(again.at("Grand.ne5p"), &program).unwrap();
    let log = &mut session.bench.log;
    session.bench.workspace.replace_bytes(id, mine.clone(), log);
    session.bench.workspace.mark_saved(id);
    session.sync();
    assert_eq!(
        again.read("Grand.ne5p"),
        mine,
        "the same contents, written again"
    );
    assert_eq!(session.said("was not saved"), 0);
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
    assert_eq!(session.bench.browser.expected(), Some("Keep mine"));
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

/// After a write fails, every working copy is written again under a new name, and the
/// ones they replace go.
#[test]
fn working_copies_written_again_after_a_failure_replace_the_old_ones() {
    let root = Temp::new();
    let mut session = Session::open(&root);
    let id = session.create();
    session.sync();
    let edited = with_gain(&session.bytes(id), "96");
    let log = &mut session.bench.log;
    session.bench.workspace.replace_bytes(id, edited, log);
    session.sync();
    let before = root.names(".drawbar/working");
    assert_eq!(before.len(), 1, "{before:?}");

    fs::remove_dir_all(root.at(".drawbar/tmp")).unwrap();
    fs::write(root.at(".drawbar/tmp"), b"not a folder").unwrap();
    let tag = session.bench.browser.tags.make("Sunday").unwrap();
    session.bench.browser.tags.set(id, tag, true);
    session.sync();
    assert_eq!(session.said("did not change as asked"), 1);

    fs::remove_file(root.at(".drawbar/tmp")).unwrap();
    fs::create_dir(root.at(".drawbar/tmp")).unwrap();
    session.sync();
    let after = root.names(".drawbar/working");
    assert_eq!(after.len(), 1, "{after:?}");
    assert_ne!(after, before, "written again under a new name");
}

#[test]
fn an_index_from_a_newer_drawbar_opens_read_only_and_is_never_written() {
    let root = Temp::new();
    fs::create_dir(root.at(".drawbar")).unwrap();
    let future = "(version: 99, next_id: 3, something_new: [1, 2])";
    fs::write(root.at(exec::INDEX), future).unwrap();
    fs::create_dir(root.at(exec::TMP)).unwrap();
    fs::write(root.at(".drawbar/tmp/theirs"), b"in flight").unwrap();
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
    assert_eq!(
        root.names(".drawbar"),
        ["library.ron", "tmp"],
        "no lock taken"
    );
    assert_eq!(root.names(exec::TMP), ["theirs"], "nothing swept");
}

#[test]
fn a_second_drawbar_on_one_library_only_reads_it() {
    let root = Temp::new();
    let mut first = Session::open(&root);
    first.create();
    first.sync();
    let second = Session::open(&root);
    assert_eq!(first.store.read_only(), None);
    let why = second.store.read_only().expect("read-only");
    assert!(why.contains("another drawbar"), "{why}");
}

/// Two drawbars can open a folder neither has written, since opening takes no lock. The
/// first to write takes it, and the other finds out at its own first write.
#[test]
fn a_drawbar_that_writes_second_to_a_new_library_turns_read_only() {
    let root = Temp::new();
    let mut first = Session::open(&root);
    let mut second = Session::open(&root);
    let mine = first.create();
    first.sync();

    let theirs = second.create();
    let Bench {
        workspace,
        browser,
        queue,
        ..
    } = &mut second.bench;
    second.store.sync(workspace, browser, queue, Pass::Last);
    assert!(second.next(), "the write answered");
    let why = second.store.read_only().expect("read-only");
    assert!(why.contains("another drawbar"), "{why}");
    assert!(
        second.bench.workspace.get(theirs).unwrap().is_unsaved(),
        "its new file counts as unsaved"
    );
    assert_eq!(second.said("read-only now"), 1);
    assert_eq!(root.names(""), [".drawbar", "untitled.ne5p"]);
    assert_eq!(root.read("untitled.ne5p"), first.bytes(mine));
}

/// Opening a folder, looking around and rescanning writes nothing: `.drawbar/` appears
/// with the first change, here a tag.
#[test]
fn a_folder_opened_is_left_as_it_was_until_something_changes() {
    let root = Temp::new();
    let program = Fresh::Program.bytes().unwrap();
    fs::write(root.at("Grand.ne5p"), &program).unwrap();

    let touched = || fs::metadata(&root.0).unwrap().modified().unwrap();
    let before = touched();

    let mut session = Session::open(&root);
    assert_eq!(session.store.read_only(), None);
    session.refocus();
    session.sync();
    assert_eq!(root.names(""), ["Grand.ne5p"], "nothing was added");
    assert_eq!(touched(), before, "nothing was written and removed");

    let id = session
        .bench
        .workspace
        .listed()
        .next()
        .expect("the file")
        .id;
    let tag = session.bench.browser.tags.make("Sunday").unwrap();
    session.bench.browser.tags.set(id, tag, true);
    session.close();
    assert_eq!(root.names(""), [".drawbar", "Grand.ne5p"]);
    assert_eq!(root.read("Grand.ne5p"), program, "the file is untouched");

    let again = Session::open(&root);
    assert!(again.bench.browser.tags.worn(id).contains(&tag));
}

/// Whether a folder can be written is found out by the first write, so opening one
/// writes nothing to find out.
#[cfg(unix)]
#[test]
fn a_folder_drawbar_cannot_write_turns_read_only_at_the_first_write_and_says_why() {
    use std::os::unix::fs::PermissionsExt;

    /// Gives the folder back its write permission, so it can be removed.
    struct Writable<'a>(&'a Temp);
    impl Drop for Writable<'_> {
        fn drop(&mut self) {
            let _ = fs::set_permissions(&self.0 .0, fs::Permissions::from_mode(0o755));
        }
    }

    let root = Temp::new();
    let program = Fresh::Program.bytes().unwrap();
    fs::write(root.at("Grand.ne5p"), &program).unwrap();
    fs::set_permissions(&root.0, fs::Permissions::from_mode(0o555)).unwrap();
    let _writable = Writable(&root);
    if fs::write(root.at("written"), b"").is_ok() {
        // Permissions do not bind this user.
        return;
    }

    let mut session = Session::open(&root);
    assert_eq!(
        session.bench.workspace.listed().count(),
        1,
        "it still shows"
    );
    let id = session.create();
    let Bench {
        workspace,
        browser,
        queue,
        ..
    } = &mut session.bench;
    session.store.sync(workspace, browser, queue, Pass::Last);
    assert!(session.next(), "the write answered");
    let why = session.store.read_only().expect("read-only");
    assert!(why.contains("cannot write here"), "{why}");
    assert_eq!(session.said("read-only now"), 1);
    assert!(
        session.bench.workspace.get(id).unwrap().is_unsaved(),
        "its new file counts as unsaved"
    );
    session.close();
    assert_eq!(root.names(""), ["Grand.ne5p"]);
}

/// A folder that lists its names but refuses a look at the files in it: every file there
/// is shown unread, and the rest of the library opens.
#[cfg(unix)]
#[test]
fn a_file_that_cannot_be_looked_at_is_shown_unread_and_the_library_opens() {
    use std::os::unix::fs::PermissionsExt;

    /// Gives the folder back its permissions, so it can be removed.
    struct Searchable(std::path::PathBuf);
    impl Drop for Searchable {
        fn drop(&mut self) {
            let _ = fs::set_permissions(&self.0, fs::Permissions::from_mode(0o755));
        }
    }

    let root = Temp::new();
    let program = Fresh::Program.bytes().unwrap();
    fs::write(root.at("Grand.ne5p"), &program).unwrap();
    fs::create_dir(root.at("Cello")).unwrap();
    fs::write(root.at("Cello/c3.ne5p"), with_gain(&program, "12")).unwrap();
    fs::set_permissions(root.at("Cello"), fs::Permissions::from_mode(0o444)).unwrap();
    let _searchable = Searchable(root.at("Cello"));
    if fs::metadata(root.at("Cello/c3.ne5p")).is_ok() {
        // Permissions do not bind this user.
        return;
    }

    let session = Session::open(&root);
    assert_eq!(session.store.read_only(), None);
    let names: Vec<&str> = session
        .bench
        .workspace
        .listed()
        .map(|entity| entity.name.as_str())
        .collect();
    assert_eq!(names, ["Grand.ne5p"]);
    let unread: Vec<&str> = session
        .bench
        .browser
        .folders
        .unread
        .iter()
        .map(|(path, _)| path.as_str())
        .collect();
    assert_eq!(unread, ["Cello/c3.ne5p"]);
}

/// An asset deleted while its first write is in flight loses its file once that write
/// answers, and a rescan meanwhile does not bring the file back as a new one.
#[test]
fn an_asset_deleted_while_its_file_is_written_goes_without_coming_back() {
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
    workspace.remove(id, log);
    session.store.sync(workspace, browser, queue, Pass::Files);
    session.store.rescan();
    while session.store.scanning() {
        assert!(session.next(), "the write and the rescan answered");
    }
    session.sync();
    session.refocus();

    assert_eq!(session.bench.workspace.listed().count(), 0);
    assert_eq!(session.said("appeared in the library folder"), 0);
    assert_eq!(session.said("deleted outside drawbar"), 0);
    assert_eq!(root.names(""), [".drawbar"], "the file is gone");
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
    assert_eq!(
        root.names(""),
        ["C3.ne5p", "c3.ne5p"],
        "nothing renamed, and nothing added"
    );
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
                crc: Some(0xdead_beef),
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
    let unread = Row {
        path: LibPath::parse("c3.ne5p"),
        name: "c3.ne5p".into(),
        fingerprint: Some(Fingerprint {
            len: 3,
            modified: None,
            crc: None,
        }),
        tags: [4].into(),
        origin: Stored::Fresh,
        working: None,
    };
    index.assets.insert(10, unread);
    let text = sidecar::write(&index).unwrap();
    assert_eq!(sidecar::read(&text), Read::Known(index.clone()));

    // An index written while every fingerprint had a CRC wrote it bare.
    let bare = text.replace("Some(3735928559)", "3735928559");
    assert_ne!(bare, text);
    assert_eq!(sidecar::read(&bare), Read::Known(index));

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
        file: None,
        crc: Some(nord_format::crc::crc32(bytes)),
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

#[test]
fn only_files_drawbar_opens_are_read_and_the_rest_are_listed_by_name() {
    let root = Temp::new();
    let program = Fresh::Program.bytes().unwrap();
    fs::write(root.at("Grand.ne5p"), &program).unwrap();
    fs::create_dir(root.at("Cello")).unwrap();
    fs::write(root.at("Cello/c3.NE5P"), with_gain(&program, "12")).unwrap();
    fs::write(root.at("Cello/notes.pdf"), b"%PDF").unwrap();
    fs::write(root.at("cover.jpg"), b"not a nord file").unwrap();
    fs::write(root.at(".DS_Store"), b"hidden").unwrap();
    #[cfg(unix)]
    {
        // A file drawbar does not open is never read, so one nobody may read says nothing.
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(root.at("cover.jpg"), fs::Permissions::from_mode(0o000)).unwrap();
    }

    let session = Session::open(&root);
    let mut held: Vec<String> = session
        .bench
        .workspace
        .listed()
        .filter_map(|entity| Some(entity.path.as_ref()?.to_string()))
        .collect();
    held.sort();
    assert_eq!(
        held,
        ["Cello/c3.NE5P", "Grand.ne5p"],
        "by extension, in any case"
    );
    let others: Vec<&str> = session
        .bench
        .browser
        .folders
        .others
        .iter()
        .map(LibPath::as_str)
        .collect();
    assert_eq!(others, ["Cello/notes.pdf", "cover.jpg"], "no hidden file");
    assert!(session.bench.browser.folders.unread.is_empty());
    session.close();
}

/// A file drawbar holds stays held whatever its name: it made it, or was given it.
#[test]
fn a_held_file_of_a_kind_drawbar_does_not_open_stays_held() {
    let root = Temp::new();
    let mut first = Session::open(&root);
    let Bench { workspace, log, .. } = &mut first.bench;
    let id = workspace.ingest("scan.pdf".into(), Origin::Fresh, b"%PDF".to_vec(), log);
    workspace.place(id, LibPath::root().join("scan.pdf"));
    first.close();

    let second = Session::open(&root);
    assert!(second.bench.workspace.get(id).is_some(), "the same asset");
    assert!(second.bench.browser.folders.others.is_empty());
}

/// A library past ten thousand entries is listed whole, however deep its files are, and
/// a file drawbar holds keeps its id and tags wherever it is.
#[test]
fn a_large_tree_is_listed_whole() {
    let root = Temp::new();
    let program = Fresh::Program.bytes().unwrap();
    for dir in ["a", "b", "c", "c/d"] {
        fs::create_dir(root.at(dir)).unwrap();
    }
    fs::write(root.at("top.ne5p"), &program).unwrap();
    fs::write(root.at("c/d/kept.ne5p"), with_gain(&program, "12")).unwrap();
    let mut first = Session::open(&root);
    let kept = first
        .bench
        .workspace
        .listed()
        .find(|entity| entity.name == "kept.ne5p")
        .expect("found while the tree is small")
        .id;
    let tag = first.bench.browser.tags.make("Sunday").unwrap();
    first.bench.browser.tags.set(kept, tag, true);
    first.close();

    const PAST: usize = 10_001;
    for n in 0..PAST {
        fs::write(root.at(&format!("a/{n:05}.jpg")), b"").unwrap();
    }
    fs::write(root.at("b/deep.ne5p"), &program).unwrap();
    let session = Session::open(&root);
    let folders = &session.bench.browser.folders;
    assert!(folders.unwalked.is_empty(), "{:?}", folders.unwalked);
    assert_eq!(folders.others.len(), PAST);
    let mut names: Vec<&str> = session
        .bench
        .workspace
        .listed()
        .map(|entity| entity.name.as_str())
        .collect();
    names.sort();
    assert_eq!(names, ["deep.ne5p", "kept.ne5p", "top.ne5p"]);
    assert!(session.bench.browser.tags.worn(kept).contains(&tag));
    assert_eq!(session.said("not listed"), 0);
}

/// A library let go before its listing has all come back keeps, in its index, the rows
/// no listed file had claimed yet.
#[test]
fn a_library_let_go_while_it_is_listed_keeps_what_its_index_held() {
    let root = Temp::new();
    let mut first = Session::open(&root);
    let id = first.create();
    let tag = first.bench.browser.tags.make("Sunday").unwrap();
    first.bench.browser.tags.set(id, tag, true);
    first.close();

    let bench = Bench::new();
    let store = Store::start(Backend::start(&bench.ctx, root.0.clone()));
    let mut second = Session { store, bench };
    assert!(second.next(), "opening answered");
    assert!(second.store.scanning(), "its listing has not come back");
    second.close();

    let third = Session::open(&root);
    assert!(third.bench.browser.tags.worn(id).contains(&tag));
    assert_eq!(third.path(id).as_deref(), Some("untitled.ne5p"));
}

/// Everything drawbar holds is in memory, so it reads only so much from one folder. A
/// file past that is shown unread, and not read at all. The file here is sparse, so it
/// takes no room on disk.
#[test]
fn a_file_past_the_most_drawbar_reads_is_shown_unread() {
    let root = Temp::new();
    let huge = fs::File::create(root.at("Huge.nsmp")).unwrap();
    huge.set_len(MOST_BYTES + 1).unwrap();
    drop(huge);
    fs::write(root.at("Small.ne5p"), Fresh::Program.bytes().unwrap()).unwrap();

    let session = Session::open(&root);
    let names: Vec<&str> = session
        .bench
        .workspace
        .listed()
        .map(|entity| entity.name.as_str())
        .collect();
    assert_eq!(names, ["Small.ne5p"]);
    let unread = &session.bench.browser.folders.unread;
    assert_eq!(unread.len(), 1, "{unread:?}");
    assert_eq!(unread[0].0.as_str(), "Huge.nsmp");
    assert!(unread[0].1.contains("at most 1 GiB"), "{}", unread[0].1);
    assert_eq!(
        session.bench.browser.folders.clash(
            &LibPath::root(),
            "huge.NSMP",
            &session.bench.workspace,
            None
        ),
        crate::folders::Clash::Taken(crate::folders::Occupant::Other),
        "its name is still taken"
    );
}

/// An id this session has given out already, here to views read before the library
/// opened, is not given to an asset of the library. The asset takes a new one, and its
/// unsaved edit moves to a working copy under it.
#[test]
fn an_asset_under_an_id_already_given_out_takes_a_new_one_with_its_edit() {
    let root = Temp::new();
    let mut first = Session::open(&root);
    let id = first.create();
    first.sync();
    let edited = with_gain(&first.bytes(id), "96");
    let log = &mut first.bench.log;
    first.bench.workspace.replace_bytes(id, edited.clone(), log);
    first.close();

    let mut bench = Bench::new();
    for _ in 0..3 {
        let Bench { workspace, log, .. } = &mut bench;
        workspace.view("seen".into(), Origin::Fresh, b"a view".to_vec(), log);
    }
    let floor = bench.workspace.next_id();
    let store = Store::start(Backend::start(&bench.ctx, root.0.clone()));
    let mut second = Session { store, bench };
    second.opened();
    let back = second
        .bench
        .workspace
        .listed()
        .next()
        .expect("the asset")
        .id;
    assert!(back >= floor, "{back} is new");
    assert_eq!(second.bytes(back), edited);
    second.sync();
    let working = root.names(".drawbar/working");
    assert_eq!(working.len(), 1, "{working:?}");
    assert!(working[0].starts_with(&format!("{back}-")), "{working:?}");
    second.close();

    let third = Session::open(&root);
    let entity = third.bench.workspace.get(back).expect("under its new id");
    assert_eq!(entity.bytes, edited);
    assert!(entity.is_unsaved());
}

/// A piano library the size a vendor ships: three strokes, each of the most blocks a
/// stroke record can state, about 200 MB in all.
fn large_piano() -> Vec<u8> {
    use nord_format::formats::npno::synthetic::{take, Build};
    use nord_format::formats::npno::Bank;

    const ROOTS: [u8; 3] = [48, 60, 72];
    Build {
        version: 0x464,
        channels: 1,
        takes: ROOTS
            .into_iter()
            .map(|root| take(root, Bank::Attack, 0, u16::MAX))
            .collect(),
        map: (21..=108)
            .map(|key: u8| {
                let root = ROOTS.into_iter().min_by_key(|root| root.abs_diff(key));
                (key, root.expect("three roots"))
            })
            .collect(),
    }
    .bytes()
    .expect("the builder lays out a library")
}

/// A piano library of a vendor's size opens resting in its file: the frame reads none of
/// its body, its document draws from the index while its row still says it is being
/// checked, and the check answers off the frame.
#[test]
fn a_large_piano_opens_and_draws_before_its_check_answers() {
    let root = Temp::new();
    let bytes = large_piano();
    let len = bytes.len() as u64;
    assert!(len > 200_000_000, "{len} bytes is a vendor's size");
    fs::write(root.at("Grand.npno"), &bytes).unwrap();
    drop(bytes);

    let mut session = Session::open(&root);
    let id = session.only();
    let entity = session.bench.workspace.get(id).unwrap();
    assert!(entity.rests().is_some(), "it rests in its file");
    assert!(entity.bytes.is_empty() && entity.saved.bytes.is_empty());
    assert_eq!(entity.size(), len);
    assert_eq!(entity.verify.note(), Some("checking…"));
    let file = entity.rests().unwrap().clone();

    let words = session.document(id);
    assert!(
        words.iter().any(|word| word == "npno 0x464"),
        "the header reads the index: {words:?}"
    );
    assert_eq!(file.take_reads(), [], "the frame read none of its body");

    let Bench { workspace, log, .. } = &mut session.bench;
    workspace.settle_files(log);
    let entity = workspace.get(id).unwrap();
    assert!(matches!(entity.verify, VerifyState::Checked));
    assert_eq!(entity.verify.note(), None);
    assert!(
        entity.saved.crc32.is_some(),
        "a slot holding it can be matched"
    );
    assert!(entity.sendable().is_ok());
}

/// A piano library whose body no longer matches its checksum opens like any other,
/// resting in its file, and its row says it failed once the check answers. Nothing sends
/// it, and its index is no longer trusted to make it a piano.
#[test]
fn a_corrupted_piano_opens_and_then_shows_it_failed_verification() {
    let root = Temp::new();
    let mut bytes = nord_format::formats::npno::synthetic::Build::new()
        .bytes()
        .unwrap();
    let last = bytes.len() - 1;
    bytes[last] ^= 0xff;
    fs::write(root.at("Broken.npno"), &bytes).unwrap();

    let mut session = Session::open(&root);
    let id = session.only();
    let entity = session.bench.workspace.get(id).unwrap();
    assert!(entity.rests().is_some());
    assert_eq!(entity.verify.note(), Some("checking…"));
    assert_eq!(Kind::of(entity), Kind::Piano);

    let Bench { workspace, log, .. } = &mut session.bench;
    workspace.settle_files(log);
    let entity = workspace.get(id).unwrap();
    assert_eq!(entity.verify.note(), Some("failed verification"));
    assert!(entity.sendable().is_err());
    assert_eq!(Kind::of(entity), Kind::Other);
}
