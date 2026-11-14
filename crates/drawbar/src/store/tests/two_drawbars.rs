//! Two drawbars on one synced folder, each with its own local state, where one changes a
//! file the other has seen.

use super::*;
use crate::browser::Act;

/// A library holding `Strings.ne5p`, open in this drawbar and in another with its own
/// local state at `theirs`.
fn strings(theirs: &Temp) -> (Temp, Session, Session) {
    let root = Temp::new();
    fs::write(root.at("Strings.ne5p"), Fresh::Program.bytes().unwrap()).unwrap();
    let here = Session::open(&root);
    let there = Session::elsewhere(&root, theirs);
    (root, here, there)
}

#[test]
fn a_tagged_file_another_drawbar_deletes_goes_at_a_refresh() {
    let theirs = Temp::new();
    let (_root, mut here, mut there) = strings(&theirs);
    let file = there.named("Strings.ne5p");
    there.tag(&[file], "Live");
    here.refresh();
    let file = here.named("Strings.ne5p");
    here.bench.act(vec![Act::Remove(file)]);
    here.sync();

    there.refresh();
    assert_eq!(
        there.bench.workspace.listed().count(),
        0,
        "{}",
        there.bench.log.transcript()
    );
    assert!(there.bench.browser.folders.missing.is_empty());
    assert_eq!(there.said("missing from the library folder"), 0);
    assert_eq!(there.said("outside drawbar"), 0);
    assert_eq!(there.said("Another drawbar deleted “Strings.ne5p”."), 1);
}

#[test]
fn an_unsaved_edit_outlasts_another_drawbars_delete_and_a_save_writes_it_back() {
    let theirs = Temp::new();
    let (root, mut here, mut there) = strings(&theirs);
    let file = there.named("Strings.ne5p");
    there.tag(&[file], "Live");
    there.read_all();
    let edited = there.edit(file, "74");
    here.refresh();
    let gone = here.named("Strings.ne5p");
    here.bench.act(vec![Act::Remove(gone)]);
    here.sync();

    there.refresh();
    assert_eq!(there.bytes(file), edited);
    assert!(there.bench.browser.folders.missing.contains(&file));
    assert_eq!(
        there.said("Another drawbar deleted “Strings.ne5p”."),
        1,
        "{}",
        there.bench.log.transcript()
    );
    there.bench.act(vec![Act::SaveDoc(file)]);
    there.sync();
    assert_eq!(root.read("Strings.ne5p"), edited);
}

#[test]
fn a_tagged_file_another_drawbar_moves_follows_quietly_at_a_refresh() {
    let theirs = Temp::new();
    let (_root, mut here, mut there) = strings(&theirs);
    let file = there.named("Strings.ne5p");
    there.tag(&[file], "Live");
    here.refresh();
    let moving = here.named("Strings.ne5p");
    here.place_at(moving, "Sound.ne5p");
    here.sync();

    there.refresh();
    assert_eq!(there.path(file).as_deref(), Some("Sound.ne5p"));
    assert_eq!(there.worn(file), ["Live"]);
    assert_eq!(
        there.said("outside drawbar"),
        0,
        "{}",
        there.bench.log.transcript()
    );
}

/// As [`strings`], with `Bass.ne5p` beside it, which this drawbar has renamed to
/// `Low.ne5p`.
fn strings_after_a_rename(theirs: &Temp) -> (Temp, Session, Session) {
    let root = Temp::new();
    fs::write(root.at("Strings.ne5p"), Fresh::Program.bytes().unwrap()).unwrap();
    let bass = with_gain(&Fresh::Program.bytes().unwrap(), "12");
    fs::write(root.at("Bass.ne5p"), bass).unwrap();
    let mut here = Session::open(&root);
    let there = Session::elsewhere(&root, theirs);
    let bass = here.named("Bass.ne5p");
    here.place_at(bass, "Low.ne5p");
    here.sync();
    (root, here, there)
}

/// Undo, which another drawbar's change since refuses: one line says so, and the next
/// undo reaches the change before.
fn undo_refused_as_changed_since(here: &mut Session) {
    let refused = here.store.undo_label().expect("a change to undo");
    here.undo();
    here.sync();
    let change = refused.trim_start_matches("Undo ");
    let told = format!("{change} cannot be undone: another drawbar has changed it since.");
    let transcript = here.bench.log.transcript();
    assert_eq!(here.said(&told), 1, "{transcript}");
    assert_eq!(here.said("has changed it since"), 1, "{transcript}");
    assert_eq!(here.said("did not change as asked"), 0, "{transcript}");
    assert_eq!(
        here.store.undo_label().as_deref(),
        Some("Undo Rename “Bass.ne5p”")
    );
}

