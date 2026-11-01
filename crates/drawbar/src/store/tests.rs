//! A library on disk, opened, edited, changed behind the app's back and opened again.
//!
//! Every test works in its own directory under the system's temp folder, never in the
//! default library.

use std::fs;

use super::exec::MOST_BYTES;
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

    /// [`Session::open`], with this install's own state for the library at `local`.
    fn open_at(root: &Temp, local: &std::path::Path) -> Session {
        let bench = Bench::new();
        let backend = Backend::start(&bench.ctx, root.0.clone(), local.to_path_buf());
        let mut session = Session {
            store: Store::start(backend),
            bench,
        };
        session.opened();
        session.read_all();
        session
    }

    /// [`Session::listed`], holding at most `budget` bytes whole.
    fn within(root: &Temp, budget: u64) -> Session {
        let bench = Bench::new();
        let mut store = Store::start(crate::testing::backend(&bench.ctx, root));
        store.budget(budget);
        let mut session = Session { store, bench };
        session.opened();
        session
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
        self.look_for_rescues();
    }

    /// Take in the rescues left, as a frame does while the instrument is idle.
    fn look_for_rescues(&mut self) {
        let Bench {
            workspace,
            browser,
            log,
            ..
        } = &mut self.bench;
        self.store.look_for_rescues(workspace, browser, log);
    }

    /// Fold in what the library answered, as a frame does.
    fn poll(&mut self) {
        let Bench {
            workspace,
            browser,
            queue,
            log,
            ..
        } = &mut self.bench;
        self.store.poll(workspace, browser, queue, log);
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

    /// Rename a folder, and send the change without waiting for it.
    fn rename_folder(&mut self, id: u64, name: &str) {
        let name = name.into();
        self.bench
            .act(vec![crate::browser::Act::RenameFolder { id, name }]);
        let Bench {
            workspace, browser, ..
        } = &mut self.bench;
        self.store.sync(workspace, browser, Pass::Files);
    }

    /// Write everything, and wait until the disk has it.
    fn sync(&mut self) {
        let Bench {
            workspace, browser, ..
        } = &mut self.bench;
        assert!(self.store.sync(workspace, browser, Pass::Last));
        self.settle();
    }

    /// Whether the library may be let go without waiting, as a browser asks each frame.
    fn settled(&mut self) -> bool {
        let Bench {
            workspace, browser, ..
        } = &mut self.bench;
        self.store.settled(workspace, browser)
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

    /// Put the tag `name` on `ids` as the browser does, making it where no tag has that
    /// name, and wait until the library has committed it.
    fn tag(&mut self, ids: &[u64], name: &str) -> u64 {
        let tags = &mut self.bench.browser.tags;
        let known = tags
            .all()
            .iter()
            .find(|tag| tag.name == name)
            .map(|tag| tag.id);
        let tag = known.unwrap_or_else(|| tags.make(name).expect("a new tag"));
        let ids = ids.to_vec();
        self.bench.act(vec![crate::browser::Act::Tag { ids, tag }]);
        self.decided();
        tag
    }

    /// Send the files the workspace needs written and the changes to the facts asked for,
    /// and wait until the library has committed each.
    fn decided(&mut self) {
        loop {
            let Bench {
                workspace,
                browser,
                queue,
                log,
                ..
            } = &mut self.bench;
            self.store.sync(workspace, browser, Pass::Files);
            self.store.poll(workspace, browser, queue, log);
            if !self.store.deciding() {
                return;
            }
            assert!(self.next(), "the library answered");
        }
    }

    /// The names of the tags `id` wears, sorted.
    fn worn(&self, id: u64) -> Vec<String> {
        let tags = &self.bench.browser.tags;
        let worn = tags.worn(id).iter().filter_map(|tag| tags.name_of(*tag));
        let mut worn: Vec<String> = worn.map(str::to_string).collect();
        worn.sort();
        worn
    }

    /// Undo this session's last change to the facts, and wait until it is committed.
    fn undo(&mut self) {
        self.store.undo();
        self.decided();
    }

    /// Redo the change to the facts undone last, and wait until it is committed.
    fn redo(&mut self) {
        self.store.redo();
        self.decided();
    }

    /// Read what other drawbars wrote to the library since.
    fn refresh(&mut self) {
        self.store.refresh();
        while self.store.scanning() {
            assert!(self.next(), "the refresh answered");
        }
    }

    /// Rename the tag `from` to `to` as the browser does, and wait until it is committed.
    fn rename_tag(&mut self, from: &str, to: &str) {
        let tags = &self.bench.browser.tags;
        let id = tags
            .all()
            .iter()
            .find(|tag| tag.name == from)
            .expect(from)
            .id;
        let name = to.to_string();
        self.bench
            .act(vec![crate::browser::Act::RenameTag { id, name }]);
        self.decided();
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
fn a_new_asset_is_a_file_that_comes_back_with_its_tags() {
    let root = Temp::new();
    let mut first = Session::open(&root);
    let id = first.create();
    first.tag(&[id], "Sunday");
    let bytes = first.bytes(id);
    first.close();
    assert_eq!(root.read("untitled.ne5p"), bytes, "the file is the asset");

    let second = Session::open(&root);
    let id = second.named("untitled.ne5p");
    let entity = second.bench.workspace.get(id).expect("the asset");
    assert_eq!(entity.bytes, bytes);
    assert!(!entity.is_unsaved());
    let tags = &second.bench.browser.tags;
    let worn: Vec<&str> = tags
        .worn(id)
        .iter()
        .filter_map(|tag| tags.name_of(*tag))
        .collect();
    assert_eq!(worn, ["Sunday"]);
}

/// Reading loses nothing: a library listed, rescanned or read lazily, with no edit, never
/// has a tab ask to stay open, and an edit then does.
#[test]
fn a_library_only_read_is_never_losing() {
    let root = Temp::new();
    programs(&root, 300);
    let mut session = Session::listed(&root);
    let losing = |session: &Session| {
        let Bench {
            workspace, browser, ..
        } = &session.bench;
        session.store.losing(workspace, browser)
    };
    assert!(!losing(&session), "once listed");

    session.store.rescan();
    assert!(!losing(&session), "while rescanning");
    while session.store.scanning() {
        assert!(session.next(), "the rescan answered");
    }

    let unread: Vec<u64> = session
        .bench
        .workspace
        .listed()
        .filter(|entity| entity.unread())
        .map(|entity| entity.id)
        .take(50)
        .collect();
    assert!(!unread.is_empty(), "files are left to read");
    for id in &unread {
        session.bench.workspace.hurry(*id);
    }
    let Bench {
        workspace,
        browser,
        queue,
        log,
        ..
    } = &mut session.bench;
    session.store.ask(workspace, browser, queue, log);
    assert!(!losing(&session), "while reading");
    session.read(&unread);

    let id = unread[0];
    let edited = with_gain(&session.bytes(id), "96");
    let Bench { workspace, log, .. } = &mut session.bench;
    workspace.replace_bytes(id, edited, log);
    assert!(losing(&session), "an unsaved edit");
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
        log,
        ..
    } = &mut session.bench;
    assert!(session.store.sync(workspace, browser, Pass::Files));
    let edited = with_gain(&workspace.get(id).unwrap().bytes, "96");
    workspace.replace_bytes(id, edited.clone(), log);
    workspace.mark_saved(id);
    assert!(
        !session.store.sync(workspace, browser, Pass::Files),
        "the save waits"
    );
    session.close();
    assert_eq!(root.read("untitled.ne5p"), edited);
}

/// A browser cannot wait for its library's answers, so it lets a library go only once it
/// is settled, after one last pass whose answers nothing reads. That pass sends no save,
/// so the index it writes holds what each save answered.
#[test]
fn a_settled_library_let_go_without_waiting_keeps_its_save() {
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
        workspace, browser, ..
    } = &mut session.bench;
    assert!(
        session.store.sync(workspace, browser, Pass::Last),
        "nothing waits"
    );
    drop(session);
    assert_eq!(root.read("untitled.ne5p"), saved, "the save landed");

    let again = Session::open(&root);
    assert_eq!(again.bench.browser.asking(), None, "nothing to ask");
}

/// Quitting right after a save waits for it to land, so the next run finds the file as
/// the save left it.
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
    let entity = second
        .bench
        .workspace
        .get(second.only())
        .expect("the asset");
    assert_eq!(*entity.saved.bytes(), saved, "the file as saved");
}

#[test]
fn a_file_renamed_outside_keeps_its_id_and_tags() {
    let root = Temp::new();
    let mut session = Session::open(&root);
    let id = session.create();
    session.tag(&[id], "Sunday");

    fs::rename(root.at("untitled.ne5p"), root.at("Grand.ne5p")).unwrap();
    session.refocus();
    assert_eq!(session.path(id).as_deref(), Some("Grand.ne5p"));
    assert_eq!(session.bench.workspace.get(id).unwrap().name, "Grand.ne5p");
    assert_eq!(session.worn(id), ["Sunday"]);
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
    let id = again.only();
    assert_eq!(again.path(id).as_deref(), Some("Pianos/Grand.ne5p"));
    assert_eq!(again.worn(id), ["Sunday"]);
}

/// A browser can hear the user leave and come back between two frames, as when a frame
/// is not drawn while the tab is hidden. The library still rescans on the return.
#[test]
fn coming_back_to_a_picked_folder_rescans_it() {
    let root = Temp::new();
    let mut session = Session::open(&root);
    let mut presence = crate::presence::Presence::default();
    session.store.focus(presence.read());

    presence.heard(false);
    fs::write(root.at("Grand.ne5p"), Fresh::Program.bytes().unwrap()).unwrap();
    presence.heard(true);
    session.store.focus(presence.read());
    assert!(
        !session.store.scanning(),
        "away for the frame that read the return"
    );
    session.store.focus(presence.read());
    assert!(session.store.scanning(), "back at the next frame");
    while session.store.scanning() {
        assert!(session.next(), "the rescan answered");
    }
    let names: Vec<&str> = session
        .bench
        .workspace
        .listed()
        .map(|entity| entity.name.as_str())
        .collect();
    assert_eq!(names, ["Grand.ne5p"]);
}

/// A file with a tag that nothing has read keeps its tag through a rename outside
/// drawbar, while drawbar runs and while it does not.
#[test]
fn a_tagged_file_never_read_keeps_its_tag_through_a_rename_outside() {
    let root = Temp::new();
    fs::write(root.at("Grand.ne5p"), Fresh::Program.bytes().unwrap()).unwrap();
    let mut first = Session::listed(&root);
    let id = first.only();
    first.tag(&[id], "Sunday");
    first.close();

    let mut second = Session::listed(&root);
    let id = second.only();
    fs::rename(root.at("Grand.ne5p"), root.at("Piano.ne5p")).unwrap();
    second.refocus();
    assert_eq!(second.path(id).as_deref(), Some("Piano.ne5p"));
    assert_eq!(second.worn(id), ["Sunday"]);
    second.close();

    fs::rename(root.at("Piano.ne5p"), root.at("Grand.ne5p")).unwrap();
    let third = Session::listed(&root);
    let id = third.only();
    assert_eq!(third.path(id).as_deref(), Some("Grand.ne5p"));
    assert_eq!(third.worn(id), ["Sunday"]);
}

