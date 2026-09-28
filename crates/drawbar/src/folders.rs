//! The folders of the library on this computer, which are directories, and what the
//! browser shows about its files beyond the assets themselves.
//!
//! A folder has an id for the session only, so a row, a selection and an open branch
//! can name it while its path changes. Changes to folders queue [`Op`]s, which
//! [`crate::store::Store`] sends to the disk in order.

use std::collections::BTreeSet;

use crate::store::{names, LibPath, Row};
use crate::workspace::{LocalEntity, Workspace};

/// One directory in the library.
pub struct Folder {
    pub id: u64,
    pub path: LibPath,
}

impl Folder {
    pub fn name(&self) -> &str {
        self.path.leaf()
    }
}

/// A change to the tree of folders, in the order it was made.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Op {
    MakeDir(LibPath),
    MoveDir {
        from: LibPath,
        to: LibPath,
    },
    /// Sent after every other change of the same sync, once whatever was in it has
    /// moved out.
    RemoveDir(LibPath),
}

/// An index row whose file is gone, for an asset drawbar does not hold: it keeps tags,
/// or the slot it came off, that would otherwise be lost.
pub struct Lost {
    pub id: u64,
    pub row: Row,
}

/// The menu item that shows the files drawbar does not open, and hides them again.
pub const SHOW_ALL_FILES: &str = "Show all files";

/// Where [`Folders::all_files`] is kept between sessions: a preference of the app, not of
/// any one library.
pub(crate) const ALL_FILES_KEY: &str = "drawbar.all_files";

/// Where the library is, for the header of This computer.
#[derive(Clone, Default)]
pub struct Where {
    pub label: String,
    /// A URL that shows the folder in the system's file manager.
    pub reveal: Option<String>,
    /// What state it is in, when that is not simply open.
    pub note: Option<String>,
    /// It is still being read.
    pub opening: bool,
}

/// What is already in a folder under a name.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Occupant {
    Asset(u64),
    Folder(u64),
    /// An index row whose file is gone.
    Lost(u64),
    /// A file drawbar does not hold: one it does not open, or one it did not read.
    Other,
}

/// What a name would collide with in a folder.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Clash {
    Free,
    Taken(Occupant),
    /// The folder already holds two entries under that name, so neither may be named:
    /// their names, for the refusal.
    Ambiguous(Vec<String>),
}

#[derive(Default)]
pub struct Folders {
    /// In path order, so parents come before their children.
    list: Vec<Folder>,
    ops: Vec<Op>,
    /// Assets whose file went missing outside drawbar while it held them.
    pub missing: BTreeSet<u64>,
    lost: Vec<Lost>,
    /// Assets whose name collides with another entry of their folder.
    pub duplicates: BTreeSet<u64>,
    pub place: Option<Where>,
    /// Files drawbar does not open, shown only while [`Folders::all_files`] is on.
    pub others: Vec<LibPath>,
    /// Files drawbar would hold that it did not read, and why.
    pub unread: Vec<(LibPath, String)>,
    /// Folders whose contents were not all listed.
    pub unwalked: BTreeSet<LibPath>,
    /// Show the files drawbar does not open, too.
    pub all_files: bool,
}

impl Folders {
    pub fn all(&self) -> &[Folder] {
        &self.list
    }

    pub fn get(&self, id: u64) -> Option<&Folder> {
        self.list.iter().find(|folder| folder.id == id)
    }

    pub(crate) fn name_of(&self, id: u64) -> Option<&str> {
        self.get(id).map(Folder::name)
    }

    pub fn path_of(&self, id: u64) -> Option<&LibPath> {
        self.get(id).map(|folder| &folder.path)
    }

    /// The folder `folder` names, or the root for `None`.
    pub fn dir(&self, folder: Option<u64>) -> Option<LibPath> {
        match folder {
            None => Some(LibPath::root()),
            Some(id) => self.path_of(id).cloned(),
        }
    }

    /// The folder at `path`.
    pub fn id_of(&self, path: &LibPath) -> Option<u64> {
        self.list
            .iter()
            .find(|folder| folder.path == *path)
            .map(|folder| folder.id)
    }