#[test]
fn a_save_undone_after_another_drawbar_renamed_its_file_is_refused_as_changed_since() {
    let theirs = Temp::new();
    let (root, mut here, mut there) = strings_after_a_rename(&theirs);
    let file = here.named("Strings.ne5p");
    here.edit(file, "74");
    here.bench.act(vec![Act::SaveDoc(file)]);
    here.sync();
    let saved = root.read("Strings.ne5p");
    there.refocus();
    let file = there.named("Strings.ne5p");
    let name = "Sound".to_string();
    there.bench.act(vec![Act::RenameLocal { id: file, name }]);
    there.sync();

    here.refocus();
    undo_refused_as_changed_since(&mut here);
    assert_eq!(root.names(""), [".drawbar", "Low.ne5p", "Sound.ne5p"]);
    assert_eq!(root.read("Sound.ne5p"), saved, "the save stands");
    here.undo();
    here.sync();
    assert_eq!(root.names(""), [".drawbar", "Bass.ne5p", "Sound.ne5p"]);
}

/// An undo of a save whose file changed outside drawbar since, as a save another drawbar
/// made whose word has not arrived, is refused, and stays for a later try.
#[test]
fn a_save_undone_after_its_file_changed_outside_is_refused_and_stays() {
    let theirs = Temp::new();
    let (root, mut here, _there) = strings_after_a_rename(&theirs);
    let file = here.named("Strings.ne5p");
    here.edit(file, "74");
    here.bench.act(vec![Act::SaveDoc(file)]);
    here.sync();
    let outside = with_gain(&Fresh::Program.bytes().unwrap(), "99");
    fs::write(root.at("Strings.ne5p"), &outside).unwrap();
    here.refocus();

    here.undo();
    here.sync();
    let transcript = here.bench.log.transcript();
    let told = "The library did not change as asked: “Strings.ne5p” is not as drawbar last \
                saw it.";
    assert_eq!(here.said(told), 1, "{transcript}");
    assert_eq!(root.read("Strings.ne5p"), outside, "the change stands");
    assert_eq!(
        here.store.undo_label().as_deref(),
        Some("Undo Save “Strings.ne5p”")
    );
}

#[test]
fn a_rename_undone_after_another_drawbar_tagged_the_file_is_refused_as_changed_since() {
    let theirs = Temp::new();
    let (root, mut here, mut there) = strings_after_a_rename(&theirs);
    let file = here.named("Strings.ne5p");
    here.place_at(file, "Sound.ne5p");
    here.sync();
    there.refocus();
    let file = there.named("Sound.ne5p");
    there.tag(&[file], "Live");

    here.refocus();
    undo_refused_as_changed_since(&mut here);
    assert_eq!(root.names(""), [".drawbar", "Low.ne5p", "Sound.ne5p"]);
    assert_eq!(here.worn(here.named("Sound.ne5p")), ["Live"]);
    here.undo();
    here.sync();
    assert_eq!(root.names(""), [".drawbar", "Bass.ne5p", "Sound.ne5p"]);
}

#[test]
fn another_drawbars_rename_of_a_file_nothing_tags_shows_at_a_refresh() {
    let theirs = Temp::new();
    let (_root, mut here, mut there) = strings(&theirs);
    let file = there.named("Strings.ne5p");
    let name = "Sound".to_string();
    there.bench.act(vec![Act::RenameLocal { id: file, name }]);
    there.sync();

    here.refresh();
    let shown: Vec<Option<String>> = (here.bench.workspace.listed())
        .map(|entity| here.path(entity.id))
        .collect();
    assert_eq!(shown, [Some("Sound.ne5p".to_string())]);
}

#[test]
fn a_file_nothing_tags_another_drawbar_deletes_goes_at_a_refresh_without_a_word() {
    let theirs = Temp::new();
    let (_root, mut here, mut there) = strings(&theirs);
    let file = there.named("Strings.ne5p");
    there.bench.act(vec![Act::Remove(file)]);
    there.sync();
    let said = here.bench.log.iter().count();

    here.refresh();
    let transcript = here.bench.log.transcript();
    assert_eq!(here.bench.workspace.listed().count(), 0, "{transcript}");
    assert_eq!(here.bench.log.iter().count(), said, "{transcript}");

    there.undo();
    there.sync();
    let said = here.bench.log.iter().count();
    here.refresh();
    let transcript = here.bench.log.transcript();
    assert_eq!(here.bench.workspace.listed().count(), 1, "{transcript}");
    assert_eq!(here.bench.log.iter().count(), said, "{transcript}");
}

