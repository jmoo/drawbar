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
        there.said("Another drawbar deleted"),
        0,
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
fn a_file_nothing_tags_another_drawbar_deletes_goes_at_a_refresh_saying_who_deleted_it() {
    let theirs = Temp::new();
    let (_root, mut here, mut there) = strings(&theirs);
    let file = there.named("Strings.ne5p");
    there.bench.act(vec![Act::Remove(file)]);
    there.sync();

    here.refresh();
    let transcript = here.bench.log.transcript();
    assert_eq!(here.bench.workspace.listed().count(), 0, "{transcript}");
    assert_eq!(
        here.said("Another drawbar deleted “Strings.ne5p”."),
        1,
        "{transcript}"
    );
    assert_eq!(here.said("outside drawbar"), 0, "{transcript}");

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
    assert_eq!(here.said("is missing from the library folder"), 1);
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
    let shown = held.map(|id| here.path(id));
    let moved = ["Sound.ne5p", "Set/Bass.ne5p"].map(|path| Some(path.to_string()));
    assert_eq!(shown, moved, "each keeps its id");
    let transcript = here.bench.log.transcript();
    assert_eq!(here.bench.log.iter().count(), said, "{transcript}");
}

#[test]
fn a_file_another_drawbar_tags_then_renames_keeps_its_id_and_wears_the_tag_with_no_outside_line() {
    let theirs = Temp::new();
    let (_root, mut here, mut there) = strings(&theirs);
    let held = here.named("Strings.ne5p");
    let file = there.named("Strings.ne5p");
    there.tag(&[file], "Live");
    let name = "Sound".to_string();
    there.bench.act(vec![Act::RenameLocal { id: file, name }]);
    there.sync();

    here.refresh();
    let transcript = here.bench.log.transcript();
    assert_eq!(
        here.path(held).as_deref(),
        Some("Sound.ne5p"),
        "{transcript}"
    );
    assert_eq!(here.worn(held), ["Live"]);
    assert_eq!(here.bench.workspace.listed().count(), 1, "{transcript}");
    assert_eq!(here.said("outside drawbar"), 0, "{transcript}");
    assert_eq!(here.said("appeared"), 0, "{transcript}");
}

#[test]
fn a_file_another_drawbar_tags_then_deletes_goes_saying_only_who_deleted_it() {
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
    assert_eq!(
        here.said("Another drawbar deleted “Strings.ne5p”."),
        1,
        "{transcript}"
    );
    assert_eq!(here.bench.log.iter().count(), said + 1, "{transcript}");
}

#[test]
fn a_file_nothing_tags_another_drawbar_renames_then_deletes_goes_at_a_refresh() {
    let theirs = Temp::new();
    let (_root, mut here, mut there) = strings(&theirs);
    here.refresh();
    let file = there.named("Strings.ne5p");
    let name = "Sound".to_string();
    there.bench.act(vec![Act::RenameLocal { id: file, name }]);
    there.sync();
    there.bench.act(vec![Act::Remove(file)]);
    there.sync();

    here.refresh();
    let transcript = here.bench.log.transcript();
    assert_eq!(here.bench.workspace.listed().count(), 0, "{transcript}");
    assert_eq!(here.said("Another drawbar deleted"), 1, "{transcript}");
    assert_eq!(here.said("outside drawbar"), 0, "{transcript}");
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
fn a_file_another_drawbar_deletes_before_renaming_its_folder_goes_at_a_refresh() {
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

    here.refresh();
    assert_eq!(
        here.bench.workspace.listed().count(),
        0,
        "{}",
        here.bench.log.transcript()
    );
    assert_eq!(here.said("Another drawbar deleted “Grand.ne5p”."), 1);
}