    /// The folder an asset is in, or `None` for the root or an asset with no file yet.
    pub fn holding(&self, entity: &LocalEntity) -> Option<u64> {
        let parent = entity.path.as_ref()?.parent();
        match parent.is_root() {
            true => None,
            false => self.id_of(&parent),
        }
    }

    /// The folders directly inside `folder`, or directly in the root for `None`.
    pub fn children(&self, folder: Option<u64>) -> Vec<u64> {
        let Some(dir) = self.dir(folder) else {
            return Vec::new();
        };
        self.list
            .iter()
            .filter(|held| held.path.parent() == dir)
            .map(|held| held.id)
            .collect()
    }

    /// The assets directly in `folder`, in list order. An asset with no file yet is in
    /// the root.
    pub(crate) fn members<'a>(
        &self,
        folder: Option<u64>,
        workspace: &'a Workspace,
    ) -> Vec<&'a LocalEntity> {
        workspace
            .listed()
            .filter(|entity| self.holding(entity) == folder)
            .collect()
    }

    /// Every file drawbar does not hold, read or not.
    pub fn strangers(&self) -> impl Iterator<Item = &LibPath> {
        self.others
            .iter()
            .chain(self.unread.iter().map(|(path, _)| path))
    }

    /// Whether a folder holds something drawbar does not, or may: a file it does not
    /// hold, or contents it did not list.
    pub fn holds_strangers(&self, dir: &LibPath) -> bool {
        self.strangers().any(|path| path.is_in(dir) && path != dir)
            || self.unwalked.iter().any(|path| path.is_in(dir))
    }

    /// The index rows whose file is gone.
    pub fn lost(&self) -> &[Lost] {
        &self.lost
    }

    pub(crate) fn lose(&mut self, id: u64, row: Row) {
        self.lost.push(Lost { id, row });
    }

    /// Drop a lost row, and whatever it kept.
    pub(crate) fn forget(&mut self, id: u64) {
        self.lost.retain(|lost| lost.id != id);
    }

    fn next_id(&self) -> u64 {
        self.list.iter().map(|folder| folder.id).max().unwrap_or(0) + 1
    }

    fn insert(&mut self, path: LibPath) -> u64 {
        if let Some(id) = self.id_of(&path) {
            return id;
        }
        let id = self.next_id();
        self.list.push(Folder { id, path });
        self.list.sort_by(|a, b| a.path.cmp(&b.path));
        id
    }

    /// Take the folders the disk has, keeping the id of each one already known, with the
    /// changes not yet sent to it applied on top.
    pub(crate) fn sync(&mut self, dirs: &[LibPath]) {
        let mut dirs: Vec<LibPath> = dirs.to_vec();
        for op in &self.ops {
            match op {
                Op::MakeDir(path) => dirs.push(path.clone()),
                Op::MoveDir { from, to } => {
                    for dir in &mut dirs {
                        if let Some(moved) = dir.moved(from, to) {
                            *dir = moved;
                        }
                    }
                }
                Op::RemoveDir(path) => dirs.retain(|dir| dir != path),
            }
        }
        self.list.retain(|folder| dirs.contains(&folder.path));
        for dir in dirs {
            self.insert(dir);
        }
    }

    /// Everything named in `dir`, with its key and what it is.
    fn entries(&self, dir: &LibPath, workspace: &Workspace) -> Vec<(String, String, Occupant)> {
        let assets = workspace.listed().filter_map(|entity| {
            let path = entity.path.as_ref()?;
            (path.parent() == *dir).then(|| (path.leaf(), Occupant::Asset(entity.id)))
        });
        let folders = self
            .list
            .iter()
            .filter(|folder| folder.path.parent() == *dir)
            .map(|folder| (folder.name(), Occupant::Folder(folder.id)));
        let lost = self.lost.iter().filter_map(|lost| {
            let path = lost.row.path.as_ref()?;
            (path.parent() == *dir).then(|| (path.leaf(), Occupant::Lost(lost.id)))
        });
        let others = self
            .strangers()
            .filter(|path| path.parent() == *dir)
            .map(|path| (path.leaf(), Occupant::Other));
        assets
            .chain(folders)
            .chain(lost)
            .chain(others)
            .map(|(name, what)| (names::key(name), name.to_string(), what))
            .collect()
    }

    /// What `name` would collide with in `dir`, leaving `except` out: the entry being
    /// renamed may keep its own name in a new case.
    pub fn clash(
        &self,
        dir: &LibPath,
        name: &str,
        workspace: &Workspace,
        except: Option<Occupant>,
    ) -> Clash {
        let key = names::key(name);
        let holding: Vec<(String, Occupant)> = self
            .entries(dir, workspace)
            .into_iter()
            .filter(|(held, _, what)| *held == key && Some(*what) != except)
            .map(|(_, name, what)| (name, what))
            .collect();
        match holding.as_slice() {
            [] => Clash::Free,
            [(_, what)] => Clash::Taken(*what),
            many => Clash::Ambiguous(many.iter().map(|(name, _)| name.clone()).collect()),
        }
    }

    /// `wanted` in `dir`, numbered until nothing there collides with it.
    pub fn free(&self, dir: &LibPath, wanted: &str, workspace: &Workspace) -> String {
        let taken: BTreeSet<String> = self
            .entries(dir, workspace)
            .into_iter()
            .map(|(key, _, _)| key)
            .collect();
        names::free(wanted, |key| taken.contains(key))
    }

    /// A new folder inside `parent`, named `New folder` or the first free number after
    /// it.
    pub(crate) fn make(&mut self, parent: &LibPath, workspace: &Workspace) -> u64 {
        let name = self.free(parent, "New folder", workspace);
        let path = parent.join(&name);
        // A folder removed and made again in one sync must not be removed after.
        self.ops.retain(|op| *op != Op::RemoveDir(path.clone()));
        self.ops.push(Op::MakeDir(path.clone()));
        self.insert(path)
    }

    /// Rename or move a folder, with everything inside it.
    pub(crate) fn relocate(&mut self, id: u64, to: LibPath, workspace: &mut Workspace) {
        let Some(from) = self.path_of(id).cloned() else {
            return;
        };
        for folder in &mut self.list {
            if let Some(moved) = folder.path.moved(&from, &to) {
                folder.path = moved;
            }
        }
        self.list.sort_by(|a, b| a.path.cmp(&b.path));
        for lost in &mut self.lost {
            if let Some(moved) = lost.row.path.as_ref().and_then(|at| at.moved(&from, &to)) {
                lost.row.path = Some(moved);
            }
        }
        workspace.relocate(&from, &to);
        self.ops.push(Op::MoveDir { from, to });
    }

    /// Remove an empty folder. The caller moves out what was in it first.
    pub(crate) fn remove(&mut self, id: u64) {
        let Some(path) = self.path_of(id).cloned() else {
            return;
        };
        self.list.retain(|folder| folder.id != id);
        self.ops.push(Op::RemoveDir(path));
    }

    /// Whether a change is waiting to be sent to the disk.
    pub(crate) fn changed(&self) -> bool {
        !self.ops.is_empty()
    }

    /// The changes made since the last call, in order.
    pub(crate) fn take_ops(&mut self) -> Vec<Op> {
        std::mem::take(&mut self.ops)
    }
}