#[test]
fn an_unsaved_edit_of_a_file_another_drawbar_deletes_is_kept_as_missing() {
    let theirs = Temp::new();
    let (_root, mut here, mut there) = strings(&theirs);
    let file = here.named("Strings.ne5p");
    let edited = here.edit(file, "74");
    let gone = there.named("Strings.ne5p");
    there.bench.act(vec![Act::Remove(gone)]);
    there.sync();

    here.refresh();
    let transcript = here.bench.log.transcript();
    assert_eq!(here.bytes(file), edited, "{transcript}");
    assert!(here.bench.browser.folders.missing.contains(&file));
    let kept = "“Strings.ne5p” is no longer in the library folder; drawbar still holds your \
                changes.";
    assert_eq!(here.said(kept), 1, "{transcript}");
    assert_eq!(here.said("outside drawbar"), 0, "{transcript}");
}

#[test]
fn a_file_and_a_folder_another_drawbar_renames_show_at_a_refresh_without_a_word() {
    let theirs = Temp::new();
    let root = Temp::new();
    let program = Fresh::Program.bytes().unwrap();
    fs::write(root.at("Strings.ne5p"), &program).unwrap();
    fs::create_dir(root.at("Gig")).unwrap();
    fs::write(root.at("Gig/Bass.ne5p"), with_gain(&program, "12")).unwrap();
    let mut here = Session::open(&root);
    let mut there = Session::elsewhere(&root, &theirs);
    let held = [here.named("Strings.ne5p"), here.named("Bass.ne5p")];
    let file = there.named("Strings.ne5p");
    let name = "Sound".to_string();
    there.bench.act(vec![Act::RenameLocal { id: file, name }]);
    let gig = LibPath::parse("Gig").unwrap();
    let folder = there.bench.browser.folders.id_of(&gig).expect("listed");
    there.rename_folder(folder, "Set");
    there.sync();
    let said = here.bench.log.iter().count();

    here.refresh();
    let shown: Vec<Option<String>> = (here.bench.workspace.listed())
        .map(|entity| here.path(entity.id))
        .collect();
    let moved = ["Set/Bass.ne5p", "Sound.ne5p"].map(|path| Some(path.to_string()));
    assert_eq!(shown, moved);
    assert!(
        held.iter()
            .all(|id| here.bench.workspace.get(*id).is_none()),
        "a file nothing tags, renamed elsewhere, is a new file"
    );
    let transcript = here.bench.log.transcript();
    assert_eq!(here.bench.log.iter().count(), said, "{transcript}");
}

#[test]
fn a_file_another_drawbar_tags_then_renames_wears_the_tag_where_it_went_with_no_outside_line() {
    let theirs = Temp::new();
    let (_root, mut here, mut there) = strings(&theirs);
    let file = there.named("Strings.ne5p");
    there.tag(&[file], "Live");
    let name = "Sound".to_string();
    there.bench.act(vec![Act::RenameLocal { id: file, name }]);
    there.sync();

    here.refresh();
    let transcript = here.bench.log.transcript();
    let held = here.named("Sound.ne5p");
    assert_eq!(here.worn(held), ["Live"], "{transcript}");
    assert_eq!(here.bench.workspace.listed().count(), 1, "{transcript}");
    assert_eq!(here.said("outside drawbar"), 0, "{transcript}");
    assert_eq!(here.said("appeared"), 0, "{transcript}");
}

#[test]
fn a_file_another_drawbar_tags_then_deletes_unseen_goes_without_a_word() {
    let theirs = Temp::new();
    let (_root, mut here, mut there) = strings(&theirs);
    let file = there.named("Strings.ne5p");
    there.tag(&[file], "Live");
    there.bench.act(vec![Act::Remove(file)]);
    there.sync();
    let said = here.bench.log.iter().count();

    here.refresh();
    let transcript = here.bench.log.transcript();
    assert_eq!(here.bench.workspace.listed().count(), 0, "{transcript}");
    assert_eq!(here.bench.log.iter().count(), said, "{transcript}");
}

#[test]
fn a_file_nothing_tags_another_drawbar_renames_then_deletes_goes_at_a_refresh_without_a_word() {
    let theirs = Temp::new();
    let (_root, mut here, mut there) = strings(&theirs);
    here.refresh();
    let file = there.named("Strings.ne5p");
    let name = "Sound".to_string();
    there.bench.act(vec![Act::RenameLocal { id: file, name }]);
    there.sync();
    there.bench.act(vec![Act::Remove(file)]);
    there.sync();
    let said = here.bench.log.iter().count();

    here.refresh();
    let transcript = here.bench.log.transcript();
    assert_eq!(here.bench.workspace.listed().count(), 0, "{transcript}");
    assert_eq!(here.bench.log.iter().count(), said, "{transcript}");
}

