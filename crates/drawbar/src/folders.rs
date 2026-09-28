//! Folders that group the list on this computer.
//!
//! Stored the same way as the list in [`crate::store`], under its own `KEY`.

use std::collections::BTreeMap;

use crate::named::{self, Line, List, Named};
use crate::workspace::{LocalEntity, Workspace};

/// Where the folders and their membership are kept between sessions.
///
/// ⚠️ Membership is by workspace id, the same id the local list is stored under. The
/// two files are read back separately into one list, so they must agree about what an id
/// means.
pub(crate) const KEY: &str = "drawbar.folders";

const VERSION: &str = "drawbar folders 1";

/// The marker that starts a folder line.
const FOLDER: &str = "f";

/// One folder on this computer.
pub type Folder = Named;

/// How the local list is grouped.
///
/// ⚠️ A folder is a view of the list, not a place where bytes live. An asset in a folder
/// is like any other asset, and a folder is not a directory, an archive, or anything the
/// instrument knows about. That is why membership is kept here and not in the workspace.
#[derive(Default)]
pub struct Folders {
    list: List,
    /// Which folder each asset is in, by workspace id. An asset with no entry is loose.
    ///
    /// ⚠️ Ordered, because [`Folders::written`] walks it, and output in a different order
    /// each session would rewrite the store each session.
    of: BTreeMap<u64, u64>,
}

impl Folders {
    pub fn all(&self) -> &[Folder] {
        self.list.all()
    }

    pub(crate) fn name_of(&self, id: u64) -> Option<&str> {
        self.list.name_of(id)
    }

    /// A new folder with a name no other folder uses, or `None` when the list has no id
    /// left ([`List::make`]).
    pub(crate) fn make(&mut self) -> Option<u64> {
        self.list.make("New folder")
    }

    pub(crate) fn rename(&mut self, id: u64, name: String) {
        self.list.rename(id, name);
    }

    /// Remove a folder. Its members become loose; a folder owns no assets, so removing
    /// one deletes nothing.
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
    /// The store keeps folders and assets in two files read back separately, and only
    /// the asset file decides what survived: an asset too big to keep, or dropped for
    /// lack of room, leaves its membership behind. Left alone, these would accumulate for
    /// as long as the app is installed.
    pub(crate) fn forget_missing(&mut self, workspace: &Workspace) {
        self.of.retain(|entity, _| workspace.get(*entity).is_some());
    }

    /// Which folder an asset is in.
    pub fn holding(&self, entity: u64) -> Option<u64> {
        self.of.get(&entity).copied()
    }

    /// This folder's members, in list order.
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
    /// `f` lines are folders and `m` lines are memberships, so an empty folder survives a
    /// session like any other.
    pub(crate) fn written(&self) -> String {
        let mut out = named::written(VERSION, FOLDER, &self.list);
        for (entity, folder) in &self.of {
            out.push_str(&named::member(*entity, *folder));
        }
        out
    }

    /// Read back what [`Folders::written`] wrote. An unknown version reads as no
    /// folders; a malformed line is dropped and the rest is read.
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
        // A membership naming a folder missing from the file would hide the asset where
        // nothing shows it.
        let Folders { list, of } = &mut folders;
        of.retain(|_, folder| list.holds(*folder));
        folders
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn removing_a_folder_leaves_what_was_in_it_on_this_computer() {
        let mut folders = Folders::default();
        let (kept, gone) = (folders.make().unwrap(), folders.make().unwrap());
        folders.file(7, Some(kept));
        folders.file(8, Some(gone));
        folders.remove(gone);

        assert_eq!(folders.holding(7), Some(kept));
        assert_eq!(folders.holding(8), None, "loose, not lost");
        // A removed folder cannot take members.
        folders.file(9, Some(gone));
        assert_eq!(folders.holding(9), None);
    }

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

        // An empty file or an unknown version reads as no folders.
        assert!(Folders::read("").all().is_empty());
        assert!(Folders::read("drawbar folders 99\nf\t1\tSunday\n")
            .all()
            .is_empty());
        let orphaned = Folders::read(&format!("{VERSION}\nm\t7\t3\n"));
        assert_eq!(orphaned.holding(7), None);
    }

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
}
