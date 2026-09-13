//! How the list on this computer is grouped.
//!
//! Beside [`crate::store`] because a grouping is stored the same way the list is, under
//! its own key: two files, read back separately, agreeing about what an id means.

use std::collections::BTreeMap;

use crate::named::{self, Line, List, Named};
use crate::workspace::{LocalEntity, Workspace};

/// Where the folders and their membership are kept between sessions.
///
/// ⚠️ Membership is by workspace id, which is the same id the local list is stored
/// under — the two files are read back into one list, so they have to agree about what
/// an id means.
pub(crate) const KEY: &str = "drawbar.folders";

const VERSION: &str = "drawbar folders 1";

/// What a folder line is headed with.
const FOLDER: &str = "f";

/// One folder on this computer.
pub type Folder = Named;

/// How the local list is grouped.
///
/// ⚠️ A folder is a **view of the list**, not a place bytes live: an asset in one is an
/// asset like any other, and nothing here is a directory, an archive or anything the
/// instrument has ever heard of. Membership is kept beside the divider rather than in
/// the workspace for that reason.
#[derive(Default)]
pub struct Folders {
    list: List,
    /// Which folder an asset is in, by its workspace id. Absent is loose.
    ///
    /// ⚠️ Ordered, because [`Folders::written`] walks it and a store that comes out in a
    /// different order every session is a store written every session.
    of: BTreeMap<u64, u64>,
}

impl Folders {
    pub fn all(&self) -> &[Folder] {
        self.list.all()
    }

    pub(crate) fn name_of(&self, id: u64) -> Option<&str> {
        self.list.name_of(id)
    }

    /// A new folder, under a name nothing else in the list is using, or nothing where
    /// the list has no id left ([`List::make`]).
    pub(crate) fn make(&mut self) -> Option<u64> {
        self.list.make("New folder")
    }

    pub(crate) fn rename(&mut self, id: u64, name: String) {
        self.list.rename(id, name);
    }

    /// Drop a folder. What was in it goes back to the loose part of the list — a folder
    /// holds nothing, so removing one cannot take anything with it.
    pub(crate) fn remove(&mut self, id: u64) {
        self.list.remove(id);
        self.of.retain(|_, held| *held != id);
    }

    pub(crate) fn file(&mut self, entity: u64, folder: Option<u64>) {
        match folder.filter(|id| self.list.holds(*id)) {
            Some(id) => self.of.insert(entity, id),
            None => self.of.remove(&entity),
        };
    }

    pub(crate) fn forget(&mut self, entity: u64) {
        self.of.remove(&entity);
    }

    /// Drop the memberships of assets the list does not hold.
    ///
    /// The store keeps the folders and the assets in two files that are read back
    /// separately, and only the asset file decides what survived — anything too big to
    /// keep, or dropped for want of room, leaves its membership behind. Left alone they
    /// accumulate for as long as the app is installed.
    pub(crate) fn forget_missing(&mut self, workspace: &Workspace) {
        self.of.retain(|entity, _| workspace.get(*entity).is_some());
    }

    /// Which folder an asset is in.
    pub fn holding(&self, entity: u64) -> Option<u64> {
        self.of.get(&entity).copied()
    }

    /// What this folder holds, in the order the list holds it.
    pub(crate) fn members<'a>(&self, id: u64, workspace: &'a Workspace) -> Vec<&'a LocalEntity> {
        workspace
            .listed()
            .filter(|entity| self.holding(entity.id) == Some(id))
            .collect()
    }
}

impl Folders {
    /// The folders and their membership as one string, for the store.
    ///
    /// `f` lines are the folders and `m` lines are what is in them, so a folder with
    /// nothing in it survives a session like any other.
    pub(crate) fn written(&self) -> String {
        let mut out = named::written(VERSION, FOLDER, &self.list);
        for (entity, folder) in &self.of {
            out.push_str(&named::member(*entity, *folder));
        }
        out
    }