#[test]
fn a_file_another_drawbar_brings_back_and_then_moves_arrives_as_theirs() {
    let root = Temp::new();
    fs::create_dir(root.at("Live")).unwrap();
    fs::write(root.at("Live/Grand.ne5p"), Fresh::Program.bytes().unwrap()).unwrap();
    let theirs = Temp::new();
    let mut here = Session::open(&root);
    let mut there = Session::elsewhere(&root, &theirs);
    let file = there.named("Grand.ne5p");
    there.bench.act(vec![Act::Remove(file)]);
    there.sync();
    here.refresh();
    there.undo();
    let live = folder_at(&there, "Live");
    there.bench.act(vec![Act::RemoveFolder(live)]);
    there.sync();

    here.refresh();
    assert_eq!(here.said("appeared"), 0, "{}", here.bench.log.transcript());
    assert!(here.path(here.named("Grand.ne5p")).is_some());
}

#[test]
fn a_file_another_drawbar_deletes_before_renaming_its_folder_goes_at_a_refresh_without_a_word() {
    let root = Temp::new();
    fs::create_dir(root.at("Live")).unwrap();
    fs::write(root.at("Live/Grand.ne5p"), Fresh::Program.bytes().unwrap()).unwrap();
    let theirs = Temp::new();
    let mut here = Session::open(&root);
    let mut there = Session::elsewhere(&root, &theirs);
    let file = there.named("Grand.ne5p");
    there.bench.act(vec![Act::Remove(file)]);
    there.sync();
    let live = folder_at(&there, "Live");
    let name = "Set".to_string();
    there.bench.act(vec![Act::RenameFolder { id: live, name }]);
    there.sync();
    let said = here.bench.log.iter().count();

    here.refresh();
    let transcript = here.bench.log.transcript();
    assert_eq!(here.bench.workspace.listed().count(), 0, "{transcript}");
    assert_eq!(here.bench.log.iter().count(), said, "{transcript}");
}

/// A library holding `Strings.ne5p` and `Bass.ne5p`, open here and in another drawbar
/// with its own local state at `theirs`, which tagged `Strings.ne5p`.
fn strings_and_bass(theirs: &Temp) -> (Temp, Session, Session) {
    let root = Temp::new();
    let program = Fresh::Program.bytes().unwrap();
    fs::write(root.at("Strings.ne5p"), &program).unwrap();
    fs::write(root.at("Bass.ne5p"), with_gain(&program, "12")).unwrap();
    let mut here = Session::open(&root);
    let mut there = Session::elsewhere(&root, theirs);
    let strings = there.named("Strings.ne5p");
    there.tag(&[strings], "Live");
    here.refresh();
    (root, here, there)
}

/// Another drawbar deletes `Bass.ne5p` and renames the tagged `Strings.ne5p` to take its
/// name, so the row of `Bass.ne5p` merges into the tagged file's at the next look.
fn merge_bass(there: &mut Session) {
    there.refresh();
    let bass = there.named("Bass.ne5p");
    there.bench.act(vec![Act::Remove(bass)]);
    there.sync();
    let strings = there.named("Strings.ne5p");
    let name = "Bass".to_string();
    there
        .bench
        .act(vec![Act::RenameLocal { id: strings, name }]);
    there.sync();
}

/// Whether `session` holds `bytes`, or a file of the library does.
fn kept(root: &Temp, session: &Session, bytes: &[u8]) -> bool {
    let mut listed = session.bench.workspace.listed();
    listed.any(|entity| *entity.bytes == *bytes) || holding(root, bytes) > 0
}

#[test]
fn a_save_in_flight_over_a_row_that_merges_keeps_its_edit() {
    let theirs = Temp::new();
    let (root, mut here, mut there) = strings_and_bass(&theirs);
    merge_bass(&mut there);
    let bass = here.named("Bass.ne5p");
    let edited = here.edit(bass, "74");
    here.bench.act(vec![Act::SaveDoc(bass)]);
    let Bench {
        workspace, browser, ..
    } = &mut here.bench;
    here.store.sync(workspace, browser, Pass::Files);
    while here.store.deciding() {
        assert!(here.next(), "answered");
    }
    here.settle();
    let transcript = here.bench.log.transcript();
    assert!(kept(&root, &here, &edited), "{transcript}");
}