#[cfg(test)]
impl Folders {
    /// Put an asset in `folder` under its own name, numbered past what is there.
    pub(crate) fn file(&self, workspace: &mut Workspace, id: u64, folder: Option<u64>) {
        let (Some(dir), Some(entity)) = (self.dir(folder), workspace.get(id)) else {
            return;
        };
        let wanted = crate::workspace::library_filename(entity);
        let name = self.free(&dir, &wanted, workspace);
        workspace.place(id, dir.join(&name));
    }
}

/// Give every kept asset without a file a path in the root, named after the asset and
/// numbered past whatever is there. Nobody is asked: the name is the app's choice.
pub(crate) fn place_new(workspace: &mut Workspace, folders: &Folders) {
    let unplaced: Vec<(u64, String)> = workspace
        .listed()
        .filter(|entity| entity.path.is_none())
        .map(|entity| (entity.id, crate::workspace::library_filename(entity)))
        .collect();
    for (id, wanted) in unplaced {
        let name = folders.free(&LibPath::root(), &wanted, workspace);
        workspace.place(id, LibPath::root().join(&name));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::log::Log;
    use crate::workspace::Fresh;

    fn workspace() -> (Workspace, Log) {
        (
            Workspace::new(eframe::egui::Context::default()),
            Log::default(),
        )
    }

    #[test]
    fn a_name_collides_with_a_file_or_a_folder_in_any_case() {
        let (mut workspace, mut log) = workspace();
        let id = workspace.create(Fresh::Program, &mut log).unwrap();
        workspace.place(id, LibPath::root().join("Grand.ne5p"));
        let mut folders = Folders::default();
        let cello = folders.make(&LibPath::root(), &workspace);

        let root = LibPath::root();
        assert_eq!(
            folders.clash(&root, "GRAND.NE5P", &workspace, None),
            Clash::Taken(Occupant::Asset(id))
        );
        assert_eq!(
            folders.clash(&root, "new FOLDER", &workspace, None),
            Clash::Taken(Occupant::Folder(cello))
        );
        assert_eq!(
            folders.clash(&root, "Grand.ne5p", &workspace, Some(Occupant::Asset(id))),
            Clash::Free,
            "an asset renamed to its own name in another case"
        );
        let inside = folders.path_of(cello).unwrap().clone();
        assert_eq!(
            folders.clash(&inside, "Grand.ne5p", &workspace, None),
            Clash::Free,
            "another folder is another namespace"
        );
    }

    #[test]
    fn two_entries_under_one_name_make_that_name_unusable() {
        let (mut workspace, mut log) = workspace();
        for name in ["c3.ne5p", "C3.ne5p"] {
            let id = workspace.create(Fresh::Program, &mut log).unwrap();
            workspace.place(id, LibPath::root().join(name));
        }
        let folders = Folders::default();
        assert_eq!(
            folders.clash(&LibPath::root(), "c3.NE5P", &workspace, None),
            Clash::Ambiguous(vec!["c3.ne5p".into(), "C3.ne5p".into()])
        );
    }

    #[test]
    fn renaming_a_folder_moves_everything_inside_it() {
        let (mut workspace, mut log) = workspace();
        let mut folders = Folders::default();
        let outer = folders.make(&LibPath::root(), &workspace);
        let outer_path = folders.path_of(outer).unwrap().clone();
        let inner = folders.make(&outer_path, &workspace);
        let id = workspace.create(Fresh::Program, &mut log).unwrap();
        workspace.place(id, folders.path_of(inner).unwrap().join("Grand.ne5p"));
        folders.take_ops();

        let to = LibPath::root().join("Pianos");
        folders.relocate(outer, to.clone(), &mut workspace);

        assert_eq!(folders.path_of(outer), Some(&to));
        assert_eq!(
            folders.path_of(inner).map(LibPath::as_str),
            Some("Pianos/New folder")
        );
        assert_eq!(
            workspace
                .get(id)
                .unwrap()
                .path
                .as_ref()
                .map(LibPath::as_str),
            Some("Pianos/New folder/Grand.ne5p")
        );
        assert_eq!(
            folders.take_ops(),
            vec![Op::MoveDir {
                from: outer_path,
                to
            }],
            "one rename on disk, not one per file"
        );
    }

    #[test]
    fn a_new_asset_is_placed_in_the_root_under_a_free_name() {
        let (mut workspace, mut log) = workspace();
        let first = workspace.create(Fresh::Program, &mut log).unwrap();
        let second = workspace.create(Fresh::Program, &mut log).unwrap();
        place_new(&mut workspace, &Folders::default());
        let path = |id| workspace.get(id).unwrap().path.clone().unwrap();
        assert_eq!(path(first).as_str(), "untitled.ne5p");
        assert_eq!(path(second).as_str(), "untitled 2.ne5p");
        assert_eq!(workspace.get(second).unwrap().name, "untitled 2.ne5p");
    }
}