    /// Read back what [`Folders::written`] wrote. Anything unaccounted for is no folders
    /// at all — half a grouping is worse than none, because a folder nobody made is one
    /// nobody can explain.
    pub(crate) fn read(text: &str) -> Folders {
        let mut folders = Folders::default();
        for line in named::read(text, VERSION, FOLDER) {
            match line {
                Line::Named { id, name } => folders.list.restore(id, name),
                Line::Member { asset, group } => {
                    folders.of.insert(asset, group);
                }
            }
        }
        // A membership naming a folder that is not in the file would be an asset nothing
        // shows and nothing can get back.
        let Folders { list, of } = &mut folders;
        of.retain(|_, folder| list.holds(*folder));
        folders
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A new folder is one nothing else is called, so two of them are two rows rather
    /// than one row twice.
    #[test]
    fn a_new_folder_gets_a_name_no_other_folder_is_using() {
        let mut folders = Folders::default();
        let names: Vec<String> = (0..3)
            .map(|_| {
                let id = folders.make().unwrap();
                folders.name_of(id).expect("it was made").to_string()
            })
            .collect();
        assert_eq!(names, ["New folder", "New folder 2", "New folder 3"]);
        // And the ids are as distinct as the names.
        let ids: Vec<u64> = folders.all().iter().map(|folder| folder.id).collect();
        assert_eq!(ids, vec![1, 2, 3]);
    }

    /// A folder holds nothing, so losing one loses nothing: what was in it is back in
    /// the loose part of the list.
    #[test]
    fn removing_a_folder_leaves_what_was_in_it_on_this_computer() {
        let mut folders = Folders::default();
        let (kept, gone) = (folders.make().unwrap(), folders.make().unwrap());
        folders.file(7, Some(kept));
        folders.file(8, Some(gone));
        folders.remove(gone);

        assert_eq!(folders.holding(7), Some(kept));
        assert_eq!(folders.holding(8), None, "loose, not lost");
        // And a folder that never existed is not a place anything can be put.
        folders.file(9, Some(gone));
        assert_eq!(folders.holding(9), None);
    }

    /// The grouping comes back as it was left, empty folders included — and a membership
    /// naming a folder the file does not hold is dropped rather than hiding an asset in
    /// a folder nobody can open.
    #[test]
    fn the_folders_and_what_is_in_them_survive_a_session() {
        let mut folders = Folders::default();
        let (sunday, empty) = (folders.make().unwrap(), folders.make().unwrap());
        folders.rename(sunday, "Sunday\tmorning".into());
        folders.file(7, Some(sunday));
        folders.file(8, Some(sunday));

        let after = Folders::read(&folders.written());
        assert_eq!(after.all().len(), 2, "an empty folder is still a folder");
        assert_eq!(after.name_of(sunday), Some("Sunday\tmorning"));
        assert_eq!(after.name_of(empty), Some("New folder 2"));
        assert_eq!(after.holding(7), Some(sunday));
        assert_eq!(after.holding(8), Some(sunday));

        // Nothing readable is no folders at all, never half a grouping.
        assert!(Folders::read("").all().is_empty());
        assert!(Folders::read("drawbar folders 99\nf\t1\tSunday\n")
            .all()
            .is_empty());
        let orphaned = Folders::read(&format!("{VERSION}\nm\t7\t3\n"));
        assert_eq!(orphaned.holding(7), None);
    }

    /// ⚠️ The store is rewritten whenever it differs from what is in it, so a grouping
    /// that writes its lines in a different order each time is a write each time.
    #[test]
    fn one_grouping_is_written_as_the_same_bytes_every_time() {
        let mut folders = Folders::default();
        let sunday = folders.make().unwrap();
        for entity in [91, 7, 40, 2, 68, 13] {
            folders.file(entity, Some(sunday));
        }

        let written = folders.written();
        assert_eq!(written, folders.written());
        let members: Vec<&str> = written
            .lines()
            .filter_map(|line| line.strip_prefix("m\t"))
            .filter_map(|line| line.split('\t').next())
            .collect();
        assert_eq!(members, ["2", "7", "13", "40", "68", "91"]);
    }

    /// A line this build did not write is dropped rather than guessed at, and the rest of
    /// the file is still read.
    #[test]
    fn a_line_that_is_not_a_line_is_dropped_and_the_rest_is_read() {
        let read = |lines: &str| Folders::read(&format!("{VERSION}\n{lines}"));

        let kept = read("f\tx\tNot a number\nf\t1\tSunday\n");
        assert_eq!(kept.all().len(), 1, "an id that is not a number");
        assert_eq!(kept.name_of(1), Some("Sunday"));

        let short = read("f\t1\tSunday\nm\t7\n");
        assert_eq!(short.holding(7), None, "a membership missing its folder");

        let wide = read("f\t1\tSunday\nm\t7\t1\textra\n");
        assert_eq!(wide.holding(7), None, "a line with a column too many");

        let unknown = read("f\t1\tSunday\nx\t7\t1\n");
        assert_eq!(unknown.all().len(), 1, "a head this build does not write");
    }

    /// ⚠️ Two `f` lines claiming one id is a file with two names for one folder, and
    /// every membership naming that id means whichever of them is kept. The first is, so
    /// the second is refused rather than quietly renaming a folder on the way in.
    #[test]
    fn a_second_folder_line_for_an_id_already_read_is_refused() {
        let folders = Folders::read(&format!("{VERSION}\nf\t1\tSunday\nf\t1\tMonday\nm\t7\t1\n"));

        assert_eq!(folders.all().len(), 1);
        assert_eq!(folders.name_of(1), Some("Sunday"));
        assert_eq!(folders.holding(7), Some(1));
    }

    /// A name holding a newline would otherwise be two lines, and the second of them a
    /// line this build refuses.
    #[test]
    fn a_folder_named_across_two_lines_comes_back_as_one_name() {
        let mut folders = Folders::default();
        let id = folders.make().unwrap();
        folders.rename(id, "Sunday\nmorning".into());

        let after = Folders::read(&folders.written());
        assert_eq!(after.name_of(id), Some("Sunday\nmorning"));
        assert_eq!(after.all().len(), 1);
    }
}