#[test]
fn a_save_at_quit_over_a_row_a_rescan_in_flight_merges_keeps_its_edit() {
    let theirs = Temp::new();
    let (root, mut here, mut there) = strings_and_bass(&theirs);
    merge_bass(&mut there);
    let bass = here.named("Bass.ne5p");
    let edited = here.edit(bass, "74");
    here.store.rescan();
    here.bench.act(vec![Act::SaveDoc(bass)]);
    let Bench {
        workspace, browser, ..
    } = &mut here.bench;
    here.store.sync(workspace, browser, Pass::Last);
    while here.store.scanning() || here.store.deciding() || here.store.writing() {
        assert!(here.next(), "answered");
    }
    let transcript = here.bench.log.transcript();
    assert!(kept(&root, &here, &edited), "{transcript}");
}

#[test]
fn a_draft_waiting_for_its_read_outlasts_a_merge_of_its_row() {
    let theirs = Temp::new();
    let (root, mut first, mut there) = strings_and_bass(&theirs);
    let bass = first.named("Bass.ne5p");
    let edited = first.edit(bass, "74");
    first.close();

    let mut second = Session::listed(&root);
    merge_bass(&mut there);
    second.refresh();
    second.read_all();
    second.autosave();
    second.close();
    let third = Session::open(&root);
    let transcript = third.bench.log.transcript();
    let mut listed = third.bench.workspace.listed();
    assert!(
        listed.any(|entity| *entity.bytes == edited[..]),
        "{transcript}"
    );
}

/// An undo refused for what the folder holds now keeps its step: once a file that came in
/// under a deleted file's name moves away, the delete is undone.
#[test]
fn a_delete_undone_over_a_file_that_came_in_under_its_name_is_undone_once_it_moves() {
    let theirs = Temp::new();
    let (root, mut here, _there) = strings_after_a_rename(&theirs);
    let original = root.read("Strings.ne5p");
    let file = here.named("Strings.ne5p");
    here.bench.act(vec![Act::Remove(file)]);
    here.sync();
    let arrived = with_gain(&Fresh::Program.bytes().unwrap(), "99");
    fs::write(root.at("Strings.ne5p"), &arrived).unwrap();
    here.refocus();
    here.undo();
    here.sync();
    let told = "Undo: “Strings.ne5p” is taken";
    assert_eq!(here.said(told), 1, "{}", here.bench.log.transcript());
    fs::rename(root.at("Strings.ne5p"), root.at("Arrived.ne5p")).unwrap();
    here.refocus();
    here.undo();
    here.sync();
    let transcript = here.bench.log.transcript();
    assert_eq!(root.read("Strings.ne5p"), original, "{transcript}");
}

/// An undo of a save refused while its file is away keeps its step, and undoes the save
/// once the file is back.
#[test]
fn a_save_undone_while_its_file_is_away_is_undone_once_it_is_back() {
    let theirs = Temp::new();
    let (root, mut here, _there) = strings_after_a_rename(&theirs);
    let original = root.read("Strings.ne5p");
    let file = here.named("Strings.ne5p");
    here.edit(file, "74");
    here.bench.act(vec![Act::SaveDoc(file)]);
    here.sync();
    fs::rename(root.at("Strings.ne5p"), theirs.at("away")).unwrap();
    here.refocus();
    here.undo();
    here.sync();
    fs::rename(theirs.at("away"), root.at("Strings.ne5p")).unwrap();
    here.refocus();
    here.undo();
    here.sync();
    let transcript = here.bench.log.transcript();
    assert_eq!(root.read("Strings.ne5p"), original, "{transcript}");
}

/// A tag that goes as another drawbar's history leaves the folder, as a sync tool's
/// eviction takes it, is said to be no drawbar's change.
#[test]
fn a_tag_whose_history_leaves_the_folder_is_said_to_be_no_drawbars_change() {
    let theirs = Temp::new();
    let (root, mut here, mut there) = strings(&theirs);
    let before = entries(&root);
    let file = there.named("Strings.ne5p");
    there.tag(&[file], "Live");
    here.refresh();
    let said = here.said("Another drawbar");
    let away = Temp::new();
    for (n, path) in entries(&root).difference(&before).enumerate() {
        fs::rename(path, away.at(&n.to_string())).unwrap();
    }
    here.refocus();
    let transcript = here.bench.log.transcript();
    assert_eq!(here.said("Another drawbar"), said, "{transcript}");
}

