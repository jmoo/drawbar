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
    /// Open the library and read every file it lists, as if each had been looked at.
    fn open(root: &Temp) -> Session {
        let mut session = Session::listed(root);
        session.read_all();
        session
    }

    /// Open the library, and read nothing that nothing asks for.
    fn listed(root: &Temp) -> Session {
        Session::within(root, MOST_BYTES)
    }

    /// [`Session::listed`], holding at most `budget` bytes whole.
    fn within(root: &Temp, budget: u64) -> Session {
        let bench = Bench::new();
        let mut store = Store::start(Backend::start(&bench.ctx, root.0.clone()));
        store.budget(budget);
        let mut session = Session { store, bench };
        session.opened();
        session
    }

    /// Open the library and fold in the first part of its listing only, so the rest is
    /// still to come.
    fn opening(root: &Temp) -> Session {
        let bench = Bench::new();
        let store = Store::start(Backend::start(&bench.ctx, root.0.clone()));
        let mut session = Session { store, bench };
        assert!(session.next(), "opening answered");
        assert!(session.next(), "the first part answered");
        assert!(session.store.listing());
        session
    }

    /// Fold in the rest of the open's listing, and any rescan or check in flight.
    fn listed_whole(&mut self) {
        while self.store.listing() || self.store.scanning() {
            assert!(self.next(), "the listing answered");
        }
    }

    /// Run frames of the store until `done` says so.
    fn until(&mut self, done: impl Fn(&Session) -> bool) {
        loop {
            let Bench {
                workspace,
                browser,
                queue,
                log,
                ..
            } = &mut self.bench;
            self.store.poll(workspace, browser, queue, log);
            if done(self) {
                return;
            }
            assert!(self.next(), "the backend answered");
        }
    }

    /// Ask for every file not read yet, and wait until each is read and decoded, and
    /// each left resting is checked.
    fn read_all(&mut self) {
        self.ask_all();
        let Bench { workspace, log, .. } = &mut self.bench;
        workspace.settle_files(log);
    }

    /// Ask for every file not read yet, and wait until each is read.
    fn ask_all(&mut self) {
        let workspace = &self.bench.workspace;
        let unread: Vec<u64> = workspace
            .listed()
            .filter(|entity| entity.unread())
            .map(|entity| entity.id)
            .collect();
        for id in unread {
            workspace.hurry(id);
        }
        self.answer_reads();
    }

    /// Send the reads asked for, and wait until each is answered, a read refused for room
    /// and asked again included. What they read is not decoded yet.
    fn answer_reads(&mut self) {
        loop {
            let Bench {
                workspace,
                browser,
                queue,
                log,
                ..
            } = &mut self.bench;
            self.store.ask(workspace, browser, queue, log);
            while self.bench.workspace.asking() {
                assert!(self.next(), "the read answered");
            }
            if !self.bench.workspace.wants() {
                return;
            }
        }
    }

    /// Something needs these assets now: read and decode each not read yet.
    fn read(&mut self, ids: &[u64]) {
        for id in ids {
            self.bench.workspace.hurry(*id);
        }
        self.answer_reads();
        let Bench { workspace, log, .. } = &mut self.bench;
        workspace.settle_files(log);
    }

    /// Two frames pass in which nothing needs any asset.
    fn later(&mut self) {
        let Bench { workspace, log, .. } = &mut self.bench;
        workspace.poll(log);
        workspace.poll(log);
    }

    /// The asset listed under `name`.
    fn named(&self, name: &str) -> u64 {
        let mut listed = self.bench.workspace.listed();
        listed.find(|entity| entity.name == name).expect(name).id
    }

    /// Wait for the open's answer, and for every part of its listing.
    fn opened(&mut self) {
        assert!(self.next(), "opening answered");
        while self.store.listing() || self.store.scanning() {
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

    /// A pass on the autosave cadence: the files, the working copies and the index.
    fn autosave(&mut self) {
        let Bench {
            workspace,
            browser,
            queue,
            ..
        } = &mut self.bench;
        self.store.sync(workspace, browser, queue, Pass::Full);
    }

    /// Rename a folder, and send the change without waiting for it.
    fn rename_folder(&mut self, id: u64, name: &str) {
        let name = name.into();
        self.bench
            .act(vec![crate::browser::Act::RenameFolder { id, name }]);
        let Bench {
            workspace,
            browser,
            queue,
            ..
        } = &mut self.bench;
        self.store.sync(workspace, browser, queue, Pass::Files);
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
        self.bench.workspace.get(id).expect("held").bytes.to_vec()
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

/// A file with a tag that nothing has read is read in the background once the library is
/// listed, and the index keeps its CRC, so the file keeps its tag through a rename outside
/// drawbar, while drawbar runs and while it does not.
#[test]
fn a_tagged_file_never_read_keeps_its_tag_through_a_rename_outside() {
    let root = Temp::new();
    fs::write(root.at("Grand.ne5p"), Fresh::Program.bytes().unwrap()).unwrap();
    let mut first = Session::listed(&root);
    let id = first.only();
    let tag = first.bench.browser.tags.make("Sunday").unwrap();
    first.bench.browser.tags.set(id, tag, true);
    first.close();

    let mut second = Session::listed(&root);
    second.answer_reads();
    let Bench { workspace, log, .. } = &mut second.bench;
    workspace.settle_files(log);
    second.autosave();
    second.settle();
    fs::rename(root.at("Grand.ne5p"), root.at("Piano.ne5p")).unwrap();
    second.refocus();
    assert_eq!(second.path(id).as_deref(), Some("Piano.ne5p"));
    assert!(second.bench.browser.tags.worn(id).contains(&tag));
    second.close();

    fs::rename(root.at("Piano.ne5p"), root.at("Grand.ne5p")).unwrap();
    let third = Session::listed(&root);
    assert_eq!(third.path(id).as_deref(), Some("Grand.ne5p"));
    assert!(third.bench.browser.tags.worn(id).contains(&tag));
    assert_eq!(third.bench.workspace.listed().count(), 1, "not a new asset");
}

/// A file at a new path shows as soon as the listing finds it, though it may be a file
/// the index names, moved. Once the listing is done and its contents match, it is that
/// file: one asset, under the index's id, with its tags.
#[test]
fn a_file_renamed_while_drawbar_was_away_shows_at_once_and_becomes_its_asset() {
    let root = Temp::new();
    let mut first = Session::open(&root);
    let id = first.create();
    let tag = first.bench.browser.tags.make("Sunday").unwrap();
    first.bench.browser.tags.set(id, tag, true);
    first.sync();
    first.close();
    fs::rename(root.at("untitled.ne5p"), root.at("Grand.ne5p")).unwrap();

    let mut second = Session::opening(&root);
    let shown = loop {
        let mut listed = second.bench.workspace.listed();
        if let Some(found) = listed.find(|entity| entity.name == "Grand.ne5p") {
            break found.id;
        }
        drop(listed);
        assert!(second.next(), "the listing answered");
    };
    assert!(second.store.listing(), "shown before the listing is done");
    assert_ne!(shown, id, "as a file of its own, until then");
    second.listed_whole();
    assert_eq!(second.only(), id);
    assert_eq!(second.path(id).as_deref(), Some("Grand.ne5p"));
    assert!(second.bench.browser.tags.worn(id).contains(&tag));
    assert!(second.bench.browser.tags.worn(shown).is_empty());
    assert_eq!(second.said("deleted outside drawbar"), 0);
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
    let Bench { workspace, log, .. } = &mut session.bench;
    workspace.settle_files(log);
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

/// A folder where the index says a file is, is not taken for that file: it is listed as
/// a folder, and the row, which held a tag, is kept as lost.
#[test]
fn a_folder_at_the_path_of_an_indexed_file_is_not_that_file() {
    let root = Temp::new();
    let mut first = Session::open(&root);
    let id = first.create();
    let tag = first.bench.browser.tags.make("Sunday").unwrap();
    first.bench.browser.tags.set(id, tag, true);
    first.close();
    fs::remove_file(root.at("untitled.ne5p")).unwrap();
    fs::create_dir(root.at("untitled.ne5p")).unwrap();

    let session = Session::listed(&root);
    assert_eq!(session.bench.workspace.listed().count(), 0, "no asset");
    let folders = &session.bench.browser.folders;
    assert!(folders
        .id_of(&LibPath::parse("untitled.ne5p").unwrap())
        .is_some());
    let lost: Vec<u64> = folders.lost().iter().map(|lost| lost.id).collect();
    assert_eq!(lost, [id]);
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

/// After an open's listing nothing is read: each file is an asset under its name, of the
/// kind its name says and the length the listing gave. Only a file something asks for, a
/// row in view or one picked, is read, and then decoded off the frame.
#[test]
fn after_the_walk_nothing_is_read_until_something_asks_for_it() {
    let root = Temp::new();
    let program = Fresh::Program.bytes().unwrap();
    fs::write(root.at("Grand.ne5p"), &program).unwrap();
    fs::write(root.at("Other.ne5p"), with_gain(&program, "12")).unwrap();
    let mut session = Session::listed(&root);
    let (grand, other) = (session.named("Grand.ne5p"), session.named("Other.ne5p"));
    for id in [grand, other] {
        let workspace = &session.bench.workspace;
        let entity = workspace.get(id).unwrap();
        assert!(entity.unread());
        assert!(entity.bytes.is_empty() && entity.entity.is_none());
        assert_eq!(Kind::of(entity), Kind::Program);
        assert_eq!(entity.size(), program.len() as u64);
        assert!(!entity.is_unsaved());
        assert!(!workspace.wanted(id));
    }
    assert_eq!(
        session.bench.workspace.reading(),
        0,
        "nothing is being read"
    );

    session.bench.workspace.in_view([grand]);
    assert_eq!(session.bench.workspace.reading(), 1);
    session.answer_reads();
    let Bench { workspace, log, .. } = &mut session.bench;
    workspace.settle_files(log);
    let entity = workspace.get(grand).unwrap();
    assert!(!entity.unread());
    assert!(matches!(entity.verify, VerifyState::Ok));
    assert_eq!(Kind::of(entity), Kind::Program);
    assert_eq!(entity.bytes, program);
    assert!(!entity.is_unsaved());
    assert!(workspace.get(other).unwrap().unread(), "not in view");
    assert_eq!(workspace.reading(), 0);
}

/// What a listing finds is enough to search a library, narrow it by kind and count its
/// folders: a row's name, kind and size come from the listing. The files here are not
/// what their names say, and nothing reads them to find out.
#[test]
fn search_kinds_and_counts_work_on_files_not_read_yet() {
    let root = Temp::new();
    fs::create_dir_all(root.at("Sets/Deep")).unwrap();
    for (at, len) in [
        ("Grand.ne5p", 10),
        ("Sets/Gig.ne5t", 20),
        ("Sets/Deep/Pad.ne5p", 30),
        ("Piano.npno", 40),
        ("notes.pdf", 50),
    ] {
        fs::write(root.at(at), vec![0xa5; len]).unwrap();
    }
    let session = Session::listed(&root);
    let Bench {
        workspace,
        browser,
        device,
        queue,
        ..
    } = &session.bench;
    let rows = |kind| {
        let filter = crate::filter::Filter {
            kind,
            ..Default::default()
        };
        crate::library::rows(workspace, &device.state, queue, &browser.tags, &filter)
    };
    let names = |rows: &[crate::library::Row]| -> Vec<(String, u64)> {
        let mut named: Vec<(String, u64)> = rows
            .iter()
            .map(|row| (row.name.clone(), row.size))
            .collect();
        named.sort();
        named
    };
    let programs = rows(Some(Kind::Program));
    assert_eq!(
        names(&programs),
        [("Grand.ne5p".into(), 10), ("Pad.ne5p".into(), 30)]
    );
    assert_eq!(names(&rows(Some(Kind::Piano))), [("Piano.npno".into(), 40)]);
    assert_eq!(names(&rows(Some(Kind::SetList))), [("Gig.ne5t".into(), 20)]);
    let all = crate::library::arrange(
        rows(None),
        "gig",
        crate::library::Column::Name,
        crate::library::Order::Up,
    );
    assert_eq!(names(&all), [("Gig.ne5t".into(), 20)]);

    let folders = &browser.folders;
    let sets = folders.id_of(&LibPath::parse("Sets").unwrap());
    let deep = folders.id_of(&LibPath::parse("Sets/Deep").unwrap());
    let counts = [None, sets, deep].map(|folder| folders.count(folder, workspace));
    assert_eq!(counts, [4, 2, 1], "the library, Sets and Sets/Deep");

    assert!(workspace.listed().all(|entity| entity.unread()));
    assert_eq!(workspace.reading(), 0, "nothing is being read");
}

/// A row of the index that holds a working copy comes back at open with its edit over
/// its file, which is read for it. A row without one is read only once something asks.
#[test]
fn an_indexed_row_with_a_working_copy_is_restored_at_open() {
    let root = Temp::new();
    let mut first = Session::open(&root);
    let (edited, other) = (first.create(), first.create());
    first.sync();
    let saved = first.bytes(edited);
    let mine = with_gain(&saved, "96");
    let log = &mut first.bench.log;
    first
        .bench
        .workspace
        .replace_bytes(edited, mine.clone(), log);
    first.close();

    let second = Session::listed(&root);
    let entity = second.bench.workspace.get(edited).expect("under its id");
    assert!(!entity.unread());
    assert_eq!(entity.bytes, mine);
    assert_eq!(entity.saved.bytes, saved, "what its file holds");
    assert!(entity.is_unsaved());
    assert!(second.bench.workspace.get(other).unwrap().unread());
}

/// A file changed after it is listed and before it is read is read as it is then, and
/// is nobody's edit. A save over it is refused only where it changes after that read.
#[test]
fn a_file_is_checked_against_what_was_read_and_not_what_was_listed() {
    let root = Temp::new();
    let program = Fresh::Program.bytes().unwrap();
    fs::write(root.at("Grand.ne5p"), &program).unwrap();
    let mut session = Session::listed(&root);
    let id = session.named("Grand.ne5p");
    let theirs = with_gain(&program, "12");
    fs::write(root.at("Grand.ne5p"), &theirs).unwrap();
    session.read_all();
    let entity = session.bench.workspace.get(id).unwrap();
    assert_eq!(entity.bytes, theirs);
    assert!(!entity.is_unsaved());

    let again = with_gain(&program, "64");
    fs::write(root.at("Grand.ne5p"), &again).unwrap();
    let mine = with_gain(&program, "96");
    let log = &mut session.bench.log;
    session.bench.workspace.replace_bytes(id, mine, log);
    session.bench.workspace.mark_saved(id);
    session.sync();
    assert_eq!(root.read("Grand.ne5p"), again, "not written over");
    assert_eq!(session.said("was not saved, because it changed on disk"), 1);
}

/// A folder renamed while the library is still being listed is renamed on disk at once.
/// Everything under it goes with it: the rows of the index, with their tags and their
/// unsaved edits, and the files the listing had still to bring back. Writes go meanwhile,
/// and a file saved while the listing is in flight is not listed twice.
#[test]
fn a_folder_renamed_while_the_library_is_listed_takes_everything_under_it() {
    let root = Temp::new();
    let program = Fresh::Program.bytes().unwrap();
    fs::create_dir_all(root.at("Gig/Deep")).unwrap();
    fs::write(root.at("Gig/Grand.ne5p"), &program).unwrap();
    fs::write(root.at("Gig/Deep/Pad.ne5p"), with_gain(&program, "12")).unwrap();
    let mut first = Session::open(&root);
    let grand = first.named("Grand.ne5p");
    let tag = first.bench.browser.tags.make("Sunday").unwrap();
    first.bench.browser.tags.set(grand, tag, true);
    let mine = with_gain(&program, "96");
    let log = &mut first.bench.log;
    first
        .bench
        .workspace
        .replace_bytes(grand, mine.clone(), log);
    first.close();
    fs::write(root.at("Gig/Deep/New.ne5p"), with_gain(&program, "64")).unwrap();

    let mut second = Session::opening(&root);
    let folders = &second.bench.browser.folders;
    let gig = folders.id_of(&LibPath::parse("Gig").unwrap());
    let gig = gig.expect("the folder of a row of the index");
    let rename = crate::browser::Act::RenameFolder {
        id: gig,
        name: "Set".into(),
    };
    second
        .bench
        .act(vec![rename, crate::browser::Act::NewFolder]);
    let new = LibPath::parse("New folder").unwrap();
    let folder = second.bench.browser.folders.id_of(&new);
    let made = second.create();
    let Bench {
        workspace,
        browser,
        queue,
        ..
    } = &mut second.bench;
    assert!(
        second.store.sync(workspace, browser, queue, Pass::Files),
        "nothing waits for the listing"
    );
    second.listed_whole();
    assert_eq!(second.bench.browser.folders.id_of(&new), folder, "it stays");
    let set = LibPath::parse("Set").unwrap();
    assert_eq!(second.bench.browser.folders.id_of(&set), Some(gig));
    second.sync();
    let mut paths: Vec<String> = second
        .bench
        .workspace
        .listed()
        .filter_map(|entity| Some(entity.path.as_ref()?.to_string()))
        .collect();
    paths.sort();
    assert_eq!(
        paths,
        [
            "Set/Deep/New.ne5p",
            "Set/Deep/Pad.ne5p",
            "Set/Grand.ne5p",
            "untitled.ne5p"
        ]
    );
    assert_eq!(second.path(made).as_deref(), Some("untitled.ne5p"));
    assert_eq!(
        root.names(""),
        [".drawbar", "New folder", "Set", "untitled.ne5p"]
    );
    assert_eq!(root.names("Set/Deep"), ["New.ne5p", "Pad.ne5p"]);
    let entity = second.bench.workspace.get(grand).unwrap();
    assert_eq!(entity.bytes, mine);
    assert!(entity.is_unsaved());
    assert!(second.bench.browser.tags.worn(grand).contains(&tag));
    second.close();

    let third = Session::listed(&root);
    assert_eq!(third.path(grand).as_deref(), Some("Set/Grand.ne5p"));
    assert!(third.bench.browser.tags.worn(grand).contains(&tag));
    let entity = third.bench.workspace.get(grand).unwrap();
    assert_eq!(entity.bytes, mine, "its edit came back");
    assert!(entity.is_unsaved());
}

/// An asset made while the library is still being listed takes an id of its own, and
/// every file the listing brings back still comes back, the files the index names and
/// the ones it does not.
#[test]
fn an_asset_made_while_the_library_is_listed_shares_no_id_with_a_file() {
    let root = Temp::new();
    let mut first = Session::open(&root);
    let indexed = first.create();
    let tag = first.bench.browser.tags.make("Sunday").unwrap();
    first.bench.browser.tags.set(indexed, tag, true);
    first.close();
    fs::write(root.at("Other.ne5p"), Fresh::Program.bytes().unwrap()).unwrap();

    let bench = Bench::new();
    let store = Store::start(Backend::start(&bench.ctx, root.0.clone()));
    let mut second = Session { store, bench };
    assert!(second.next(), "opening answered");
    let before = second.create();
    assert!(second.next(), "the rows of the index answered");
    let after = second.create();
    second.listed_whole();

    let workspace = &second.bench.workspace;
    let mut names: Vec<&str> = workspace.listed().map(|e| e.name.as_str()).collect();
    names.sort();
    assert_eq!(
        names,
        [
            "Other.ne5p",
            "untitled.ne5p",
            "untitled.ne5p",
            "untitled.ne5p"
        ]
    );
    let file = workspace
        .listed()
        .find(|entity| {
            entity
                .path
                .as_ref()
                .is_some_and(|at| at.as_str() == "untitled.ne5p")
        })
        .expect("the file the index names");
    assert!(second.bench.browser.tags.worn(file.id).contains(&tag));
    for made in [before, after] {
        assert_eq!(second.path(made), None, "not placed yet");
    }
}

/// A folder rename the disk refuses while the library is still being listed leaves
/// everything where the disk has it: the files listed before the refusal and after it
/// keep their ids and tags under the old name, and nothing reads as deleted or new.
#[test]
fn a_folder_rename_refused_while_the_library_is_listed_leaves_everything_where_it_was() {
    const FILES: usize = 300;
    let root = Temp::new();
    fs::create_dir(root.at("Gig")).unwrap();
    for n in 0..FILES {
        fs::write(root.at(&format!("Gig/{n:03}.ne5p")), vec![0xa5; 10 + n]).unwrap();
    }
    let mut session = Session::opening(&root);
    let gig = LibPath::parse("Gig").unwrap();
    let folder = session.bench.browser.folders.id_of(&gig).expect("listed");
    let listed: Vec<u64> = session.bench.workspace.listed().map(|e| e.id).collect();
    assert!(
        !listed.is_empty() && listed.len() < FILES,
        "{} listed",
        listed.len()
    );
    let tag = session.bench.browser.tags.make("Sunday").unwrap();
    session.bench.browser.tags.set(listed[0], tag, true);
    // A folder the listing has not shown takes the name first.
    fs::create_dir(root.at("Set")).unwrap();
    let rename = crate::browser::Act::RenameFolder {
        id: folder,
        name: "Set".into(),
    };
    session.bench.act(vec![rename]);
    let Bench {
        workspace,
        browser,
        queue,
        ..
    } = &mut session.bench;
    session.store.sync(workspace, browser, queue, Pass::Files);
    session.listed_whole();
    session.settle();

    let workspace = &session.bench.workspace;
    let mut paths: Vec<String> = workspace
        .listed()
        .filter_map(|entity| Some(entity.path.as_ref()?.to_string()))
        .collect();
    paths.sort();
    let expected: Vec<String> = (0..FILES).map(|n| format!("Gig/{n:03}.ne5p")).collect();
    assert_eq!(paths, expected);
    for &id in &listed {
        let entity = workspace.get(id).expect("the same id");
        assert!(
            entity.path.as_ref().unwrap().is_in(&gig),
            "{:?}",
            entity.path
        );
    }
    assert!(session.bench.browser.tags.worn(listed[0]).contains(&tag));
    assert_eq!(session.bench.browser.folders.id_of(&gig), Some(folder));
    assert_eq!(session.said("deleted outside drawbar"), 0);
    assert_eq!(session.said("appeared in the library folder"), 0);
    assert_eq!(root.names("Gig").len(), FILES);
    assert_eq!(
        session.said("did not change as asked: moving Gig to Set"),
        1
    );
}

/// A library of one folder, `Gig`, holding one tagged file nothing has read, and the
/// folder's id.
fn gig() -> (Temp, Session, u64, u64) {
    let root = Temp::new();
    fs::create_dir(root.at("Gig")).unwrap();
    fs::write(root.at("Gig/a.ne5p"), Fresh::Program.bytes().unwrap()).unwrap();
    let mut session = Session::listed(&root);
    let tag = session.bench.browser.tags.make("Sunday").unwrap();
    session.bench.browser.tags.set(session.only(), tag, true);
    let gig = LibPath::parse("Gig").unwrap();
    let folder = session.bench.browser.folders.id_of(&gig).expect("listed");
    (root, session, folder, tag)
}

/// Two renames of one folder, the second sent before the first answers. The disk
/// refuses the first, since a folder drawbar does not know has the name; that folder is
/// left alone, and the second renames the folder drawbar holds.
#[test]
fn a_second_rename_waits_for_a_first_the_disk_refuses() {
    let (root, mut session, folder, tag) = gig();
    let id = session.only();
    fs::create_dir(root.at("Set")).unwrap();
    fs::write(root.at("Set/theirs.pdf"), b"theirs").unwrap();
    session.rename_folder(folder, "Set");
    session.rename_folder(folder, "Sets");
    session.settle();
    session.sync();
    assert_eq!(root.names(""), [".drawbar", "Set", "Sets"]);
    assert_eq!(root.names("Set"), ["theirs.pdf"], "theirs is untouched");
    assert_eq!(root.names("Sets"), ["a.ne5p"]);
    assert_eq!(session.only(), id);
    assert_eq!(session.path(id).as_deref(), Some("Sets/a.ne5p"));
    assert!(session.bench.browser.tags.worn(id).contains(&tag));
    assert_eq!(
        session.said("did not change as asked: moving Gig to Set"),
        1
    );
}

/// Two renames of one folder, the second sent before the first answers, both land.
#[test]
fn a_second_rename_waits_for_a_first_that_lands() {
    let (root, mut session, folder, tag) = gig();
    let id = session.only();
    session.rename_folder(folder, "Set");
    session.rename_folder(folder, "Sets");
    session.settle();
    session.sync();
    assert_eq!(root.names(""), [".drawbar", "Sets"]);
    assert_eq!(root.names("Sets"), ["a.ne5p"]);
    assert_eq!(session.path(id).as_deref(), Some("Sets/a.ne5p"));
    assert!(session.bench.browser.tags.worn(id).contains(&tag));
    assert_eq!(session.said("did not change as asked"), 0);
}

/// A file moved into a folder whose rename has not answered waits for it. Where the
/// disk refuses the rename, the file goes into the folder under its old name, never into
/// the folder outside drawbar that has the new one.
#[test]
fn a_file_moved_into_a_folder_whose_rename_is_unanswered_waits_for_it() {
    let (root, mut session, folder, _) = gig();
    fs::write(
        root.at("f.ne5p"),
        with_gain(&Fresh::Program.bytes().unwrap(), "12"),
    )
    .unwrap();
    session.refocus();
    let id = session.named("f.ne5p");
    fs::create_dir(root.at("Set")).unwrap();
    fs::write(root.at("Set/theirs.pdf"), b"theirs").unwrap();
    session.rename_folder(folder, "Set");
    let Bench {
        workspace,
        browser,
        queue,
        ..
    } = &mut session.bench;
    browser.folders.file(workspace, id, Some(folder));
    assert_eq!(
        workspace.get(id).unwrap().path,
        LibPath::parse("Set/f.ne5p")
    );
    assert!(
        !session.store.sync(workspace, browser, queue, Pass::Files),
        "the move waits"
    );
    session.settle();
    session.sync();
    assert_eq!(root.names("Set"), ["theirs.pdf"], "theirs is untouched");
    assert_eq!(root.names("Gig"), ["a.ne5p", "f.ne5p"]);
    assert_eq!(session.path(id).as_deref(), Some("Gig/f.ne5p"));
}

/// A folder renamed while a rename of a file in it has not answered waits for it. Where
/// the disk refuses the file's rename, since a file drawbar does not know has the name,
/// the file keeps its old name inside the renamed folder, with its tag.
#[test]
fn a_folder_renamed_while_a_file_in_it_is_renamed_waits_for_it() {
    let (root, mut session, folder, tag) = gig();
    let id = session.only();
    fs::write(root.at("Gig/b.ne5p"), b"theirs").unwrap();
    session
        .bench
        .workspace
        .place(id, LibPath::parse("Gig/b.ne5p").unwrap());
    let Bench {
        workspace,
        browser,
        queue,
        ..
    } = &mut session.bench;
    session.store.sync(workspace, browser, queue, Pass::Files);
    session.rename_folder(folder, "Set");
    session.settle();
    session.sync();
    assert_eq!(root.names(""), [".drawbar", "Set"]);
    assert_eq!(root.names("Set"), ["a.ne5p", "b.ne5p"]);
    assert_eq!(fs::read(root.at("Set/b.ne5p")).unwrap(), b"theirs");
    assert_eq!(session.path(id).as_deref(), Some("Set/a.ne5p"));
    assert!(session.bench.browser.tags.worn(id).contains(&tag));
    assert_eq!(session.said("did not change as asked"), 1);
}

/// Parts of a listing that arrive while a rename of their folder waits are shown where
/// that rename puts them. When it lands, nothing is moved back, and nothing fails.
#[test]
fn a_listing_inside_a_folder_whose_rename_waits_lands_with_it() {
    const FILES: usize = 300;
    let root = Temp::new();
    fs::create_dir(root.at("Gig")).unwrap();
    for n in 0..FILES {
        fs::write(root.at(&format!("Gig/{n:03}.ne5p")), vec![0xa5; 10 + n]).unwrap();
    }
    let mut session = Session::opening(&root);
    let gig = LibPath::parse("Gig").unwrap();
    let folder = session.bench.browser.folders.id_of(&gig).expect("listed");
    assert!(
        session.bench.workspace.listed().count() < FILES,
        "more to come"
    );
    // A file's rename in the folder holds the folder's rename until it answers.
    let first = session.named("000.ne5p");
    let renamed = LibPath::parse("Gig/first.ne5p").unwrap();
    session.bench.workspace.place(first, renamed);
    let Bench {
        workspace,
        browser,
        queue,
        ..
    } = &mut session.bench;
    session.store.sync(workspace, browser, queue, Pass::Files);
    session.rename_folder(folder, "Sets");
    session.listed_whole();
    session.sync();

    assert_eq!(session.said("did not change as asked"), 0);
    assert_eq!(root.names(""), [".drawbar", "Sets"]);
    assert_eq!(root.names("Sets").len(), FILES);
    let workspace = &session.bench.workspace;
    assert_eq!(workspace.listed().count(), FILES);
    let sets = LibPath::parse("Sets").unwrap();
    assert!(workspace
        .listed()
        .all(|entity| entity.path.as_ref().is_some_and(|at| at.is_in(&sets))));
    assert_eq!(session.path(first).as_deref(), Some("Sets/first.ne5p"));
    let folders = &session.bench.browser.folders;
    assert_eq!(folders.id_of(&gig), None);
    assert_eq!(folders.id_of(&sets), Some(folder));
}

/// A folder removed while the library is still being listed waits until everything in
/// it has been listed, and then the usual rules apply: it goes, and what was in it moves
/// up, unless it holds a file drawbar does not.
#[test]
fn a_folder_removed_while_the_library_is_listed_is_listed_first() {
    let program = Fresh::Program.bytes().unwrap();
    for stranger in [false, true] {
        let root = Temp::new();
        fs::create_dir_all(root.at("Old/Inner")).unwrap();
        fs::write(root.at("top.ne5p"), &program).unwrap();
        fs::write(root.at("Old/a.ne5p"), &program).unwrap();
        fs::write(root.at("Old/Inner/b.ne5p"), &program).unwrap();
        if stranger {
            fs::write(root.at("Old/Inner/scan.pdf"), b"").unwrap();
        }
        let mut session = Session::opening(&root);
        let old = LibPath::parse("Old").unwrap();
        let id = session.bench.browser.folders.id_of(&old).expect("listed");
        session
            .bench
            .act(vec![crate::browser::Act::RemoveFolder(id)]);
        assert!(
            session.bench.browser.folders.id_of(&old).is_some(),
            "not before it is listed"
        );
        session.until(|session| session.bench.browser.folders.listed_whole(&old));
        session.bench.act(Vec::new());
        session.listed_whole();
        session.sync();
        match stranger {
            false => {
                assert_eq!(root.names(""), [".drawbar", "Inner", "a.ne5p", "top.ne5p"]);
                assert_eq!(root.names("Inner"), ["b.ne5p"]);
            }
            true => {
                assert_eq!(root.names(""), ["Old", "top.ne5p"]);
                assert_eq!(session.said("holds files drawbar does not hold"), 1);
            }
        }
    }
}

/// A send held while the library is still being listed is let go once the files read so
/// far are looked at again, without a rescan of the whole tree.
#[test]
fn a_send_held_while_the_library_is_listed_goes_once_what_was_read_is_checked() {
    let root = Temp::new();
    fs::write(root.at("Grand.ne5p"), Fresh::Program.bytes().unwrap()).unwrap();
    let mut session = Session::opening(&root);
    assert!(session.store.hold_send(), "the send waits");
    assert!(session.store.scanning());
    while session.store.holds_send() {
        assert!(session.next(), "the check answered");
    }
    assert_eq!(session.said("nothing was sent"), 0);
    session.listed_whole();
}

/// An act on a file not read yet waits for it to be read, and decodes it before it runs.
#[test]
fn acting_on_a_file_not_read_yet_reads_and_decodes_it_first() {
    let root = Temp::new();
    let program = Fresh::Program.bytes().unwrap();
    fs::write(root.at("Grand.ne5p"), &program).unwrap();
    let mut session = Session::listed(&root);
    let grand = session.named("Grand.ne5p");

    let open = crate::browser::Act::Open(crate::browser::Item::Local(grand));
    session.bench.act(vec![open]);
    assert!(!session.bench.tabs.holds(grand), "not before it is read");
    assert!(session.bench.workspace.wanted(grand));
    session.answer_reads();
    assert!(session.bench.workspace.get(grand).unwrap().reading());
    session.bench.act(Vec::new());
    assert!(session.bench.tabs.holds(grand), "its tab is open");
    let entity = session.bench.workspace.get(grand).unwrap();
    assert_eq!(Kind::of(entity), Kind::Program);
    assert!(matches!(entity.verify, VerifyState::Ok));
    assert_eq!(entity.bytes, program);
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
    assert!(second.store.listing(), "its listing has not come back");
    second.close();

    let third = Session::open(&root);
    assert!(third.bench.browser.tags.worn(id).contains(&tag));
    assert_eq!(third.path(id).as_deref(), Some("untitled.ne5p"));
}

/// A row the open gives a new id keeps its unsaved edit through a quit before the
/// listing is complete, whether a listed file had claimed the row by then or not.
#[test]
fn an_edit_under_a_new_id_survives_a_quit_while_the_library_is_listed() {
    for claimed in [false, true] {
        let root = Temp::new();
        let mut first = Session::open(&root);
        let id = first.create();
        first.sync();
        let edited = with_gain(&first.bytes(id), "96");
        let log = &mut first.bench.log;
        first.bench.workspace.replace_bytes(id, edited.clone(), log);
        first.close();

        // Ids a library open before gave out, as when switching from one to another.
        let mut bench = Bench::new();
        for _ in 0..3 {
            let Bench { workspace, log, .. } = &mut bench;
            workspace.view("seen".into(), Origin::Fresh, b"a view".to_vec(), log);
        }
        assert!(bench.workspace.next_id() > id, "the row takes a new id");
        let store = Store::start(Backend::start(&bench.ctx, root.0.clone()));
        let mut second = Session { store, bench };
        assert!(second.next(), "opening answered");
        if claimed {
            assert!(second.next(), "the rows of the index answered");
            assert_eq!(second.bytes(second.only()), edited);
        }
        assert!(second.store.listing(), "its listing has not all come back");
        second.close();

        let third = Session::open(&root);
        let entity = third.bench.workspace.get(third.only()).unwrap();
        assert_eq!(
            entity.bytes, edited,
            "the edit came back (claimed: {claimed})"
        );
        assert!(entity.is_unsaved());
    }
}

/// Everything drawbar holds whole is in memory, so it reads only so much of one library.
/// A file past that is listed like any other, and a read of it is refused before
/// anything is read, and says why. The file here is sparse, so it takes no room on disk.
#[test]
fn a_file_past_the_most_drawbar_reads_is_listed_and_its_read_refused() {
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
    assert_eq!(names, ["Huge.nsmp", "Small.ne5p"]);
    let huge = session
        .bench
        .workspace
        .get(session.named("Huge.nsmp"))
        .unwrap();
    assert!(huge.unread());
    assert_eq!(huge.size(), MOST_BYTES + 1);
    let VerifyState::NotRead(why) = &huge.verify else {
        panic!("{}", huge.verify.detail());
    };
    assert!(why.contains("at most 1 GiB"), "{why}");
    let small = session.bench.workspace.get(session.named("Small.ne5p"));
    assert!(matches!(small.unwrap().verify, VerifyState::Ok));
}

/// Reads in flight count against what drawbar holds whole, so two asked one after the
/// other never read past it together. A read refused for want of room is asked for again
/// once room frees, here as an asset held whole is removed.
#[test]
fn a_read_refused_for_room_waits_for_room_and_reads_in_flight_count() {
    let root = Temp::new();
    let program = Fresh::Program.bytes().unwrap();
    let len = program.len() as u64;
    for (name, gain) in [("A.ne5p", "12"), ("B.ne5p", "24"), ("C.ne5p", "36")] {
        fs::write(root.at(name), with_gain(&program, gain)).unwrap();
    }
    let mut session = Session::listed(&root);
    let budget = 2 * len + 1;
    session.store.budget(budget);
    let [a, b, c] = ["A.ne5p", "B.ne5p", "C.ne5p"].map(|name| session.named(name));

    session.bench.workspace.in_view([a]);
    let Bench {
        workspace,
        browser,
        queue,
        log,
        ..
    } = &mut session.bench;
    session.store.ask(workspace, browser, queue, log);
    session.bench.workspace.in_view([b, c]);
    session.answer_reads();
    let workspace = &session.bench.workspace;
    assert!(
        workspace.held_whole() <= budget,
        "{} held",
        workspace.held_whole()
    );
    let unread: Vec<u64> = [a, b, c]
        .into_iter()
        .filter(|id| workspace.get(*id).unwrap().unread())
        .collect();
    assert_eq!(unread, [c]);
    let VerifyState::NotRead(why) = &workspace.get(c).unwrap().verify else {
        panic!("{}", workspace.get(c).unwrap().verify.detail());
    };
    assert!(why.contains("at most 1 GiB"), "{why}");

    session.answer_reads();
    assert!(
        session.bench.workspace.get(c).unwrap().unread(),
        "no room yet"
    );
    let Bench { workspace, log, .. } = &mut session.bench;
    workspace.remove(a, log);
    session.answer_reads();
    let entity = session.bench.workspace.get(c).unwrap();
    assert!(!entity.unread(), "read once there is room");
    assert_eq!(entity.saved.bytes, with_gain(&program, "36"));
}

/// Past the budget, a read lets go of the clean assets needed least recently, each unread
/// again until something needs it, and never of one that is unsaved, waiting to be sent,
/// or needed now. Where nothing else can go, the read is refused, and is read once
/// something can.
#[test]
fn a_read_past_the_budget_lets_go_of_the_assets_needed_least_recently() {
    let root = Temp::new();
    let program = Fresh::Program.bytes().unwrap();
    let len = program.len() as u64;
    let names = ["A.ne5p", "B.ne5p", "C.ne5p", "D.ne5p", "E.ne5p", "F.ne5p"];
    let files: Vec<Vec<u8>> = (1..=6)
        .map(|n| with_gain(&program, &(n * 12).to_string()))
        .collect();
    for (name, bytes) in names.iter().zip(&files) {
        fs::write(root.at(name), bytes).unwrap();
    }
    let mut session = Session::listed(&root);
    let budget = 5 * len + 1;
    session.store.budget(budget);
    let [a, b, c, d, e, f] = names.map(|name| session.named(name));
    let unread = |session: &Session| -> Vec<u64> {
        let listed = session.bench.workspace.listed();
        listed
            .filter(|entity| entity.unread())
            .map(|entity| entity.id)
            .collect()
    };

    session.read(&[a]);
    session.later();
    session.read(&[b, c, d, e]);
    session.later();
    session.read(&[f]);
    assert_eq!(unread(&session), [a], "the one needed least recently");
    let evicted = session.bench.workspace.get(a).unwrap();
    assert!(evicted.bytes.is_empty() && evicted.entity.is_none());
    assert_eq!(evicted.verify.note(), None, "it still draws as read");
    assert_eq!(Kind::of(evicted), Kind::Program);
    assert!(evicted.saved.crc32.is_some(), "it still matches its slot");
    assert!(session.bench.workspace.held_whole() <= budget);

    let Bench {
        workspace,
        device,
        queue,
        log,
        ..
    } = &mut session.bench;
    workspace.replace_bytes(b, with_gain(&program, "96"), log);
    device.pretend_attached();
    let slot = Location { bank: 0, slot: 0 };
    crate::queue::enqueue(workspace, device, queue, log, c, ObjectClass::Program, slot);
    assert!(queue.holds(c));
    let budget = 6 * len + 1;
    session.store.budget(budget);
    session.later();
    session.bench.workspace.in_view([e, f]);
    session.read(&[a]);
    assert_eq!(unread(&session), [d], "unsaved, queued and needed now stay");

    session.read(&[d]);
    let refused = session.bench.workspace.get(d).unwrap();
    assert!(refused.unread(), "nothing else can go");
    let VerifyState::NotRead(why) = &refused.verify else {
        panic!("{}", refused.verify.detail());
    };
    assert!(why.contains("at most 1 GiB"), "{why}");

    session.later();
    session.answer_reads();
    assert!(!session.bench.workspace.get(d).unwrap().unread());
    assert!(session.bench.workspace.held_whole() <= budget);
    for id in [b, c] {
        assert!(!session.bench.workspace.get(id).unwrap().unread());
    }

    session.sync();
    for (name, bytes) in names.iter().zip(&files) {
        assert_eq!(&root.read(name), bytes, "{name} was not written");
    }
}

/// Reads refused for want of room are asked for again smallest first, and once room
/// cannot be made for one, the larger ones wait without another pass over every asset.
#[test]
fn reads_waiting_for_room_look_for_it_once_a_frame() {
    let root = Temp::new();
    let program = Fresh::Program.bytes().unwrap();
    let len = program.len() as u64;
    let names: Vec<String> = (0..12).map(|n| format!("P{n:02}.ne5p")).collect();
    for (n, name) in names.iter().enumerate() {
        fs::write(root.at(name), with_gain(&program, &(n * 3).to_string())).unwrap();
    }
    let mut session = Session::listed(&root);
    session.store.budget(2 * len + 1);
    let ids: Vec<u64> = names.iter().map(|name| session.named(name)).collect();
    session.read(&ids[..2]);
    session.read(&ids);
    let refused = ids.iter().filter(|id| {
        let entity = session.bench.workspace.get(**id).unwrap();
        entity.unread()
    });
    assert_eq!(refused.count(), 10, "only two fit, and both are needed now");

    session.store.looked_for_room = 0;
    session.read(&ids);
    assert_eq!(
        session.store.looked_for_room, 1,
        "one pass, not one per read"
    );
}

/// Once the library is listed, each file the index tracks, here by a tag, is read and
/// decoded in the background, so it can match its slot before anything shows it. A file
/// the index does not track waits until something needs it.
#[test]
fn after_open_a_tracked_file_is_read_without_being_asked() {
    let root = Temp::new();
    let program = Fresh::Program.bytes().unwrap();
    fs::write(root.at("Tagged.ne5p"), with_gain(&program, "12")).unwrap();
    fs::write(root.at("Plain.ne5p"), with_gain(&program, "24")).unwrap();
    let mut first = Session::listed(&root);
    let tag = first.bench.browser.tags.make("Sunday").unwrap();
    let tagged = first.named("Tagged.ne5p");
    first.bench.browser.tags.set(tagged, tag, true);
    first.close();

    let mut second = Session::listed(&root);
    second.answer_reads();
    let Bench { workspace, log, .. } = &mut second.bench;
    workspace.settle_files(log);
    let entity = workspace.get(tagged).unwrap();
    assert!(!entity.unread(), "read without being asked");
    assert!(matches!(entity.verify, VerifyState::Ok));
    assert!(entity.saved.crc32.is_some(), "it can match its slot");
    let plain = second.named("Plain.ne5p");
    assert!(second.bench.workspace.get(plain).unwrap().unread());
}

/// Past the budget, files the index does not track go before those it tracks, whenever
/// they were needed. A read in the background lets go of no tracked file: one with no
/// room is left unread, as if never asked for, until something needs it.
#[test]
fn untracked_files_go_before_tracked_ones_at_the_budget() {
    let root = Temp::new();
    let program = Fresh::Program.bytes().unwrap();
    let len = program.len() as u64;
    let names = ["T1.ne5p", "T2.ne5p", "T3.ne5p", "U.ne5p", "V.ne5p"];
    for (n, name) in names.iter().enumerate() {
        fs::write(
            root.at(name),
            with_gain(&program, &(12 * n + 12).to_string()),
        )
        .unwrap();
    }
    let mut first = Session::listed(&root);
    let tag = first.bench.browser.tags.make("Sunday").unwrap();
    for name in &names[..3] {
        let id = first.named(name);
        first.bench.browser.tags.set(id, tag, true);
    }
    first.close();

    let mut second = Session::within(&root, 2 * len + 1);
    second.answer_reads();
    let unread = |session: &Session| -> Vec<String> {
        let listed = session.bench.workspace.listed();
        let unread = listed.filter(|entity| entity.unread());
        let mut names: Vec<String> = unread.map(|entity| entity.name.clone()).collect();
        names.sort();
        names
    };
    assert_eq!(unread(&second), ["T3.ne5p", "U.ne5p", "V.ne5p"]);
    let t3 = second.bench.workspace.get(second.named("T3.ne5p")).unwrap();
    assert_eq!(t3.verify.note(), Some("reading…"), "not refused, only left");

    let [u, v] = ["U.ne5p", "V.ne5p"].map(|name| second.named(name));
    second.read(&[u]);
    assert_eq!(unread(&second), ["T1.ne5p", "T3.ne5p", "V.ne5p"]);
    second.later();
    second.read(&[v]);
    assert_eq!(
        unread(&second),
        ["T1.ne5p", "T3.ne5p", "U.ne5p"],
        "U was needed after T2, and goes first"
    );
}

/// A file read and not edited is held once: the asset's bytes and what it was saved as
/// are one allocation, counted once against what drawbar holds whole. An edit holds a
/// second copy, and a revert lets it go.
#[test]
fn a_clean_asset_holds_its_bytes_once() {
    let root = Temp::new();
    let program = Fresh::Program.bytes().unwrap();
    let len = program.len() as u64;
    fs::write(root.at("Grand.ne5p"), &program).unwrap();
    let mut session = Session::open(&root);
    let id = session.named("Grand.ne5p");
    let held = |session: &Session| {
        let workspace = &session.bench.workspace;
        let entity = workspace.get(id).unwrap();
        let shared = entity.bytes.shares(&entity.saved.bytes);
        (shared, workspace.held_whole())
    };
    assert_eq!(held(&session), (true, len));

    let Bench { workspace, log, .. } = &mut session.bench;
    workspace.replace_bytes(id, with_gain(&program, "96"), log);
    assert_eq!(held(&session), (false, 2 * len));

    let Bench { workspace, log, .. } = &mut session.bench;
    workspace.revert(id, log);
    assert_eq!(held(&session), (true, len));
    assert_eq!(session.bench.workspace.get(id).unwrap().bytes, program);
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

    let mut session = Session::listed(&root);
    session.ask_all();
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

    let mut session = Session::listed(&root);
    session.ask_all();
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

/// The cache file a session that remembers keeps, under the shelf's folder.
const CACHE: &str = "library-cache.ron";

impl Session {
    /// [`Session::listed`], remembering what was read in `shelf`, as the app keeps it in
    /// its own data between sessions.
    fn remembering(root: &Temp, shelf: &Temp) -> Session {
        Session::kept_at(root, shelf.at(CACHE))
    }

    /// [`Session::remembering`] in the cache file `file`.
    fn kept_at(root: &Temp, file: std::path::PathBuf) -> Session {
        let bench = Bench::new();
        let cache = Cache::at(&bench.ctx, file, &root.0);
        let backend = Backend::start(&bench.ctx, root.0.clone());
        let store = Store::start(backend).remembering(cache);
        let mut session = Session { store, bench };
        session.opened();
        let Bench {
            workspace,
            browser,
            log,
            ..
        } = &mut session.bench;
        session.store.recall_kept(workspace, browser, log);
        session.answer_reads();
        session
    }

    /// How many files have been asked of the library to read, in the background or for
    /// something that needs them.
    fn reads(&self) -> usize {
        self.store.asked_files
    }
}

/// Give a file the modification time `seconds` after the epoch.
fn touch(path: &std::path::Path, seconds: u64) {
    let time = std::time::UNIX_EPOCH + std::time::Duration::from_secs(seconds);
    fs::File::options()
        .write(true)
        .open(path)
        .and_then(|file| file.set_modified(time))
        .unwrap_or_else(|e| panic!("{}: {e}", path.display()));
}

/// An Electro 5 program filed under a Stage 3 set list's extension, so what its name says
/// and what a read of it found cannot be mistaken for each other.
fn misnamed() -> (String, Vec<u8>) {
    let name = format!("Gig.{}", nord_format::formats::ns3::song::FORMAT);
    (name, with_gain(&Fresh::Program.bytes().unwrap(), "12"))
}

/// A file read in one session draws in the next as it did once read, without being read:
/// its kind and family are what it holds, and it matches, or differs from, the slot that
/// reports its checksum.
#[test]
fn a_file_read_once_draws_as_read_next_time_without_being_read() {
    let (root, shelf) = (Temp::new(), Temp::new());
    let (name, bytes) = misnamed();
    fs::write(root.at(&name), &bytes).unwrap();
    let mut first = Session::remembering(&root, &shelf);
    first.read_all();
    let read = first.bench.workspace.get(first.named(&name)).unwrap();
    assert_eq!(Kind::of(read), Kind::Program);
    let crc32 = read.saved.crc32.expect("a program is a container");
    let families = first.bench.workspace.families_present();
    first.close();

    let mut second = Session::remembering(&root, &shelf);
    let id = second.named(&name);
    let entity = second.bench.workspace.get(id).unwrap();
    assert!(entity.unread(), "its bytes are not held");
    assert!(entity.bytes.is_empty() && entity.entity.is_none());
    assert_eq!(
        Kind::of(entity),
        Kind::Program,
        "not the set list its name says"
    );
    assert_eq!(entity.tag(), "ne5p");
    assert_eq!(
        entity.verify.note(),
        None,
        "it does not say it is being read"
    );
    assert_eq!(second.bench.workspace.families_present(), families);
    assert_eq!(entity.saved.crc32, Some(crc32));

    let Bench {
        workspace,
        device,
        queue,
        browser,
        ..
    } = &mut second.bench;
    let row = |workspace: &crate::workspace::Workspace, device: &crate::device::Device| {
        let item = crate::browser::Item::Local(id);
        let row = crate::library::row_of(item, workspace, &device.state, queue, &browser.tags);
        row.expect("a row").where_
    };
    device.pretend_bodies(ObjectClass::Program, 1, &[Some(("Gig", crc32))]);
    device.relink(workspace);
    let slot = (ObjectClass::Program, Location::from_user(1, 1));
    assert_eq!(workspace.get(id).unwrap().link, Some(slot));
    assert_eq!(
        row(workspace, device),
        crate::library::Where::Both(Some(true))
    );
    device.pretend_bodies(ObjectClass::Program, 1, &[Some(("Gig", crc32 ^ 1))]);
    device.relink(workspace);
    assert_eq!(
        row(workspace, device),
        crate::library::Where::Both(Some(false))
    );
    assert_eq!(second.reads(), 0, "nothing was read");
}

/// A row in view draws what was read of its file before, and is not read for it, even
/// where it came into view before the cache said what it remembers. Something that needs
/// it whole reads it.
#[test]
fn a_remembered_row_in_view_is_read_only_when_something_needs_it_whole() {
    let (root, shelf) = (Temp::new(), Temp::new());
    let program = Fresh::Program.bytes().unwrap();
    fs::write(root.at("Grand.ne5p"), &program).unwrap();
    fs::write(root.at("Other.ne5p"), with_gain(&program, "12")).unwrap();
    let mut first = Session::remembering(&root, &shelf);
    let grand = first.named("Grand.ne5p");
    first.read(&[grand]);
    first.close();

    let bench = Bench::new();
    let cache = Cache::at(&bench.ctx, shelf.at(CACHE), &root.0);
    let store = Store::start(Backend::start(&bench.ctx, root.0.clone())).remembering(cache);
    let mut second = Session { store, bench };
    second.opened();
    let (grand, other) = (second.named("Grand.ne5p"), second.named("Other.ne5p"));
    second.bench.workspace.in_view([grand, other]);
    let Bench {
        workspace,
        browser,
        queue,
        log,
        ..
    } = &mut second.bench;
    second.store.ask(workspace, browser, queue, log);
    assert_eq!(
        second.store.asked_files, 0,
        "reads wait for what the cache remembers"
    );
    second.store.recall_kept(workspace, browser, log);
    second.answer_reads();
    assert_eq!(second.reads(), 1, "only the one nothing is remembered of");
    assert!(second.bench.workspace.get(grand).unwrap().unread());

    second.read(&[grand]);
    assert_eq!(second.reads(), 2);
    let entity = second.bench.workspace.get(grand).unwrap();
    assert!(!entity.unread());
    assert_eq!(entity.bytes, program);
}

/// What the cache remembers may arrive before the library has answered its open, and
/// still stands for the files the listing then finds.
#[test]
fn what_is_remembered_stands_when_it_arrives_before_the_listing() {
    let (root, shelf) = (Temp::new(), Temp::new());
    fs::write(root.at("Grand.ne5p"), Fresh::Program.bytes().unwrap()).unwrap();
    let mut first = Session::remembering(&root, &shelf);
    first.read_all();
    first.close();

    let bench = Bench::new();
    let cache = Cache::at(&bench.ctx, shelf.at(CACHE), &root.0);
    let store = Store::start(Backend::start(&bench.ctx, root.0.clone())).remembering(cache);
    let mut second = Session { store, bench };
    let Bench {
        workspace,
        browser,
        log,
        ..
    } = &mut second.bench;
    second.store.recall_kept(workspace, browser, log);
    second.opened();
    let entity = second.bench.workspace.get(second.only()).unwrap();
    assert!(entity.unread() && !entity.reading(), "remembered");
    assert_eq!(second.store.cache().entries().len(), 1, "and still kept");
}

/// What was read of a file stands only while its length and time are the ones it was
/// read at. Once either moves, the file draws by its name until it is read again.
#[test]
fn a_file_whose_length_or_time_moved_is_read_again() {
    let (root, shelf) = (Temp::new(), Temp::new());
    let program = Fresh::Program.bytes().unwrap();
    fs::write(root.at("Touched.ne5p"), &program).unwrap();
    fs::write(root.at("Longer.ne5p"), &program).unwrap();
    let mut first = Session::remembering(&root, &shelf);
    first.read_all();
    first.close();

    fs::write(root.at("Touched.ne5p"), with_gain(&program, "96")).unwrap();
    touch(&root.at("Touched.ne5p"), 1_000_000);
    let mut longer = program.clone();
    longer.push(0);
    fs::write(root.at("Longer.ne5p"), &longer).unwrap();
    let mut second = Session::remembering(&root, &shelf);
    let ids = ["Touched.ne5p", "Longer.ne5p"].map(|name| second.named(name));
    for id in ids {
        let entity = second.bench.workspace.get(id).unwrap();
        assert!(entity.reading(), "{}", entity.name);
        assert_eq!(entity.verify.note(), Some("reading…"));
        assert_eq!(entity.saved.crc32, None);
    }
    assert_eq!(second.reads(), 0);

    second.read(&ids);
    assert_eq!(second.reads(), 2);
    let touched = second.bench.workspace.get(ids[0]).unwrap();
    assert_eq!(touched.bytes, with_gain(&program, "96"));
}

/// A cache file that does not read, or that another version wrote, is taken as empty:
/// nothing is remembered, nothing is said about it beyond the log's detail, and the next
/// session writes it again.
#[test]
fn a_cache_that_does_not_read_or_is_another_version_is_taken_as_empty() {
    let (root, shelf) = (Temp::new(), Temp::new());
    fs::write(root.at("Grand.ne5p"), Fresh::Program.bytes().unwrap()).unwrap();
    let mut first = Session::remembering(&root, &shelf);
    first.read_all();
    first.close();
    let written = fs::read_to_string(shelf.at(CACHE)).unwrap();
    let version = format!("version:{}", super::cache::VERSION);
    assert!(written.contains(&version), "{written}");
    let newer = written.replace(&version, "version:4096");

    for kept in ["not a cache at all".to_string(), newer] {
        fs::write(shelf.at(CACHE), &kept).unwrap();
        let mut second = Session::remembering(&root, &shelf);
        let entity = second.bench.workspace.get(second.only()).unwrap();
        assert!(entity.reading(), "nothing is remembered from {kept:?}");
        let loud = second.bench.log.iter().filter(|entry| {
            entry.level != crate::log::Level::Info && entry.text.contains("read before")
        });
        assert_eq!(loud.count(), 0);
        second.read_all();
        second.close();
        let third = Session::remembering(&root, &shelf);
        let entity = third.bench.workspace.get(third.only()).unwrap();
        assert!(!entity.reading(), "written again over {kept:?}");
        third.close();
    }
}

/// A file renamed or moved with its folder inside drawbar is remembered at its new path.
#[test]
fn a_rename_inside_drawbar_keeps_what_was_read() {
    let (root, shelf) = (Temp::new(), Temp::new());
    fs::create_dir(root.at("Sets")).unwrap();
    let program = Fresh::Program.bytes().unwrap();
    fs::write(root.at("Sets/Grand.ne5p"), &program).unwrap();
    fs::write(root.at("Sets/Pad.ne5p"), with_gain(&program, "12")).unwrap();
    let mut first = Session::remembering(&root, &shelf);
    first.read_all();
    let grand = first.named("Grand.ne5p");
    let name = "Upright".to_string();
    first
        .bench
        .act(vec![crate::browser::Act::RenameLocal { id: grand, name }]);
    first.sync();
    let sets = first
        .bench
        .browser
        .folders
        .id_of(&LibPath::parse("Sets").unwrap())
        .unwrap();
    first.rename_folder(sets, "Gigs");
    first.sync();
    first.close();
    assert_eq!(root.names("Gigs"), ["Pad.ne5p", "Upright.ne5p"]);

    let second = Session::remembering(&root, &shelf);
    for name in ["Upright.ne5p", "Pad.ne5p"] {
        let entity = second.bench.workspace.get(second.named(name)).unwrap();
        assert!(entity.unread() && !entity.reading(), "{name} is remembered");
    }
    assert_eq!(second.reads(), 0);
}

/// A file renamed outside drawbar, and recognized by its contents as the asset it was,
/// is remembered at its new path.
#[test]
fn a_rename_outside_recognized_by_contents_keeps_what_was_read() {
    let (root, shelf) = (Temp::new(), Temp::new());
    fs::write(root.at("Grand.ne5p"), Fresh::Program.bytes().unwrap()).unwrap();
    let mut first = Session::remembering(&root, &shelf);
    let id = first.only();
    let tag = first.bench.browser.tags.make("Sunday").unwrap();
    first.bench.browser.tags.set(id, tag, true);
    first.read_all();
    first.close();
    fs::rename(root.at("Grand.ne5p"), root.at("Upright.ne5p")).unwrap();

    let second = Session::remembering(&root, &shelf);
    let entity = second.bench.workspace.get(id).expect("the same asset");
    assert_eq!(entity.name, "Upright.ne5p");
    assert!(
        entity.unread() && !entity.reading(),
        "remembered at its new path"
    );
    assert_eq!(second.reads(), 0);
}

/// Two libraries keep their entries apart in one cache file, even for files of one path,
/// length and time.
#[test]
fn two_libraries_share_no_entries() {
    let (a, b, shelf) = (Temp::new(), Temp::new(), Temp::new());
    let program = Fresh::Program.bytes().unwrap();
    fs::write(a.at("Grand.ne5p"), with_gain(&program, "12")).unwrap();
    fs::write(b.at("Grand.ne5p"), with_gain(&program, "24")).unwrap();
    for root in [&a, &b] {
        touch(&root.at("Grand.ne5p"), 1_000_000);
    }
    let mut first = Session::remembering(&a, &shelf);
    first.read_all();
    let crc32 = first.bench.workspace.get(first.only()).unwrap().saved.crc32;
    first.close();

    let mut other = Session::remembering(&b, &shelf);
    let entity = other.bench.workspace.get(other.only()).unwrap();
    assert!(
        entity.reading(),
        "the other library's file is not this one's"
    );
    other.read_all();
    other.close();

    let again = Session::remembering(&a, &shelf);
    let entity = again.bench.workspace.get(again.only()).unwrap();
    assert!(!entity.reading(), "the first library kept its own");
    assert_eq!(entity.saved.crc32, crc32);
}

/// Which projects name a WAV is answered from what was read of them, this session or
/// before, without reading them again. A project never read is said to be unknown.
#[test]
fn the_projects_naming_a_wav_are_answered_from_what_was_read() {
    use nord_format::formats::nsmpproj::{NewZone, Project};

    let (root, shelf) = (Temp::new(), Temp::new());
    fs::create_dir_all(root.at("Marimba/audio")).unwrap();
    let zone = NewZone {
        path: "audio/c4.wav".into(),
        sample_rate: 44_100,
        frames: 44_100,
        root_key: 60,
    };
    let project = Project::new("Marimba", &[zone], 0).unwrap();
    let bytes = nord_format::to_bytes(&nord_format::Entity::SampleProject(project)).unwrap();
    fs::write(root.at("Marimba/Marimba.nsmpproj"), bytes).unwrap();
    fs::write(root.at("Marimba/audio/c4.wav"), crate::testing::wav_bytes()).unwrap();
    let wav = LibPath::parse("Marimba/audio/c4.wav").unwrap();

    let mut first = Session::remembering(&root, &shelf);
    let id = first.named("Marimba.nsmpproj");
    let naming = first.bench.workspace.projects_naming(&wav);
    assert_eq!(
        (naming.by, naming.unknown),
        (vec![], vec![id]),
        "not read yet"
    );
    first.read_all();
    let naming = first.bench.workspace.projects_naming(&wav);
    assert_eq!(naming.by, [id], "read this session");
    first.close();

    let second = Session::remembering(&root, &shelf);
    let id = second.named("Marimba.nsmpproj");
    let workspace = &second.bench.workspace;
    let naming = workspace.projects_naming(&wav);
    assert_eq!((naming.by, naming.unknown), (vec![id], vec![]));
    let elsewhere = LibPath::parse("audio/c4.wav").unwrap();
    assert!(workspace.projects_naming(&elsewhere).by.is_empty());
    assert_eq!(second.reads(), 0);
}

/// Nothing of the cache is written into the library: a library opened, read and closed
/// holds only its own files, and a cache whose place would be inside the library is kept
/// in memory only.
#[test]
fn the_cache_is_never_written_inside_the_library() {
    let (root, shelf) = (Temp::new(), Temp::new());
    fs::write(root.at("Grand.ne5p"), Fresh::Program.bytes().unwrap()).unwrap();
    let mut first = Session::remembering(&root, &shelf);
    first.read_all();
    first.close();
    assert_eq!(root.names(""), ["Grand.ne5p"]);
    assert!(shelf.at(CACHE).is_file());

    let inside = root.at("Support/drawbar").join(CACHE);
    let mut second = Session::kept_at(&root, inside.clone());
    second.read_all();
    second.close();
    assert!(
        !root.at("Support").exists(),
        "nothing was made in the library"
    );
    let third = Session::kept_at(&root, inside);
    let entity = third.bench.workspace.get(third.only()).unwrap();
    assert!(entity.reading(), "it was kept for that session only");
}

/// A tracked file whose summary is remembered is not read in the background: what it is
/// and the slot it matches are known without it.
#[test]
fn a_remembered_tracked_file_is_not_read_in_the_background() {
    let (root, shelf) = (Temp::new(), Temp::new());
    fs::write(root.at("Tagged.ne5p"), Fresh::Program.bytes().unwrap()).unwrap();
    let mut first = Session::remembering(&root, &shelf);
    let tagged = first.only();
    let tag = first.bench.browser.tags.make("Sunday").unwrap();
    first.bench.browser.tags.set(tagged, tag, true);
    first.read_all();
    first.close();

    let second = Session::remembering(&root, &shelf);
    let entity = second.bench.workspace.get(tagged).unwrap();
    assert!(second.bench.browser.tags.worn(tagged).contains(&tag));
    assert!(entity.unread() && !entity.reading());
    assert!(entity.saved.crc32.is_some(), "it can match its slot");
    assert_eq!(second.reads(), 0, "not read in the background");
}

/// The cache keeps only what a whole listing of the library finds: a file deleted
/// between sessions is forgotten.
#[test]
fn a_file_gone_from_the_library_is_forgotten() {
    let (root, shelf) = (Temp::new(), Temp::new());
    let program = Fresh::Program.bytes().unwrap();
    fs::write(root.at("Kept.ne5p"), &program).unwrap();
    fs::write(root.at("Gone.ne5p"), with_gain(&program, "12")).unwrap();
    let mut first = Session::remembering(&root, &shelf);
    first.read_all();
    first.close();
    fs::remove_file(root.at("Gone.ne5p")).unwrap();

    let second = Session::remembering(&root, &shelf);
    let paths: Vec<&str> = second
        .store
        .cache()
        .entries()
        .keys()
        .map(LibPath::as_str)
        .collect();
    assert_eq!(paths, ["Kept.ne5p"]);
}
