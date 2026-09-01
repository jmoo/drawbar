//! How the list on this computer is grouped.
//!
//! Beside [`crate::store`] because a grouping is stored the same way the list is, under
//! its own key: two files, read back separately, agreeing about what an id means.

use std::collections::HashMap;

use crate::store::{escape, unescape};
use crate::workspace::{LocalEntity, Workspace};

/// Where the folders and their membership are kept between sessions.
///
/// ⚠️ Membership is by workspace id, which is the same id the local list is stored
/// under — the two files are read back into one list, so they have to agree about what
/// an id means.
pub(crate) const KEY: &str = "drawbar.folders";

const VERSION: &str = "drawbar folders 1";

/// One folder on this computer.
pub struct Folder {
    pub id: u64,
    pub name: String,
}

/// How the local list is grouped.
///
/// ⚠️ A folder is a **view of the list**, not a place bytes live: an asset in one is an
/// asset like any other, and nothing here is a directory, an archive or anything the
/// instrument has ever heard of. Membership is kept beside the divider rather than in
/// the workspace for that reason.
#[derive(Default)]
pub struct Folders {
    list: Vec<Folder>,
    /// Which folder an asset is in, by its workspace id. Absent is loose.
    of: HashMap<u64, u64>,
}

impl Folders {
    pub fn all(&self) -> &[Folder] {
        &self.list
    }

    pub(crate) fn name_of(&self, id: u64) -> Option<&str> {
        self.list
            .iter()
            .find(|folder| folder.id == id)
            .map(|folder| folder.name.as_str())
    }

    /// A new folder, under a name nothing else in the list is using.
    pub(crate) fn make(&mut self) -> u64 {
        let id = self.list.iter().map(|folder| folder.id).max().unwrap_or(0) + 1;
        let taken = |name: &str| self.list.iter().any(|folder| folder.name == name);
        let mut name = "New folder".to_string();
        for nth in 2.. {
            if !taken(&name) {
                break;
            }
            name = format!("New folder {nth}");
        }
        self.list.push(Folder { id, name });
        id
    }

    pub(crate) fn rename(&mut self, id: u64, name: String) {
        if let Some(folder) = self.list.iter_mut().find(|folder| folder.id == id) {
            folder.name = name;
        }
    }

    /// Drop a folder. What was in it goes back to the loose part of the list — a folder
    /// holds nothing, so removing one cannot take anything with it.
    pub(crate) fn remove(&mut self, id: u64) {
        self.list.retain(|folder| folder.id != id);
        self.of.retain(|_, held| *held != id);
    }

    pub(crate) fn file(&mut self, entity: u64, folder: Option<u64>) {
        match folder.filter(|id| self.name_of(*id).is_some()) {
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
        let mut out = format!("{VERSION}\n");
        for folder in &self.list {
            out.push_str(&format!("f\t{}\t{}\n", folder.id, escape(&folder.name)));
        }
        for (entity, folder) in &self.of {
            out.push_str(&format!("m\t{entity}\t{folder}\n"));
        }
        out
    }

    /// Read back what [`Folders::written`] wrote. Anything unaccounted for is no folders
    /// at all — half a grouping is worse than none, because a folder nobody made is one
    /// nobody can explain.
    pub(crate) fn read(text: &str) -> Folders {
        let mut lines = text.lines();
        if lines.next() != Some(VERSION) {
            return Folders::default();
        }
        let mut folders = Folders::default();
        for line in lines {
            let mut parts = line.split('\t');
            match (parts.next(), parts.next(), parts.next()) {
                (Some("f"), Some(id), Some(name)) => {
                    if let Ok(id) = id.parse() {
                        folders.list.push(Folder {
                            id,
                            name: unescape(name),
                        });
                    }
                }
                (Some("m"), Some(entity), Some(folder)) => {
                    if let (Ok(entity), Ok(folder)) = (entity.parse(), folder.parse()) {
                        folders.of.insert(entity, folder);
                    }
                }
                _ => {}
            }
        }
        // A membership naming a folder that is not in the file would be an asset nothing
        // shows and nothing can get back.
        let known: Vec<u64> = folders.list.iter().map(|folder| folder.id).collect();
        folders.of.retain(|_, folder| known.contains(folder));
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
                let id = folders.make();
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
        let (kept, gone) = (folders.make(), folders.make());
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
        let (sunday, empty) = (folders.make(), folders.make());
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
}