/// The entries of the writers of the library at `root`.
fn entries(root: &Temp) -> std::collections::BTreeSet<std::path::PathBuf> {
    let writers = fs::read_dir(root.at(".drawbar/writers.2"))
        .into_iter()
        .flatten();
    let entries = writers
        .flatten()
        .flat_map(|writer| fs::read_dir(writer.path()).into_iter().flatten());
    let entries = entries.flatten().map(|entry| entry.path());
    entries
        .filter(|path| path.extension().is_some_and(|ext| ext == "txt"))
        .collect()
}

/// Undoing the first tag ever made takes back only that tag, though another drawbar has
/// made a tag of its own since.
#[test]
fn undoing_the_first_tag_takes_back_only_that_tag_after_another_drawbar_made_one() {
    let theirs = Temp::new();
    let (root, mut here, mut there) = strings(&theirs);
    fs::write(root.at("Bass.ne5p"), Fresh::Program.bytes().unwrap()).unwrap();
    here.refocus();
    there.refocus();
    let strings = here.named("Strings.ne5p");
    here.tag(&[strings], "Studio");
    there.refresh();
    let bass = there.named("Bass.ne5p");
    there.tag(&[bass], "Gig");
    here.refresh();
    here.undo();
    here.sync();
    let transcript = here.bench.log.transcript();
    assert_eq!(here.said("cannot be undone"), 0, "{transcript}");
    assert_eq!(here.tag_names(), ["Gig"], "{transcript}");
}

/// An unsaved edit of a tagged file that went stays shown missing with its edit once
/// another drawbar lets go of the file's tags.
#[test]
fn an_edit_of_a_lost_file_stays_missing_once_another_drawbar_forgets_it() {
    let theirs = Temp::new();
    let (root, mut here, mut there) = strings(&theirs);
    let file = here.named("Strings.ne5p");
    here.tag(&[file], "Live");
    here.read_all();
    let edited = here.edit(file, "74");
    fs::remove_file(root.at("Strings.ne5p")).unwrap();
    here.refocus();
    assert!(here.bench.browser.folders.missing.contains(&file));
    there.refocus();
    let lost: Vec<u64> = there
        .bench
        .browser
        .folders
        .lost()
        .iter()
        .map(|l| l.id)
        .collect();
    let [lost] = lost[..] else {
        panic!("one row is lost: {lost:?}");
    };
    there.bench.act(vec![Act::Forget(lost)]);
    there.decided();

    here.refresh();
    let transcript = here.bench.log.transcript();
    assert!(here.bytes(file) == edited, "{transcript}");
    assert!(
        here.bench.browser.folders.missing.contains(&file),
        "{transcript}"
    );
}

/// Another drawbar's save at the length and time of the file it replaced, as a coarse
/// clock gives, surfaces against an unsaved edit here instead of being saved over.
#[test]
fn another_drawbars_save_at_one_length_and_time_surfaces_against_an_unsaved_edit() {
    let theirs = Temp::new();
    let (root, mut here, mut there) = strings(&theirs);
    let at = fs::metadata(root.at("Strings.ne5p"))
        .unwrap()
        .modified()
        .unwrap();
    let file = here.named("Strings.ne5p");
    here.read_all();
    here.edit(file, "74");
    let theirs_file = there.named("Strings.ne5p");
    there.read_all();
    let saved = there.edit(theirs_file, "12");
    there.bench.act(vec![Act::SaveDoc(theirs_file)]);
    there.sync();
    let strings = fs::File::options()
        .write(true)
        .open(root.at("Strings.ne5p"));
    strings.unwrap().set_modified(at).unwrap();

    here.refresh();
    let transcript = here.bench.log.transcript();
    assert!(root.read("Strings.ne5p") == saved, "{transcript}");
    assert_eq!(
        here.said("changed on disk while drawbar held an unsaved edit"),
        1,
        "{transcript}"
    );
}

/// A tagged file that arrives at an unsaved copy's path, and is not the copy's file,
/// wears its tags as an asset of its own.
#[test]
fn a_tagged_file_arriving_at_an_unsaved_copys_path_wears_its_own_tags() {
    let root = Temp::new();
    let program = Fresh::Program.bytes().unwrap();
    fs::write(root.at("Bass.ne5p"), with_gain(&program, "12")).unwrap();
    let mut first = Session::open(&root);
    let bass = first.named("Bass.ne5p");
    first.edit(bass, "74");
    first.close();
    fs::remove_file(root.at("Bass.ne5p")).unwrap();
    let mut here = Session::open(&root);
    let copy = here.only();

    let theirs = Temp::new();
    let mut there = Session::elsewhere(&root, &theirs);
    fs::write(root.at("Bass.ne5p"), with_gain(&program, "40")).unwrap();
    there.refocus();
    let arrived = there.named("Bass.ne5p");
    there.tag(&[arrived], "Live");
    here.refocus();
    here.read_all();

    let transcript = here.bench.log.transcript();
    assert_eq!(here.worn(copy), Vec::<String>::new(), "{transcript}");
    let missing = &here.bench.browser.folders.missing;
    let mut listed = here.bench.workspace.listed().map(|entity| entity.id);
    let file =
        listed.find(|id| !missing.contains(id) && here.path(*id).as_deref() == Some("Bass.ne5p"));
    let file = file.expect("the arrived file is an asset");
    assert_eq!(here.worn(file), ["Live"], "{transcript}");
}