/// A file written again elsewhere, its time new and its length the same, is the asset
/// moved when its contents are what the asset's file held.
#[test]
fn a_file_copied_to_a_new_path_and_deleted_is_matched_by_its_contents() {
    let root = Temp::new();
    let mut session = Session::open(&root);
    let id = session.create();
    session.tag(&[id], "Sunday");

    let bytes = root.read("untitled.ne5p");
    fs::remove_file(root.at("untitled.ne5p")).unwrap();
    fs::write(root.at("Grand.ne5p"), &bytes).unwrap();
    session.refocus();
    assert_eq!(session.path(id).as_deref(), Some("Grand.ne5p"));
    assert_eq!(session.worn(id), ["Sunday"]);
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
    assert_eq!(
        *entity.saved.bytes(),
        theirs,
        "a save would write over theirs"
    );

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
fn keep_both_writes_only_the_new_file() {
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
    let then = std::time::UNIX_EPOCH + std::time::Duration::from_secs(1_000_000_000);
    fs::File::options()
        .write(true)
        .open(root.at("untitled.ne5p"))
        .and_then(|file| file.set_modified(then))
        .unwrap();
    let modified = || {
        fs::metadata(root.at("untitled.ne5p"))
            .unwrap()
            .modified()
            .unwrap()
    };

    session.refocus();
    let acts = session.bench.browser.answer("Keep both");
    session.bench.act(acts);
    session.sync();
    assert_eq!(root.read("untitled 2.ne5p"), mine);
    assert_eq!(root.read("untitled.ne5p"), theirs);
    assert_eq!(modified(), then, "theirs was written again");
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

/// `count` programs in three folders of `root`.
fn programs(root: &Temp, count: usize) {
    let program = Fresh::Program.bytes().unwrap();
    let dirs = ["Live", "Studio", "Studio/Old"];
    for dir in dirs {
        fs::create_dir(root.at(dir)).unwrap();
    }
    for n in 0..count {
        let path = format!("{}/{n:05}.ne5p", dirs[n % dirs.len()]);
        fs::write(root.at(&path), &program).unwrap();
    }
}

/// A whole file a write to the instrument could have left: a small piano's container.
fn a_rescue() -> Vec<u8> {
    let at = nord_usb::Location::from_user(1, 4);
    nord_usb::envelope::wrap("npno", at, 0, b"a piano").unwrap()
}

/// Where an interrupted write to the instrument leaves a slot's former occupant while the
/// library in `root` is open: this install's own folder for it.
fn rescued(root: &Temp) -> std::path::PathBuf {
    root.local().join("rescued")
}

/// The names in the folder at `dir`, sorted.
fn names_at(dir: &std::path::Path) -> Vec<String> {
    let mut names: Vec<String> = fs::read_dir(dir)
        .unwrap_or_else(|e| panic!("{}: {e}", dir.display()))
        .map(|entry| entry.unwrap().file_name().into_string().unwrap())
        .collect();
    names.sort();
    names
}

/// A library drawbar has written, with a slot's former occupant an interrupted write
/// left in this install's folder for it.
fn with_rescue(root: &Temp) {
    let mut first = Session::open(root);
    first.create();
    first.close();
    fs::create_dir_all(rescued(root)).unwrap();
    fs::write(rescued(root).join("nord-rescued-1-4.npno"), a_rescue()).unwrap();
}

impl Session {
    /// Answer the question showing as a click on `label` would, and run the rescue acts
    /// it gives as the app does.
    fn choose(&mut self, label: &str) {
        for act in self.bench.browser.answer(label) {
            let crate::browser::Act::Rescue(rescue, what) = act else {
                panic!("{act:?} is not a rescue's act")
            };
            let Bench { browser, log, .. } = &mut self.bench;
            self.store.rescue(rescue, what, browser, log);
        }
        self.sync();
    }
}

/// A rescue still being written when drawbar stopped is never the slot's only copy, since
/// the slot is not touched until it is whole. It is deleted, in this install's folder and
/// in drawbar's own data alike, and nothing of it reaches the library.
#[test]
fn a_rescue_cut_short_by_a_crash_is_deleted_and_never_reaches_the_library() {
    let (root, shelf) = (Temp::new(), Temp::new());
    let mut first = Session::open(&root);
    first.create();
    first.close();
    let partial = "nord-rescued-1-4.npno.partial";
    fs::create_dir_all(rescued(&root)).unwrap();
    fs::write(rescued(&root).join(partial), a_rescue()).unwrap();
    fs::write(shelf.at(partial), a_rescue()).unwrap();
    let bench = Bench::new();
    let store = Store::start(crate::testing::backend(&bench.ctx, &root));
    let mut session = Session {
        store: store.shelving(Some(shelf.0.clone())),
        bench,
    };
    session.opened();

    session.sync();
    assert_eq!(session.bench.browser.asking(), None, "nothing is offered");
    assert_eq!(names_at(&rescued(&root)), Vec::<String>::new());
    assert_eq!(shelf.names(""), Vec::<String>::new());
    let rescued = |name: &String| name.starts_with("nord-rescued");
    assert!(!root.names("").iter().any(rescued), "{:?}", root.names(""));
}

/// A rescue file that does not read, such as one an older drawbar left half written, is
/// no copy of the slot. It is said so, and offered only to be discarded.
#[test]
fn a_rescue_file_that_does_not_decode_is_reported_not_offered() {
    let root = Temp::new();
    let mut first = Session::open(&root);
    first.create();
    first.close();
    let mut half = a_rescue();
    half[..8].fill(0);
    fs::create_dir_all(rescued(&root)).unwrap();
    fs::write(rescued(&root).join("nord-rescued-1-4.npno"), half).unwrap();

    let mut second = Session::open(&root);
    let (title, answers) = second.bench.browser.asking().expect("a question is asked");
    assert!(title.contains("nord-rescued-1-4.npno"), "{title}");
    assert!(title.contains("does not read"), "{title}");
    assert_eq!(answers, ["Later", "Show the file", "Discard"]);
    second.choose("Discard");
    assert_eq!(names_at(&rescued(&root)), Vec::<String>::new());
}

/// A rescue a write left goes into the library's top level once the library opens, as an
/// asset like any other, an entity that says which slot it came off, and is deleted where
/// it was. The copy is one change, which undo takes back and redo makes again, and the
/// next open finds nothing more to take in.
#[test]
fn a_rescue_left_goes_into_the_library_once_with_its_slot_as_origin() {
    let root = Temp::new();
    with_rescue(&root);

    let mut second = Session::open(&root);
    second.sync();
    assert_eq!(second.bench.browser.asking(), None, "nothing is asked");
    assert_eq!(names_at(&rescued(&root)), Vec::<String>::new());
    assert_eq!(root.read("nord-rescued-1-4.npno"), a_rescue());

    second.undo();
    assert!(!root.at("nord-rescued-1-4.npno").exists(), "undone");
    second.redo();
    assert_eq!(root.read("nord-rescued-1-4.npno"), a_rescue(), "redone");
    second.close();

    let third = Session::open(&root);
    let rescues: Vec<String> = root
        .names("")
        .into_iter()
        .filter(|name| name.starts_with("nord-rescued"))
        .collect();
    assert_eq!(rescues, ["nord-rescued-1-4.npno"], "taken in once");
    let kept = third.named("nord-rescued-1-4.npno");
    let origin = &third.bench.workspace.get(kept).unwrap().origin;
    assert!(
        matches!(origin, Origin::Rescued { at } if *at == Location::from_user(1, 4)),
        "{}",
        origin.label()
    );
    assert!(
        matches!(third.store.key(kept), Some(Key::Entity(_))),
        "an entity says where it came from"
    );
}

/// A rescue a write leaves while the library is open goes into it once the instrument is
/// idle, once however often drawbar looks again before its copy lands.
#[test]
fn a_rescue_a_write_leaves_goes_into_the_open_library_once() {
    let root = Temp::new();
    let mut session = Session::open(&root);
    fs::create_dir_all(rescued(&root)).unwrap();
    fs::write(rescued(&root).join("nord-rescued-2-3.npno"), a_rescue()).unwrap();
    for _ in 0..2 {
        session.store.rescue_due();
        session.look_for_rescues();
    }
    session.sync();
    session.store.rescue_due();
    session.look_for_rescues();
    session.sync();

    assert_eq!(names_at(&rescued(&root)), Vec::<String>::new());
    let rescues: Vec<String> = root
        .names("")
        .into_iter()
        .filter(|name| name.starts_with("nord-rescued"))
        .collect();
    assert_eq!(rescues, ["nord-rescued-2-3.npno"]);
    let kept = session.named("nord-rescued-2-3.npno");
    let origin = &session.bench.workspace.get(kept).unwrap().origin;
    assert!(
        matches!(origin, Origin::Rescued { at } if *at == Location::from_user(2, 3)),
        "{}",
        origin.label()
    );
}

/// A library that cannot be written leaves a rescue where it is and asks about it: it may
/// be shown or discarded, or left for the next open of a library that can take it.
#[test]
fn a_rescue_a_read_only_library_cannot_take_stays_and_is_offered() {
    let (root, later) = newer_drawbar_wrote();
    fs::create_dir_all(rescued(&root)).unwrap();
    fs::write(rescued(&root).join("nord-rescued-1-4.npno"), a_rescue()).unwrap();
    let mut second = Session::open(&root);
    assert!(second.store.read_only().is_some(), "opened read-only");
    let (title, answers) = second.bench.browser.asking().expect("a question is asked");
    assert_eq!(
        title,
        "A write to the instrument left “nord-rescued-1-4.npno”"
    );
    assert_eq!(answers, ["Later", "Show the file", "Discard"]);
    second.choose("Discard");
    let (title, _) = second.bench.browser.asking().expect("asked again");
    assert_eq!(title, "Discard “nord-rescued-1-4.npno”?");
    second.choose("Later");
    assert_eq!(names_at(&rescued(&root)), ["nord-rescued-1-4.npno"]);
    second.close();
    fs::remove_file(later).unwrap();

    let mut third = Session::open(&root);
    third.sync();
    assert_eq!(names_at(&rescued(&root)), Vec::<String>::new());
    assert_eq!(root.read("nord-rescued-1-4.npno"), a_rescue());
}

/// A rescue's name says the slot it came off.
#[test]
fn a_rescue_names_the_slot_it_came_off() {
    let slot = |name: &str| Rescue::slot_of(name);
    assert_eq!(
        slot("nord-rescued-3-12.ne5p"),
        Some(Location::from_user(3, 12))
    );
    assert_eq!(
        slot("nord-rescued-1-4 2.npno"),
        Some(Location::from_user(1, 4))
    );
    assert_eq!(slot("nord-rescued-0-4.npno"), None);
    assert_eq!(slot("Grand.npno"), None);
}

/// A copy fetched off a slot keeps saying which slot once drawbar opens the library
/// again.
#[test]
fn a_copy_off_a_slot_still_says_which_once_reopened() {
    let (root, fetched) = (Temp::new(), Temp::new());
    let program = Fresh::Program.bytes().unwrap();
    fs::write(fetched.at("Grand.ne5p"), &program).unwrap();
    let mut first = Session::open(&root);
    let at = Location::from_user(2, 7);
    let origin = Origin::Device {
        class: ObjectClass::Program,
        at,
    };
    let len = program.len() as u64;
    let to = LibPath::root().join("Grand.ne5p");
    first
        .bench
        .workspace
        .arrive_fetched(to, origin, fetched.at("Grand.ne5p"), len);
    first.sync();
    first.close();

    let second = Session::open(&root);
    let origin = &second.bench.workspace.get(second.only()).unwrap().origin;
    assert!(
        matches!(origin, Origin::Device { class: ObjectClass::Program, at: held } if *held == at),
        "{}",
        origin.label()
    );
}

/// Every rescue left, in this install's folder for the library and in drawbar's own data
/// where no library could take one, goes into the library that opens.
#[test]
fn every_rescue_left_goes_into_the_library_that_opens() {
    let (root, shelf) = (Temp::new(), Temp::new());
    fs::create_dir_all(rescued(&root)).unwrap();
    fs::write(rescued(&root).join("nord-rescued-1-4.npno"), a_rescue()).unwrap();
    fs::write(rescued(&root).join("nord-rescued-1-4 2.npno"), a_rescue()).unwrap();
    fs::write(shelf.at("nord-rescued-2-1.nsmp"), a_rescue()).unwrap();
    let bench = Bench::new();
    let store = Store::start(crate::testing::backend(&bench.ctx, &root));
    let mut session = Session {
        store: store.shelving(Some(shelf.0.clone())),
        bench,
    };
    session.opened();
    session.sync();

    assert_eq!(session.bench.browser.asking(), None, "nothing is asked");
    assert_eq!(names_at(&rescued(&root)), Vec::<String>::new());
    assert_eq!(shelf.names(""), Vec::<String>::new());
    for name in [
        "nord-rescued-1-4.npno",
        "nord-rescued-1-4 2.npno",
        "nord-rescued-2-1.nsmp",
    ] {
        assert_eq!(root.read(name), a_rescue(), "{name}");
    }
}

/// Rows in view are read a batch at a time, and an asset opened meanwhile goes in the next
/// read, ahead of the rows still waiting.
#[test]
fn an_asset_opened_while_rows_are_read_is_read_before_the_rest_of_them() {
    let root = Temp::new();
    let program = Fresh::Program.bytes().unwrap();
    for n in 0..80 {
        fs::write(root.at(&format!("Row {n:02}.ne5p")), &program).unwrap();
    }
    let mut session = Session::listed(&root);
    let ids: Vec<u64> = session.bench.workspace.listed().map(|e| e.id).collect();
    let (rows, open) = (&ids[..79], ids[79]);
    session.bench.workspace.in_view(rows.iter().copied());
    let Bench {
        workspace,
        browser,
        queue,
        log,
        ..
    } = &mut session.bench;
    session.store.ask(workspace, browser, queue, log);
    session.store.ask(workspace, browser, queue, log);
    assert_eq!(session.store.asked_files, 32, "one read in flight");
    session.bench.workspace.hurry(open);

    assert!(session.next(), "the first read answered");
    assert!(session.next(), "the next read answered");
    let workspace = &session.bench.workspace;
    assert!(
        !workspace.get(open).unwrap().unread(),
        "the open asset is read"
    );
    let waiting = rows
        .iter()
        .filter(|id| workspace.get(**id).unwrap().unread());
    assert_eq!(
        waiting.count(),
        rows.len() - 32,
        "the rest of the rows wait"
    );
}

/// A save the folder refuses, which nothing may be added to or taken from, leaves the
/// file as it was and the edit unsaved.
#[cfg(unix)]
#[test]
fn a_write_that_fails_leaves_the_file_as_it_was() {
    use std::os::unix::fs::PermissionsExt;

    let root = Temp::new();
    let mut session = Session::open(&root);
    let id = session.create();
    session.sync();
    let file = root.read("untitled.ne5p");
    let mode = |mode| fs::set_permissions(&root.0, fs::Permissions::from_mode(mode)).unwrap();
    mode(0o555);
    let edited = with_gain(&file, "96");
    let log = &mut session.bench.log;
    session.bench.workspace.replace_bytes(id, edited, log);
    session.bench.workspace.mark_saved(id);
    session.sync();
    mode(0o755);
    assert_eq!(root.read("untitled.ne5p"), file, "the file was not touched");
    assert!(
        session.bench.workspace.get(id).unwrap().is_unsaved(),
        "the edit is still unsaved"
    );
    assert_eq!(session.said("was not saved"), 1);
}

/// A save whose file could not be put in place, with another change after it and a
/// restart, leaves the file holding its old bytes or its new ones, never nothing.
#[test]
fn a_save_whose_file_could_not_be_put_in_place_leaves_a_file_through_a_restart() {
    let root = Temp::new();
    let mut session = Session::open(&root);
    let id = session.create();
    session.sync();
    let old = root.read("untitled.ne5p");
    let new = with_gain(&old, "96");
    session.store.fault(Fault::Refuse(
        |io| matches!(io, toshokan::Io::Rename { to, .. } if to.as_str() == "untitled.ne5p"),
    ));
    let log = &mut session.bench.log;
    session.bench.workspace.replace_bytes(id, new.clone(), log);
    session.bench.workspace.mark_saved(id);
    session.sync();
    assert_eq!(session.said("was not saved"), 1, "the save failed");
    assert_eq!(
        session.said("is missing"),
        0,
        "the file waits for the save to finish"
    );
    session.make_tag("Sunday");
    assert_eq!(session.said("Finished saving “untitled.ne5p”."), 1);
    session.settle();
    assert_eq!(session.said("changed on disk"), 0);
    let held = session.bench.workspace.get(id).expect("the asset");
    assert!(!held.is_unsaved(), "the edit is what the file holds");
    session.close();

    let again = Session::open(&root);
    let held = fs::read(root.at("untitled.ne5p")).ok();
    assert!(
        held.as_ref() == Some(&old) || held.as_ref() == Some(&new),
        "the file holds {:?} bytes",
        held.map(|bytes| bytes.len())
    );
    let entity = again.bench.workspace.get(again.only()).expect("the asset");
    assert_eq!(
        Some(entity.saved.bytes().to_vec()),
        held,
        "the asset is its file"
    );
}

/// A save that cannot be finished, as when another program put a file where it goes,
/// leaves the library read-only, saying what to move, until it is opened again.
#[test]
fn a_save_that_cannot_be_finished_leaves_the_library_read_only_until_it_opens_again() {
    let root = Temp::new();
    let mut session = Session::open(&root);
    let id = session.create();
    session.sync();
    let new = with_gain(&root.read("untitled.ne5p"), "96");
    session.store.fault(Fault::Refuse(
        |io| matches!(io, toshokan::Io::Rename { to, .. } if to.as_str() == "untitled.ne5p"),
    ));
    let log = &mut session.bench.log;
    session.bench.workspace.replace_bytes(id, new.clone(), log);
    session.bench.workspace.mark_saved(id);
    session.sync();
    fs::write(root.at("untitled.ne5p"), b"theirs").unwrap();
    session.make_tag("Sunday");
    let why = session.store.read_only().unwrap_or_default().to_string();
    assert!(
        why.contains("in the way") && why.contains("untitled.ne5p"),
        "read-only: {why:?}"
    );
    assert_eq!(session.said("read-only now"), 1);
    let later = with_gain(&new, "12");
    let log = &mut session.bench.log;
    session
        .bench
        .workspace
        .replace_bytes(id, later.clone(), log);
    session.close();

    fs::rename(root.at("untitled.ne5p"), root.local().join("theirs")).unwrap();
    let again = Session::open(&root);
    assert_eq!(again.store.read_only(), None);
    assert_eq!(root.read("untitled.ne5p"), new, "the save is finished");
    let entity = again.bench.workspace.get(again.only()).expect("the asset");
    assert_eq!(
        *entity.bytes, later,
        "the edit made while read-only was kept"
    );
}

/// A save the next change finishes joins Undo, before that change, and keeps what its
/// undo needs.
#[test]
fn a_save_the_next_change_finishes_can_be_undone_after_it() {
    let root = Temp::new();
    let mut session = Session::open(&root);
    let id = session.create();
    session.sync();
    let old = root.read("untitled.ne5p");
    let new = with_gain(&old, "96");
    session.store.fault(Fault::Refuse(
        |io| matches!(io, toshokan::Io::Rename { to, .. } if to.as_str() == "untitled.ne5p"),
    ));
    let log = &mut session.bench.log;
    session.bench.workspace.replace_bytes(id, new.clone(), log);
    session.bench.workspace.mark_saved(id);
    session.sync();
    session.make_tag("Sunday");
    assert_eq!(root.read("untitled.ne5p"), new, "the save is finished");
    session.undo();
    assert!(session.tag_names().is_empty(), "the tag is undone");
    assert_eq!(
        session.store.undo_label().as_deref(),
        Some("Undo Save “untitled.ne5p”")
    );
    session.undo();
    assert_eq!(root.read("untitled.ne5p"), old, "the save is undone");
}

/// An undo the next change finishes leaves Undo at the step before the one it undid.
#[test]
fn an_undo_the_next_change_finishes_leaves_undo_at_the_step_before() {
    let root = Temp::new();
    let mut session = Session::open(&root);
    let id = session.create();
    session.sync();
    let old = root.read("untitled.ne5p");
    let log = &mut session.bench.log;
    session
        .bench
        .workspace
        .replace_bytes(id, with_gain(&old, "96"), log);
    session.bench.workspace.mark_saved(id);
    session.sync();
    session.store.fault(Fault::Refuse(
        |io| matches!(io, toshokan::Io::Rename { to, .. } if to.as_str() == "untitled.ne5p"),
    ));
    session.undo();
    session.make_tag("Sunday");
    assert_eq!(root.read("untitled.ne5p"), old, "the undo is finished");
    session.undo();
    assert!(session.tag_names().is_empty(), "the tag is undone");
    assert_eq!(
        session.store.undo_label().as_deref(),
        Some("Undo New “untitled.ne5p”")
    );
}

/// A library a change of drawbar's own left read-only still keeps unsaved edits when it
/// opens read-only again.
#[test]
fn a_library_reopened_while_a_change_still_cannot_be_finished_keeps_edits() {
    let root = Temp::new();
    let mut session = Session::open(&root);
    let id = session.create();
    session.sync();
    let old = root.read("untitled.ne5p");
    session.store.fault(Fault::Refuse(
        |io| matches!(io, toshokan::Io::Rename { to, .. } if to.as_str() == "b.ne5p"),
    ));
    let rename = crate::browser::Act::RenameLocal {
        id,
        name: "b.ne5p".into(),
    };
    session.bench.act(vec![rename]);
    session.sync();
    fs::write(root.at("b.ne5p"), with_gain(&old, "96")).unwrap();
    session.make_tag("Sunday");
    assert!(
        session.store.read_only().is_some(),
        "the move cannot finish"
    );
    session.close();

    let mut again = Session::open(&root);
    assert!(
        again.store.read_only().is_some(),
        "the move still cannot finish"
    );
    let id = again.named("untitled.ne5p");
    let edit = with_gain(&old, "12");
    let log = &mut again.bench.log;
    again.bench.workspace.replace_bytes(id, edit.clone(), log);
    again.close();

    fs::remove_file(root.at("b.ne5p")).unwrap();
    let last = Session::open(&root);
    assert_eq!(last.store.read_only(), None);
    let entity = last.bench.workspace.get(last.only()).expect("the asset");
    assert_eq!(
        *entity.bytes, edit,
        "the edit made after opening read-only again was kept"
    );
}

/// A copy of a library whose hidden folder holds, in this install's own log, a change a
/// newer drawbar wrote, and the segment of the log that holds it.
fn newer_drawbar_wrote() -> (Temp, std::path::PathBuf) {
    use toshokan::log::{Entry, EntryKind};

    let running = Temp::new();
    let mut first = Session::open(&running);
    first.make_tag("Sunday");
    // A copy taken while it runs, before closing folds its log into a snapshot.
    let root = Temp::new();
    copy_tree(&running.0, &root.0);
    copy_tree(&running.local(), &root.local());
    let writers = fs::read_dir(root.at(".drawbar/library/writers.2")).unwrap();
    let writer = writers.map(|entry| entry.unwrap().path()).next().unwrap();
    let segment = fs::read_dir(&writer)
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .find(|file| file.extension().is_some_and(|ext| ext == "txt"))
        .expect("a segment of the writer's log");
    let held = fs::read(&segment).unwrap();
    let lines = held.split_inclusive(|byte| *byte == b'\n');
    let head = lines.filter_map(|line| Entry::parse(line).ok()).next_back();
    let head = head.expect("an entry");
    // A later build logs after this one, in a segment of its own.
    let future = toshokan::Raw::new(r#"{"kind":"from a later build"}"#).unwrap();
    let entry = Entry::encode(
        head.hash(),
        head.n() + 1,
        head.at(),
        EntryKind::Unknown(future),
    )
    .unwrap();
    fs::write(writer.join("later.txt"), entry.to_bytes()).unwrap();
    (root, writer.join("later.txt"))
}

/// A library whose hidden folder holds, in this install's own log, a change a newer
/// drawbar wrote opens read-only: this build would write past what it cannot read.
#[test]
fn a_library_a_newer_drawbar_wrote_opens_read_only() {
    let (root, _) = newer_drawbar_wrote();
    let second = Session::open(&root);
    let why = second.store.read_only().unwrap_or_default();
    assert!(why.contains("update drawbar"), "opened read-only: {why:?}");
}

/// A delete that fails the same way twice is logged twice: only a commit's retries are
/// quiet.
#[test]
fn a_delete_that_fails_again_is_logged_again() {
    let root = Temp::new();
    let mut session = Session::open(&root);
    session.create();
    session.sync();
    let failed = "did not change as asked: deleting untitled.ne5p";
    for nth in 1..=2 {
        let id = session.only();
        // A file that changed on disk since drawbar read it is left.
        let mut changed = root.read("untitled.ne5p");
        changed.push(0);
        fs::write(root.at("untitled.ne5p"), changed).unwrap();
        let Bench { workspace, log, .. } = &mut session.bench;
        workspace.remove(id, log);
        session.sync();
        assert_eq!(session.said(failed), nth);
    }
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
    session.tag(&[id], "Sunday");
    session.close();
    assert_eq!(root.names(""), [".drawbar", "Grand.ne5p"]);
    assert_eq!(root.read("Grand.ne5p"), program, "the file is untouched");

    let again = Session::open(&root);
    assert_eq!(again.worn(again.only()), ["Sunday"]);
}

/// A rescan that finds several changes made outside drawbar says each in the log and all
/// of them in the status line; one delete or move is said by its own line. Only a file
/// the library says something about is followed through a move.
#[test]
fn a_rescan_reports_what_changed() {
    let root = Temp::new();
    let program = Fresh::Program.bytes().unwrap();
    for (name, gain) in [("Bass.ne5p", "10"), ("Keys.ne5p", "20"), ("Pad.ne5p", "30")] {
        fs::write(root.at(name), with_gain(&program, gain)).unwrap();
    }
    let mut first = Session::open(&root);
    let keys = first.named("Keys.ne5p");
    first.tag(&[keys], "Sunday");
    first.close();
    let mut session = Session::open(&root);
    fs::write(root.at("Lead.ne5p"), with_gain(&program, "40")).unwrap();
    fs::remove_file(root.at("Bass.ne5p")).unwrap();
    fs::rename(root.at("Keys.ne5p"), root.at("Organ.ne5p")).unwrap();
    session.refocus();

    assert_eq!(session.said("“Bass.ne5p” was deleted outside drawbar."), 1);
    assert_eq!(
        session.said("“Keys.ne5p” was moved to Organ.ne5p outside drawbar"),
        1
    );
    assert_eq!(
        session.bench.log.status().1,
        "Outside drawbar, 1 file appeared, 1 was deleted and 1 was moved."
    );

    fs::remove_file(root.at("Pad.ne5p")).unwrap();
    session.refocus();
    assert_eq!(
        session.bench.log.status().1,
        "“Pad.ne5p” was deleted outside drawbar."
    );
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
        log,
        ..
    } = &mut session.bench;
    assert!(session.store.sync(workspace, browser, Pass::Files));
    workspace.remove(id, log);
    session.store.sync(workspace, browser, Pass::Files);
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
    session.tag(&[tagged], "Sunday");
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
    let lost: Vec<(u64, String)> = again
        .bench
        .browser
        .folders
        .lost()
        .iter()
        .map(|lost| (lost.id, lost.path.to_string()))
        .collect();
    let [(id, path)] = lost.as_slice() else {
        panic!("one asset is lost: {lost:?}");
    };
    assert_eq!(
        path, "untitled.ne5p",
        "the tagged file, kept by the library"
    );
    assert_eq!(again.worn(*id), ["Sunday"]);
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

/// The pair is flagged on every disk, since a library copied to one that ignores case
/// could hold only one of them, and the warning claims nothing about this disk.
#[test]
fn two_names_differing_only_by_case_are_flagged_without_claiming_the_disk_ignores_case() {
    let Bench {
        mut workspace,
        mut browser,
        mut log,
        ..
    } = Bench::new();
    for name in ["Case.ne5p", "case.ne5p"] {
        let id = workspace.create(Fresh::Program, &mut log).unwrap();
        workspace.place(id, LibPath::root().join(name));
    }
    let before = log.iter().count();
    mirror::flag_duplicates(&workspace, &mut browser, &mut log);
    let said: Vec<&str> = log
        .iter()
        .skip(before)
        .map(|entry| entry.text.as_str())
        .collect();
    assert_eq!(
        said,
        [
            "“Case.ne5p” and “case.ne5p” in the library differ only by case and would collide \
          on macOS or Windows; rename one of them."
        ]
    );
    assert_eq!(browser.folders.duplicates.len(), 2);
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
    assert_eq!(session.said("differ only by case"), 1);
    session.refocus();
    assert_eq!(session.said("differ only by case"), 1, "said once");
    session.close();
    assert_eq!(
        root.names(""),
        ["C3.ne5p", "c3.ne5p"],
        "nothing renamed, and nothing added"
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

/// An asset made with New keeps saying so once its file is written and drawbar opens
/// the library again.
#[test]
fn a_new_asset_with_a_file_is_still_new_after_a_restart() {
    let root = Temp::new();
    let mut first = Session::open(&root);
    first.create();
    first.close();

    let second = Session::open(&root);
    let origin = &second.bench.workspace.get(second.only()).unwrap().origin;
    assert!(matches!(origin, Origin::Fresh), "{}", origin.label());
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
    assert!(matches!(entity.verify(), VerifyState::Ok));
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

/// A library of one folder, `Gig`, holding one tagged file nothing has read, and the
/// folder's id.
fn gig() -> (Temp, Session, u64, u64) {
    let root = Temp::new();
    fs::create_dir(root.at("Gig")).unwrap();
    fs::write(root.at("Gig/a.ne5p"), Fresh::Program.bytes().unwrap()).unwrap();
    let mut session = Session::listed(&root);
    let id = session.only();
    let tag = session.tag(&[id], "Sunday");
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
        workspace, browser, ..
    } = &mut session.bench;
    browser.folders.file(workspace, id, Some(folder));
    assert_eq!(
        workspace.get(id).unwrap().path,
        LibPath::parse("Set/f.ne5p")
    );
    assert!(
        !session.store.sync(workspace, browser, Pass::Files),
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
        workspace, browser, ..
    } = &mut session.bench;
    session.store.sync(workspace, browser, Pass::Files);
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
    assert!(matches!(entity.verify(), VerifyState::Ok));
    assert_eq!(entity.bytes, program);
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
    let verify = huge.verify();
    let VerifyState::NotRead(why) = &verify else {
        panic!("{}", verify.detail());
    };
    assert!(why.contains("at most 1 GiB"), "{why}");
    let small = session.bench.workspace.get(session.named("Small.ne5p"));
    assert!(matches!(small.unwrap().verify(), VerifyState::Ok));
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
    let verify = workspace.get(c).unwrap().verify();
    let VerifyState::NotRead(why) = &verify else {
        panic!("{}", verify.detail());
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
    assert_eq!(*entity.saved.bytes(), with_gain(&program, "36"));
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
    assert_eq!(evicted.verify().note(), None, "it still draws as read");
    assert_eq!(Kind::of(evicted), Kind::Program);
    assert!(evicted.saved.crc32().is_some(), "it still matches its slot");
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
    let verify = refused.verify();
    let VerifyState::NotRead(why) = &verify else {
        panic!("{}", verify.detail());
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

/// Once the library is listed, each file the library tracks, here by a tag, is read and
/// decoded in the background, so it can match its slot before anything shows it. A file
/// the library does not track waits until something needs it.
#[test]
fn after_open_a_tracked_file_is_read_without_being_asked() {
    let root = Temp::new();
    let program = Fresh::Program.bytes().unwrap();
    fs::write(root.at("Tagged.ne5p"), with_gain(&program, "12")).unwrap();
    fs::write(root.at("Plain.ne5p"), with_gain(&program, "24")).unwrap();
    let mut first = Session::listed(&root);
    let tagged = first.named("Tagged.ne5p");
    first.tag(&[tagged], "Sunday");
    first.close();

    let mut second = Session::listed(&root);
    let tagged = second.named("Tagged.ne5p");
    second.answer_reads();
    let Bench { workspace, log, .. } = &mut second.bench;
    workspace.settle_files(log);
    let entity = workspace.get(tagged).unwrap();
    assert!(!entity.unread(), "read without being asked");
    assert!(matches!(entity.verify(), VerifyState::Ok));
    assert!(entity.saved.crc32().is_some(), "it can match its slot");
    let plain = second.named("Plain.ne5p");
    assert!(second.bench.workspace.get(plain).unwrap().unread());
}

/// Past the budget, files the library does not track go before those it tracks, whenever
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
    let tracked: Vec<u64> = names[..3].iter().map(|name| first.named(name)).collect();
    first.tag(&tracked, "Sunday");
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
    assert_eq!(
        t3.verify().note(),
        Some("reading…"),
        "not refused, only left"
    );

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
        let shared = entity.bytes.shares(entity.saved.bytes());
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

/// A piano library the size a vendor ships: three strokes, each of the most blocks a
/// stroke record can state, about 200 MB in all.
/// A row drawn for a resting piano reads its index and nothing more: its checksum waits
/// until it is opened, which reads it through.
#[test]
fn a_drawn_row_of_a_resting_piano_is_not_read_through_until_it_is_opened() {
    let root = Temp::new();
    fs::write(root.at("Grand.npno"), large_piano()).unwrap();
    let mut session = Session::listed(&root);
    let id = session.only();
    session.bench.workspace.in_view([id]);
    session.answer_reads();
    session.bench.workspace.settle_files(&mut session.bench.log);
    let entity = session.bench.workspace.get(id).unwrap();
    let file = entity.rests().expect("rests in its file").clone();
    assert_eq!(entity.verify().badge(), "not checked");
    assert_eq!(file.take_reads(), [], "read past its index");
    assert_eq!(file.take_checks(), 0, "read through for its checksum");

    session.bench.workspace.hurry(id);
    session.bench.workspace.settle_files(&mut session.bench.log);
    let entity = session.bench.workspace.get(id).unwrap();
    assert_eq!(entity.verify().badge(), "ok");
    assert_eq!(file.take_checks(), 1, "opening reads it through");
}

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
    assert!(entity.bytes.is_empty() && entity.saved.bytes().is_empty());
    assert_eq!(entity.size(), len);
    assert_eq!(entity.verify().note(), Some("checking…"));
    let file = entity.rests().unwrap().clone();

    let words = session.document(id);
    assert!(
        words.iter().any(|word| word == "npno 6.x"),
        "the header reads the index: {words:?}"
    );
    assert_eq!(file.take_reads(), [], "the frame read none of its body");

    let Bench { workspace, log, .. } = &mut session.bench;
    workspace.settle_files(log);
    let entity = workspace.get(id).unwrap();
    assert!(matches!(entity.verify(), VerifyState::Checked));
    assert_eq!(entity.verify().note(), None);
    assert!(
        entity.saved.crc32().is_some(),
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
    assert_eq!(entity.verify().note(), Some("checking…"));
    assert_eq!(Kind::of(entity), Kind::Piano);

    let Bench { workspace, log, .. } = &mut session.bench;
    workspace.settle_files(log);
    let entity = workspace.get(id).unwrap();
    assert_eq!(entity.verify().note(), Some("failed verification"));
    assert!(entity.sendable().is_err());
    assert_eq!(Kind::of(entity), Kind::Other);
}

impl Session {
    /// How many files have been asked of the library to read, in the background or for
    /// something that needs them.
    fn reads(&self) -> usize {
        self.store.asked_files
    }
}

/// Which projects name a WAV is answered from what was read of them this session,
/// without reading them again. A project not read yet is said to be unknown.
#[test]
fn the_projects_naming_a_wav_are_answered_from_what_was_read() {
    use nord_format::formats::nsmpproj::{NewZone, Project};

    let root = Temp::new();
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

    let mut session = Session::listed(&root);
    let id = session.named("Marimba.nsmpproj");
    let naming = session.bench.workspace.projects_naming(&wav);
    assert_eq!(
        (naming.by, naming.unknown),
        (vec![], vec![id]),
        "not read yet"
    );
    session.read_all();
    let naming = session.bench.workspace.projects_naming(&wav);
    assert_eq!(naming.by, [id], "read this session");
    let elsewhere = LibPath::parse("audio/c4.wav").unwrap();
    assert!(session
        .bench
        .workspace
        .projects_naming(&elsewhere)
        .by
        .is_empty());
}

/// A reopen knows nothing of what the last session read: every file is unread, a row is
/// read only once it is drawn, and closing kept no summary anywhere, in the library or in
/// this install's own state for it.
#[test]
fn a_reopen_reads_summaries_only_for_drawn_rows_and_nothing_was_kept() {
    let root = Temp::new();
    let program = Fresh::Program.bytes().unwrap();
    fs::write(root.at("Grand.ne5p"), &program).unwrap();
    fs::write(root.at("Upright.ne5p"), with_gain(&program, "12")).unwrap();
    let local = |root: &Temp| -> Vec<String> {
        let mut found = Vec::new();
        let mut dirs = vec![root.local()];
        while let Some(dir) = dirs.pop() {
            for entry in fs::read_dir(&dir).into_iter().flatten().flatten() {
                match entry.file_type().is_ok_and(|kind| kind.is_dir()) {
                    true => dirs.push(entry.path()),
                    false => found.push(entry.file_name().to_string_lossy().into_owned()),
                }
            }
        }
        found.sort();
        found
    };
    let first = Session::open(&root);
    let before = local(&root);
    first.close();
    assert_eq!(root.names(""), ["Grand.ne5p", "Upright.ne5p"]);
    let after = local(&root);
    assert!(
        after
            .iter()
            .all(|name| before.contains(name) || !name.contains("cache")),
        "{after:?}"
    );

    let mut second = Session::listed(&root);
    let rows: Vec<u64> = second
        .bench
        .workspace
        .listed()
        .map(|entity| entity.id)
        .collect();
    for id in &rows {
        let entity = second.bench.workspace.get(*id).unwrap();
        assert!(
            entity.unread() && entity.reading(),
            "{}: nothing known",
            entity.name
        );
    }
    assert_eq!(second.reads(), 0, "nothing is read before a row is drawn");
    let grand = second.named("Grand.ne5p");
    second.bench.workspace.in_view([grand]);
    second.answer_reads();
    assert_eq!(second.reads(), 1, "the drawn row is read");
    let upright = second.named("Upright.ne5p");
    assert!(second.bench.workspace.get(upright).unwrap().unread());
}

/// A file from outside is copied into the folder it was dropped on, and left where it
/// came from. The copy of a sample instrument rests, never held whole, and the copy of a
/// small file is read as any file of the library's own once something needs it.
#[test]
fn a_file_from_outside_is_copied_in_and_read_as_the_librarys_own() {
    let (root, outside) = (Temp::new(), Temp::new());
    fs::create_dir(root.at("Gigs")).unwrap();
    let (sample, program) = (
        crate::testing::sample_bytes(),
        Fresh::Program.bytes().unwrap(),
    );
    fs::write(outside.at("Marimba.nsmp"), &sample).unwrap();
    fs::write(outside.at("Grand.ne5p"), &program).unwrap();
    let mut session = Session::open(&root);
    let gigs = LibPath::parse("Gigs").unwrap();
    session.bench.act(vec![
        crate::browser::Act::Take {
            from: outside.at("Marimba.nsmp"),
            dir: gigs.clone(),
            name: "Marimba.nsmp".into(),
        },
        crate::browser::Act::Take {
            from: outside.at("Grand.ne5p"),
            dir: LibPath::root(),
            name: "Grand.ne5p".into(),
        },
    ]);
    session.sync();

    assert_eq!(root.read("Gigs/Marimba.nsmp"), sample);
    assert_eq!(root.read("Grand.ne5p"), program);
    assert_eq!(
        outside.read("Marimba.nsmp"),
        sample,
        "the outside file stays"
    );
    let marimba = session.named("Marimba.nsmp");
    let entity = session.bench.workspace.get(marimba).unwrap();
    assert_eq!(session.path(marimba).as_deref(), Some("Gigs/Marimba.nsmp"));
    assert!(entity.rests().is_some(), "the copy rests in its file");
    assert_eq!(entity.held_whole(), 0);

    let grand = session.named("Grand.ne5p");
    assert!(session.bench.workspace.get(grand).unwrap().unread());
    session.read(&[grand]);
    assert_eq!(session.bytes(grand), program);
    assert_eq!(session.said("is on this computer"), 2);
}

/// A stored bundle of `members`, as `(archive path, file)`.
fn bundle_of(members: &[(&str, &[u8])]) -> Vec<u8> {
    use nord_format::bundle::archive::{DosTime, Entry, Writer};
    let mut writer = Writer::new(Vec::new());
    for (path, bytes) in members {
        let size = u32::try_from(bytes.len()).unwrap();
        let crc = nord_format::crc::crc32(bytes);
        let entry = Entry::new(path.to_string(), size, crc, DosTime::default());
        writer.member(entry, &mut &bytes[..]).unwrap();
    }
    writer.finish(&[]).unwrap()
}

/// A bundle from outside unpacks into a new folder named after it, beside one that
/// already has its name, each member copied out under its own name and the manifest
/// left behind.
#[test]
fn a_bundle_from_outside_unpacks_into_a_new_flat_folder() {
    let (root, outside) = (Temp::new(), Temp::new());
    fs::create_dir(root.at("Gig")).unwrap();
    let (sample, program) = (
        crate::testing::sample_bytes(),
        Fresh::Program.bytes().unwrap(),
    );
    let bundle = bundle_of(&[
        ("Samp Lib/Samp Lib/Marimba.nsmp", &sample),
        ("Program/Bank 1/Grand.ne5p", &program),
        ("meta.xml", b"<bundle/>"),
    ]);
    fs::write(outside.at("Gig.ne5pbundle"), &bundle).unwrap();
    let mut session = Session::open(&root);
    session.bench.act(vec![crate::browser::Act::Take {
        from: outside.at("Gig.ne5pbundle"),
        dir: LibPath::root(),
        name: "Gig.ne5pbundle".into(),
    }]);
    let unbundled = loop {
        session.bench.workspace.poll(&mut session.bench.log);
        let unbundled = session.bench.workspace.take_unbundled();
        if !unbundled.is_empty() {
            break unbundled;
        }
        std::thread::yield_now();
    };
    let unpack = unbundled.into_iter().map(crate::browser::Act::Unpack);
    session.bench.act(unpack.collect());
    session.sync();

    assert_eq!(root.names("Gig 2"), ["Grand.ne5p", "Marimba.nsmp"]);
    assert_eq!(root.read("Gig 2/Marimba.nsmp"), sample);
    assert_eq!(root.read("Gig 2/Grand.ne5p"), program);
    assert!(
        !root.at("Gig.ne5pbundle").exists(),
        "the bundle is not kept"
    );
}

/// A member whose bytes do not match the bundle's checksum for them is refused, and the
/// rest of the bundle still arrives.
#[test]
fn a_damaged_member_of_a_bundle_is_refused_and_the_rest_arrive() {
    let (root, outside) = (Temp::new(), Temp::new());
    let (sample, program) = (
        crate::testing::sample_bytes(),
        Fresh::Program.bytes().unwrap(),
    );
    let mut bundle = bundle_of(&[
        ("Samp Lib/Samp Lib/Marimba.nsmp", &sample),
        ("Program/Bank 1/Grand.ne5p", &program),
    ]);
    let in_bundle = bundle
        .windows(sample.len())
        .position(|w| w == sample.as_slice())
        .unwrap();
    bundle[in_bundle + sample.len() / 2] ^= 1;
    fs::write(outside.at("Gig.ne5pbundle"), &bundle).unwrap();
    let mut session = Session::open(&root);
    session.bench.act(vec![crate::browser::Act::Take {
        from: outside.at("Gig.ne5pbundle"),
        dir: LibPath::root(),
        name: "Gig.ne5pbundle".into(),
    }]);
    let unbundled = loop {
        session.bench.workspace.poll(&mut session.bench.log);
        let unbundled = session.bench.workspace.take_unbundled();
        if !unbundled.is_empty() {
            break unbundled;
        }
        std::thread::yield_now();
    };
    let unpack = unbundled.into_iter().map(crate::browser::Act::Unpack);
    session.bench.act(unpack.collect());
    session.sync();

    assert_eq!(root.names("Gig"), ["Grand.ne5p"]);
    assert_eq!(
        session.said("“Marimba.nsmp” was not made, because the bundle's copy of it is damaged"),
        1
    );
}

/// A set list exports with the one program on this computer that claims a slot it
/// plays, and a checked sample instrument with the bytes of the file it rests in. A program
/// claiming a slot the set list does not play is left out.
#[test]
fn a_set_list_exports_with_its_program_and_a_resting_sample() {
    use nord_format::bundle::archive::{copy_member, Directory};
    use nord_format::formats::ne5;
    use nord_format::{Entity, Program, Song};

    let root = Temp::new();
    let program_at = |slot: u16| {
        let file = ne5::program::new((0, slot).try_into().unwrap());
        nord_format::to_bytes(&Entity::Program(Program::Electro5(file))).unwrap()
    };
    let one = (0, 1).try_into().unwrap();
    let song = ne5::song::new((0, 0).try_into().unwrap(), 1, [one; 4]).unwrap();
    let song = nord_format::to_bytes(&Entity::Song(Song::Electro5(song))).unwrap();
    let sample = crate::testing::sample_bytes();
    fs::write(root.at("First.ne5p"), program_at(1)).unwrap();
    fs::write(root.at("Other.ne5p"), program_at(2)).unwrap();
    fs::write(root.at("Sunday.ne5t"), &song).unwrap();
    fs::write(root.at("Marimba.nsmp"), &sample).unwrap();
    let session = Session::open(&root);
    let ids = [session.named("Sunday.ne5t"), session.named("Marimba.nsmp")];

    let laid = crate::bundle::lay_out(&ids, &session.bench.workspace, &session.bench.device.state);
    let Ok(crate::bundle::Laid::Ready(export)) = laid else {
        panic!("the bundle was not laid out");
    };
    assert_eq!(export.name, "Sunday.ne5tbundle");
    let out = root.at("Sunday.ne5tbundle");
    nord_usb::block_on(crate::bundle::write_to(&export, &out)).unwrap();

    let mut file = fs::File::open(&out).unwrap();
    let directory = Directory::read_from(&mut file).unwrap();
    let members: Vec<(String, Vec<u8>)> = directory
        .members
        .iter()
        .map(|member| {
            let mut bytes = Vec::new();
            copy_member(&mut file, member, &mut bytes).unwrap();
            (member.entry.name.clone(), bytes)
        })
        .collect();
    let names: Vec<&str> = members.iter().map(|(name, _)| name.as_str()).collect();
    assert_eq!(
        names,
        [
            "Samp Lib/Samp Lib/Marimba.nsmp",
            "Program/Bank 1/First.ne5p",
            "Set List/Set List 1/Sunday.ne5t",
            "meta.xml",
        ]
    );
    assert_eq!(members[0].1, sample);
    assert_eq!(members[1].1, program_at(1));
    assert_eq!(members[2].1, song);
    let manifest = String::from_utf8(members[3].1.clone()).unwrap();
    assert!(
        manifest.contains(
            r#"<file name="Set List/Set List 1/Sunday.ne5t" depCnt="1" dep0="Program/Bank 1/First.ne5p"/>"#
        ),
        "{manifest}"
    );
}

/// A set list's program that is still to be read is read before the bundle is laid out,
/// and where two programs claim the slot the set list plays, neither is picked and the
/// bundle says why.
#[test]
fn a_set_list_waits_for_unread_programs_and_names_two_that_claim_its_slot() {
    use crate::bundle::{lay_out, Laid};
    use nord_format::formats::ne5;
    use nord_format::{Entity, Program, Song};

    let root = Temp::new();
    let program_at = |slot: u16| {
        let file = ne5::program::new((0, slot).try_into().unwrap());
        nord_format::to_bytes(&Entity::Program(Program::Electro5(file))).unwrap()
    };
    let one = (0, 1).try_into().unwrap();
    let song = ne5::song::new((0, 0).try_into().unwrap(), 1, [one; 4]).unwrap();
    fs::write(
        root.at("Sunday.ne5t"),
        nord_format::to_bytes(&Entity::Song(Song::Electro5(song))).unwrap(),
    )
    .unwrap();
    fs::write(root.at("First.ne5p"), program_at(1)).unwrap();
    fs::write(root.at("Again.ne5p"), program_at(1)).unwrap();
    let mut session = Session::listed(&root);
    let sunday = session.named("Sunday.ne5t");
    session.read(&[sunday]);

    let programs = [session.named("Again.ne5p"), session.named("First.ne5p")];
    let state = &session.bench.device.state;
    let laid = lay_out(&[sunday], &session.bench.workspace, state);
    assert!(
        matches!(&laid, Ok(Laid::Read(ids)) if ids.contains(&programs[0]) && ids.contains(&programs[1])),
        "the unread programs were not asked for"
    );

    session.read(&programs);
    let state = &session.bench.device.state;
    let Ok(Laid::Ready(export)) = lay_out(&[sunday], &session.bench.workspace, state) else {
        panic!("the bundle was not laid out once they were read");
    };
    let paths: Vec<&str> = export
        .plan
        .members
        .iter()
        .map(|m| m.path.as_str())
        .collect();
    assert_eq!(
        paths,
        ["Set List/Set List 1/Sunday.ne5t"],
        "neither claimant is picked"
    );
    assert_eq!(
        export.left_out,
        ["more than one file on this computer is the program at 1:2"]
    );
}

/// An object the instrument was read into a file for lands at the top of the library
/// under its slot's name, past one already there, and the file it was read into goes.
#[test]
fn a_fetched_object_lands_under_its_slot_name_and_its_file_goes() {
    let (root, scratch) = (Temp::new(), Temp::new());
    let sample = crate::testing::sample_bytes();
    fs::write(root.at("Marimba.nsmp"), &sample).unwrap();
    fs::write(scratch.at("fetched-0-4.nsmp"), &sample).unwrap();
    let mut session = Session::open(&root);
    session
        .bench
        .act(vec![crate::browser::Act::Arrive(crate::device::Fetched {
            class: ObjectClass::Sample,
            at: Location::from_user(1, 5),
            name: "Marimba".into(),
            tag: "nsmp".into(),
            file: scratch.at("fetched-0-4.nsmp"),
            len: sample.len() as u64,
        })]);
    session.sync();

    assert_eq!(root.read("Marimba 2.nsmp"), sample);
    assert!(
        !scratch.at("fetched-0-4.nsmp").exists(),
        "the fetched file goes"
    );
}

/// A file from outside whose name is taken asks first. Overwrite copies it over the
/// file there, which keeps its id and tags; Keep both copies it beside under a free name.
#[test]
fn a_file_from_outside_onto_a_taken_name_asks_and_overwrite_copies_over() {
    let (root, outside) = (Temp::new(), Temp::new());
    let program = Fresh::Program.bytes().unwrap();
    let theirs = with_gain(&program, "12");
    fs::write(root.at("Grand.ne5p"), &program).unwrap();
    fs::write(outside.at("Grand.ne5p"), &theirs).unwrap();
    let mut session = Session::open(&root);
    let grand = session.only();
    session.tag(&[grand], "Sunday");
    let take = || crate::browser::Act::Take {
        from: outside.at("Grand.ne5p"),
        dir: LibPath::root(),
        name: "Grand.ne5p".into(),
    };

    session.bench.act(vec![take()]);
    let (_, answers) = session.bench.browser.asking().expect("a question");
    assert_eq!(answers, ["Cancel", "Keep both", "Overwrite"]);
    let acts = session.bench.browser.answer("Keep both");
    session.bench.act(acts);
    session.sync();
    assert_eq!(root.read("Grand 2.ne5p"), theirs);
    assert_eq!(root.read("Grand.ne5p"), program, "beside, not over");

    session.bench.act(vec![take()]);
    let acts = session.bench.browser.answer("Overwrite");
    session.bench.act(acts);
    session.sync();
    assert_eq!(root.read("Grand.ne5p"), theirs);
    session.read(&[grand]);
    assert_eq!(
        session.bytes(grand),
        theirs,
        "the same asset holds the copy"
    );
    assert_eq!(session.worn(grand), ["Sunday"]);
}

/// A library holding a three-zone sample instrument, opened, with the instrument resting
/// in its file, and the instrument's bytes.
fn resting_sample(root: &Temp) -> (Session, u64, Vec<u8>) {
    let bytes = crate::testing::zoned_sample(nord_format::formats::nsmp::codec::Layout::V2, 92);
    fs::write(root.at("Zoned.nsmp"), &bytes).unwrap();
    let mut session = Session::listed(root);
    session.ask_all();
    let id = session.only();
    assert!(session.bench.workspace.get(id).unwrap().rests().is_some());
    (session, id, bytes)
}

impl Session {
    /// Hold an edit renaming the sample instrument `id` rests in, as its document does,
    /// and ask for it to be saved.
    fn rename_resting(&mut self, id: u64, name: &str) {
        let workspace = &mut self.bench.workspace;
        let sets = vec![("name".to_string(), name.to_string())];
        workspace.hold_edit(id, Some(crate::rewrite::Edit::Sample(sets)));
        assert!(workspace.save_edit(id));
    }
}

/// An edit of a sample instrument resting in its file is saved by copying the file
/// through with the edited sections in place: the file then holds what a whole decode,
/// the same edit and a whole write make, and the asset rests in it, holding nothing
/// whole and nothing unsaved.
#[test]
fn an_edit_of_a_resting_sample_is_saved_through_its_file() {
    let root = Temp::new();
    let (mut session, id, bytes) = resting_sample(&root);
    let before = session
        .bench
        .workspace
        .get(id)
        .unwrap()
        .rests()
        .unwrap()
        .serial;

    session.rename_resting(id, "Vibes");
    session.sync();

    let whole = crate::document::sample::apply(&bytes, &[("name".into(), "Vibes".into())]);
    assert!(root.read("Zoned.nsmp") == whole.unwrap());
    let workspace = &session.bench.workspace;
    let entity = workspace.get(id).unwrap();
    let file = entity.rests().expect("it rests in the file the save wrote");
    assert_ne!(file.serial, before);
    assert!(!entity.is_unsaved() && !workspace.saving_edit(id));
    assert!(workspace.edit_of(id).is_none());
    assert_eq!(entity.held_whole(), 0);
    assert_eq!(
        root.names(""),
        [".drawbar", "Zoned.nsmp"],
        "no copy is left"
    );
}

/// A file changed in place under its index, keeping its length and time, is not saved
/// over: the copy's checksum is not the one the edit restated. The file keeps what was
/// written there, and the edit is kept, unsaved.
#[test]
fn a_resting_sample_changed_in_place_before_its_save_is_not_saved_over() {
    let root = Temp::new();
    let (mut session, id, bytes) = resting_sample(&root);
    let path = root.at("Zoned.nsmp");
    let time = fs::metadata(&path).unwrap().modified().unwrap();
    let mut theirs = bytes.clone();
    let last = theirs.len() - 3;
    theirs[last] ^= 0x40;
    fs::write(&path, &theirs).unwrap();
    fs::File::options()
        .write(true)
        .open(&path)
        .and_then(|file| file.set_modified(time))
        .unwrap();
    assert_eq!(fs::metadata(&path).unwrap().modified().unwrap(), time);

    session.rename_resting(id, "Vibes");
    session.sync();

    assert!(
        root.read("Zoned.nsmp") == theirs,
        "nothing was written over it"
    );
    let workspace = &session.bench.workspace;
    assert!(workspace.get(id).unwrap().is_unsaved());
    assert!(workspace.edit_of(id).is_some() && !workspace.saving_edit(id));
    assert_eq!(session.said("was not saved"), 1);
    assert_eq!(root.names(""), ["Zoned.nsmp"], "no copy is left");
}

/// A plan over a piano library resting in its file is saved by laying the library out
/// again from its file, each kept stroke read by its range: the file then holds what a
/// whole read and a whole layout write make, and the asset rests in it.
#[test]
fn a_plan_over_a_resting_piano_is_saved_through_its_file() {
    use nord_format::formats::npno;

    let root = Temp::new();
    let bytes = large_piano();
    fs::write(root.at("Grand.npno"), &bytes).unwrap();
    let mut session = Session::listed(&root);
    session.ask_all();
    let id = session.only();
    let trim = |library: &mut npno::Library<'_>| {
        library.set_name("Trimmed").unwrap();
        library.retain_strokes(|stroke| stroke.root != 60);
    };

    let workspace = &mut session.bench.workspace;
    let plan = crate::document::piano::Plan::trimmed("Trimmed", 60, 0);
    workspace.hold_edit(id, Some(crate::rewrite::Edit::Piano(plan)));
    assert!(workspace.save_edit(id));
    session.sync();

    let mut whole = npno::Library::borrow(&bytes).unwrap();
    trim(&mut whole);
    let whole = whole.to_piano().unwrap();
    assert!(
        root.read("Grand.npno")
            == nord_format::to_bytes(&nord_format::Entity::Piano(whole)).unwrap()
    );
    let entity = session.bench.workspace.get(id).unwrap();
    assert!(entity.rests().is_some() && !entity.is_unsaved());
    assert_eq!(entity.held_whole(), 0);
}

impl Session {
    /// Hold an edit renaming the sample instrument `id` rests in, as its document does,
    /// without saving it, and return the bytes a whole edit makes of `bytes`.
    fn edit_resting(&mut self, id: u64, bytes: &[u8]) -> Vec<u8> {
        self.rename_resting(id, "Vibes");
        let workspace = &mut self.bench.workspace;
        workspace.edit_not_saved(id);
        crate::document::sample::apply(bytes, &[("name".into(), "Vibes".into())]).unwrap()
    }
}

/// A sample instrument resting in its file is duplicated by copying the file, beside
/// it, and the copy rests in its own file. Nothing holds either whole.
#[test]
fn a_resting_sample_is_duplicated_by_copying_its_file() {
    let root = Temp::new();
    let (mut session, id, bytes) = resting_sample(&root);
    let file = session
        .bench
        .workspace
        .get(id)
        .unwrap()
        .rests()
        .unwrap()
        .clone();
    session
        .bench
        .act(vec![crate::browser::Act::DuplicateLocal(id)]);
    session.sync();

    assert_eq!(file.take_reads(), [], "nothing read it");
    assert!(root.read("Zoned copy.nsmp") == bytes);
    assert!(root.read("Zoned.nsmp") == bytes);
    let workspace = &session.bench.workspace;
    let copy = session.named("Zoned copy.nsmp");
    assert!(
        workspace.get(copy).unwrap().rests().is_some(),
        "the copy rests"
    );
    assert_eq!(workspace.held_whole(), 0);
}

/// A duplicate of a sample instrument holding an unsaved edit over its file carries the
/// edit, written through as the file is copied, and leaves the edit unsaved where it was.
#[test]
fn a_duplicate_of_a_resting_samples_edit_carries_the_edit() {
    let root = Temp::new();
    let (mut session, id, bytes) = resting_sample(&root);
    let edited = session.edit_resting(id, &bytes);
    session
        .bench
        .act(vec![crate::browser::Act::DuplicateLocal(id)]);
    session.sync();

    assert!(root.read("Zoned copy.nsmp") == edited);
    assert!(
        root.read("Zoned.nsmp") == bytes,
        "the original is not saved"
    );
    let workspace = &session.bench.workspace;
    assert!(workspace.get(id).unwrap().is_unsaved());
    assert_eq!(workspace.held_whole(), 0);
}

/// Keep both, over a sample instrument holding an unsaved edit over its file, writes the
/// edit beside the file under a free name and takes the file as it is.
#[test]
fn keep_both_writes_a_resting_samples_edit_beside_its_file() {
    let root = Temp::new();
    let (mut session, id, bytes) = resting_sample(&root);
    let edited = session.edit_resting(id, &bytes);
    let file = session
        .bench
        .workspace
        .get(id)
        .unwrap()
        .rests()
        .unwrap()
        .clone();
    session.bench.act(vec![crate::browser::Act::KeepBoth(id)]);
    session.sync();

    assert_eq!(file.take_reads(), [], "nothing read it");
    assert!(root.read("Zoned 2.nsmp") == edited);
    assert!(root.read("Zoned.nsmp") == bytes);
    let workspace = &session.bench.workspace;
    assert!(
        !workspace.get(id).unwrap().is_unsaved(),
        "the file is taken as it is"
    );
    assert!(workspace.edit_of(id).is_none());
    let mine = session.named("Zoned 2.nsmp");
    assert!(session.bench.workspace.get(mine).unwrap().rests().is_some());
}

/// A sample instrument resting in its file, renamed onto a taken name and overwriting
/// what is there, is copied over that file, which keeps its asset; its own file goes in
/// the same change, which one undo takes back.
#[test]
fn a_resting_sample_renamed_over_another_is_copied_over_its_file() {
    let root = Temp::new();
    let ours = crate::testing::zoned_sample(nord_format::formats::nsmp::codec::Layout::V2, 92);
    let theirs = crate::testing::zoned_sample(nord_format::formats::nsmp::codec::Layout::V2, 184);
    fs::write(root.at("Kept.nsmp"), &theirs).unwrap();
    fs::write(root.at("Moved.nsmp"), &ours).unwrap();
    let mut session = Session::listed(&root);
    session.ask_all();
    let (kept, moved) = (session.named("Kept.nsmp"), session.named("Moved.nsmp"));
    let workspace = &session.bench.workspace;
    let file = workspace.get(moved).unwrap().rests().unwrap().clone();
    session.tag(&[kept], "Sunday");

    session.bench.act(vec![crate::browser::Act::RenameLocal {
        id: moved,
        name: "Kept.nsmp".into(),
    }]);
    let acts = session.bench.browser.answer("Overwrite");
    session.bench.act(acts);
    session.sync();
    session.let_go();
    session.sync();

    assert!(root.read("Kept.nsmp") == ours);
    assert_eq!(file.take_reads(), [], "nothing read it");
    assert!(
        !root.at("Moved.nsmp").exists(),
        "the file it came from is gone"
    );
    assert!(session.bench.workspace.get(moved).is_none());
    assert_eq!(session.worn(kept), ["Sunday"]);
    let entity = session.bench.workspace.get(kept).unwrap();
    assert!(entity.rests().is_some() && entity.held_whole() == 0);

    session.undo();
    session.settle();
    assert!(
        root.read("Kept.nsmp") == theirs,
        "one undo puts back what was there"
    );
    assert!(root.read("Moved.nsmp") == ours, "and the file it came from");
}

/// A copy over another file that does not land leaves the asset it was moved from, and
/// that asset's file: nothing else holds what it holds.
#[test]
fn a_copy_over_that_does_not_land_keeps_the_file_it_came_from() {
    let root = Temp::new();
    let ours = crate::testing::zoned_sample(nord_format::formats::nsmp::codec::Layout::V2, 92);
    let theirs = crate::testing::zoned_sample(nord_format::formats::nsmp::codec::Layout::V2, 184);
    fs::write(root.at("Kept.nsmp"), &theirs).unwrap();
    fs::write(root.at("Moved.nsmp"), &ours).unwrap();
    let mut session = Session::listed(&root);
    session.ask_all();
    let moved = session.named("Moved.nsmp");

    session.bench.act(vec![crate::browser::Act::RenameLocal {
        id: moved,
        name: "Kept.nsmp".into(),
    }]);
    let acts = session.bench.browser.answer("Overwrite");
    session.bench.act(acts);
    // Saved over outside drawbar before the copy runs, so the copy is refused.
    let outside = crate::testing::zoned_sample(nord_format::formats::nsmp::codec::Layout::V2, 276);
    fs::write(root.at("Kept.nsmp"), &outside).unwrap();
    session.sync();
    session.sync();

    assert!(root.read("Kept.nsmp") == outside, "not written over");
    assert!(
        root.read("Moved.nsmp") == ours,
        "the file it came from stays"
    );
    let entity = session.bench.workspace.get(moved).expect("the asset stays");
    assert!(entity.kept);
    assert_eq!(session.said("was not overwritten"), 1);
}

/// ⚠️ An overwrite with bytes held whole whose save is refused leaves the asset it was
/// moved from, and that asset's file: until a pass keeps a working copy, nothing else
/// holds what it holds, and a crash before that pass would lose it.
#[test]
fn an_overwrite_whose_save_does_not_land_keeps_the_file_it_came_from() {
    let root = Temp::new();
    let program = Fresh::Program.bytes().unwrap();
    let (ours, theirs) = (with_gain(&program, "96"), with_gain(&program, "12"));
    fs::write(root.at("Kept.ne5p"), &theirs).unwrap();
    fs::write(root.at("Moved.ne5p"), &ours).unwrap();
    let mut session = Session::open(&root);
    let moved = session.named("Moved.ne5p");

    session.bench.act(vec![crate::browser::Act::RenameLocal {
        id: moved,
        name: "Kept.ne5p".into(),
    }]);
    let acts = session.bench.browser.answer("Overwrite");
    assert!(
        matches!(acts[..], [crate::browser::Act::Overwrite { .. }]),
        "{acts:?}"
    );
    session.bench.act(acts);
    // Saved over outside drawbar before the save runs, so the save is refused.
    let outside = with_gain(&program, "40");
    fs::write(root.at("Kept.ne5p"), [&outside[..], b"!"].concat()).unwrap();
    while !session.settled() {
        assert!(session.next(), "the save answered");
    }
    drop(session);

    assert_eq!(root.read("Moved.ne5p"), ours, "the file it came from stays");
}

/// A save sent before an overwrite answers for itself: the asset the overwrite came from
/// waits for the overwrite's own save, and stays where that one does not land.
#[test]
fn an_overwrite_waits_for_its_own_save_and_not_the_one_before() {
    let root = Temp::new();
    let program = Fresh::Program.bytes().unwrap();
    let (ours, theirs) = (with_gain(&program, "96"), with_gain(&program, "12"));
    fs::write(root.at("Kept.ne5p"), &theirs).unwrap();
    fs::write(root.at("Moved.ne5p"), &ours).unwrap();
    let mut session = Session::open(&root);
    let (kept, moved) = (session.named("Kept.ne5p"), session.named("Moved.ne5p"));
    let Bench { workspace, log, .. } = &mut session.bench;
    workspace.replace_bytes(kept, with_gain(&program, "40"), log);
    workspace.mark_saved(kept);
    assert!(!session.settled(), "the save before is in flight");

    session.bench.act(vec![crate::browser::Act::RenameLocal {
        id: moved,
        name: "Kept.ne5p".into(),
    }]);
    let acts = session.bench.browser.answer("Overwrite");
    session.bench.act(acts);
    assert!(session.next(), "the save before answered");
    assert_eq!(root.read("Kept.ne5p"), with_gain(&program, "40"));
    fs::write(root.at("Kept.ne5p"), b"saved over outside").unwrap();
    while !session.settled() {
        assert!(session.next(), "the overwrite's save answered");
    }
    drop(session);

    assert_eq!(root.read("Moved.ne5p"), ours, "the file it came from stays");
}

impl Session {
    /// Remove what the store let go since the last call, as the app does each frame.
    fn let_go(&mut self) {
        let left = self.store.take_left();
        let acts = left.into_iter().map(crate::browser::Act::Remove).collect();
        self.bench.act(acts);
    }
}

/// The asset an overwrite came from goes, once the overwrite lands, as an asset removed by
/// hand goes: its tab closes and its place in the send queue goes with it.
#[test]
fn an_overwrite_lets_its_source_go_as_a_removal_does() {
    let root = Temp::new();
    let program = Fresh::Program.bytes().unwrap();
    let (ours, theirs) = (with_gain(&program, "96"), with_gain(&program, "12"));
    fs::write(root.at("Kept.ne5p"), &theirs).unwrap();
    fs::write(root.at("Moved.ne5p"), &ours).unwrap();
    let mut session = Session::open(&root);
    let moved = session.named("Moved.ne5p");
    session.bench.tabs.open(moved);
    let Bench {
        workspace,
        device,
        queue,
        log,
        ..
    } = &mut session.bench;
    let slot = Location { bank: 0, slot: 2 };
    crate::queue::enqueue(
        workspace,
        device,
        queue,
        log,
        moved,
        ObjectClass::Program,
        slot,
    );
    assert!(session.bench.queue.holds(moved), "queued");

    session.bench.act(vec![crate::browser::Act::RenameLocal {
        id: moved,
        name: "Kept.ne5p".into(),
    }]);
    let acts = session.bench.browser.answer("Overwrite");
    session.bench.act(acts);
    session.sync();
    session.let_go();
    session.sync();

    assert!(session.bench.workspace.get(moved).is_none());
    assert!(!session.bench.tabs.holds(moved), "its tab closed");
    assert!(!session.bench.queue.holds(moved), "it left the queue");
    assert!(
        !root.at("Moved.ne5p").exists(),
        "the file it came from is gone"
    );
}

/// An edit made to the asset an overwrite came from while the overwrite was in flight is
/// in nothing the overwrite wrote, so the asset stays, with its edit and its file.
#[test]
fn an_overwrite_keeps_its_source_where_it_was_edited_meanwhile() {
    let root = Temp::new();
    let program = Fresh::Program.bytes().unwrap();
    let (ours, theirs) = (with_gain(&program, "96"), with_gain(&program, "12"));
    fs::write(root.at("Kept.ne5p"), &theirs).unwrap();
    fs::write(root.at("Moved.ne5p"), &ours).unwrap();
    let mut session = Session::open(&root);
    let moved = session.named("Moved.ne5p");

    session.bench.act(vec![crate::browser::Act::RenameLocal {
        id: moved,
        name: "Kept.ne5p".into(),
    }]);
    let acts = session.bench.browser.answer("Overwrite");
    session.bench.act(acts);
    let meanwhile = with_gain(&program, "40");
    let Bench { workspace, log, .. } = &mut session.bench;
    workspace.replace_bytes(moved, meanwhile.clone(), log);
    session.sync();
    session.let_go();
    session.sync();

    assert_eq!(root.read("Kept.ne5p"), ours, "the overwrite landed");
    let entity = session
        .bench
        .workspace
        .get(moved)
        .expect("the source stays");
    assert_eq!(entity.bytes, meanwhile, "with its edit");
    assert_eq!(root.read("Moved.ne5p"), ours, "and its file");
}

/// An overwrite with bytes held whole removes the asset it was moved from once its save
/// has landed, and that asset's file with it.
#[test]
fn an_overwrite_removes_the_file_it_came_from_once_it_lands() {
    let root = Temp::new();
    let program = Fresh::Program.bytes().unwrap();
    let (ours, theirs) = (with_gain(&program, "96"), with_gain(&program, "12"));
    fs::write(root.at("Kept.ne5p"), &theirs).unwrap();
    fs::write(root.at("Moved.ne5p"), &ours).unwrap();
    let mut session = Session::open(&root);
    let (kept, moved) = (session.named("Kept.ne5p"), session.named("Moved.ne5p"));

    session.bench.act(vec![crate::browser::Act::RenameLocal {
        id: moved,
        name: "Kept.ne5p".into(),
    }]);
    let acts = session.bench.browser.answer("Overwrite");
    session.bench.act(acts);
    session.sync();
    session.let_go();
    session.sync();

    assert_eq!(root.read("Kept.ne5p"), ours);
    assert!(
        !root.at("Moved.ne5p").exists(),
        "the file it came from is gone"
    );
    assert!(session.bench.workspace.get(moved).is_none());
    assert!(!session.bench.workspace.get(kept).unwrap().is_unsaved());
}

/// A sample instrument holding an unsaved edit over its file, whose file is saved over
/// outside drawbar, is not read whole to keep the edit apart: it rests in the new file,
/// which its edit follows where it still applies, and drawbar asks what to do.
#[test]
fn a_resting_sample_changed_outside_under_an_edit_is_never_read_whole() {
    let root = Temp::new();
    let (mut session, id, bytes) = resting_sample(&root);
    session.edit_resting(id, &bytes);
    let before = session
        .bench
        .workspace
        .get(id)
        .unwrap()
        .rests()
        .unwrap()
        .clone();
    let theirs = crate::testing::zoned_sample(nord_format::formats::nsmp::codec::Layout::V2, 184);
    fs::write(root.at("Zoned.nsmp"), &theirs).unwrap();

    session.settle();

    assert_eq!(before.take_reads(), [], "nothing read the old file");
    let entity = session.bench.workspace.get(id).unwrap();
    let file = entity.rests().expect("it rests in the file there now");
    assert_eq!(file.len, theirs.len() as u64);
    assert_eq!(entity.held_whole(), 0);
    let (title, _) = session.bench.browser.asking().expect("a question");
    assert!(title.contains("Zoned.nsmp"), "{title}");
}

/// A piano library rooted at each of `roots`, every key mapped to the nearest, each
/// stroke `blocks` blocks of audio.
fn piano_rooted(roots: &[u8], blocks: u16) -> Vec<u8> {
    use nord_format::formats::npno::synthetic::{take, Build};
    use nord_format::formats::npno::Bank;

    Build {
        version: 0x464,
        channels: 1,
        takes: roots
            .iter()
            .map(|root| take(*root, Bank::Attack, 0, blocks))
            .collect(),
        map: (21..=108)
            .map(|key: u8| {
                let root = roots.iter().min_by_key(|root| root.abs_diff(key));
                (key, *root.expect("a root"))
            })
            .collect(),
    }
    .bytes()
    .expect("the builder lays out a library")
}

/// A library holding a piano library resting in its file under a plan that renames it
/// and drops the strokes rooted at 60, and that plan.
fn planned_piano(root: &Temp) -> (Session, u64, crate::document::piano::Plan) {
    fs::write(root.at("Grand.npno"), piano_rooted(&[48, 60, 72], 4)).unwrap();
    let mut session = Session::listed(root);
    session.ask_all();
    let id = session.only();
    let plan = crate::document::piano::Plan::trimmed("Trimmed", 60, 0);
    let workspace = &mut session.bench.workspace;
    workspace.hold_edit(id, Some(crate::rewrite::Edit::Piano(plan.clone())));
    assert!(workspace.get(id).unwrap().is_unsaved());
    (session, id, plan)
}

/// A piano library whose file is saved over outside drawbar under an unsaved plan keeps
/// the plan, made again over the file as it is now, and drawbar asks whose to keep.
/// Keeping mine saves the plan over their file.
#[test]
fn a_plan_over_a_resting_piano_changed_outside_is_made_again_over_theirs() {
    let root = Temp::new();
    let (mut session, id, plan) = planned_piano(&root);
    let theirs = piano_rooted(&[48, 60, 72], 5);
    fs::write(root.at("Grand.npno"), &theirs).unwrap();

    session.settle();

    let workspace = &session.bench.workspace;
    let entity = workspace.get(id).unwrap();
    let file = entity.rests().expect("it rests in their file");
    assert_eq!(file.len, theirs.len() as u64);
    let (over, _) = workspace
        .edit_of(id)
        .expect("the plan applies to their file");
    assert!(std::sync::Arc::ptr_eq(over, file));
    assert!(entity.is_unsaved());
    let (title, answers) = session.bench.browser.asking().expect("a question");
    assert_eq!(title, "“Grand.npno” changed on disk");
    assert_eq!(answers, ["Keep mine", "Keep both", "Take theirs"]);

    session.bench.browser.answer("Keep mine");
    assert!(session.bench.workspace.save_edit(id));
    session.sync();
    let made = crate::document::piano::rebuild(&theirs, &plan).unwrap();
    assert!(root.read("Grand.npno") == made, "their file under my plan");
    assert!(!session.bench.workspace.get(id).unwrap().is_unsaved());
}

/// A plan that no longer applies to the file saved over it outside drawbar is kept as
/// it was, unsaved, and drawbar asks whose to keep. It is neither saved nor copied
/// beside the file, and taking theirs lets it go.
#[test]
fn a_plan_that_no_longer_applies_to_its_changed_file_is_kept_until_theirs_is_taken() {
    let root = Temp::new();
    let (mut session, id, plan) = planned_piano(&root);
    // Dropping the strokes rooted at 60 would leave this library none.
    let theirs = piano_rooted(&[60], 4);
    fs::write(root.at("Grand.npno"), &theirs).unwrap();

    session.settle();

    let workspace = &session.bench.workspace;
    assert_eq!(
        workspace.edit(id),
        Some(&crate::rewrite::Edit::Piano(plan)),
        "the plan is kept as it was"
    );
    assert!(workspace.unapplied(id).is_some());
    assert!(workspace.get(id).unwrap().is_unsaved());
    let (title, _) = session.bench.browser.asking().expect("a question");
    assert_eq!(title, "“Grand.npno” changed on disk");
    assert!(session.said("does not apply to the file as it is now") > 0);

    let acts = session.bench.browser.answer("Keep both");
    session.bench.act(acts);
    session.sync();
    let files: Vec<String> = root
        .names("")
        .into_iter()
        .filter(|name| name != ".drawbar")
        .collect();
    assert_eq!(files, ["Grand.npno"], "nothing was copied");
    assert_eq!(session.said("was not copied"), 1);
    assert!(
        session.bench.workspace.edit(id).is_some(),
        "the plan is still kept"
    );
    assert!(
        !session.bench.workspace.save_edit(id),
        "and cannot be saved"
    );

    session.bench.act(vec![crate::browser::Act::Revert(id)]);
    session.sync();
    let workspace = &session.bench.workspace;
    assert!(workspace.edit(id).is_none());
    assert!(!workspace.get(id).unwrap().is_unsaved());
    assert!(root.read("Grand.npno") == theirs);
}

/// A sample instrument's edit that no longer applies to the file saved over it outside
/// drawbar is kept, unsaved, rather than let go.
#[test]
fn a_sample_edit_that_no_longer_applies_to_its_changed_file_is_kept() {
    let root = Temp::new();
    let (mut session, id, _) = resting_sample(&root);
    let sets = vec![("zone3.root_key".to_string(), "C5".to_string())];
    let edit = crate::rewrite::Edit::Sample(sets);
    let workspace = &mut session.bench.workspace;
    workspace.hold_edit(id, Some(edit.clone()));
    assert!(
        workspace.edit_of(id).is_some(),
        "the edit applies to the file"
    );
    let theirs = crate::testing::sample_bytes();
    fs::write(root.at("Zoned.nsmp"), &theirs).unwrap();

    session.settle();

    let workspace = &session.bench.workspace;
    assert_eq!(workspace.edit(id), Some(&edit));
    assert!(workspace.unapplied(id).is_some(), "their file has one zone");
    assert!(workspace.get(id).unwrap().is_unsaved());
    assert!(session.bench.browser.asking().is_some());
}

/// A file saved over outside drawbar with what an unsaved edit of it makes leaves the
/// edit nothing to change, so it is let go and nothing is asked.
#[test]
fn an_edit_their_file_already_holds_is_let_go_without_asking() {
    let root = Temp::new();
    let (mut session, id, bytes) = resting_sample(&root);
    let theirs = session.edit_resting(id, &bytes);
    fs::write(root.at("Zoned.nsmp"), &theirs).unwrap();

    session.settle();

    let workspace = &session.bench.workspace;
    assert!(workspace.edit(id).is_none());
    assert!(!workspace.get(id).unwrap().is_unsaved());
    assert!(session.bench.browser.asking().is_none(), "nothing to ask");
}

/// A file saved over outside drawbar with what an unsaved piano plan over it makes leaves
/// the plan nothing to change, so it is let go and nothing is asked.
#[test]
fn a_piano_plan_their_file_already_holds_is_let_go_without_asking() {
    let root = Temp::new();
    let bytes = piano_rooted(&[48, 60, 72], 4);
    fs::write(root.at("Grand.npno"), &bytes).unwrap();
    let mut session = Session::listed(&root);
    session.ask_all();
    let id = session.only();
    let plan = crate::document::piano::Plan::trimmed("Trimmed", 60, 0);
    let theirs = crate::document::piano::rebuild(&bytes, &plan).unwrap();
    let workspace = &mut session.bench.workspace;
    workspace.hold_edit(id, Some(crate::rewrite::Edit::Piano(plan)));
    assert!(workspace.edit_of(id).is_some(), "the plan applies");
    fs::write(root.at("Grand.npno"), &theirs).unwrap();

    session.settle();

    let workspace = &session.bench.workspace;
    assert!(workspace.edit(id).is_none());
    assert!(!workspace.get(id).unwrap().is_unsaved());
    assert!(session.bench.browser.asking().is_none(), "nothing to ask");
}

impl Session {}

#[test]
fn the_demo_sounds_land_as_files_in_their_folder_and_are_not_filed_twice() {
    use crate::demo::{self, published, FOLDER};

    let root = Temp::new();
    let mut first = Session::listed(&root);
    let Bench {
        workspace,
        browser,
        log,
        ..
    } = &mut first.bench;
    let filed = demo::file(published(), workspace, &mut browser.folders, log);
    assert_eq!(filed, demo::FILES.len());
    first.close();
    let mut names: Vec<String> = published().into_iter().map(|(name, _)| name).collect();
    names.sort();
    assert_eq!(root.names(FOLDER), names);
    for (name, bytes) in published() {
        assert_eq!(root.read(&format!("{FOLDER}/{name}")), bytes, "{name}");
    }

    let mut second = Session::listed(&root);
    let unsure = demo::unsure(&published(), &second.bench.workspace);
    assert_eq!(
        unsure.len(),
        demo::FILES.len(),
        "unread, each might be a demo"
    );
    second.read_all();
    assert_eq!(
        demo::unsure(&published(), &second.bench.workspace),
        Vec::<u64>::new()
    );
    let Bench {
        workspace,
        browser,
        log,
        ..
    } = &mut second.bench;
    assert_eq!(
        demo::file(published(), workspace, &mut browser.folders, log),
        0
    );
    second.close();
    assert_eq!(root.names(FOLDER), names);
}

#[test]
fn a_failed_answer_counts_each_step_and_how_it_failed_once() {
    let path = LibPath::parse("Strings/Cello.npno").unwrap();
    let denied = || Err(Failure::Io("permission denied".into()));
    let read = Event::Read(vec![
        (1, denied()),
        (2, denied()),
        (3, Err(Failure::Room(1 << 30))),
    ]);
    assert_eq!(read.faults(), ["read-io", "read-room"].into());
    let saved = Event::Saved {
        id: 1,
        path: path.clone(),
        result: Err(Failure::Moved),
    };
    assert_eq!(saved.faults(), ["save-changed"].into());
    let moved = Event::Moved {
        from: path.clone(),
        to: path,
        result: Err(Failure::Io("taken".into())),
    };
    assert_eq!(moved.faults(), ["move"].into());
    let refused = Event::Committed {
        made: None,
        undoable: false,
        forgot: false,
        finished: None,
        waiting: Waiting::Nothing,
        label: "Tag".into(),
        result: Err("read-only".into()),
    };
    assert_eq!(refused.faults(), ["intent"].into());
}

/// A library of real files, as the user leaves them: two programs, a sample instrument's
/// WAV and a file drawbar does not open.
fn mixed(root: &Temp) {
    let program = Fresh::Program.bytes().unwrap();
    fs::create_dir(root.at("Live")).unwrap();
    fs::write(root.at("Live/Grand.ne5p"), with_gain(&program, "12")).unwrap();
    fs::write(root.at("Live/Organ.ne5p"), with_gain(&program, "24")).unwrap();
    fs::write(root.at("Strings.ne5p"), with_gain(&program, "36")).unwrap();
    fs::write(root.at("cello.wav"), crate::testing::wav_bytes()).unwrap();
    fs::write(root.at("Live/set list.pdf"), b"%PDF").unwrap();
}

/// What each asset of a session wears, by name, leaving out the untagged.
fn tagged(session: &Session) -> Vec<(String, Vec<String>)> {
    let mut tagged: Vec<(String, Vec<String>)> = session
        .bench
        .workspace
        .listed()
        .map(|entity| (entity.name.clone(), session.worn(entity.id)))
        .filter(|(_, worn)| !worn.is_empty())
        .collect();
    tagged.sort();
    tagged
}

fn wears(name: &str, tags: &[&str]) -> (String, Vec<String>) {
    let tags = tags.iter().map(|tag| tag.to_string()).collect();
    (name.to_string(), tags)
}

/// The whole of what the library keeps on toshokan, through the browser's acts: a folder
/// of real files opens as a library, a tag and a tag's rename are each one change, they
/// come back from the folder at the next open, another drawbar's change shows at a
/// refresh and says whose it was, and an undo takes back this session's last change alone.
#[test]
fn tags_are_kept_by_the_library_shared_by_writers_and_undone_one_change_at_a_time() {
    let root = Temp::new();
    mixed(&root);
    let mut first = Session::listed(&root);
    let mut names: Vec<String> = first
        .bench
        .workspace
        .listed()
        .map(|entity| entity.path.as_ref().unwrap().to_string())
        .collect();
    names.sort();
    assert_eq!(
        names,
        ["Live/Grand.ne5p", "Live/Organ.ne5p", "Strings.ne5p"]
    );
    let others: Vec<&str> = first
        .bench
        .browser
        .folders
        .others
        .iter()
        .map(LibPath::as_str)
        .collect();
    assert_eq!(others, ["Live/set list.pdf", "cello.wav"], "listed by name");

    let grand = first.named("Grand.ne5p");
    let organ = first.named("Organ.ne5p");
    first.tag(&[grand], "Sunday");
    first.tag(&[organ], "Sunday");
    first.rename_tag("Sunday", "Gig");
    let both = vec![wears("Grand.ne5p", &["Gig"]), wears("Organ.ne5p", &["Gig"])];
    assert_eq!(tagged(&first), both);
    first.undo();
    let renamed_back = vec![
        wears("Grand.ne5p", &["Sunday"]),
        wears("Organ.ne5p", &["Sunday"]),
    ];
    assert_eq!(tagged(&first), renamed_back, "the rename is one change");
    first.redo();
    assert_eq!(tagged(&first), both);
    first.close();

    let mut first = Session::listed(&root);
    assert_eq!(tagged(&first), both, "read from the folder");
    let strings = first.named("Strings.ne5p");
    first.tag(&[strings], "Loud");

    let mut second = Session::listed(&root);
    let organ = second.named("Organ.ne5p");
    second.tag(&[organ], "Ballad");
    assert_eq!(first.said("changed the tags of"), 0, "not yet read");
    first.refresh();
    assert_eq!(
        first.said("Another drawbar changed the tags of “Organ.ne5p”."),
        1
    );
    let all = vec![
        wears("Grand.ne5p", &["Gig"]),
        wears("Organ.ne5p", &["Ballad", "Gig"]),
        wears("Strings.ne5p", &["Loud"]),
    ];
    assert_eq!(tagged(&first), all);

    first.undo();
    let undone = vec![
        wears("Grand.ne5p", &["Gig"]),
        wears("Organ.ne5p", &["Ballad", "Gig"]),
    ];
    assert_eq!(
        tagged(&first),
        undone,
        "this session's last change, not theirs"
    );
    first.redo();
    assert_eq!(tagged(&first), all);
    first.close();
    second.close();
    let again = Session::listed(&root);
    assert_eq!(
        tagged(&again),
        all,
        "both writers' changes, from the folder"
    );
}

/// A file nothing is said about is no entity, and keeps its id when the library makes it
/// one to tag it, and again when an undo lets that entity go.
#[test]
fn a_file_keeps_its_id_as_it_becomes_an_entity_and_stops_being_one() {
    let root = Temp::new();
    mixed(&root);
    let mut session = Session::listed(&root);
    let grand = session.named("Grand.ne5p");
    assert_eq!(
        session.store.key(grand),
        Some(Key::File(LibPath::parse("Live/Grand.ne5p").unwrap()))
    );
    session.tag(&[grand], "Sunday");
    assert!(
        matches!(session.store.key(grand), Some(Key::Entity(_))),
        "an entity once tagged"
    );
    assert_eq!(session.named("Grand.ne5p"), grand);
    session.undo();
    assert_eq!(
        session.store.key(grand),
        Some(Key::File(LibPath::parse("Live/Grand.ne5p").unwrap())),
        "a file again"
    );
    assert_eq!(session.named("Grand.ne5p"), grand, "the same asset");
    assert_eq!(session.bench.workspace.listed().count(), 3);
    assert!(session.worn(grand).is_empty());
}

/// Untagging an asset takes its tag off it alone; a tag nothing wears any more leaves
/// the list once the library says so.
#[test]
fn a_tag_taken_off_its_last_asset_leaves_the_list() {
    let root = Temp::new();
    mixed(&root);
    let mut session = Session::listed(&root);
    let (grand, organ) = (session.named("Grand.ne5p"), session.named("Organ.ne5p"));
    let tag = session.tag(&[grand, organ], "Sunday");
    session.bench.act(vec![crate::browser::Act::Untag {
        ids: vec![grand],
        tag,
    }]);
    session.decided();
    assert_eq!(tagged(&session), [wears("Organ.ne5p", &["Sunday"])]);
    session.bench.act(vec![crate::browser::Act::RemoveTag(tag)]);
    session.decided();
    assert!(tagged(&session).is_empty());
    assert!(session.bench.browser.tags.all().is_empty());
}

/// Each change to a file is one undo step: undoing a delete, a rename, a save and the
/// file's making puts the folder back as it was before each, and the asset follows its
/// file without a word about changes outside drawbar.
#[test]
fn a_file_made_saved_renamed_and_deleted_is_undone_a_step_at_a_time() {
    let root = Temp::new();
    let mut session = Session::open(&root);
    let id = session.create();
    session.sync();
    let made = root.read("untitled.ne5p");
    let saved = with_gain(&made, "96");
    let Bench { workspace, log, .. } = &mut session.bench;
    workspace.replace_bytes(id, saved.clone(), log);
    workspace.mark_saved(id);
    session.sync();
    let renamed = LibPath::root().join("Renamed.ne5p");
    session.bench.workspace.place(id, renamed);
    session.sync();
    let Bench { workspace, log, .. } = &mut session.bench;
    workspace.remove(id, log);
    session.sync();
    assert_eq!(root.names(""), [".drawbar"]);

    session.undo();
    session.refocus();
    assert_eq!(root.read("Renamed.ne5p"), saved, "the delete is undone");
    let back = session.only();
    session.undo();
    session.refocus();
    assert_eq!(root.names(""), [".drawbar", "untitled.ne5p"]);
    assert_eq!(
        session.path(back).as_deref(),
        Some("untitled.ne5p"),
        "the asset follows its file back"
    );
    session.undo();
    session.refocus();
    session.read_all();
    assert_eq!(root.read("untitled.ne5p"), made, "the save is undone");
    assert_eq!(session.bytes(back), made);
    session.undo();
    session.refocus();
    assert_eq!(root.names(""), [".drawbar"], "the making is undone");
    assert_eq!(session.bench.workspace.listed().count(), 0);
    for outside in ["outside drawbar", "changed on disk", "appeared"] {
        assert_eq!(session.said(outside), 0, "{outside}");
    }
}

impl Session {
    /// The names of the library's tags, sorted.
    fn tag_names(&self) -> Vec<String> {
        let tags = self.bench.browser.tags.all().iter();
        let mut names: Vec<String> = tags.map(|tag| tag.name.clone()).collect();
        names.sort();
        names
    }

    /// Make a tag that nothing wears, as New tag… does with nothing picked, and wait
    /// until the library has committed it.
    fn make_tag(&mut self, name: &str) {
        self.bench.browser.tags.make(name).expect("a new tag");
        self.decided();
    }

    /// Remove the tag `name` as the browser does, and wait until it is committed.
    fn remove_tag(&mut self, name: &str) {
        let tags = &self.bench.browser.tags;
        let id = tags
            .all()
            .iter()
            .find(|tag| tag.name == name)
            .expect(name)
            .id;
        self.bench.act(vec![crate::browser::Act::RemoveTag(id)]);
        self.decided();
    }
}

/// A tag that nothing wears is kept by the library as a tag that wears: made, renamed
/// and removed, it is there or gone once drawbar opens the library again, and another
/// drawbar sees it at a refresh.
#[test]
fn a_tag_nothing_wears_is_kept_until_it_is_removed() {
    let root = Temp::new();
    let mut first = Session::listed(&root);
    first.make_tag("Spare");
    first.make_tag("Wedding");
    first.rename_tag("Wedding", "Gig");
    let mut second = Session::listed(&root);
    assert_eq!(second.tag_names(), ["Gig", "Spare"], "read from the folder");
    first.remove_tag("Spare");
    first.close();

    second.refresh();
    assert_eq!(second.tag_names(), ["Gig"], "another drawbar's removal");
    second.close();
    let third = Session::listed(&root);
    assert_eq!(third.tag_names(), ["Gig"]);
}

/// Renaming a tag to the name of another merges the two, here as in the library: one
/// row, worn by the assets of both, which a removal then takes off them all.
#[test]
fn renaming_a_tag_to_another_tags_name_merges_them() {
    let root = Temp::new();
    let mut session = Session::open(&root);
    let ids = [session.create(), session.create()];
    session.sync();
    session.tag(&[ids[0]], "Live");
    session.tag(&[ids[1]], "Studio");
    session.rename_tag("Live", "Studio");
    assert_eq!(session.tag_names(), ["Studio"], "one row");
    let studio = session.bench.browser.tags.all()[0].id;
    assert_eq!(session.bench.browser.tags.count(studio), 2);
    assert_eq!(session.worn(ids[0]), ["Studio"]);
    assert_eq!(session.worn(ids[1]), ["Studio"]);

    session.undo();
    assert_eq!(session.tag_names(), ["Live", "Studio"], "undone");
    assert_eq!(session.worn(ids[0]), ["Live"]);
    assert_eq!(session.worn(ids[1]), ["Studio"]);
    session.redo();
    session.remove_tag("Studio");
    assert_eq!(session.tag_names(), Vec::<String>::new());
    assert_eq!(session.worn(ids[0]), Vec::<String>::new());
    assert_eq!(session.worn(ids[1]), Vec::<String>::new());
}

impl Session {
    /// Edit the asset `id` held whole, leaving the edit unsaved.
    fn edit(&mut self, id: u64, gain: &str) -> Vec<u8> {
        let edited = with_gain(&self.bytes(id), gain);
        let Bench { workspace, log, .. } = &mut self.bench;
        workspace.replace_bytes(id, edited.clone(), log);
        edited
    }

    /// Open a view of program slot `at` of `instrument`, holding `bytes`, as a double
    /// click does, and let the store see it.
    fn view_slot(&mut self, instrument: &str, at: Location, bytes: Vec<u8>) -> u64 {
        self.store.attached(Some(instrument.to_string()));
        let origin = Origin::Device {
            class: ObjectClass::Program,
            at,
        };
        let Bench { workspace, log, .. } = &mut self.bench;
        let id = workspace.view("Grand".to_string(), origin, bytes, log);
        self.autosave();
        id
    }

    /// The autosave's pass, and every command it sent run through.
    fn autosave(&mut self) {
        let Bench {
            workspace, browser, ..
        } = &mut self.bench;
        self.store.sync(workspace, browser, Pass::Full);
        self.settle();
    }
}

/// Copy the folder at `from` to `to`, as it is now.
fn copy_tree(from: &std::path::Path, to: &std::path::Path) {
    fs::create_dir_all(to).unwrap();
    for entry in fs::read_dir(from).unwrap() {
        let entry = entry.unwrap();
        let target = to.join(entry.file_name());
        match entry.file_type().unwrap().is_dir() {
            true => copy_tree(&entry.path(), &target),
            false => _ = fs::copy(entry.path(), target).unwrap(),
        }
    }
}

/// An unsaved edit is kept as a draft, outside the library folder, and is there again,
/// still unsaved, when drawbar opens the library after a quit.
#[test]
fn an_unsaved_edit_comes_back_after_a_quit() {
    let root = Temp::new();
    let mut first = Session::open(&root);
    let id = first.create();
    first.sync();
    let file = root.read("untitled.ne5p");
    let edited = first.edit(id, "96");
    first.close();
    assert_eq!(root.read("untitled.ne5p"), file, "the file is not written");

    let second = Session::open(&root);
    let id = second.only();
    let entity = second.bench.workspace.get(id).unwrap();
    assert!(entity.is_unsaved());
    assert!(second.bytes(id) == edited);
    assert_eq!(second.said("has the unsaved edit it had"), 1);
    assert_eq!(second.said("appeared"), 0, "what was there is not news");
}

/// Opening a library and keeping an unsaved edit of one of its files writes nothing in
/// its folder: the edit is kept in this install's own state.
#[test]
fn keeping_an_unsaved_edit_writes_nothing_in_the_folder() {
    let root = Temp::new();
    fs::write(root.at("a.ne5p"), Fresh::Program.bytes().unwrap()).unwrap();
    let mut first = Session::open(&root);
    let a = first.named("a.ne5p");
    let edited = first.edit(a, "96");
    first.autosave();
    first.close();
    assert_eq!(root.names(""), ["a.ne5p"], "nothing beside the file");

    let again = Session::open(&root);
    assert!(again.bytes(again.named("a.ne5p")) == edited);
}

/// A whole view sent mid-session, as when most of what is shown changed at once, keeps
/// every asset shown before with its id, its tags and its unsaved edit, and adds the rest.
#[test]
fn a_whole_view_mid_session_keeps_what_was_shown_and_adds_the_rest() {
    let root = Temp::new();
    let mut session = Session::open(&root);
    let id = session.create();
    session.sync();
    session.tag(&[id], "Live");
    let edited = session.edit(id, "96");
    fs::create_dir(root.at("Many")).unwrap();
    for i in 0..1100 {
        fs::write(root.at(&format!("Many/{i:04}.wav")), b"wave").unwrap();
    }
    session.settle();
    let shown = session.store.shown().expect("shown");
    let shown = (shown.files.len(), shown.others().len());
    assert_eq!(shown, (1, 1100), "every file is shown once");
    let entity = session
        .bench
        .workspace
        .get(id)
        .expect("the asset keeps its id");
    assert!(entity.is_unsaved());
    assert!(session.bytes(id) == edited);
    assert_eq!(session.worn(id), ["Live"]);
    assert_eq!(session.bench.browser.folders.others.len(), 1100);
    assert_eq!(session.said("deleted outside drawbar"), 0);
}

/// A library that stops answering in the middle of a command is asked about once it has
/// shown no life for a while, and letting it go names the edits it could not keep
/// rather than waiting for it.
#[test]
fn a_library_that_stops_answering_is_asked_about_and_let_go() {
    let root = Temp::new();
    let mut session = Session::open(&root);
    let id = session.create();
    session.sync();
    session.edit(id, "96");
    session.store.patience(std::time::Duration::from_millis(50));
    session.store.fault(Fault::Stall);
    session.store.rescan();
    std::thread::sleep(std::time::Duration::from_millis(100));
    session.poll();
    let (title, answers) = session.bench.browser.asking().expect("asked");
    assert_eq!(title, "The library is not answering");
    assert_eq!(answers, ["Keep waiting", "Close and reopen"]);
    let Bench {
        workspace,
        browser,
        queue,
        log,
        ..
    } = &mut session.bench;
    session.store.close(workspace, browser, queue, log);
    let said = session.said("could not be written, since the library stopped answering");
    assert_eq!(said, 1, "the edit is named");
}

/// A library whose thread stops says so, and letting it go names the edits it could
/// not keep.
#[test]
fn a_library_whose_thread_stops_says_so_and_is_let_go() {
    let root = Temp::new();
    let mut session = Session::open(&root);
    let id = session.create();
    session.sync();
    session.edit(id, "96");
    session.store.fault(Fault::Trap);
    session.store.rescan();
    for _ in 0..1000 {
        if session.store.stopped().is_some() {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    assert!(session.store.stopped().is_some(), "the thread stopped");
    let Bench {
        workspace,
        browser,
        queue,
        log,
        ..
    } = &mut session.bench;
    session.store.close(workspace, browser, queue, log);
    let said = session.said("could not be written, since the library stopped answering");
    assert_eq!(said, 1, "the edit is named");
}

/// An unsaved edit waits out a first scan that failed, and comes back with the first
/// that lists every file.
#[cfg(unix)]
#[test]
fn an_unsaved_edit_waits_out_a_first_scan_that_failed() {
    use std::os::unix::fs::PermissionsExt;

    let root = Temp::new();
    let mut first = Session::open(&root);
    let id = first.create();
    first.sync();
    let edited = first.edit(id, "96");
    first.close();
    // A file whose time moved has its identity read and kept, which a local root that
    // cannot be written fails.
    let later = std::time::SystemTime::now() + std::time::Duration::from_secs(60);
    let file = fs::File::options()
        .write(true)
        .open(root.at("untitled.ne5p"))
        .unwrap();
    file.set_modified(later).unwrap();
    let mode = |mode| fs::set_permissions(root.local(), fs::Permissions::from_mode(mode));
    mode(0o555).unwrap();

    let mut second = Session::listed(&root);
    mode(0o755).unwrap();
    assert_eq!(
        second.said("reading the library again"),
        1,
        "the first scan failed"
    );
    assert_eq!(
        second.said("on this computer."),
        0,
        "no count of files unlisted"
    );
    second.settle();
    second.read_all();
    let id = second.only();
    assert!(second.bench.workspace.get(id).unwrap().is_unsaved());
    assert!(second.bytes(id) == edited);
}

/// The autosave keeps an unsaved edit as a draft, so a drawbar stopped without a quit
/// finds it again, from the folder and this install's own state as it left them.
#[test]
fn an_unsaved_edit_the_autosave_kept_survives_a_crash() {
    let root = Temp::new();
    let mut first = Session::open(&root);
    let id = first.create();
    first.sync();
    let edited = first.edit(id, "96");
    first.autosave();
    let crashed = Temp::new();
    copy_tree(&root.0, &crashed.0);
    copy_tree(&root.local(), &crashed.local());

    let second = Session::open(&crashed);
    let id = second.only();
    assert!(second.bench.workspace.get(id).unwrap().is_unsaved());
    assert!(second.bytes(id) == edited);
}

/// A draft over a file that changed outside drawbar since is shown over the file as it
/// is now, and drawbar asks which to keep.
#[test]
fn an_unsaved_edit_over_a_file_changed_since_asks_which_to_keep() {
    let root = Temp::new();
    let mut first = Session::open(&root);
    let id = first.create();
    first.sync();
    let file = root.read("untitled.ne5p");
    let edited = first.edit(id, "96");
    first.close();
    fs::write(root.at("untitled.ne5p"), with_gain(&file, "12")).unwrap();

    let second = Session::open(&root);
    let id = second.only();
    assert!(second.bytes(id) == edited, "the edit shows");
    let (title, answers) = second.bench.browser.asking().expect("a question is asked");
    assert_eq!(title, "“untitled.ne5p” changed on disk");
    assert_eq!(answers, ["Keep mine", "Keep both", "Take theirs"]);
}

/// An unsaved edit of a slot's view is kept by the slot, outside the library folder,
/// and is there again when the same instrument's slot is viewed after a quit.
#[test]
fn a_slot_views_unsaved_edit_comes_back_when_the_slot_is_viewed_again() {
    let root = Temp::new();
    let program = Fresh::Program.bytes().unwrap();
    let at = Location::from_user(2, 7);
    let mut first = Session::open(&root);
    let id = first.view_slot("N1", at, program.clone());
    let edited = first.edit(id, "96");
    first.close();
    assert_eq!(
        root.names(""),
        Vec::<String>::new(),
        "nothing is written in the folder"
    );

    let mut second = Session::open(&root);
    let other = second.view_slot("N2", at, program.clone());
    assert!(second.bytes(other) == program, "another instrument's slot");
    let elsewhere = second.view_slot("N1", Location::from_user(2, 8), program.clone());
    assert!(second.bytes(elsewhere) == program, "another slot");
    let id = second.view_slot("N1", at, program.clone());
    assert!(second.bench.workspace.get(id).unwrap().is_unsaved());
    assert!(second.bytes(id) == edited);
    assert_eq!(second.said("has the unsaved edit it had"), 1);
    assert_eq!(second.said("appeared"), 0, "what was there is not news");
}

/// The autosave keeps a slot view's unsaved edit, so a drawbar stopped without a quit
/// finds it again.
#[test]
fn a_slot_views_unsaved_edit_the_autosave_kept_survives_a_crash() {
    let root = Temp::new();
    let program = Fresh::Program.bytes().unwrap();
    let at = Location::from_user(1, 1);
    let mut first = Session::open(&root);
    let id = first.view_slot("N1", at, program.clone());
    let edited = first.edit(id, "96");
    first.autosave();
    let crashed = Temp::new();
    copy_tree(&root.0, &crashed.0);
    copy_tree(&root.local(), &crashed.local());

    let mut second = Session::open(&crashed);
    let id = second.view_slot("N1", at, program);
    assert!(second.bytes(id) == edited);
}

/// A slot view's edit this install's own state could not keep is kept by the next
/// autosave once it can, and is there again after a quit.
#[cfg(unix)]
#[test]
fn a_slot_views_edit_the_autosave_failed_to_keep_is_kept_by_the_next() {
    use std::os::unix::fs::PermissionsExt;

    let root = Temp::new();
    let program = Fresh::Program.bytes().unwrap();
    let at = Location::from_user(1, 1);
    let mut first = Session::open(&root);
    let id = first.view_slot("N1", at, program.clone());
    first.edit(id, "96");
    first.autosave();
    let kept = root.local().join("unsaved");
    let mode = |mode| fs::set_permissions(&kept, fs::Permissions::from_mode(mode));
    mode(0o555).unwrap();
    let edited = first.edit(id, "12");
    first.autosave();
    mode(0o755).unwrap();
    assert_eq!(
        first.said("could not be kept for later"),
        1,
        "the keep failed"
    );
    first.autosave();
    first.close();

    let mut second = Session::open(&root);
    let id = second.view_slot("N1", at, program);
    assert!(second.bytes(id) == edited, "the latest edit");
}

/// A slot view's edit kept loses nothing at a tab's close, and one whose keep has not
/// landed does.
#[test]
fn a_slot_views_edit_whose_keep_has_not_landed_is_losing() {
    let root = Temp::new();
    let program = Fresh::Program.bytes().unwrap();
    let mut session = Session::open(&root);
    let id = session.view_slot("N1", Location::from_user(1, 1), program);
    session.edit(id, "96");
    session.autosave();
    let Bench {
        workspace, browser, ..
    } = &session.bench;
    assert!(!session.store.losing(workspace, browser), "once kept");

    session.edit(id, "12");
    session.store.fault(Fault::Stall);
    let Bench {
        workspace, browser, ..
    } = &mut session.bench;
    session.store.sync(workspace, browser, Pass::Full);
    let losing = session.store.losing(workspace, browser);
    session.let_go_stalled();
    assert!(losing, "while the keep of the next edit is stalled");
}

impl Session {
    /// Close a library whose thread never finishes the command it took, once it is
    /// asked about: a library not asked about would be waited for.
    fn let_go_stalled(mut self) {
        self.store.patience(std::time::Duration::ZERO);
        for _ in 0..1000 {
            self.poll();
            if self.bench.browser.asking().is_some() {
                return self.close();
            }
            std::thread::sleep(std::time::Duration::from_millis(1));
        }
        panic!("the stalled library was never asked about");
    }

    /// The autosave's pass, with nothing it sent answered yet.
    fn autosave_unanswered(&mut self) {
        let Bench {
            workspace, browser, ..
        } = &mut self.bench;
        self.store.sync(workspace, browser, Pass::Full);
    }
}

/// A slot view reverted while the keep of its edit is in flight lets go of that edit,
/// so a drawbar stopped without a quit does not bring it back.
#[test]
fn a_slot_view_reverted_while_its_keep_is_in_flight_leaves_no_kept_edit() {
    let root = Temp::new();
    let program = Fresh::Program.bytes().unwrap();
    let at = Location::from_user(1, 1);
    let mut first = Session::open(&root);
    let id = first.view_slot("N1", at, program.clone());
    first.edit(id, "96");
    first.autosave_unanswered();
    let Bench { workspace, log, .. } = &mut first.bench;
    workspace.revert(id, log);
    first.autosave();
    let crashed = Temp::new();
    copy_tree(&root.0, &crashed.0);
    copy_tree(&root.local(), &crashed.local());

    let mut second = Session::open(&crashed);
    let id = second.view_slot("N1", at, program);
    assert!(!second.bench.workspace.get(id).unwrap().is_unsaved());
    assert_eq!(second.said("has the unsaved edit it had"), 0);
}

/// The answer to an older keep, folded in after a newer edit, marks only the older
/// edit kept: the newer one, whose own keep failed, is kept by the next autosave.
#[test]
fn an_older_keeps_answer_never_marks_a_newer_edit_kept() {
    let root = Temp::new();
    let program = Fresh::Program.bytes().unwrap();
    let at = Location::from_user(1, 1);
    let mut first = Session::open(&root);
    let id = first.view_slot("N1", at, program.clone());
    first.edit(id, "96");
    first.store.fault(Fault::Next(Box::new(Fault::Unread)));
    first.autosave_unanswered();
    let newer = first.edit(id, "12");
    first.autosave_unanswered();
    first.settle();
    let Bench {
        workspace, browser, ..
    } = &first.bench;
    assert!(
        first.store.losing(workspace, browser),
        "the newer edit is not kept"
    );
    first.autosave();
    let Bench {
        workspace, browser, ..
    } = &first.bench;
    assert!(!first.store.losing(workspace, browser), "kept once resent");
    first.close();

    let mut second = Session::open(&root);
    let id = second.view_slot("N1", at, program);
    assert!(second.bytes(id) == newer);
}

/// A keep that fails at every autosave is said once, and again only once a keep has
/// succeeded since.
#[cfg(unix)]
#[test]
fn a_keep_that_keeps_failing_is_said_once_per_run_of_failures() {
    use std::os::unix::fs::PermissionsExt;

    let root = Temp::new();
    let program = Fresh::Program.bytes().unwrap();
    let mut session = Session::open(&root);
    let id = session.view_slot("N1", Location::from_user(1, 1), program);
    session.edit(id, "96");
    session.autosave();
    let kept = root.local().join("unsaved");
    let mode = |mode| fs::set_permissions(&kept, fs::Permissions::from_mode(mode));
    let failing = "could not be kept for later";
    mode(0o555).unwrap();
    session.edit(id, "12");
    for _ in 0..3 {
        session.autosave();
    }
    assert_eq!(session.said(failing), 1, "the first failure only");
    mode(0o755).unwrap();
    session.autosave();
    mode(0o555).unwrap();
    session.edit(id, "24");
    session.autosave();
    mode(0o755).unwrap();
    assert_eq!(session.said(failing), 2, "again after a keep succeeded");
}

/// A slot view's kept edit, over a slot that holds something else since, is shown over
/// the view, and drawbar asks which to keep.
#[test]
fn a_slot_views_unsaved_edit_over_a_slot_changed_since_asks_which_to_keep() {
    let root = Temp::new();
    let program = Fresh::Program.bytes().unwrap();
    let at = Location::from_user(1, 1);
    let mut first = Session::open(&root);
    let id = first.view_slot("N1", at, program.clone());
    let edited = first.edit(id, "96");
    first.close();

    let mut second = Session::open(&root);
    let id = second.view_slot("N1", at, with_gain(&program, "12"));
    assert!(second.bytes(id) == edited, "the edit shows");
    let (title, answers) = second.bench.browser.asking().expect("a question is asked");
    assert_eq!(title, "“Grand” changed on the instrument");
    assert_eq!(answers, ["Keep mine", "Keep both", "Take theirs"]);
}

/// A slot view edited before the edit an earlier session kept of its slot is read
/// asks which to keep, and nothing kept is lost without being asked.
#[test]
fn a_slot_view_edited_before_its_kept_edit_is_read_asks_which_to_keep() {
    let root = Temp::new();
    let program = Fresh::Program.bytes().unwrap();
    let at = Location::from_user(1, 1);
    let mut first = Session::open(&root);
    let id = first.view_slot("N1", at, program.clone());
    let earlier = first.edit(id, "96");
    first.close();

    let mut second = Session::open(&root);
    second.store.attached(Some("N1".to_string()));
    let origin = Origin::Device {
        class: ObjectClass::Program,
        at,
    };
    let Bench { workspace, log, .. } = &mut second.bench;
    let id = workspace.view("Grand".to_string(), origin, program.clone(), log);
    let mine = second.edit(id, "12");
    second.autosave();
    assert!(
        second.bytes(id) == mine,
        "nothing is put over the edit unasked"
    );
    let (title, answers) = second.bench.browser.asking().expect("a question is asked");
    assert_eq!(title, "“Grand” has unsaved edits from before");
    assert_eq!(answers, ["Keep mine", "Keep both", "Take theirs"]);
    let acts = second.bench.browser.answer("Take theirs");
    second.bench.act(acts);
    assert!(second.bytes(id) == earlier, "the earlier edit");
    assert!(second.bench.workspace.get(id).unwrap().is_unsaved());
}

/// A slot view whose edit was sent, or let go of, leaves no edit for the next view.
#[test]
fn a_slot_view_sent_leaves_no_kept_edit() {
    let root = Temp::new();
    let program = Fresh::Program.bytes().unwrap();
    let at = Location::from_user(1, 1);
    let mut first = Session::open(&root);
    let id = first.view_slot("N1", at, program.clone());
    let edited = first.edit(id, "96");
    first.autosave();
    first.bench.workspace.mark_saved(id);
    first.autosave();
    first.close();

    let mut second = Session::open(&root);
    let id = second.view_slot("N1", at, edited.clone());
    assert!(!second.bench.workspace.get(id).unwrap().is_unsaved());
    assert_eq!(second.said("has the unsaved edit it had"), 0);
}

/// Saving an edit lets its draft go: the next open finds the file as it was saved, and
/// nothing unsaved.
#[test]
fn a_saved_edit_leaves_no_draft() {
    let root = Temp::new();
    let mut first = Session::open(&root);
    let id = first.create();
    first.sync();
    let edited = first.edit(id, "96");
    first.autosave();
    first.bench.workspace.mark_saved(id);
    first.sync();
    first.autosave();
    first.close();

    let second = Session::open(&root);
    let id = second.only();
    assert!(!second.bench.workspace.get(id).unwrap().is_unsaved());
    assert!(second.bytes(id) == edited);
    assert_eq!(second.said("has the unsaved edit it had"), 0);
}

/// An edit held of a sample instrument resting in its file comes back as that edit,
/// over the file, which is not read whole.
#[test]
fn an_edit_of_a_resting_sample_comes_back_as_an_edit() {
    let root = Temp::new();
    let (mut first, id, _) = resting_sample(&root);
    let sets = vec![("name".to_string(), "Vibes".to_string())];
    let edit = crate::rewrite::Edit::Sample(sets);
    first.bench.workspace.hold_edit(id, Some(edit.clone()));
    first.close();

    let (second, id, _) = resting_sample_again(&root);
    let workspace = &second.bench.workspace;
    assert!(workspace.get(id).unwrap().rests().is_some());
    assert_eq!(workspace.kept_edit(id).map(|(held, _)| held), Some(&edit));
}

/// The library `resting_sample` made, opened again.
fn resting_sample_again(root: &Temp) -> (Session, u64, ()) {
    let mut session = Session::listed(root);
    session.ask_all();
    let id = session.only();
    (session, id, ())
}

/// A library past ten thousand entries is listed whole, however deep its files are, and
/// a file drawbar holds keeps its tags wherever it is. A folder whose name starts with a
/// dot is not listed at all.
#[test]
fn a_large_tree_is_listed_whole() {
    let root = Temp::new();
    let program = Fresh::Program.bytes().unwrap();
    for dir in ["a", "b", "c", "c/d", ".git"] {
        fs::create_dir(root.at(dir)).unwrap();
    }
    fs::write(root.at("top.ne5p"), &program).unwrap();
    fs::write(root.at("c/d/kept.ne5p"), with_gain(&program, "12")).unwrap();
    fs::write(root.at(".git/hidden.ne5p"), &program).unwrap();
    let mut first = Session::listed(&root);
    let kept = first.named("kept.ne5p");
    first.tag(&[kept], "Sunday");
    first.close();

    const PAST: usize = 10_001;
    for n in 0..PAST {
        fs::write(root.at(&format!("a/{n:05}.jpg")), b"").unwrap();
    }
    fs::write(root.at("b/deep.ne5p"), &program).unwrap();
    let session = Session::listed(&root);
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
    assert_eq!(tagged(&session), [wears("kept.ne5p", &["Sunday"])]);
    let dirs: Vec<&str> = folders.all().iter().map(|dir| dir.path.as_str()).collect();
    assert_eq!(dirs, ["a", "b", "c", "c/d"]);
}

/// Many folders drawbar may not read are reported in one line that names the first few
/// and counts the rest.
#[cfg(unix)]
#[test]
fn many_folders_that_cannot_be_read_are_named_a_few_at_a_time() {
    use std::os::unix::fs::PermissionsExt;

    let root = Temp::new();
    let locked = ["A", "B", "C", "D", "E"];
    let mode = |mode| {
        for dir in locked {
            fs::set_permissions(root.at(dir), fs::Permissions::from_mode(mode)).unwrap();
        }
    };
    for dir in locked {
        fs::create_dir(root.at(dir)).unwrap();
    }
    mode(0o000);
    let session = Session::listed(&root);
    mode(0o755);
    assert_eq!(
        session.said("drawbar could not read “A”, “B”, “C” and 2 more, so"),
        1
    );
}

/// A folder drawbar may not read is reported and left out, and the rest of the library
/// opens; once it can be read, a rescan lists what is in it.
#[cfg(unix)]
#[test]
fn a_folder_that_cannot_be_read_is_reported_and_the_rest_opens() {
    use std::os::unix::fs::PermissionsExt;

    let root = Temp::new();
    let program = Fresh::Program.bytes().unwrap();
    fs::create_dir(root.at("Locked")).unwrap();
    fs::write(root.at("Locked/inside.ne5p"), &program).unwrap();
    fs::write(root.at("outside.ne5p"), &program).unwrap();
    let mode = |mode| {
        let locked = fs::Permissions::from_mode(mode);
        fs::set_permissions(root.at("Locked"), locked).unwrap();
    };
    mode(0o000);
    let mut session = Session::listed(&root);
    mode(0o755);
    let names = |session: &Session| {
        let listed = session.bench.workspace.listed();
        let mut names: Vec<String> = listed.map(|entity| entity.name.clone()).collect();
        names.sort();
        names
    };
    assert_eq!(names(&session), ["outside.ne5p"]);
    let unwalked: Vec<&str> = session
        .bench
        .browser
        .folders
        .unwalked
        .iter()
        .map(LibPath::as_str)
        .collect();
    assert_eq!(unwalked, ["Locked"]);
    assert_eq!(session.said("drawbar could not read “Locked”"), 1);

    session.refocus();
    assert_eq!(names(&session), ["inside.ne5p", "outside.ne5p"]);
    assert!(session.bench.browser.folders.unwalked.is_empty());
}

/// A tagged file deleted outside drawbar stays a lost row under one id, rescan after
/// rescan, until the user forgets it; the row goes once the library has let it go.
#[test]
fn a_lost_row_keeps_its_id_until_it_is_forgotten() {
    let root = Temp::new();
    let mut first = Session::open(&root);
    let id = first.create();
    first.tag(&[id], "Sunday");
    first.close();
    fs::remove_file(root.at("untitled.ne5p")).unwrap();

    let mut session = Session::listed(&root);
    let lost = |session: &Session| -> Vec<u64> {
        let lost = session.bench.browser.folders.lost().iter();
        lost.map(|lost| lost.id).collect()
    };
    let [row] = lost(&session)[..] else {
        panic!("one row is lost: {:?}", lost(&session));
    };
    session.refocus();
    session.refocus();
    assert_eq!(lost(&session), [row], "the same id");

    session.bench.act(vec![crate::browser::Act::Forget(row)]);
    assert_eq!(lost(&session), [row], "shown until the library lets it go");
    session.decided();
    assert_eq!(lost(&session), Vec::<u64>::new());
    session.close();
    let again = Session::listed(&root);
    assert_eq!(lost(&again), Vec::<u64>::new());
}

/// A rename that changes only case, on a disk that ignores case, leaves one asset with its
/// tags under the new name, rescan after rescan, and deleting it leaves nothing behind.
/// A disk that tells case apart has nothing to show here.
#[test]
fn a_rename_of_case_alone_leaves_one_asset() {
    let root = Temp::new();
    fs::write(root.at("probe"), b"").unwrap();
    if !root.at("PROBE").exists() {
        return;
    }
    fs::remove_file(root.at("probe")).unwrap();
    let mut session = Session::open(&root);
    let id = session.create();
    session.place_at(id, "c3.ne5p");
    session.sync();
    let untagged = session.create();
    session.place_at(untagged, "d3.ne5p");
    session.sync();
    session.tag(&[id], "Sunday");
    session.place_at(id, "C3.ne5p");
    session.place_at(untagged, "D3.ne5p");
    session.sync();
    session.refocus();
    session.refocus();

    let mut names: Vec<String> = session
        .bench
        .workspace
        .listed()
        .map(|entity| entity.name.clone())
        .collect();
    names.sort();
    assert_eq!(names, ["C3.ne5p", "D3.ne5p"], "one asset each");
    assert_eq!(root.names(""), [".drawbar", "C3.ne5p", "D3.ne5p"]);
    assert_eq!(tagged(&session), [wears("C3.ne5p", &["Sunday"])]);
    assert_eq!(session.said("outside drawbar"), 0);

    for id in [id, untagged] {
        let Bench { workspace, log, .. } = &mut session.bench;
        workspace.remove(id, log);
    }
    session.sync();
    session.refocus();
    assert_eq!(root.names(""), [".drawbar"]);
    assert_eq!(session.bench.workspace.listed().count(), 0);
}

impl Session {
    /// Put the asset `id`'s file at `name` in the top level, as a rename does.
    fn place_at(&mut self, id: u64, name: &str) {
        let path = LibPath::root().join(name);
        self.bench.workspace.place(id, path);
    }
}

/// Each answer is taken in by what it changed, and what drawbar shows after it is what a
/// fresh open of the library shows.
#[test]
fn each_answer_lists_what_the_whole_view_lists() {
    let root = Temp::new();
    fs::write(root.at("outside.ne5p"), Fresh::Program.bytes().unwrap()).unwrap();
    fs::write(
        root.at("gone.ne5p"),
        with_gain(&Fresh::Program.bytes().unwrap(), "12"),
    )
    .unwrap();
    fs::write(root.at("notes.pdf"), b"notes").unwrap();
    let mut session = Session::open(&root);
    let step = |session: &mut Session, what: &str| {
        let fresh = Session::listed(&root);
        assert_eq!(session.store.shown(), fresh.store.shown(), "after {what}");
        fresh.close();
    };
    let id = session.create();
    session.sync();
    step(&mut session, "a save");
    let outside = session.named("outside.ne5p");
    session.tag(&[id, outside], "Sunday");
    step(&mut session, "a tag");
    session.place_at(id, "moved.ne5p");
    session.sync();
    step(&mut session, "a move");
    session.undo();
    step(&mut session, "an undo");
    session.redo();
    step(&mut session, "a redo");
    let other = Temp::new();
    let mut theirs = Session::elsewhere(&root, &other);
    let moved = theirs.named("moved.ne5p");
    theirs.tag(&[moved], "Theirs");
    theirs.place_at(moved, "theirs.ne5p");
    theirs.sync();
    theirs.close();
    session.refresh();
    step(&mut session, "a refresh after another drawbar's changes");
    fs::write(root.at("more.pdf"), b"more").unwrap();
    fs::remove_file(root.at("gone.ne5p")).unwrap();
    session.settle();
    step(&mut session, "a rescan");
    session.remove_tag("Sunday");
    step(&mut session, "a tag removed");
    let names = |session: &Session| {
        let mut names: Vec<(String, Vec<String>)> = session
            .bench
            .workspace
            .listed()
            .map(|entity| {
                (
                    session.path(entity.id).unwrap_or_default(),
                    session.worn(entity.id),
                )
            })
            .collect();
        names.sort();
        names
    };
    let shown = names(&session);
    let others = session.bench.browser.folders.others.clone();
    session.close();
    let fresh = Session::open(&root);
    assert_eq!(names(&fresh), shown, "the assets and their tags");
    assert_eq!(
        fresh.bench.browser.folders.others, others,
        "the files drawbar does not open"
    );
}

/// A tagged file saved in drawbar and then moved outside it keeps its tags at its new
/// name: the library knows it by what drawbar wrote.
#[test]
fn a_tagged_file_saved_then_moved_outside_keeps_its_tags() {
    let root = Temp::new();
    let mut session = Session::open(&root);
    let id = session.create();
    session.sync();
    session.tag(&[id], "Sunday");
    session.edit(id, "96");
    session.bench.workspace.mark_saved(id);
    session.sync();
    fs::create_dir(root.at("Moved")).unwrap();
    fs::rename(root.at("untitled.ne5p"), root.at("Moved/kept.ne5p")).unwrap();
    session.refocus();

    assert_eq!(session.path(id).as_deref(), Some("Moved/kept.ne5p"));
    assert_eq!(tagged(&session), [wears("kept.ne5p", &["Sunday"])]);
}

/// A folder restored from a backup is shown as it is at the next read: what the backup
/// does not hold is gone.
#[test]
fn a_folder_restored_from_a_backup_is_shown_as_it_is() {
    let root = Temp::new();
    let mut first = Session::open(&root);
    first.create();
    first.close();
    let backup = Temp::new();
    copy_tree(&root.0, &backup.0);

    let mut second = Session::open(&root);
    second.tag(&[second.only()], "Sunday");
    second.close();
    fs::remove_dir_all(&root.0).unwrap();
    copy_tree(&backup.0, &root.0);

    let mut third = Session::listed(&root);
    assert_eq!(tagged(&third), Vec::<(String, Vec<String>)>::new());
    third.settle();
    assert_eq!(tagged(&third), Vec::<(String, Vec<String>)>::new());
}

/// A file another drawbar tagged since this one last looked is tagged again on the
/// entity that one made, and is not made an entity a second time.
#[test]
fn a_file_tagged_by_another_drawbar_meanwhile_is_tagged_on_its_entity() {
    let root = Temp::new();
    fs::write(root.at("Grand.ne5p"), Fresh::Program.bytes().unwrap()).unwrap();
    let mut first = Session::listed(&root);
    let mut second = Session::listed(&root);
    let grand = second.named("Grand.ne5p");
    second.tag(&[grand], "Ballad");
    let grand = first.named("Grand.ne5p");
    first.tag(&[grand], "Sunday");

    assert_eq!(tagged(&first), [wears("Grand.ne5p", &["Ballad", "Sunday"])]);
    second.refresh();
    assert_eq!(
        tagged(&second),
        [wears("Grand.ne5p", &["Ballad", "Sunday"])]
    );
}

/// A tag asked for an asset whose file could not be written is not waited on forever:
/// it is said not to reach that asset, and the tags asked after it go on.
#[cfg(unix)]
#[test]
fn a_tag_on_an_asset_with_no_file_is_refused_and_the_next_goes_on() {
    use std::os::unix::fs::PermissionsExt;

    let root = Temp::new();
    fs::write(root.at("Grand.ne5p"), Fresh::Program.bytes().unwrap()).unwrap();
    let mut session = Session::open(&root);
    let mode = |mode| fs::set_permissions(&root.0, fs::Permissions::from_mode(mode)).unwrap();
    mode(0o555);
    let homeless = session.create();
    session.sync();
    mode(0o755);
    session.tag(&[homeless], "Sunday");
    let grand = session.named("Grand.ne5p");
    session.tag(&[grand], "Ballad");

    assert!(!session.store.deciding(), "nothing waits");
    assert_eq!(
        session.said("“untitled.ne5p” has no file in the library, so “Sunday” was not put on it."),
        1
    );
    assert_eq!(tagged(&session), [wears("Grand.ne5p", &["Ballad"])]);
}

/// Assets deleted together in one gesture are one change: one undo brings every file
/// back. A deletion asked for later is a change of its own.
#[test]
fn a_selection_deleted_together_is_one_undo_step() {
    let root = Temp::new();
    let mut session = Session::open(&root);
    let ids = [session.create(), session.create(), session.create()];
    session.sync();
    let files = root.names("");
    let acts = ids[..2].iter().map(|id| crate::browser::Act::Remove(*id));
    session.bench.act(acts.collect());
    session.sync();
    assert_eq!(
        session.store.undo_label().as_deref(),
        Some("Undo Delete 2 files")
    );
    session.bench.act(vec![crate::browser::Act::Remove(ids[2])]);
    session.sync();
    assert_eq!(root.names(""), [".drawbar"]);
    session.undo();
    session.undo();
    assert_eq!(root.names(""), files, "two undos bring all three back");
}

/// The segments of drawbar's history in the library folder, each with what it holds.
fn segments(root: &Temp) -> Vec<(std::path::PathBuf, Vec<u8>)> {
    let writers = root.at(".drawbar/library/writers.2");
    let dirs = fs::read_dir(writers)
        .unwrap()
        .map(|dir| dir.unwrap().path());
    let files = dirs.flat_map(|dir| fs::read_dir(dir).unwrap().map(|file| file.unwrap().path()));
    let segments = files.filter(|path| path.extension().is_some_and(|ext| ext == "txt"));
    segments
        .map(|path| (path.clone(), fs::read(path).unwrap()))
        .collect()
}

#[test]
fn an_older_copy_of_drawbars_history_restored_from_a_backup_keeps_nothing_apart() {
    let root = Temp::new();
    mixed(&root);
    let mut session = Session::listed(&root);
    let grand = session.named("Grand.ne5p");
    let tag = session.tag(&[grand], "Gig");
    let older = segments(&root);
    session.bench.act(vec![crate::browser::Act::Untag {
        ids: vec![grand],
        tag,
    }]);
    session.decided();
    session.close();
    for (path, bytes) in older {
        fs::write(path, bytes).unwrap();
    }
    let session = Session::listed(&root);
    assert!(session.bench.browser.folders.kept.is_empty());
    assert!(tagged(&session).is_empty());
}

#[test]
fn a_library_this_computer_never_opened_opens_without_trouble() {
    let root = Temp::new();
    mixed(&root);
    let session = Session::listed(&root);
    let log = session.bench.log.iter();
    let trouble: Vec<&str> = log
        .filter(|entry| entry.level == crate::log::Level::Error)
        .map(|entry| entry.text.as_str())
        .collect();
    assert!(trouble.is_empty(), "{trouble:?}");
}

#[test]
fn an_undo_another_drawbar_refuses_is_let_go_of_for_the_change_before_it() {
    let root = Temp::new();
    mixed(&root);
    let mut first = Session::listed(&root);
    let grand = first.named("Grand.ne5p");
    let organ = first.named("Organ.ne5p");
    first.tag(&[grand], "Gig");
    first.tag(&[organ], "Gig");
    let elsewhere = Temp::new();
    let mut second = Session::open_at(&root, &elsewhere.0);
    let organ = second.named("Organ.ne5p");
    let tags = second.bench.browser.tags.all();
    let tag = tags
        .iter()
        .find(|tag| tag.name == "Gig")
        .expect("the tag")
        .id;
    second.bench.act(vec![crate::browser::Act::Untag {
        ids: vec![organ],
        tag,
    }]);
    second.decided();
    first.refresh();
    let refused = first.store.undo_label().expect("a change to undo");
    first.undo();
    let change = refused.trim_start_matches("Undo ");
    let told = format!("{change} cannot be undone: another drawbar has changed it since.");
    assert_eq!(first.said(&told), 1);
    assert_ne!(first.store.undo_label(), Some(refused));
    first.undo();
    assert!(tagged(&first).is_empty());
}

/// Assets moved together in one gesture are one change: one undo moves every file back.
#[test]
fn a_selection_moved_together_is_one_undo_step() {
    let root = Temp::new();
    fs::create_dir(root.at("Gig")).unwrap();
    let mut session = Session::open(&root);
    let ids = [session.create(), session.create()];
    session.sync();
    let top = root.names("");
    let gig = LibPath::parse("Gig").unwrap();
    let folder = session.bench.browser.folders.id_of(&gig).expect("listed");
    let acts = ids.iter().map(|id| crate::browser::Act::File {
        id: *id,
        folder: Some(folder),
    });
    session.bench.act(acts.collect());
    session.sync();
    assert_eq!(root.names("Gig").len(), 2, "both moved");
    assert_eq!(
        session.store.undo_label().as_deref(),
        Some("Undo Move 2 files")
    );
    session.undo();
    session.settle();
    assert_eq!(root.names("Gig"), Vec::<String>::new(), "both moved back");
    assert_eq!(root.names(""), top, "beside the folder");
}

/// Files dropped together are one change: one undo takes every one back.
#[test]
fn files_dropped_together_are_one_undo_step() {
    let root = Temp::new();
    let mut session = Session::open(&root);
    let program = Fresh::Program.bytes().unwrap();
    let dropped = ["a.ne5p", "b.ne5p"].map(|name| crate::browser::Act::Import {
        name: name.into(),
        bytes: program.clone(),
    });
    session.bench.act(dropped.into());
    session.sync();
    assert_eq!(root.names(""), [".drawbar", "a.ne5p", "b.ne5p"]);
    assert_eq!(
        session.store.undo_label().as_deref(),
        Some("Undo New 2 files")
    );
    session.undo();
    assert_eq!(root.names(""), [".drawbar"], "both taken back");
}

/// A file renamed onto another's name and written over it is one change with the
/// deletion of the file it came from: one undo puts both back.
#[test]
fn an_overwrite_and_the_file_it_came_from_are_one_undo_step() {
    let root = Temp::new();
    let program = Fresh::Program.bytes().unwrap();
    let (ours, theirs) = (with_gain(&program, "96"), with_gain(&program, "12"));
    fs::write(root.at("Kept.ne5p"), &theirs).unwrap();
    fs::write(root.at("Moved.ne5p"), &ours).unwrap();
    let mut session = Session::open(&root);
    let moved = session.named("Moved.ne5p");
    session.bench.act(vec![crate::browser::Act::RenameLocal {
        id: moved,
        name: "Kept.ne5p".into(),
    }]);
    let acts = session.bench.browser.answer("Overwrite");
    session.bench.act(acts);
    session.sync();
    session.let_go();
    session.sync();
    assert_eq!(root.names(""), [".drawbar", "Kept.ne5p"]);
    assert!(root.read("Kept.ne5p") == ours);

    session.undo();
    session.settle();
    assert!(root.read("Kept.ne5p") == theirs, "what was there");
    assert!(root.read("Moved.ne5p") == ours, "and the file it came from");
}

/// A tag put on a view of a slot keeps the view on this computer and tags its new file
/// in one change: one undo takes back both.
#[test]
fn a_tag_on_a_slot_view_is_one_undo_step() {
    let root = Temp::new();
    let mut session = Session::open(&root);
    let at = Location::from_user(1, 1);
    let id = session.view_slot("N1", at, Fresh::Program.bytes().unwrap());
    let tag = session.bench.browser.tags.make("Sunday").unwrap();
    session
        .bench
        .act(vec![crate::browser::Act::Tag { ids: vec![id], tag }]);
    session.sync();
    session.decided();
    assert_eq!(session.worn(id), ["Sunday"]);
    assert_eq!(
        session.store.undo_label().as_deref(),
        Some("Undo New “Grand.ne5p”")
    );
    session.undo();
    assert_eq!(
        root.names(""),
        [".drawbar"],
        "the file and its tag are taken back"
    );
}

/// A bundle unpacked into a new folder is one change: one undo takes back the folder
/// and every file in it.
#[test]
fn a_bundle_unpacked_is_one_undo_step() {
    let (root, outside) = (Temp::new(), Temp::new());
    let program = Fresh::Program.bytes().unwrap();
    let bundle = bundle_of(&[
        ("Program/Bank 1/Grand.ne5p", &program),
        ("Program/Bank 1/Upright.ne5p", &with_gain(&program, "12")),
    ]);
    fs::write(outside.at("Gig.ne5pbundle"), &bundle).unwrap();
    let mut session = Session::open(&root);
    session.bench.act(vec![crate::browser::Act::Take {
        from: outside.at("Gig.ne5pbundle"),
        dir: LibPath::root(),
        name: "Gig.ne5pbundle".into(),
    }]);
    let unbundled = loop {
        session.bench.workspace.poll(&mut session.bench.log);
        let unbundled = session.bench.workspace.take_unbundled();
        if !unbundled.is_empty() {
            break unbundled;
        }
        std::thread::yield_now();
    };
    let unpack = unbundled.into_iter().map(crate::browser::Act::Unpack);
    session.bench.act(unpack.collect());
    session.sync();
    assert_eq!(root.names("Gig"), ["Grand.ne5p", "Upright.ne5p"]);
    session.undo();
    session.settle();
    assert_eq!(
        root.names(""),
        [".drawbar"],
        "the folder and its files are taken back"
    );
}

/// A new folder is a change of its own, which undo takes back.
#[test]
fn a_new_folder_is_undone() {
    let root = Temp::new();
    let mut session = Session::open(&root);
    session.bench.act(vec![crate::browser::Act::NewFolder]);
    session.sync();
    let made = root.names("");
    assert_eq!(made.len(), 2, "{made:?}");
    session.undo();
    session.settle();
    assert_eq!(root.names(""), [".drawbar"], "the folder is gone");
}

/// Removing a folder that holds files and folders moves them up and removes it in one
/// change: one undo puts everything back where it was, and one redo removes it again.
#[test]
fn removing_a_folder_with_folders_in_it_is_one_undo_step() {
    let root = Temp::new();
    let program = Fresh::Program.bytes().unwrap();
    fs::create_dir_all(root.at("Gig/Set")).unwrap();
    fs::write(root.at("Gig/a.ne5p"), &program).unwrap();
    fs::write(root.at("Gig/Set/b.ne5p"), with_gain(&program, "12")).unwrap();
    let mut session = Session::open(&root);
    let gig = LibPath::parse("Gig").unwrap();
    let folder = session.bench.browser.folders.id_of(&gig).expect("listed");
    session
        .bench
        .act(vec![crate::browser::Act::RemoveFolder(folder)]);
    session.sync();
    assert_eq!(root.names(""), [".drawbar", "Set", "a.ne5p"]);
    assert_eq!(
        session.store.undo_label().as_deref(),
        Some("Undo Remove folder “Gig”"),
        "named by the gesture"
    );
    session.undo();
    session.settle();
    assert_eq!(root.names(""), [".drawbar", "Gig"]);
    assert_eq!(root.names("Gig"), ["Set", "a.ne5p"]);
    assert_eq!(root.names("Gig/Set"), ["b.ne5p"]);
    session.redo();
    session.settle();
    assert_eq!(
        root.names(""),
        [".drawbar", "Set", "a.ne5p"],
        "removed again"
    );
    assert_eq!(root.names("Set"), ["b.ne5p"]);
}

/// A new tag made on a selection and named in its editor is one change: one undo takes
/// it off every asset and out of the list.
#[test]
fn a_new_tag_named_on_a_selection_is_one_undo_step() {
    let root = Temp::new();
    let mut session = Session::open(&root);
    let ids = [session.create(), session.create()];
    session.sync();
    let before = session.store.undo_label();
    session
        .bench
        .act(vec![crate::browser::Act::NewTag(ids.to_vec())]);
    session.decided();
    let tag = session.bench.browser.tags.all()[0].id;
    session.bench.act(vec![crate::browser::Act::RenameTag {
        id: tag,
        name: "Sunday".into(),
    }]);
    session.decided();
    assert_eq!(session.worn(ids[0]), ["Sunday"]);
    assert_eq!(session.worn(ids[1]), ["Sunday"]);
    assert_eq!(
        session.store.undo_label().as_deref(),
        Some("Undo New tag “Sunday”")
    );
    session.undo();
    assert_eq!(session.worn(ids[0]), Vec::<String>::new());
    assert_eq!(session.worn(ids[1]), Vec::<String>::new());
    assert_eq!(session.tag_names(), Vec::<String>::new(), "out of the list");
    assert_eq!(session.store.undo_label(), before, "one step");
}

/// Quitting while a new tag's name is still being typed keeps the tag, on the assets
/// it was made on, under the name it had.
#[test]
fn a_new_tag_still_being_named_at_a_quit_is_kept() {
    let root = Temp::new();
    let mut session = Session::open(&root);
    let id = session.create();
    session.sync();
    session
        .bench
        .act(vec![crate::browser::Act::NewTag(vec![id])]);
    session.decided();
    session.close();

    let again = Session::open(&root);
    assert_eq!(again.worn(again.only()), ["New tag"]);
}

/// The Edit menu says what Undo takes back and Redo makes again: the change, and what it
/// names. A change that failed is no step of either.
#[test]
fn undo_and_redo_say_what_they_take_back() {
    let root = Temp::new();
    let mut session = Session::open(&root);
    assert_eq!(session.store.undo_label(), None);
    let id = session.create();
    session.sync();
    session.tag(&[id], "Sunday");
    session.place_at(id, "Grand.ne5p");
    session.sync();
    assert_eq!(
        session.store.undo_label().as_deref(),
        Some("Undo Rename “untitled.ne5p”")
    );
    session.undo();
    assert_eq!(
        session.store.undo_label().as_deref(),
        Some("Undo New tag “Sunday”"),
        "a tag made and put on is one change"
    );
    assert_eq!(
        session.store.redo_label().as_deref(),
        Some("Redo Rename “untitled.ne5p”")
    );
    session.redo();
    assert_eq!(session.store.redo_label(), None);

    fs::write(root.at("Grand.ne5p"), b"changed outside").unwrap();
    let Bench { workspace, log, .. } = &mut session.bench;
    workspace.remove(id, log);
    session.sync();
    assert_eq!(
        session.store.undo_label().as_deref(),
        Some("Undo Rename “untitled.ne5p”"),
        "the refused delete is no step"
    );
}

/// Undoing a folder's rename puts its files back under the old name and leaves no
/// folder of the new one; a redo moves them back under it.
#[test]
fn a_folder_rename_undone_leaves_no_folder_behind() {
    let root = Temp::new();
    let program = Fresh::Program.bytes().unwrap();
    fs::create_dir_all(root.at("Live/Deep")).unwrap();
    fs::write(root.at("Live/a.ne5p"), &program).unwrap();
    fs::write(root.at("Live/Deep/b.ne5p"), &program).unwrap();
    let mut session = Session::open(&root);
    let live = LibPath::root().join("Live");
    let folder = session.bench.browser.folders.id_of(&live).unwrap();
    session.rename_folder(folder, "Gig");
    session.sync();
    assert_eq!(root.names(""), [".drawbar", "Gig"]);

    session.undo();
    assert_eq!(root.names(""), [".drawbar", "Live"], "no Gig is left");
    assert_eq!(root.names("Live"), ["Deep", "a.ne5p"]);
    let dirs: Vec<&str> = session
        .bench
        .browser
        .folders
        .all()
        .iter()
        .map(|dir| dir.path.as_str())
        .collect();
    assert_eq!(dirs, ["Live", "Live/Deep"]);

    session.redo();
    session.refocus();
    assert_eq!(root.names(""), [".drawbar", "Gig"]);
    assert_eq!(root.names("Gig/Deep"), ["b.ne5p"]);
}

/// An unsaved edit of a file in a folder renamed, and the rename undone, follows the
/// file: the asset keeps its id, and the edit comes back at the file's name after a
/// quit.
#[test]
fn an_unsaved_edit_follows_its_file_through_a_folder_rename_and_its_undo() {
    let root = Temp::new();
    let program = Fresh::Program.bytes().unwrap();
    fs::create_dir_all(root.at("Live")).unwrap();
    fs::write(root.at("Live/a.ne5p"), &program).unwrap();
    let mut session = Session::open(&root);
    let id = session.named("a.ne5p");
    let edited = session.edit(id, "96");
    let live = LibPath::root().join("Live");
    let folder = session.bench.browser.folders.id_of(&live).unwrap();
    session.rename_folder(folder, "Gig");
    session.sync();
    session.autosave();
    session.undo();
    session.autosave();
    assert_eq!(session.named("a.ne5p"), id, "the same asset");
    assert_eq!(session.path(id).as_deref(), Some("Live/a.ne5p"));
    session.close();

    let again = Session::open(&root);
    let id = again.named("a.ne5p");
    assert_eq!(again.path(id).as_deref(), Some("Live/a.ne5p"));
    assert!(again.bytes(id) == edited, "the edit came back");
}

/// Two windows that kept unsaved edits of one file: the newer comes back, and the
/// older is let go, with a word.
#[test]
fn of_two_windows_edits_of_one_file_the_newer_comes_back() {
    let root = Temp::new();
    fs::write(root.at("a.ne5p"), Fresh::Program.bytes().unwrap()).unwrap();
    let mut first = Session::open(&root);
    let mut second = Session::open(&root);
    let a = first.named("a.ne5p");
    first.edit(a, "12");
    first.close();
    std::thread::sleep(std::time::Duration::from_millis(20));
    let a = second.named("a.ne5p");
    let newer = second.edit(a, "24");
    second.close();

    let again = Session::open(&root);
    let a = again.named("a.ne5p");
    assert!(again.bytes(a) == newer, "the newer edit");
    assert_eq!(
        again.said("the newer is shown, and the older was let go"),
        1
    );
    again.close();
    let last = Session::open(&root);
    assert_eq!(last.said("the older was let go"), 0, "the older is gone");
    assert!(last.bytes(last.named("a.ne5p")) == newer);
}

/// Two windows of this install keep their unsaved edits of two files. Opened again with
/// one window, both edits come back.
#[test]
fn both_windows_unsaved_edits_come_back_in_one() {
    let root = Temp::new();
    let program = Fresh::Program.bytes().unwrap();
    fs::write(root.at("a.ne5p"), &program).unwrap();
    fs::write(root.at("b.ne5p"), &program).unwrap();
    let mut first = Session::open(&root);
    let mut second = Session::open(&root);
    let a = first.named("a.ne5p");
    let edited_a = first.edit(a, "12");
    let b = second.named("b.ne5p");
    let edited_b = second.edit(b, "24");
    first.close();
    second.close();

    let mut again = Session::open(&root);
    let (a, b) = (again.named("a.ne5p"), again.named("b.ne5p"));
    assert!(again.bytes(a) == edited_a, "the first window's edit");
    assert!(again.bytes(b) == edited_b, "the second window's edit");
    assert_eq!(again.said("has the unsaved edit it had"), 2);
    again.autosave();
    again.close();

    let last = Session::open(&root);
    let (a, b) = (last.named("a.ne5p"), last.named("b.ne5p"));
    assert!(last.bytes(a) == edited_a && last.bytes(b) == edited_b);
    assert_eq!(
        last.said("has the unsaved edit it had"),
        2,
        "each edit is kept once, by the window that took it up"
    );
}

/// toshokan's file system for a library, stopped as a crash stops it right after a
/// rename `stops` names, and renaming folders whole only where `rename_dir` says.
struct Crashing {
    native: toshokan::blocking::Native,
    stops: fn(&toshokan::Io) -> bool,
    rename_dir: bool,
    stopped: bool,
}

impl toshokan::blocking::Backend for Crashing {
    fn capabilities(&self, root: toshokan::Root) -> toshokan::io::Capabilities {
        let native = self.native.capabilities(root);
        toshokan::io::Capabilities {
            rename_dir: native.rename_dir && self.rename_dir,
            ..native
        }
    }

    fn perform(&mut self, io: toshokan::Io) -> toshokan::IoResult {
        assert!(!self.stopped, "crashed");
        let stops = (self.stops)(&io);
        let done = self.native.perform(io);
        self.stopped = stops;
        done
    }
}

/// Leave in `root` a save another install's drawbar stopped partway, and return the bytes
/// it was saving.
fn leave_unfinished_save(root: &Temp) -> Vec<u8> {
    let elsewhere = Temp::new();
    leave_unfinished_save_in(root, &elsewhere.0).0
}

/// Leave in `root` a save a drawbar keeping its own state in `local` stopped partway,
/// and return the bytes it was saving, with that drawbar, which holds its writer until
/// it is dropped.
fn leave_unfinished_save_in(
    root: &Temp,
    local: &std::path::Path,
) -> (Vec<u8>, toshokan::blocking::Library<Crashing>) {
    let program = Fresh::Program.bytes().unwrap();
    fs::write(root.at("Grand.ne5p"), &program).unwrap();
    let saved = with_gain(&program, "77");
    let at = toshokan::RelPath::new("Grand.ne5p").unwrap();
    let expect =
        toshokan::Expect::Holds(keys::identity_of_bytes(&path("Grand.ne5p"), &program).unwrap());
    let bytes = saved.clone();
    fn into_trash(io: &toshokan::Io) -> bool {
        matches!(io, toshokan::Io::Rename { to, .. } if to.as_str().contains("/trash/"))
    }
    let other = interrupted(root, local, (into_trash, true), |other| {
        let _ = other.intent("Save").save_file(&at, bytes, expect).commit();
    });
    (saved, other)
}

/// Leave in `root` a rename of the folder “Bulk”, which holds `a.ne5p` and `b.ne5p`, to
/// “Bulk2” that another install's drawbar, moving one file at a time as a browser does,
/// stopped after the first.
fn leave_unfinished_folder_rename(root: &Temp) {
    let program = Fresh::Program.bytes().unwrap();
    fs::create_dir(root.at("Bulk")).unwrap();
    fs::write(root.at("Bulk/a.ne5p"), &program).unwrap();
    fs::write(root.at("Bulk/b.ne5p"), &program).unwrap();
    let elsewhere = Temp::new();
    fn into_bulk2(io: &toshokan::Io) -> bool {
        matches!(io, toshokan::Io::Rename { to, .. } if to.as_str().starts_with("Bulk2/"))
    }
    let (from, to) = (
        toshokan::RelPath::new("Bulk").unwrap(),
        toshokan::RelPath::new("Bulk2").unwrap(),
    );
    interrupted(root, &elsewhere.0, (into_bulk2, false), |other| {
        let _ = other.intent("Rename").move_tree(&from, &to).commit();
    });
}

/// Open the library in `root` as a drawbar keeping its own state in `local`, and make
/// `change` there, which a crash stops right after a rename `stops` names. The drawbar
/// renames folders whole where `rename_dir` says. Returns that drawbar, which holds its
/// writer until it is dropped.
fn interrupted(
    root: &Temp,
    local: &std::path::Path,
    (stops, rename_dir): (fn(&toshokan::Io) -> bool, bool),
    change: impl FnOnce(&mut toshokan::blocking::Library<Crashing>),
) -> toshokan::blocking::Library<Crashing> {
    use std::panic::AssertUnwindSafe;
    use toshokan::blocking::{Library, Native};

    let crashing = Crashing {
        native: Native::new(&root.0, local),
        stops,
        rename_dir,
        stopped: false,
    };
    let env = toshokan::Env {
        clock: Box::new(toshokan::env::SystemClock),
        random: Box::new(toshokan::env::OsRandom::new()),
        identify: std::rc::Rc::new(keys::Contents),
        names: Box::new(keys::Volume::probe(&root.0)),
        label: "drawbar".into(),
    };
    let (mut other, _) = Library::open(crashing, keys::layout(), &keys::schema(), env).unwrap();
    other.rescan().unwrap();
    let stopped = std::panic::catch_unwind(AssertUnwindSafe(|| change(&mut other)));
    assert!(stopped.is_err(), "the other drawbar stopped partway");
    other
}

/// A change another window of this install left, which stopped after this one opened,
/// is finished without asking at the next read, as one left before it opened is.
#[test]
fn a_change_a_window_left_after_this_one_opened_is_finished() {
    let root = Temp::new();
    let mut session = Session::listed(&root);
    let (saved, window) = leave_unfinished_save_in(&root, &root.local());
    drop(window);
    session.settle();
    session.poll();
    assert!(session.bench.browser.asking().is_none(), "nothing is asked");
    assert!(root.read("Grand.ne5p") == saved, "the save is finished");
}

/// A change a live window of this install is making is not asked about, and once that
/// window stops, the next read finishes it.
#[test]
fn a_change_a_live_window_left_is_finished_once_the_window_stops() {
    let root = Temp::new();
    let (saved, window) = leave_unfinished_save_in(&root, &root.local());
    let mut session = Session::listed(&root);
    session.poll();
    assert_eq!(session.said("is in progress in another drawbar window"), 1);
    drop(window);
    session.settle();
    session.poll();
    assert!(session.bench.browser.asking().is_none(), "nothing is asked");
    assert!(root.read("Grand.ne5p") == saved, "the save is finished");
}

impl Session {
    /// Let a minute pass, and read the library again where the store asks to.
    fn a_minute_later(&mut self) {
        self.at(60.0);
    }

    /// Let the clock reach `seconds`, and read the library again where the store asks
    /// to.
    fn at(&mut self, seconds: f64) {
        let later = eframe::egui::RawInput {
            time: Some(seconds),
            ..Default::default()
        };
        crate::testing::run(&self.bench.ctx, later, |_| {});
        self.poll();
        while self.store.scanning() {
            assert!(self.next(), "the library is read again");
        }
        self.poll();
    }
}

/// A save another install's drawbar left unfinished, as on a computer that syncs the
/// folder, is not settled behind the user's back: once it has stood a while, since that
/// drawbar may be making it still, drawbar asks, and Finish completes it.
#[test]
fn a_change_another_install_left_unfinished_is_asked_about() {
    let root = Temp::new();
    let saved = leave_unfinished_save(&root);
    let mut session = Session::listed(&root);
    session.poll();
    assert!(
        session.bench.browser.asking().is_none(),
        "nothing is asked while the change may be in progress"
    );
    session.a_minute_later();
    let (title, answers) = session.bench.browser.asking().expect("a question is asked");
    assert_eq!(title, "Another drawbar left “Save” unfinished");
    assert_eq!(answers, ["Later", "Roll back", "Leave as is", "Finish"]);
    for act in session.bench.browser.answer("Finish") {
        let crate::browser::Act::Settle(orphan, how) = act else {
            panic!("{act:?} settles nothing");
        };
        session.store.settle_unfinished(orphan, how);
    }
    session.decided();
    assert!(root.read("Grand.ne5p") == saved, "the save is finished");
}

/// A save of drawbar's own that can no longer be finished, since its new bytes are gone,
/// leaves the library writable: drawbar asks whether to roll it back or leave it, and
/// Roll back puts the old bytes back.
#[test]
fn a_save_that_can_no_longer_be_finished_is_asked_about_and_rolls_back() {
    let root = Temp::new();
    let mut session = Session::open(&root);
    let id = session.create();
    session.sync();
    let old = root.read("untitled.ne5p");
    session.store.fault(Fault::Refuse(
        |io| matches!(io, toshokan::Io::Rename { to, .. } if to.as_str() == "untitled.ne5p"),
    ));
    let log = &mut session.bench.log;
    session
        .bench
        .workspace
        .replace_bytes(id, with_gain(&old, "96"), log);
    session.bench.workspace.mark_saved(id);
    session.sync();
    let writers = root.at(".drawbar/library/writers.2");
    let writer = fs::read_dir(&writers).unwrap().next().unwrap().unwrap();
    for staged in fs::read_dir(writer.path().join("tmp")).unwrap() {
        fs::remove_file(staged.unwrap().path()).unwrap();
    }

    session.make_tag("Sunday");
    assert_eq!(
        session.store.read_only(),
        None,
        "the library stays writable"
    );
    assert_eq!(session.tag_names(), ["Sunday"], "the next change is made");
    assert_eq!(session.said("Undo no longer reaches"), 1);
    session.settle();
    assert_eq!(
        session.said("Finished saving"),
        0,
        "the save was not finished"
    );
    assert!(
        session.bench.workspace.get(id).unwrap().is_unsaved(),
        "the edit is kept"
    );
    session.a_minute_later();
    let (title, answers) = session.bench.browser.asking().expect("a question is asked");
    assert_eq!(title, "drawbar could not finish “Save”");
    assert_eq!(answers, ["Later", "Leave as is", "Roll back"]);
    for act in session.bench.browser.answer("Roll back") {
        let crate::browser::Act::Settle(orphan, how) = act else {
            panic!("{act:?} settles nothing");
        };
        session.store.settle_unfinished(orphan, how);
    }
    session.decided();
    assert_eq!(root.read("untitled.ne5p"), old, "the old bytes are back");
}

/// The same, where that save was the session's first change: drawbar still says Undo
/// began again, and never that the save was finished.
#[test]
fn a_first_save_that_can_no_longer_be_finished_is_not_said_to_be_finished() {
    let root = Temp::new();
    let program = Fresh::Program.bytes().unwrap();
    fs::write(root.at("Grand.ne5p"), &program).unwrap();
    let mut session = Session::listed(&root);
    let id = session.named("Grand.ne5p");
    session.store.fault(Fault::Refuse(
        |io| matches!(io, toshokan::Io::Rename { to, .. } if to.as_str() == "Grand.ne5p"),
    ));
    let log = &mut session.bench.log;
    session
        .bench
        .workspace
        .replace_bytes(id, with_gain(&program, "96"), log);
    session.bench.workspace.mark_saved(id);
    session.sync();
    let writers = root.at(".drawbar/library/writers.2");
    let writer = fs::read_dir(&writers).unwrap().next().unwrap().unwrap();
    for staged in fs::read_dir(writer.path().join("tmp")).unwrap() {
        fs::remove_file(staged.unwrap().path()).unwrap();
    }

    session.make_tag("Sunday");
    session.settle();
    assert_eq!(session.said("Undo no longer reaches"), 1);
    assert_eq!(
        session.said("Finished saving"),
        0,
        "the save was not finished"
    );
    session.a_minute_later();
    let (title, _) = session.bench.browser.asking().expect("a question is asked");
    assert_eq!(title, "drawbar could not finish “Save”");
}

/// A folder's rename another drawbar left unfinished, finished or rolled back, moves its
/// files as drawbar's own change: the log says nothing of files changed outside drawbar.
#[test]
fn settling_a_rename_another_drawbar_left_is_no_change_outside() {
    for (how, at) in [("Finish", "Bulk2"), ("Roll back", "Bulk")] {
        let root = Temp::new();
        leave_unfinished_folder_rename(&root);
        let mut session = Session::listed(&root);
        session.a_minute_later();
        let (title, _) = session.bench.browser.asking().expect("a question is asked");
        assert_eq!(title, "Another drawbar left “Rename” unfinished");
        for act in session.bench.browser.answer(how) {
            let crate::browser::Act::Settle(orphan, how) = act else {
                panic!("{act:?} settles nothing");
            };
            session.store.settle_unfinished(orphan, how);
        }
        session.decided();
        session.settle();
        assert_eq!(root.names(at), ["a.ne5p", "b.ne5p"], "{how}");
        let said: Vec<&str> = session
            .bench
            .log
            .iter()
            .map(|entry| entry.text.as_str())
            .collect();
        assert_eq!(session.said("utside drawbar"), 0, "{how}: {said:?}");
    }
}

/// A change whose paths keep moving between reads is being carried out still, however
/// long it takes, and is asked about only once two reads a while apart find it still.
#[test]
fn a_change_whose_paths_move_is_asked_about_only_once_they_stand() {
    let root = Temp::new();
    leave_unfinished_save(&root);
    let mut session = Session::listed(&root);
    session.poll();
    fs::write(root.at("Grand.ne5p"), b"more of the copy").unwrap();
    session.a_minute_later();
    assert!(session.bench.browser.asking().is_none(), "it moved");
    session.at(120.0);
    let (title, _) = session.bench.browser.asking().expect("it stood");
    assert_eq!(title, "Another drawbar left “Save” unfinished");
}

/// A change another install's drawbar finishes before it has stood a while is never
/// asked about: it was in progress.
#[test]
fn a_change_another_install_finishes_soon_is_not_asked_about() {
    let root = Temp::new();
    leave_unfinished_save(&root);
    let mut session = Session::listed(&root);
    session.poll();
    let writers = root.at(".drawbar/library/writers.2");
    for writer in fs::read_dir(writers).unwrap() {
        let pending = writer.unwrap().path().join("pending");
        for record in fs::read_dir(pending).into_iter().flatten() {
            fs::remove_file(record.unwrap().path()).unwrap();
        }
    }
    session.a_minute_later();
    assert!(session.bench.browser.asking().is_none());
}

fn path(text: &str) -> LibPath {
    LibPath::parse(text).unwrap()
}

/// A pending record of another writer that does not read is left alone, and said so
/// once.
#[test]
fn a_record_that_does_not_read_is_left_alone_and_said_so() {
    let root = Temp::new();
    let mut first = Session::open(&root);
    first.create();
    first.close();
    let writers = root.at(".drawbar/library/writers.2");
    let other = writers.join("0123456789abcdef0123456789abcdef");
    fs::create_dir_all(other.join("pending")).unwrap();
    fs::write(
        other.join("pending/fedcba9876543210fedcba9876543210.json"),
        b"{\"not\":\"a record\"}",
    )
    .unwrap();

    let second = Session::listed(&root);
    assert_eq!(
        second.said("drawbar left alone 1 unfinished change of other drawbars"),
        1,
        "{:?}",
        second
            .bench
            .log
            .iter()
            .map(|entry| entry.text.clone())
            .collect::<Vec<_>>()
    );
}

impl Session {
    /// Open the library as another install would, with its own state at `local`.
    fn elsewhere(root: &Temp, local: &Temp) -> Session {
        let bench = Bench::new();
        let backend = Backend::start(&bench.ctx, root.0.clone(), local.0.clone());
        let mut session = Session {
            store: Store::start(backend),
            bench,
        };
        session.opened();
        session
    }
}

/// An unsaved edit kept of a file by its path comes back though another drawbar made
/// the file an entity since, and is kept by the entity from then on.
#[test]
fn an_edit_kept_by_path_comes_back_once_its_file_is_an_entity() {
    let root = Temp::new();
    fs::write(root.at("Grand.ne5p"), Fresh::Program.bytes().unwrap()).unwrap();
    let mut first = Session::open(&root);
    let grand = first.named("Grand.ne5p");
    let edited = first.edit(grand, "12");
    first.close();
    let other = Temp::new();
    let mut theirs = Session::elsewhere(&root, &other);
    let grand = theirs.named("Grand.ne5p");
    theirs.tag(&[grand], "Sunday");
    theirs.close();

    let mut again = Session::open(&root);
    let grand = again.named("Grand.ne5p");
    assert!(again.bytes(grand) == edited, "the edit is back");
    assert_eq!(tagged(&again), [wears("Grand.ne5p", &["Sunday"])]);
    again.autosave();
    again.close();
    let last = Session::open(&root);
    let grand = last.named("Grand.ne5p");
    assert!(last.bytes(grand) == edited);
    assert_eq!(last.said("has the unsaved edit it had"), 1, "kept once");
}

/// A tagged file deleted outside drawbar and saved back from drawbar is its entity's
/// file again: its tags stay with it.
#[test]
fn a_tagged_file_saved_back_keeps_its_tags() {
    let root = Temp::new();
    fs::write(root.at("Grand.ne5p"), Fresh::Program.bytes().unwrap()).unwrap();
    let mut session = Session::open(&root);
    let grand = session.named("Grand.ne5p");
    session.tag(&[grand], "Sunday");
    fs::remove_file(root.at("Grand.ne5p")).unwrap();
    session.refocus();
    assert!(session.bench.browser.folders.missing.contains(&grand));
    session.edit(grand, "12");
    session.bench.workspace.mark_saved(grand);
    session.sync();
    let listed: Vec<u64> = session
        .bench
        .workspace
        .listed()
        .map(|entity| entity.id)
        .collect();
    assert_eq!(listed, [grand], "one asset, and its file is back");
    assert_eq!(tagged(&session), [wears("Grand.ne5p", &["Sunday"])]);
    assert!(!session.bench.browser.folders.missing.contains(&grand));
    session.close();

    let again = Session::listed(&root);
    assert_eq!(tagged(&again), [wears("Grand.ne5p", &["Sunday"])]);
    assert!(
        again.bench.browser.folders.lost().is_empty(),
        "nothing is lost"
    );
}

/// A save asked for behind a rename waits for the rename's answer: where the rename is
/// refused, as onto a file holding the same bytes, the save writes the file where it
/// still is, and the other file is left as it was.
#[test]
fn a_save_behind_a_refused_rename_writes_the_file_where_it_is() {
    let root = Temp::new();
    let program = Fresh::Program.bytes().unwrap();
    fs::write(root.at("a.ne5p"), &program).unwrap();
    fs::write(root.at("b.ne5p"), &program).unwrap();
    let mut session = Session::open(&root);
    let a = session.named("a.ne5p");
    session.place_at(a, "b.ne5p");
    let edited = session.edit(a, "12");
    session.bench.workspace.mark_saved(a);
    while !session.settled() {
        assert!(session.next(), "the library answered");
    }

    assert!(
        root.read("b.ne5p") == program,
        "the other file is left as it was"
    );
    assert!(
        root.read("a.ne5p") == edited,
        "the save landed where the file is"
    );
}

/// The library in `root` with an asset tagged “New”, open in two sessions that write as
/// one writer, as a copied install does: the first tags it “Here”, the second “There”,
/// and each has read what the other did. Each branch of the writer's history shows as
/// changes kept apart. Returns the sessions, the asset's id in each, and the copy.
fn forked(root: &Temp) -> (Session, Session, u64, u64, Temp) {
    let mut first = Session::open(root);
    let id = first.create();
    first.sync();
    first.tag(&[id], "New");
    first.close();
    let copy = Temp::new();
    copy_tree(&root.local(), &copy.0);
    let mut here = Session::open(root);
    let mut there = Session::open_at(root, &copy.0);
    let (at_here, at_there) = (here.only(), there.only());
    here.tag(&[at_here], "Here");
    there.tag(&[at_there], "There");
    here.settle();
    there.settle();
    (here, there, at_here, at_there, copy)
}

impl Session {
    /// The run kept apart that holds a change described as `said` says.
    fn kept(&self, said: &str) -> crate::folders::Kept {
        let kept = self.bench.browser.folders.kept.iter();
        let mut found = kept.filter(|kept| kept.rows.iter().any(|row| row.said.contains(said)));
        found
            .next()
            .cloned()
            .unwrap_or_else(|| panic!("{said:?} in {:?}", self.bench.browser.folders.kept))
    }

    fn take_kept(&mut self, run: toshokan::log::Run, changes: Vec<KeptKey>) {
        self.store.take_kept(run, changes);
        self.decided();
    }

    /// Dismiss `run` as Kept apart does: asked first, then dismissed.
    fn dismiss_kept(&mut self, run: toshokan::log::Run) {
        self.bench.browser.ask_dismiss_kept(run);
        let acts = self.bench.browser.answer("Discard");
        assert!(
            matches!(&acts[..], [crate::browser::Act::DismissKept(asked)] if **asked == run),
            "{acts:?}"
        );
        self.store.dismiss_kept(run);
        self.decided();
    }
}

/// Two sessions writing as one writer fork its history: each branch shows as changes
/// kept apart, merged into neither, and the log says so once.
#[test]
fn a_forked_writers_branches_show_as_changes_kept_apart() {
    let root = Temp::new();
    let (here, there, at_here, at_there, _copy) = forked(&root);
    for (session, id) in [(&here, at_here), (&there, at_there)] {
        let kept = &session.bench.browser.folders.kept;
        assert_eq!(kept.len(), 2, "one per branch: {kept:?}");
        assert!(kept.iter().all(|kept| kept.kind == KeptKind::Run));
        assert_eq!(session.worn(id), ["New"], "neither branch is merged");
        assert_eq!(session.said("kept apart from the library"), 1);
    }
    let kept = here.kept("gains the tag “Here”");
    let mut said: Vec<&str> = kept.rows.iter().map(|row| row.said.as_str()).collect();
    said.sort();
    assert_eq!(said, ["“untitled” gains the tag “Here”"]);
    let row = kept.rows.iter().find(|row| row.id.is_some()).unwrap();
    assert_eq!(row.id, Some(at_here));
    assert!(row.takes);
    assert!(kept.title.starts_with("Another drawbar"), "{}", kept.title);
}

/// Taking one change kept apart makes it, as one change undo takes back.
#[test]
fn a_change_kept_apart_is_taken_and_undone() {
    let root = Temp::new();
    let (mut here, _there, at_here, _, _copy) = forked(&root);
    let kept = here.kept("gains the tag “Here”");
    let row = kept
        .rows
        .iter()
        .find(|row| row.id == Some(at_here))
        .unwrap();
    here.take_kept(kept.run, vec![row.key.clone()]);
    assert_eq!(here.worn(at_here), ["Here", "New"]);
    here.undo();
    assert_eq!(here.worn(at_here), ["New"], "undone");
}

/// Taking one tag a branch put on an asset leaves the other tag it put on kept apart.
#[test]
fn taking_one_tag_kept_apart_takes_no_other_tag_on_that_asset() {
    let root = Temp::new();
    let mut first = Session::open(&root);
    first.create();
    first.sync();
    first.close();
    let copy = Temp::new();
    copy_tree(&root.local(), &copy.0);
    let mut here = Session::open(&root);
    let mut there = Session::open_at(&root, &copy.0);
    let (at_here, at_there) = (here.only(), there.only());
    here.tag(&[at_here], "Here");
    there.tag(&[at_there], "Live");
    there.tag(&[at_there], "Gig");
    here.settle();
    let kept = here.kept("gains the tag “Live”");
    let row = kept.rows.iter().find(|row| row.said.contains("“Live”"));
    here.take_kept(kept.run, vec![row.unwrap().key.clone()]);
    assert_eq!(here.worn(at_here), ["Live"], "neither branch is merged");
}

/// Taking a change to an asset a branch made makes the asset, at its file, with it.
#[test]
fn a_change_to_an_asset_a_branch_made_is_taken_with_the_asset() {
    let root = Temp::new();
    let mut first = Session::open(&root);
    first.create();
    first.sync();
    first.close();
    let copy = Temp::new();
    copy_tree(&root.local(), &copy.0);
    let mut here = Session::open(&root);
    let mut there = Session::open_at(&root, &copy.0);
    here.tag(&[here.only()], "Here");
    there.create();
    there.sync();
    here.settle();
    let kept = here.kept("its origin becomes");
    let row = kept
        .rows
        .iter()
        .find(|row| row.said.contains("its origin becomes"));
    here.take_kept(kept.run, vec![row.unwrap().key.clone()]);
    assert_eq!(here.said("did not change as asked"), 0);
    let left = here.bench.browser.folders.kept.iter();
    let left = left.filter(|left| left.run == kept.run);
    assert!(
        left.flat_map(|left| &left.rows).next().is_none(),
        "the asset is made at its file: {:?}",
        here.bench.browser.folders.kept
    );
}

/// Two copies of one install's data, each tagging `Live/Grand.ne5p` before either
/// reads the other, then the one at `root` tagging it “Later”: each branch's change is
/// kept apart, and the library has since recorded the file as an asset of its own.
/// Returns that session, and the asset.
fn forked_over_one_file(root: &Temp) -> (Session, u64) {
    mixed(root);
    let mut first = Session::open(root);
    let organ = first.named("Organ.ne5p");
    first.tag(&[organ], "New");
    first.close();
    let copy = Temp::new();
    copy_tree(&root.local(), &copy.0);
    let mut here = Session::open(root);
    let mut there = Session::open_at(root, &copy.0);
    here.tag(&[here.named("Grand.ne5p")], "Here");
    there.tag(&[there.named("Grand.ne5p")], "There");
    here.settle();
    let grand = here.named("Grand.ne5p");
    here.tag(&[grand], "Later");
    (here, grand)
}

/// A branch's change to a file the library has since recorded as an asset of its own is
/// listed and taken as a change to that asset, and once taken nothing of it is left.
#[test]
fn a_branchs_change_to_a_file_the_library_recorded_since_is_taken_onto_its_asset() {
    let root = Temp::new();
    let (mut here, grand) = forked_over_one_file(&root);
    let theirs = here.kept("gains the tag “There”");
    assert_eq!(
        theirs
            .rows
            .iter()
            .map(|row| row.said.as_str())
            .collect::<Vec<_>>(),
        ["“Grand” gains the tag “There”"]
    );
    here.take_kept(theirs.run, Vec::new());
    assert_eq!(here.said("did not change as asked"), 0);
    assert_eq!(here.worn(grand), ["Later", "There"]);
    let mut left = here.bench.browser.folders.kept.iter();
    let left = left.find(|left| left.run == theirs.run);
    let left = left.expect("the run stays until it is dismissed");
    assert_eq!(left.rows, Vec::new(), "nothing left to review");
}

/// A change to an asset that only a dismissed branch made is not listed, since nothing
/// could take it; the tag it made still is.
#[test]
fn a_change_to_an_asset_only_a_dismissed_branch_made_is_not_listed() {
    let root = Temp::new();
    let (mut here, _) = forked_over_one_file(&root);
    let theirs = here.kept("gains the tag “There”").run;
    let ours = here.kept("gains the tag “Here”").run;
    here.dismiss_kept(ours);
    let mut left = here.bench.browser.folders.kept.iter();
    let left = left.find(|left| left.run == theirs);
    let left = left.expect("the run stays until it is dismissed");
    let said: Vec<&str> = left.rows.iter().map(|row| row.said.as_str()).collect();
    assert_eq!(said, ["Made the tag “There”"]);
}

/// Taking all of a run's changes makes each, and dismissing a run leaves it shown to no
/// reader again.
#[test]
fn changes_kept_apart_are_taken_whole_or_dismissed_for_every_reader() {
    let root = Temp::new();
    let (mut here, mut there, at_here, at_there, _copy) = forked(&root);
    let theirs = here.kept("gains the tag “There”");
    here.take_kept(theirs.run, Vec::new());
    assert_eq!(here.worn(at_here), ["New", "There"]);
    let ours = here.kept("gains the tag “Here”");
    here.dismiss_kept(ours.run);
    assert!(
        !here
            .bench
            .browser
            .folders
            .kept
            .iter()
            .any(|kept| kept.run == ours.run),
        "dismissed"
    );
    there.settle();
    assert!(
        !there
            .bench
            .browser
            .folders
            .kept
            .iter()
            .any(|kept| kept.run == ours.run),
        "dismissed for the other reader too"
    );
    assert_eq!(there.worn(at_there), ["New", "There"]);
}

/// What another drawbar logged after a part of its history the folder does not hold yet
/// waits apart, to take or leave, and is merged once that part arrives.
#[test]
fn changes_after_history_a_sync_has_not_brought_wait_apart_until_it_does() {
    let root = Temp::new();
    fs::write(root.at("Grand.ne5p"), Fresh::Program.bytes().unwrap()).unwrap();
    let other = Temp::new();
    let mut theirs = Session::elsewhere(&root, &other);
    let grand = theirs.named("Grand.ne5p");
    theirs.tag(&[grand], "Sunday");
    theirs.close();
    let mut theirs = Session::elsewhere(&root, &other);
    let grand = theirs.named("Grand.ne5p");
    theirs.tag(&[grand], "Monday");
    theirs.settle();
    let writers = fs::read_dir(root.at(".drawbar/library/writers.2")).unwrap();
    let writer = writers.map(|entry| entry.unwrap().path()).next().unwrap();
    let snapshot = fs::read_dir(&writer)
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .find(|file| file.extension().is_some_and(|ext| ext == "json"))
        .expect("the snapshot the first session left");
    let held = Temp::new();
    let away = held.0.join("snapshot.json");
    fs::rename(&snapshot, &away).unwrap();

    let mut ours = Session::listed(&root);
    let kept = ours.kept("gains the tag “Monday”");
    assert_eq!(kept.kind, KeptKind::Gap);
    assert_eq!(crate::browser::kept::offers(kept.kind), (true, false));
    assert_eq!(tagged(&ours), Vec::<(String, Vec<String>)>::new());

    fs::rename(&away, &snapshot).unwrap();
    ours.settle();
    assert_eq!(ours.bench.browser.folders.kept, Vec::new(), "merged");
    assert_eq!(tagged(&ours), [wears("Grand.ne5p", &["Monday", "Sunday"])]);
}

/// A run kept apart says how many changes it holds, and once its every change is taken
/// it offers only to be dismissed.
#[test]
fn a_run_kept_apart_counts_its_changes_and_offers_dismissal_once_nothing_is_left() {
    let root = Temp::new();
    let (mut here, _there, _, _, _copy) = forked(&root);
    let theirs = here.kept("gains the tag “There”");
    assert!(
        theirs
            .title
            .starts_with("Another drawbar made 1 change on a branch of its own"),
        "{}",
        theirs.title
    );
    here.take_kept(theirs.run, Vec::new());
    let left = here
        .bench
        .browser
        .folders
        .kept
        .iter()
        .find(|kept| kept.run == theirs.run)
        .cloned()
        .expect("the run stays until it is dismissed");
    assert_eq!(left.rows, Vec::new(), "nothing left to review");
    assert_eq!(crate::browser::kept::offers(left.kind), (true, true));
    here.dismiss_kept(left.run);
    assert!(!here
        .bench
        .browser
        .folders
        .kept
        .iter()
        .any(|kept| kept.run == left.run));
}

/// A file a branch kept apart renamed is listed only while the asset's file is elsewhere,
/// is taken as one change, and a take of it while the file is gone is refused, saying so
/// in the user's words.
#[test]
fn a_file_a_branch_kept_apart_renamed_is_taken_where_the_folder_holds_it() {
    let root = Temp::new();
    let mut first = Session::open(&root);
    let id = first.create();
    first.sync();
    first.close();
    let copy = Temp::new();
    copy_tree(&root.local(), &copy.0);
    let mut here = Session::open(&root);
    let mut there = Session::open_at(&root, &copy.0);
    let (at_here, at_there) = (here.only(), there.only());
    assert_eq!(at_here, id);
    here.tag(&[at_here], "Here");
    there.bench.act(vec![crate::browser::Act::RenameLocal {
        id: at_there,
        name: "Upright".to_string(),
    }]);
    there.sync();
    there.settle();
    here.settle();
    let moves = |s: &Session| {
        let kept = s.bench.browser.folders.kept.iter();
        let mut rows = kept.flat_map(|kept| kept.rows.iter().map(move |row| (kept.run, row)));
        rows.find(|(_, row)| row.said.contains(" is at ") || row.said.contains(" moves to "))
            .map(|(run, row)| (run, row.key.clone(), row.said.clone()))
    };
    assert_eq!(
        moves(&here),
        None,
        "the file is already where the branch put it"
    );

    let gone = Temp::new();
    fs::rename(root.at("Upright.ne5p"), gone.0.join("Upright.ne5p")).unwrap();
    here.settle();
    let (run, key, said) = moves(&here).expect("the file the branch names");
    assert_eq!(said, "“Upright” is at “Upright”");
    here.take_kept(run, vec![key.clone()]);
    assert_eq!(
        here.said(
            "did not change as asked: the file at Upright.ne5p cannot be taken: no file is there"
        ),
        1
    );

    fs::rename(gone.0.join("Upright.ne5p"), root.at("Upright.ne5p")).unwrap();
    here.settle();
    here.take_kept(run, vec![key]);
    assert_eq!(moves(&here), None, "taken");
    let ours = here.kept("gains the tag “Here”");
    here.take_kept(ours.run, Vec::new());
    assert_eq!(
        here.worn(here.named("Upright.ne5p")),
        ["Here"],
        "the file is the entity's again"
    );
}

/// Each session whose history another copy of the install's data also continued says,
/// once, that it goes on with a history of its own.
#[test]
fn a_session_whose_history_forked_says_it_goes_on_with_its_own() {
    let root = Temp::new();
    let (mut here, mut there, _, _, _copy) = forked(&root);
    for session in [&mut here, &mut there] {
        session.settle();
        assert_eq!(session.said("their histories split"), 1);
    }
}

/// An unsaved edit an earlier session kept, of a file that goes before drawbar reads it,
/// stays kept for when the file is back.
#[test]
fn an_edit_waiting_for_its_file_to_be_read_is_kept_when_the_file_goes() {
    let root = Temp::new();
    fs::write(root.at("Grand.ne5p"), Fresh::Program.bytes().unwrap()).unwrap();
    let mut first = Session::open(&root);
    let grand = first.named("Grand.ne5p");
    let edited = first.edit(grand, "12");
    first.autosave();
    first.close();

    let mut second = Session::listed(&root);
    let away = Temp::new();
    fs::rename(root.at("Grand.ne5p"), away.at("Grand.ne5p")).unwrap();
    second.settle();
    second.autosave();
    assert_eq!(
        second.said("The unsaved edit of “Grand.ne5p” is still kept"),
        1
    );
    second.close();

    fs::rename(away.at("Grand.ne5p"), root.at("Grand.ne5p")).unwrap();
    let third = Session::open(&root);
    let grand = third.named("Grand.ne5p");
    assert!(third.bytes(grand) == edited, "the edit is back");
}

/// An unsaved edit sent to be kept behind a command the library's thread faults on is
/// named as lost when the library is let go, since it was never kept.
#[test]
fn an_edit_sent_to_be_kept_behind_a_fault_is_named_as_lost() {
    let root = Temp::new();
    let mut session = Session::open(&root);
    let id = session.create();
    session.sync();
    session.edit(id, "96");
    session.store.fault(Fault::Trap);
    let Bench {
        workspace, browser, ..
    } = &mut session.bench;
    session.store.sync(workspace, browser, Pass::Full);
    for _ in 0..1000 {
        if session.store.stopped().is_some() {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    assert!(session.store.stopped().is_some(), "the thread stopped");
    let Bench {
        workspace,
        browser,
        queue,
        log,
        ..
    } = &mut session.bench;
    session.store.close(workspace, browser, queue, log);
    let said = session.said("could not be written, since the library stopped answering");
    assert_eq!(said, 1, "the edit is named");
}

/// An unsaved edit's draft stays while a save of it is in flight, so a save that fails
/// leaves the edit kept for the next session whatever happens next.
#[cfg(unix)]
#[test]
fn a_draft_stays_while_its_save_is_in_flight() {
    use std::os::unix::fs::PermissionsExt;

    let root = Temp::new();
    let mut session = Session::open(&root);
    let id = session.create();
    session.sync();
    let edited = session.edit(id, "96");
    session.autosave();
    let mode = |mode| fs::set_permissions(&root.0, fs::Permissions::from_mode(mode)).unwrap();
    mode(0o555);
    session.bench.workspace.mark_saved(id);
    let Bench {
        workspace, browser, ..
    } = &mut session.bench;
    session.store.sync(workspace, browser, Pass::Files);
    session.store.sync(workspace, browser, Pass::Full);
    session.settle();
    mode(0o755);
    assert!(session.bench.workspace.get(id).unwrap().is_unsaved());
    let crashed = Temp::new();
    copy_tree(&root.0, &crashed.0);
    copy_tree(&root.local(), &crashed.local());

    let again = Session::open(&crashed);
    let id = again.only();
    assert!(again.bytes(id) == edited, "the edit is back");
}

/// eframe's store holding the texts drawbar 0.10 kept its list of This computer under:
/// its assets, its folders and its tags.
fn kept_before(assets: &str, folders: &str, tags: &str) -> crate::testing::Fake {
    let mut storage = crate::testing::Fake::default();
    for (key, text) in [
        ("drawbar.this_computer", assets),
        ("drawbar.folders", folders),
        ("drawbar.tags", tags),
    ] {
        eframe::Storage::set_string(&mut storage, key, text.to_string());
    }
    storage
}

/// The list `storage` holds.
fn older_list(storage: &crate::testing::Fake) -> older::List {
    match older::found(storage) {
        older::Found::List(list) => list,
        other => panic!("{other:?}"),
    }
}

/// A list as drawbar 0.10's `store::save`, `folders.rs` and `tags.rs` wrote it: a program
/// off slot 2:8 named Grand, two assets named Pad in the folder Gig, one with an unsaved
/// edit, an empty folder, and a tag nothing wears.
fn gig_list() -> (older::List, Vec<u8>, Vec<u8>, Vec<u8>) {
    use base64::prelude::{Engine as _, BASE64_STANDARD};

    let program = Fresh::Program.bytes().unwrap();
    let (soft, edit) = (with_gain(&program, "12"), with_gain(&program, "40"));
    let b64 = |bytes: &[u8]| BASE64_STANDARD.encode(bytes);
    let assets = format!(
        "drawbar 2\n4\n\
         1\tdevice:4:1:7\tGrand\t{program}\n\
         2\tfresh\tPad\t{soft}\t{edit}\n\
         3\tfile:Pad.ne5p\tPad\t{program}\n",
        program = b64(&program),
        soft = b64(&soft),
        edit = b64(&edit),
    );
    let storage = kept_before(
        &assets,
        "drawbar folders 1\nf\t1\tGig\nf\t2\tEmpty\nm\t2\t1\nm\t3\t1\n",
        "drawbar tags 1\nt\t1\tLive\nt\t2\tSoft\nt\t3\tUnworn\nm\t1\t1\nm\t2\t1\nm\t2\t2\n",
    );
    (older_list(&storage), program, soft, edit)
}

impl Session {
    /// Take `list` in, as a frame does, and wait until the library has answered. Returns
    /// where the take-in said it puts each asset that came with an unsaved edit.
    fn take_in(&mut self, list: &older::List) -> Vec<(usize, LibPath)> {
        let Bench {
            workspace,
            browser,
            log,
            ..
        } = &mut self.bench;
        let mut kept = Vec::new();
        self.store.take_in(list, workspace, browser, log, |placed| {
            kept = placed.to_vec()
        });
        self.decided();
        kept
    }

    /// Whether `list` is the library's to keep now, so its keys may go.
    fn handed_over(&self, list: &older::List) -> bool {
        self.store.handed_over(&list.hash, &self.bench.workspace)
    }
}

/// [`gig_list`] as a restart finds it, with the record its take-in kept, if any.
fn gig_list_again(placed: Option<Vec<(usize, LibPath)>>) -> older::List {
    let (list, ..) = gig_list();
    older::List {
        placed: placed.map(|placed| placed.into_iter().collect()),
        ..list
    }
}

/// A session that ends after the take-in lands and before it keeps its unsaved edit
/// leaves the list; the next puts the edit back over the file the take-in made, and
/// lets the list go once it keeps the edit.
#[test]
fn an_unsaved_edit_a_crash_left_unkept_is_put_back_over_its_file() {
    let root = Temp::new();
    let (list, _, soft, edit) = gig_list();
    let mut first = Session::open(&root);
    let placed = first.take_in(&list);
    assert_eq!(placed, [(1, LibPath::parse("Gig/Pad.ne5p").unwrap())]);
    assert!(first.store.has_taken_in(&list.hash));
    assert!(!first.handed_over(&list), "its edit is not kept yet");
    drop(first);

    let list = gig_list_again(Some(placed));
    let mut again = Session::open(&root);
    again.take_in(&list);
    assert_eq!(
        root.names("Gig"),
        ["Pad 2.ne5p", "Pad.ne5p"],
        "nothing made again"
    );
    assert_eq!(root.read("Gig/Pad.ne5p"), soft);
    let pad = again.named("Pad.ne5p");
    assert!(again.bench.workspace.get(pad).unwrap().is_unsaved());
    assert!(again.bytes(pad) == edit, "the edit is back over its file");
    assert!(!again.handed_over(&list), "not until the edit is kept");
    again.sync();
    assert!(again.handed_over(&list));
    drop(again);

    let last = Session::open(&root);
    let pad = last.named("Pad.ne5p");
    assert!(last.bytes(pad) == edit, "kept as any unsaved edit is");
}

/// A file the take-in made that something outside drawbar changed before its unsaved
/// edit was kept asks, as for any edit whose file changed since, rather than putting the
/// edit back as though nothing had.
#[test]
fn an_unsaved_edit_a_crash_left_unkept_over_a_file_changed_since_asks() {
    let root = Temp::new();
    let (list, program, ..) = gig_list();
    let mut first = Session::open(&root);
    let placed = first.take_in(&list);
    drop(first);
    fs::write(root.at("Gig/Pad.ne5p"), with_gain(&program, "99")).unwrap();

    let list = gig_list_again(Some(placed));
    let mut again = Session::open(&root);
    again.take_in(&list);
    let (title, _) = again.bench.browser.asking().expect("a question");
    assert_eq!(title, "“Pad.ne5p” changed on disk");
}

#[test]
fn a_draft_that_fails_keeps_the_list_until_one_lands() {
    let root = Temp::new();
    let (list, ..) = gig_list();
    let mut session = Session::open(&root);
    session.take_in(&list);
    session.store.fault(Fault::Refuse(
        |io| matches!(io, toshokan::Io::Create { path, .. } if path.as_str().starts_with("unsaved/")),
    ));
    session.sync();
    assert!(
        session.said("could not be kept for later") > 0,
        "the draft failed"
    );
    assert!(!session.handed_over(&list));
    session.sync();
    assert!(session.handed_over(&list));
}

/// Without the record of where the take-in put it, an unsaved edit is brought in as a
/// file of its own rather than lost.
#[test]
fn an_unsaved_edit_whose_file_is_not_known_comes_in_as_a_file_of_its_own() {
    let root = Temp::new();
    let (list, _, soft, edit) = gig_list();
    let mut first = Session::open(&root);
    first.take_in(&list);
    drop(first);

    let list = gig_list_again(None);
    let mut again = Session::open(&root);
    again.take_in(&list);
    again.sync();
    assert_eq!(root.names("Gig"), ["Pad 2.ne5p", "Pad 3.ne5p", "Pad.ne5p"]);
    assert_eq!(root.read("Gig/Pad.ne5p"), soft);
    assert_eq!(root.read("Gig/Pad 3.ne5p"), edit);
    assert_eq!(
        again.said("its unsaved edit is a file of its own, “Pad 3.ne5p”"),
        1
    );
    assert!(again.handed_over(&list));
}

/// The library already holds a file named Grand and a folder named Gig, so what comes in
/// is numbered past them, and the two assets named Pad past each other.
#[test]
fn the_list_an_older_drawbar_kept_comes_in_as_files_with_their_folders_tags_and_origins() {
    let root = Temp::new();
    let program = Fresh::Program.bytes().unwrap();
    fs::write(root.at("Grand.ne5p"), &program).unwrap();
    fs::create_dir(root.at("Gig")).unwrap();
    let (list, program, soft, edit) = gig_list();
    let mut session = Session::open(&root);
    session.take_in(&list);

    assert_eq!(
        root.names(""),
        [
            ".drawbar",
            "Empty",
            "Gig",
            "Gig 2",
            "Grand 2.ne5p",
            "Grand.ne5p"
        ]
    );
    assert_eq!(root.names("Gig 2"), ["Pad 2.ne5p", "Pad.ne5p"]);
    assert_eq!(root.read("Grand 2.ne5p"), program);
    assert_eq!(root.read("Gig 2/Pad.ne5p"), soft, "the saved bytes");
    assert_eq!(root.read("Gig 2/Pad 2.ne5p"), program);
    let (grand, pad) = (session.named("Grand 2.ne5p"), session.named("Pad.ne5p"));
    assert_eq!(session.worn(grand), ["Live"]);
    assert_eq!(session.worn(pad), ["Live", "Soft"]);
    assert_eq!(session.tag_names(), ["Live", "Soft", "Unworn"]);
    let at = Location { bank: 1, slot: 7 };
    let origin = &session.bench.workspace.get(grand).unwrap().origin;
    assert!(
        matches!(origin, Origin::Device { class: ObjectClass::Program, at: held } if *held == at),
        "the slot it came off"
    );
    assert!(session.bench.workspace.get(pad).unwrap().is_unsaved());
    assert!(session.bytes(pad) == edit, "the edit is held over the file");
    assert_eq!(
        session.said(
            "drawbar brought 3 sounds an earlier version kept into the library, with their \
             tags and folders."
        ),
        1
    );
    assert!(session.store.has_taken_in(&list.hash));
    assert_eq!(
        session.store.undo_label().as_deref(),
        Some("Undo Bring in kept sounds")
    );
    session.close();

    let again = Session::open(&root);
    let pad = again.named("Pad.ne5p");
    assert!(again.bench.workspace.get(pad).unwrap().is_unsaved());
    assert!(
        again.bytes(pad) == edit,
        "the edit is kept as any unsaved edit is"
    );
    assert_eq!(again.worn(pad), ["Live", "Soft"]);
}

/// Bytes that do not decode come in as they are, as drawbar keeps any file it does not
/// read: `AAEC` is the base64 of 00 01 02.
#[test]
fn bytes_that_do_not_decode_come_in_byte_for_byte() {
    let root = Temp::new();
    let storage = kept_before(
        "drawbar 2\n3\n1\tfresh\tBroken.ne5p\tAAEC\n2\tfresh\tJunk\tAAEC\n",
        "",
        "",
    );
    let mut session = Session::open(&root);
    session.take_in(&older_list(&storage));
    assert_eq!(root.read("Broken.ne5p"), [0, 1, 2]);
    assert_eq!(root.read("Junk.bin"), [0, 1, 2]);
}

#[test]
fn one_undo_takes_the_whole_take_in_back() {
    let root = Temp::new();
    let (list, ..) = gig_list();
    let mut session = Session::open(&root);
    session.take_in(&list);
    session.undo();
    session.settle();

    assert_eq!(root.names(""), [".drawbar"]);
    assert!(session.tag_names().is_empty());
    assert!(!session.store.has_taken_in(&list.hash));
    // As for any file that goes, an asset holding an unsaved edit or a slot's contents
    // stays, shown missing, until it is saved again or let go of.
    let missing: Vec<&str> = session
        .bench
        .workspace
        .listed()
        .filter(|entity| session.bench.browser.folders.missing.contains(&entity.id))
        .map(|entity| entity.name.as_str())
        .collect();
    assert_eq!(missing, ["Grand.ne5p", "Pad.ne5p"]);
    assert_eq!(session.bench.workspace.listed().count(), 2);
}

/// A crash after the take-in landed and before the keys were emptied leaves the list:
/// the library remembers it, here and in another install, and takes it in once.
#[test]
fn a_list_the_library_took_in_is_not_taken_in_again() {
    let root = Temp::new();
    let (list, ..) = gig_list();
    let mut first = Session::open(&root);
    first.take_in(&list);
    first.close();
    let names = root.names("");

    let mut again = Session::open(&root);
    assert!(again.store.has_taken_in(&list.hash));
    again.take_in(&list);
    assert_eq!(root.names(""), names);
    assert_eq!(again.said("drawbar brought"), 0);
    again.close();
    let elsewhere = Temp::new();
    let other = Session::elsewhere(&root, &elsewhere);
    assert!(other.store.has_taken_in(&list.hash), "by any install");
}

/// Two windows of one install that each listed the library before either took the list
/// in bring it in once: the second finds it taken in and puts the unsaved edit back over
/// the file the first made.
#[test]
fn two_windows_take_the_list_in_once() {
    let root = Temp::new();
    let (list, _, soft, edit) = gig_list();
    let mut first = Session::open(&root);
    let mut second = Session::open(&root);
    first.take_in(&list);
    let placed = second.take_in(&list);
    second.settle();

    assert_eq!(root.names(""), [".drawbar", "Empty", "Gig", "Grand.ne5p"]);
    assert_eq!(second.said("drawbar brought"), 0);
    assert_eq!(second.said("could not bring"), 0, "no failure");
    assert!(second.store.has_taken_in(&list.hash));
    second.take_in(&gig_list_again(Some(placed)));
    second.read_all();
    assert_eq!(
        root.names("Gig"),
        ["Pad 2.ne5p", "Pad.ne5p"],
        "no file of its own"
    );
    assert_eq!(root.read("Gig/Pad.ne5p"), soft);
    let pad = second.named("Pad.ne5p");
    assert!(second.bytes(pad) == edit, "the edit is back over its file");
    second.sync();
    assert!(second.handed_over(&list));
}

#[test]
fn a_take_in_that_writes_nothing_leaves_the_list_for_the_next_session() {
    let root = Temp::new();
    let (list, ..) = gig_list();
    let mut session = Session::open(&root);
    session.store.fault(Fault::Refuse(
        |io| matches!(io, toshokan::Io::Write { path, .. } if path.as_str().contains("/tmp/")),
    ));
    session.take_in(&list);
    session.settle();

    assert_eq!(root.names(""), [".drawbar"], "nothing was written");
    assert!(!session.store.has_taken_in(&list.hash));
    assert_eq!(session.bench.workspace.listed().count(), 0);
    assert!(session.bench.browser.folders.all().is_empty());
    assert_eq!(
        session.said("drawbar could not bring the sounds an earlier version kept"),
        1
    );
    assert_eq!(
        session.said("did not change as asked"),
        0,
        "said once, for the whole"
    );
    assert_eq!(session.said("was not saved"), 0, "said once, for the whole");
    session.take_in(&list);
    assert_eq!(root.names(""), [".drawbar"], "tried once a session");
    session.close();

    let mut next = Session::open(&root);
    next.take_in(&list);
    assert!(next.store.has_taken_in(&list.hash));
    assert_eq!(root.names(""), [".drawbar", "Empty", "Gig", "Grand.ne5p"]);
}

#[test]
fn a_read_only_library_takes_nothing_in() {
    let (list, ..) = gig_list();
    let (root, _) = newer_drawbar_wrote();
    let mut session = Session::open(&root);
    assert!(session.store.read_only().is_some());
    let names = root.names("");
    session.take_in(&list);
    assert_eq!(root.names(""), names);
    assert!(!session.store.has_taken_in(&list.hash));
}

/// A dismissal is final: undo after it takes back the change before it, and the run
/// stays dismissed.
#[test]
fn undo_after_a_dismissal_takes_back_the_change_before_it() {
    let root = Temp::new();
    let (mut here, _there, at_here, _, _copy) = forked(&root);
    let theirs = here.kept("gains the tag “There”");
    here.take_kept(theirs.run, Vec::new());
    assert_eq!(here.worn(at_here), ["New", "There"]);
    let ours = here.kept("gains the tag “Here”");
    let taken = here.store.undo_label();
    here.dismiss_kept(ours.run);
    assert_eq!(
        here.store.undo_label(),
        taken,
        "the Edit menu offers the take"
    );
    here.undo();
    assert_eq!(here.worn(at_here), ["New"], "the take is undone");
    let kept = &here.bench.browser.folders.kept;
    assert!(
        !kept.iter().any(|kept| kept.run == ours.run),
        "still dismissed"
    );
}

/// A change whose answers are lost leaves the Edit menu offering nothing it holds, so
/// it never names a step the library may not have taken; the next change is offered,
/// and undo takes it back.
#[test]
fn a_change_whose_answers_are_lost_leaves_undo_to_the_next_change() {
    let root = Temp::new();
    let mut session = Session::open(&root);
    let id = session.create();
    session.sync();
    session.tag(&[id], "A");
    assert!(session.store.undo_label().is_some());
    session.store.fault(Fault::Unread);
    session.tag(&[id], "B");
    session.settle();
    assert_eq!(
        session.store.undo_label(),
        None,
        "no step it may not have taken"
    );
    session.tag(&[id], "C");
    assert!(session
        .store
        .undo_label()
        .is_some_and(|label| label.contains("“C”")));
    session.undo();
    assert_eq!(session.worn(id), ["A", "B"], "the last change is undone");
}

/// A move whose answers are lost is not said to have failed: a look at the folder finds
/// the file where the move put it.
#[test]
fn a_move_whose_answers_are_lost_is_found_where_it_went() {
    let root = Temp::new();
    fs::write(root.at("Grand.ne5p"), Fresh::Program.bytes().unwrap()).unwrap();
    let mut session = Session::open(&root);
    let grand = session.named("Grand.ne5p");
    session.store.fault(Fault::Unread);
    session.bench.act(vec![crate::browser::Act::RenameLocal {
        id: grand,
        name: "Upright".to_string(),
    }]);
    session.sync();
    session.settle();
    assert_eq!(session.said("did not change as asked"), 0);
    assert!(!root.at("Grand.ne5p").exists() && root.at("Upright.ne5p").exists());
    session.named("Upright.ne5p");
}

/// A save that landed and whose answers are lost ends saved once the look finds the
/// file holds it, with no question of which to keep.
#[test]
fn a_landed_save_whose_answers_are_lost_ends_saved_without_asking() {
    let root = Temp::new();
    let mut session = Session::open(&root);
    let id = session.create();
    session.sync();
    let edited = session.edit(id, "96");
    session.store.fault(Fault::Unread);
    session.bench.workspace.mark_saved(id);
    session.sync();
    session.settle();
    assert!(root.read("untitled.ne5p") == edited, "the save landed");
    assert!(!session.bench.workspace.get(id).unwrap().is_unsaved());
    assert_eq!(
        session.bench.browser.asking(),
        None,
        "nothing to choose between"
    );
    assert_eq!(session.said("was not saved"), 0);
}

/// An unsaved edit kept by its file's path, once the file becomes an entity, is kept
/// under the entity before the draft under the path goes: a library that stops right
/// after keeps it.
#[test]
fn an_edit_kept_under_a_new_key_is_kept_before_the_old_draft_goes() {
    let root = Temp::new();
    fs::write(root.at("Grand.ne5p"), Fresh::Program.bytes().unwrap()).unwrap();
    let mut session = Session::open(&root);
    let grand = session.named("Grand.ne5p");
    let edited = session.edit(grand, "96");
    session.autosave();
    session.tag(&[grand], "Live");
    session.store.fault(Fault::Next(Box::new(Fault::Trap)));
    let Bench {
        workspace, browser, ..
    } = &mut session.bench;
    session.store.sync(workspace, browser, Pass::Full);
    for _ in 0..200 {
        if session.store.stopped().is_some() {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    let crashed = Temp::new();
    copy_tree(&root.0, &crashed.0);
    copy_tree(&root.local(), &crashed.local());

    let again = Session::open(&crashed);
    let grand = again.named("Grand.ne5p");
    assert!(again.bytes(grand) == edited, "the edit is back");
}
