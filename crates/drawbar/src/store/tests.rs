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
            let Bench {
                workspace,
                browser,
                log,
                ..
            } = &mut self.bench;
            self.store.rescue(rescue, what, workspace, browser, log);
        }
        self.sync();
    }
}

/// A rescue still being written when drawbar stopped is never the slot's only copy, since
/// the slot is not touched until it is whole. It is not offered, and is deleted, in the
/// library and in drawbar's own data alike.
#[test]
fn a_partial_rescue_file_is_never_offered() {
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

    assert_eq!(session.bench.browser.asking(), None, "nothing is offered");
    assert_eq!(names_at(&rescued(&root)), Vec::<String>::new());
    assert_eq!(shelf.names(""), Vec::<String>::new());
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

/// Keep in library copies a rescue into the library's top level, where it is an asset
/// like any other, and deletes it where it was.
#[test]
fn a_rescue_kept_moves_into_the_library_as_an_asset() {
    let root = Temp::new();
    with_rescue(&root);

    let mut second = Session::open(&root);
    second.choose("Keep in library");
    assert_eq!(names_at(&rescued(&root)), Vec::<String>::new());
    assert_eq!(root.read("nord-rescued-1-4.npno"), a_rescue());
    let names: Vec<&str> = second
        .bench
        .workspace
        .listed()
        .map(|entity| entity.name.as_str())
        .collect();
    assert!(names.contains(&"nord-rescued-1-4.npno"), "{names:?}");

    let third = Session::open(&root);
    assert_eq!(
        third.bench.browser.asking(),
        None,
        "nothing is left to offer"
    );
    let kept = third.named("nord-rescued-1-4.npno");
    let origin = &third.bench.workspace.get(kept).unwrap().origin;
    assert!(
        matches!(origin, Origin::Rescued { at } if *at == Location::from_user(1, 4)),
        "{}",
        origin.label()
    );
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

/// Discard asks first, and deletes the rescue only once that is answered.
#[test]
fn a_rescue_discarded_is_deleted_once_confirmed() {
    let root = Temp::new();
    with_rescue(&root);

    let mut second = Session::open(&root);
    second.choose("Discard…");
    let (title, _) = second.bench.browser.asking().expect("asked again");
    assert_eq!(title, "Discard “nord-rescued-1-4.npno”?");
    assert_eq!(names_at(&rescued(&root)), ["nord-rescued-1-4.npno"]);
    second.choose("Discard");
    assert_eq!(names_at(&rescued(&root)), Vec::<String>::new());
    assert!(!root
        .names("")
        .contains(&"nord-rescued-1-4.npno".to_string()));
}

/// A rescue in drawbar's own data, where no library could take it, is offered with the
/// library's own, and Keep in library copies it in and deletes it there; Discard deletes
/// it.
#[test]
fn a_rescue_in_drawbars_own_data_is_kept_or_discarded() {
    let (root, shelf) = (Temp::new(), Temp::new());
    for name in ["nord-rescued-1-4.npno", "nord-rescued-2-1.nsmp"] {
        fs::write(shelf.at(name), a_rescue()).unwrap();
    }
    let bench = Bench::new();
    let store = Store::start(crate::testing::backend(&bench.ctx, &root));
    let mut session = Session {
        store: store.shelving(Some(shelf.0.clone())),
        bench,
    };
    session.opened();

    let (title, _) = session.bench.browser.asking().expect("asked");
    assert!(title.contains("nord-rescued-1-4.npno"), "{title}");
    session.choose("Keep in library");
    assert_eq!(root.read("nord-rescued-1-4.npno"), a_rescue());
    assert_eq!(shelf.names(""), ["nord-rescued-2-1.nsmp"]);

    let (title, _) = session.bench.browser.asking().expect("asked");
    assert!(title.contains("nord-rescued-2-1.nsmp"), "{title}");
    session.choose("Discard…");
    session.choose("Discard");
    assert_eq!(shelf.names(""), Vec::<String>::new());
    assert!(!root
        .names("")
        .contains(&"nord-rescued-2-1.nsmp".to_string()));
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
        words.iter().any(|word| word == "npno 0x464"),
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
        let backend = crate::testing::backend(&bench.ctx, root);
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
    let crc32 = read.saved.crc32().expect("a program is a container");
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
        entity.verify().note(),
        None,
        "it does not say it is being read"
    );
    assert_eq!(second.bench.workspace.families_present(), families);
    assert_eq!(entity.saved.crc32(), Some(crc32));

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
    let store = Store::start(crate::testing::backend(&bench.ctx, &root)).remembering(cache);
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
    let store = Store::start(crate::testing::backend(&bench.ctx, &root)).remembering(cache);
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
        assert_eq!(entity.verify().note(), Some("reading…"));
        assert_eq!(entity.saved.crc32(), None);
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
    first.tag(&[id], "Sunday");
    first.read_all();
    first.close();
    fs::rename(root.at("Grand.ne5p"), root.at("Upright.ne5p")).unwrap();

    let second = Session::remembering(&root, &shelf);
    let id = second.only();
    assert_eq!(second.worn(id), ["Sunday"], "the same asset");
    let entity = second.bench.workspace.get(id).unwrap();
    assert_eq!(entity.name, "Upright.ne5p");
    assert!(
        entity.unread() && !entity.reading(),
        "remembered at its new path"
    );
    assert_eq!(second.reads(), 0);
}

/// A file rewritten under the same length and time is remembered as it was, but a
/// read takes it as it is, and the cache then holds its new summary.
#[test]
fn a_read_corrects_what_a_file_rewritten_under_its_old_stat_was_remembered_as() {
    let (root, shelf) = (Temp::new(), Temp::new());
    let program = Fresh::Program.bytes().unwrap();
    let (old, new) = (with_gain(&program, "12"), with_gain(&program, "96"));
    assert_eq!(old.len(), new.len());
    fs::write(root.at("Grand.ne5p"), &old).unwrap();
    let mut first = Session::remembering(&root, &shelf);
    let id = first.only();
    first.read_all();
    let was = first.bench.workspace.get(id).unwrap().saved.crc32();
    first.close();

    let at = fs::metadata(root.at("Grand.ne5p"))
        .unwrap()
        .modified()
        .unwrap();
    fs::write(root.at("Grand.ne5p"), &new).unwrap();
    fs::File::options()
        .write(true)
        .open(root.at("Grand.ne5p"))
        .and_then(|file| file.set_modified(at))
        .unwrap();
    let mut second = Session::remembering(&root, &shelf);
    let id = second.only();
    assert_eq!(second.bench.workspace.get(id).unwrap().saved.crc32(), was);
    second.read(&[id]);
    let now = second.bench.workspace.get(id).unwrap().saved.crc32();
    assert_ne!(now, was);
    second.close();

    let third = Session::remembering(&root, &shelf);
    let id = third.only();
    assert_eq!(third.bench.workspace.get(id).unwrap().saved.crc32(), now);
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
    let crc32 = first
        .bench
        .workspace
        .get(first.only())
        .unwrap()
        .saved
        .crc32();
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
    assert_eq!(entity.saved.crc32(), crc32);
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

/// The default library is a folder of drawbar's own data, and the cache sits in that
/// data beside it, not inside it, so the cache is kept between sessions.
#[test]
fn the_cache_beside_a_library_in_the_same_data_is_kept() {
    let data = Temp::new();
    fs::create_dir_all(data.at("library")).unwrap();
    let root = Temp(data.at("library"));
    fs::write(root.at("Grand.ne5p"), Fresh::Program.bytes().unwrap()).unwrap();
    let mut first = Session::kept_at(&root, data.at(CACHE));
    first.read_all();
    first.close();
    assert!(data.at(CACHE).is_file());
    assert_eq!(root.names(""), ["Grand.ne5p"]);

    let second = Session::kept_at(&root, data.at(CACHE));
    let entity = second.bench.workspace.get(second.only()).unwrap();
    assert!(entity.unread() && !entity.reading(), "remembered");
}

/// A tracked file whose summary is remembered is not read in the background: what it is
/// and the slot it matches are known without it.
#[test]
fn a_remembered_tracked_file_is_not_read_in_the_background() {
    let (root, shelf) = (Temp::new(), Temp::new());
    fs::write(root.at("Tagged.ne5p"), Fresh::Program.bytes().unwrap()).unwrap();
    let mut first = Session::remembering(&root, &shelf);
    let tagged = first.only();
    first.tag(&[tagged], "Sunday");
    first.read_all();
    first.close();

    let second = Session::remembering(&root, &shelf);
    let tagged = second.only();
    let entity = second.bench.workspace.get(tagged).unwrap();
    assert_eq!(second.worn(tagged), ["Sunday"]);
    assert!(entity.unread() && !entity.reading());
    assert!(entity.saved.crc32().is_some(), "it can match its slot");
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
    assert_eq!(
        root.names(""),
        [".drawbar", "Zoned.nsmp"],
        "no copy is left"
    );
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
/// what is there, is copied over that file, which keeps its asset; its own file goes.
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
    assert!(root.at("Moved.nsmp").exists(), "kept until the copy lands");
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
        result: Err("taken".into()),
    };
    assert_eq!(moved.faults(), ["move"].into());
    let refused = Event::Committed {
        made: None,
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

/// Each answer is taken in by what it changed, and what drawbar lists after it is what
/// the whole view lists, and what a fresh open of the library shows.
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
        assert!(session.store.lists_whole(), "after {what}");
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

/// A library folder restored from a backup wins: what this computer showed of changes
/// the backup does not hold is let go at the next open, which says so once.
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

    let third = Session::listed(&root);
    assert_eq!(tagged(&third), Vec::<(String, Vec<String>)>::new());
    assert_eq!(third.said("no longer holds"), 1);
    third.close();
    let fourth = Session::listed(&root);
    assert_eq!(fourth.said("no longer holds"), 0, "said once");
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

/// Two windows of this install keep their unsaved edits as two writers' drafts. Opened
/// again with one window, both edits come back.
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
/// file moves into a writer's trash.
struct Crashing {
    native: toshokan::blocking::Native,
    stopped: bool,
}

impl toshokan::blocking::Backend for Crashing {
    fn capabilities(&self, root: toshokan::Root) -> toshokan::io::Capabilities {
        self.native.capabilities(root)
    }

    fn perform(&mut self, io: toshokan::Io) -> toshokan::IoResult {
        assert!(!self.stopped, "crashed");
        let stops =
            matches!(&io, toshokan::Io::Rename { to, .. } if to.as_str().contains("/trash/"));
        let done = self.native.perform(io);
        self.stopped = stops;
        done
    }
}

/// A save another install's drawbar left unfinished, as on a computer that syncs the
/// folder, is not settled behind the user's back: drawbar asks, and Finish completes
/// it.
#[test]
fn a_change_another_install_left_unfinished_is_asked_about() {
    use std::panic::AssertUnwindSafe;
    use toshokan::blocking::{Library, Native};

    let root = Temp::new();
    let program = Fresh::Program.bytes().unwrap();
    fs::write(root.at("Grand.ne5p"), &program).unwrap();
    let elsewhere = Temp::new();
    let crashing = Crashing {
        native: Native::new(&root.0, &elsewhere.0),
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
    let saved = with_gain(&program, "77");
    let at = toshokan::RelPath::new("Grand.ne5p").unwrap();
    let expect =
        toshokan::Expect::Holds(keys::identity_of_bytes(&path("Grand.ne5p"), &program).unwrap());
    let bytes = saved.clone();
    let stopped = std::panic::catch_unwind(AssertUnwindSafe(move || {
        let _ = other.intent("Save").save_file(&at, bytes, expect).commit();
    }));
    assert!(stopped.is_err(), "the other install stopped mid-save");

    let mut session = Session::listed(&root);
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
    let writers = root.at(".drawbar/library/writers");
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

/// What another install wrote and this computer showed is kept while the folder lacks
/// that drawbar's history entirely, as while a sync has not brought it yet: only a
/// folder that holds the history without those changes was restored.
#[test]
fn changes_a_sync_has_not_brought_yet_are_not_let_go() {
    let root = Temp::new();
    fs::write(root.at("Grand.ne5p"), Fresh::Program.bytes().unwrap()).unwrap();
    let other = Temp::new();
    let mut theirs = Session::elsewhere(&root, &other);
    let grand = theirs.named("Grand.ne5p");
    theirs.tag(&[grand], "Sunday");
    theirs.close();
    let mut ours = Session::listed(&root);
    ours.refresh();
    assert_eq!(tagged(&ours), [wears("Grand.ne5p", &["Sunday"])]);
    ours.make_tag("Spare");
    ours.close();
    // Their writer's id is in their install's own state.
    let genesis = fs::read_dir(&other.0)
        .unwrap()
        .next()
        .unwrap()
        .unwrap()
        .path();
    let head = fs::read_to_string(genesis.join("head.json")).unwrap();
    let id = head.split('"').nth(3).unwrap().to_string();
    let held = Temp::new();
    let theirs_dir = root.at(&format!(".drawbar/library/writers/{id}"));
    fs::rename(&theirs_dir, held.0.join(&id)).unwrap();

    let ours = Session::listed(&root);
    assert_eq!(ours.said("no longer holds"), 0, "nothing is let go");
    assert_eq!(tagged(&ours), [wears("Grand.ne5p", &["Sunday"])]);
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