/// As [`strings`], where the other drawbar tagged `Strings.ne5p` and moved it to
/// `Sound.ne5p`, and this drawbar showed it tagged before the move.
fn strings_moved(theirs: &Temp) -> (Temp, Session) {
    let (root, mut here, mut there) = strings(theirs);
    let file = there.named("Strings.ne5p");
    there.tag(&[file], "Live");
    here.refresh();
    there.place_at(file, "Sound.ne5p");
    there.sync();
    (root, here)
}

/// A tagged file another drawbar moved, whose file is not at its destination, is said
/// to be on its way there.
#[test]
fn a_tagged_file_another_drawbar_moved_that_is_not_here_yet_is_said_on_its_way() {
    let (theirs, away) = (Temp::new(), Temp::new());
    let (root, mut here) = strings_moved(&theirs);
    fs::rename(root.at("Sound.ne5p"), away.at("Sound.ne5p")).unwrap();

    here.refresh();
    let moved = "Another drawbar moved “Strings.ne5p” to “Sound.ne5p”; it isn't here yet.";
    let transcript = here.bench.log.transcript();
    assert_eq!(here.said(moved), 1, "{transcript}");
}

/// A tagged file another drawbar moved onto a path where another tagged file is now
/// is not said to be on its way: drawbar cannot tell the file there is not it.
#[test]
fn a_tagged_file_another_drawbar_moved_where_another_file_is_now_is_not_said_on_its_way() {
    let (theirs, third) = (Temp::new(), Temp::new());
    let (root, mut here) = strings_moved(&theirs);
    let program = Fresh::Program.bytes().unwrap();
    fs::write(root.at("Sound.ne5p"), with_gain(&program, "12")).unwrap();
    // A third drawbar tags the file now there before it reads the others' logs.
    let writers = root.at(".drawbar/library/writers.2");
    let held = Temp::new();
    fs::rename(&writers, held.at("writers.2")).unwrap();
    let mut unseen = Session::elsewhere(&root, &third);
    let sound = unseen.named("Sound.ne5p");
    unseen.tag(&[sound], "Studio");
    for writer in fs::read_dir(&writers).unwrap() {
        let writer = writer.unwrap();
        fs::rename(writer.path(), held.at("writers.2").join(writer.file_name())).unwrap();
    }
    fs::remove_dir(&writers).unwrap();
    fs::rename(held.at("writers.2"), &writers).unwrap();

    here.refresh();
    let transcript = here.bench.log.transcript();
    assert_eq!(here.said("Another drawbar moved"), 0, "{transcript}");
    let sound = here.named("Sound.ne5p");
    assert_eq!(here.worn(sound), ["Studio"], "{transcript}");
}

/// As [`strings_and_bass`], where this drawbar edits `Strings.ne5p` and then `Bass.ne5p`,
/// and the other drawbar merges the rows of the two, so the older edit is kept apart from
/// its row at the next look, which finds `bytes` arrived at its path.
fn an_edit_whose_row_ends_as_a_file_arrives(bytes: &[u8]) -> (Temp, Session, u64, Vec<u8>) {
    let theirs = Temp::new();
    let (root, mut here, mut there) = strings_and_bass(&theirs);
    let strings = here.named("Strings.ne5p");
    let edited = here.edit(strings, "74");
    let bass = here.named("Bass.ne5p");
    here.edit(bass, "75");
    merge_bass(&mut there);
    fs::write(root.at("Strings.ne5p"), bytes).unwrap();
    (root, here, strings, edited)
}

/// How many lines `session` said holding `words` since its log was `from` entries long.
fn said_since(session: &Session, from: usize, words: &str) -> usize {
    let since = session.bench.log.iter().skip(from);
    since.filter(|entry| entry.text.contains(words)).count()
}

/// An edit kept apart from its row as its own file arrives at its path goes back onto
/// that file, and is never said to be held apart from it.
#[test]
fn an_edit_whose_row_ends_as_its_file_arrives_goes_back_onto_the_file() {
    let program = Fresh::Program.bytes().unwrap();
    let (_root, mut here, file, edited) = an_edit_whose_row_ends_as_a_file_arrives(&program);
    let from = here.bench.log.len();
    here.refresh();
    here.settle();
    let transcript = here.bench.log.transcript();
    assert!(here.bytes(file) == edited, "{transcript}");
    assert_eq!(
        here.path(file).as_deref(),
        Some("Strings.ne5p"),
        "{transcript}"
    );
    let missing = &here.bench.browser.folders.missing;
    assert!(!missing.contains(&file), "{transcript}");
    let held = said_since(&here, from, "drawbar still holds your changes");
    assert_eq!(held, 0, "{transcript}");
}

/// An edit kept apart from its row as another file arrives at its path stays apart from
/// that file, which is an asset of its own, and is said to be once the file is read.
#[test]
fn an_edit_whose_row_ends_as_another_file_arrives_is_said_held_apart_from_it() {
    let other = with_gain(&Fresh::Program.bytes().unwrap(), "12");
    let (_root, mut here, file, edited) = an_edit_whose_row_ends_as_a_file_arrives(&other);
    let from = here.bench.log.len();
    here.refresh();
    here.settle();
    let transcript = here.bench.log.transcript();
    assert!(here.bytes(file) == edited, "{transcript}");
    assert!(
        here.bench.browser.folders.missing.contains(&file),
        "{transcript}"
    );
    let arrived = here.named("Strings.ne5p");
    assert!(here.bytes(arrived) == other, "{transcript}");
    let apart = "“Strings.ne5p” on disk is another file; drawbar still holds your changes as \
                 “Strings 2.ne5p”.";
    assert_eq!(said_since(&here, from, apart), 1, "{transcript}");
    let gone = said_since(&here, from, "no longer in the library folder");
    assert_eq!(gone, 0, "{transcript}");
}

/// An edit kept apart that is deleted while a file at its path is read for it leaves
/// nothing to say of it.
#[test]
fn an_edit_kept_apart_deleted_while_a_file_at_its_path_is_read_leaves_nothing_to_say() {
    let program = Fresh::Program.bytes().unwrap();
    let (_root, mut here, strings, _) = an_edit_whose_row_ends_as_a_file_arrives(&program);
    here.store.refresh();
    while here.store.claiming.is_empty() {
        assert!(here.next(), "the refresh answered");
    }
    here.bench.act(vec![Act::Remove(strings)]);
    here.sync();
    assert!(
        here.store.unsaid.is_empty(),
        "{}",
        here.bench.log.transcript()
    );
}

/// Another drawbar's save at the length and time of the file it replaced, whose file
/// arrives after the refresh that read it, is refused under a save of an edit made over
/// the file before, which then asks whose to keep and keeps the edit.
#[test]
fn a_save_over_a_rewrite_that_arrived_after_its_log_asks_whose_to_keep() {
    let (ours, theirs) = (Temp::new(), Temp::new());
    fs::write(ours.at("Strings.ne5p"), Fresh::Program.bytes().unwrap()).unwrap();
    let at = fs::metadata(ours.at("Strings.ne5p"))
        .unwrap()
        .modified()
        .unwrap();
    let mut here = Session::open(&ours);
    copy_tree(&ours.0, &theirs.0);
    let mut there = Session::open(&theirs);
    let mine = here.named("Strings.ne5p");
    let edited = here.edit(mine, "74");
    let file = there.named("Strings.ne5p");
    let saved = there.edit(file, "12");
    there.bench.act(vec![Act::SaveDoc(file)]);
    there.sync();
    let writers = ".drawbar/library/writers.2";
    for dir in fs::read_dir(theirs.at(writers)).unwrap() {
        let dir = dir.unwrap();
        copy_tree(&dir.path(), &ours.at(writers).join(dir.file_name()));
    }
    here.refresh();
    fs::write(ours.at("Strings.ne5p"), &saved).unwrap();
    let strings = fs::File::options()
        .write(true)
        .open(ours.at("Strings.ne5p"));
    strings.unwrap().set_modified(at).unwrap();
    here.refocus();

    here.bench.workspace.mark_saved(mine);
    here.sync();
    let transcript = here.bench.log.transcript();
    assert!(ours.read("Strings.ne5p") == saved, "{transcript}");
    let asking = here.bench.browser.asking().map(|(title, _)| title);
    let asks = "“Strings.ne5p” changed on disk".to_string();
    assert_eq!(asking, Some(asks), "{transcript}");
    let entity = here.bench.workspace.get(mine).unwrap();
    assert!(
        entity.bytes == edited && entity.is_unsaved(),
        "{transcript}"
    );
}
