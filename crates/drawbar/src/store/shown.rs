//! What a library's view shows the app: each row drawbar lists, by its FileId, with the
//! tags its entity wears, and the folders. The backend that holds the library sends it whole
//! once, as the library opens, and after that only what each answer changed, so the app
//! never holds a view of its own.

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};
use toshokan::rows::{DirWhat, RowChange};
use toshokan::{EntityId, FileId, RelPath, RowState, View};

use super::keys::{Came, ORIGIN, TAGS, TAG_NAMES, TAKEN_IN};
use super::{LibPath, Stat};

/// A row of the view, as drawbar shows it.
#[derive(Clone, PartialEq, Debug, Serialize, Deserialize)]
pub struct Row {
    pub path: LibPath,
    pub state: RowState,
    pub entity: Option<EntityId>,
    /// The tags its entity wears, and those of each entity joined into it.
    pub worn: BTreeSet<String>,
    /// Whether it wears a tag, or one that does not read.
    pub wears: bool,
    /// Where its asset came from, where its entity says.
    pub came: Option<Came>,
}

/// How drawbar shows a row.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Class {
    /// A file drawbar opens, that the folder holds or that is under a path no scan can
    /// list now.
    File,
    /// A file drawbar does not open.
    Other,
    /// The row of an entity wearing tags whose file is nowhere. One that says only where
    /// its file came from keeps nothing worth showing without the file.
    Lost,
    /// Any other row whose file is nowhere: a path a new file must not take, since the
    /// row's file may still arrive there.
    Absent,
}

impl Row {
    pub fn class(&self) -> Class {
        match self.state {
            RowState::Missing if self.wears => Class::Lost,
            RowState::Missing => Class::Absent,
            _ if !super::opens(self.path.leaf()) => Class::Other,
            RowState::Present { .. } | RowState::Unknown => Class::File,
        }
    }

    /// What the last scan found at its path; `None` where none can list it now.
    pub fn stat(&self) -> Option<Stat> {
        match self.state {
            RowState::Present { len, modified } => Some(Stat { len, modified }),
            RowState::Unknown | RowState::Missing => None,
        }
    }

    /// `row` of `view` as drawbar shows it; nothing under a name that starts with a dot.
    fn of(view: &View, row: &toshokan::Row) -> Option<Row> {
        let path = visible(&row.path)?;
        let entities = || {
            row.entity
                .into_iter()
                .flat_map(|entity| showing_on(view, entity))
        };
        let tags = || entities().map(|entity| entity.members(TAGS));
        let came = row.entity.and_then(|entity| view.entity(entity));
        Some(Row {
            path,
            state: row.state,
            entity: row.entity,
            worn: tags().flat_map(|tags| tags.values).collect(),
            wears: tags().any(|tags| !tags.values.is_empty() || !tags.unreadable.is_empty()),
            came: came.and_then(|entity| toshokan::Field::shown(&entity.get(ORIGIN)).copied()),
        })
    }
}

#[derive(Clone, Default, PartialEq, Debug, Serialize, Deserialize)]
pub struct Shown {
    pub rows: BTreeMap<FileId, Row>,
    /// The library's list of tags.
    pub names: BTreeSet<String>,
    /// The lists older drawbars kept that the library took in, by their hashes.
    pub taken: BTreeSet<String>,
    /// Every folder below the root.
    pub dirs: BTreeSet<LibPath>,
    /// The row of each entity bound to a file, the rows wearing tags, the rows of files
    /// drawbar does not open, and the rows whose file is nowhere.
    #[serde(skip)]
    rows_of: BTreeMap<EntityId, FileId>,
    #[serde(skip)]
    tagged: BTreeSet<FileId>,
    #[serde(skip)]
    others: BTreeSet<FileId>,
    #[serde(skip)]
    missing: BTreeSet<FileId>,
}

/// What an answer's view changed of what the app shows.
#[derive(Clone, PartialEq, Debug, Serialize, Deserialize)]
pub enum Change {
    /// Everything, as the library opens, once the app lost an answer, or where most
    /// of what it shows changed.
    Whole(Shown),
    /// Each row the view's changes reach as it shows now, `None` for a row shown no
    /// more, and the list of tags, the lists taken in and each folder where they
    /// changed.
    Rows {
        rows: Vec<(FileId, Option<Row>)>,
        names: Option<BTreeSet<String>>,
        taken: Option<BTreeSet<String>>,
        /// Each folder made, or removed where `false`.
        dirs: Vec<(LibPath, bool)>,
    },
}

/// What changed of what the app shows, and who changed each row.
pub struct Changes {
    pub change: Change,
    /// Each change of a row since the view the app was last shown, with who made it;
    /// empty where it was shown none.
    pub rows: Vec<RowChange>,
}

/// What the app shows changed by an answer: the rows it reached, every one where it
/// reached them all, and whether the folders changed.
pub struct Reached {
    pub rows: Option<BTreeSet<FileId>>,
    pub dirs: bool,
}

impl Shown {
    /// What `view` shows, leaving out every path with a name that starts with a dot and
    /// every path that is no [`LibPath`].
    pub fn of(view: &View) -> Shown {
        let mut shown = Shown {
            names: values(view, TAG_NAMES),
            taken: values(view, TAKEN_IN),
            dirs: view.dirs().filter_map(visible).collect(),
            ..Shown::default()
        };
        for row in view.rows() {
            if let Some(shown_row) = Row::of(view, row) {
                shown.take(row.file, shown_row);
            }
        }
        shown
    }

    /// Take in `change`.
    pub fn apply(&mut self, change: Change) -> Reached {
        let (rows, names, taken, dirs) = match change {
            Change::Whole(shown) => {
                *self = shown;
                for (file, row) in std::mem::take(&mut self.rows) {
                    self.take(file, row);
                }
                return Reached {
                    rows: None,
                    dirs: true,
                };
            }
            Change::Rows {
                rows,
                names,
                taken,
                dirs,
            } => (rows, names, taken, dirs),
        };
        if let Some(names) = names {
            self.names = names;
        }
        if let Some(taken) = taken {
            self.taken = taken;
        }
        let reached = Reached {
            rows: Some(rows.iter().map(|(file, _)| *file).collect()),
            dirs: !dirs.is_empty(),
        };
        for (dir, made) in dirs {
            match made {
                true => self.dirs.insert(dir),
                false => self.dirs.remove(&dir),
            };
        }
        for (file, _) in &rows {
            self.forget(*file);
        }
        for (file, row) in rows {
            if let Some(row) = row {
                self.take(file, row);
            }
        }
        reached
    }

    /// The rows shown as `class`, each with its FileId.
    pub fn class(&self, class: Class) -> impl Iterator<Item = (FileId, &Row)> + '_ {
        let rows = self.rows.iter().map(|(file, row)| (*file, row));
        rows.filter(move |(_, row)| row.class() == class)
    }

    /// The rows whose file is nowhere shown as `class`, each with its FileId.
    pub fn missing(&self, class: Class) -> impl Iterator<Item = (FileId, &Row)> + '_ {
        let rows = self
            .missing
            .iter()
            .filter_map(|file| Some((*file, self.rows.get(file)?)));
        rows.filter(move |(_, row)| row.class() == class)
    }

    /// The row `file`, where drawbar shows it as `class`.
    pub fn row(&self, file: FileId, class: Class) -> Option<&Row> {
        self.rows.get(&file).filter(|row| row.class() == class)
    }

    /// The files drawbar does not open, in path order.
    pub fn others(&self) -> Vec<LibPath> {
        sorted(
            self.others
                .iter()
                .filter_map(|file| self.path(*file).cloned()),
        )
    }

    /// The paths of the absent rows, which a new file must not take, in order.
    pub fn held(&self) -> Vec<LibPath> {
        let held = self.missing(Class::Absent).map(|(_, row)| row.path.clone());
        sorted(held)
    }

    /// The entities wearing the tag `name`.
    pub fn wearing(&self, name: &str) -> Vec<EntityId> {
        let worn = self.tagged().filter(|(_, row)| row.worn.contains(name));
        worn.filter_map(|(_, row)| row.entity).collect()
    }

    /// The rows wearing tags, each with its FileId.
    pub fn tagged(&self) -> impl Iterator<Item = (FileId, &Row)> + '_ {
        self.tagged
            .iter()
            .filter_map(|file| Some((*file, self.rows.get(file)?)))
    }

    /// The row of `entity`'s file, where it is one of these.
    pub fn row_of(&self, entity: EntityId) -> Option<FileId> {
        self.rows_of.get(&entity).copied()
    }

    /// The entity bound to the file of the row `file`.
    pub fn entity(&self, file: FileId) -> Option<EntityId> {
        self.rows.get(&file)?.entity
    }

    /// Where the row `file` is.
    pub fn path(&self, file: FileId) -> Option<&LibPath> {
        Some(&self.rows.get(&file)?.path)
    }

    /// Whether the row `file`'s file is nowhere.
    pub fn gone(&self, file: FileId) -> bool {
        self.missing.contains(&file)
    }

    /// What this shows, each row by what it shows: two views of one library number
    /// their rows each their own way.
    #[cfg(test)]
    pub fn rows_shown(&self) -> impl PartialEq + std::fmt::Debug + '_ {
        let mut rows: Vec<String> = self.rows.values().map(|row| format!("{row:?}")).collect();
        rows.sort();
        let fields = (&self.names, &self.taken, &self.dirs);
        (rows, fields)
    }

    fn forget(&mut self, file: FileId) {
        let Some(row) = self.rows.remove(&file) else {
            return;
        };
        if let Some(entity) = row
            .entity
            .filter(|entity| self.rows_of.get(entity) == Some(&file))
        {
            self.rows_of.remove(&entity);
        }
        self.tagged.remove(&file);
        self.others.remove(&file);
        self.missing.remove(&file);
    }

    fn take(&mut self, file: FileId, row: Row) {
        if let Some(entity) = row.entity {
            self.rows_of.insert(entity, file);
        }
        if !row.worn.is_empty() {
            self.tagged.insert(file);
        }
        match row.class() {
            Class::Other => _ = self.others.insert(file),
            Class::Lost | Class::Absent => _ = self.missing.insert(file),
            Class::File => {}
        }
        self.rows.insert(file, row);
    }
}

/// The entities whose tags show on `entity`'s row: itself, and each joined into it.
fn showing_on(view: &View, entity: EntityId) -> impl Iterator<Item = toshokan::EntityView<'_>> {
    let joined = view
        .joined()
        .iter()
        .filter(move |joined| joined.into == entity);
    let joined = joined.filter_map(|joined| view.entity(joined.entity));
    view.entity(entity).into_iter().chain(joined)
}

impl Change {
    /// What `view` changed since `before`, the view the app was last shown, with who
    /// changed each row; everything where there was none, or where the change reaches
    /// most of what `view` shows, as the first listing of every file does.
    pub fn since(view: &View, before: Option<&View>) -> Changes {
        let Some(before) = before else {
            return Changes {
                change: Change::Whole(Shown::of(view)),
                rows: Vec::new(),
            };
        };
        let changed = view.changes_since(before);
        let mut entities: BTreeSet<EntityId> =
            view.folded().changed(before.folded()).into_iter().collect();
        let (joined, was): (BTreeSet<_>, BTreeSet<_>) = (
            view.joined().iter().copied().collect(),
            before.joined().iter().copied().collect(),
        );
        for join in joined.symmetric_difference(&was) {
            entities.extend([join.entity, join.into]);
        }
        let into = joined
            .union(&was)
            .filter(|join| entities.contains(&join.entity));
        let into: Vec<EntityId> = into.map(|join| join.into).collect();
        entities.extend(into);
        let mut files: BTreeSet<FileId> = changed.files.iter().map(|change| change.file).collect();
        let rows_of = |entity: &EntityId| {
            let now = view.row_of(*entity).map(|row| row.file);
            now.into_iter()
                .chain(before.row_of(*entity).map(|row| row.file))
        };
        files.extend(entities.iter().flat_map(rows_of));
        let shown = || view.rows().count() + view.entities().len();
        let reach = files.len() + entities.len();
        if reach > MOST_KEYS && 2 * reach > shown() {
            return Changes {
                change: Change::Whole(Shown::of(view)),
                rows: changed.files,
            };
        }
        let values_changed = |key| {
            let now = values(view, key);
            (now != values(before, key)).then_some(now)
        };
        let rows = files.into_iter().map(|file| {
            let row = view.row(file).and_then(|row| Row::of(view, row));
            (file, row)
        });
        let dirs = changed
            .dirs
            .iter()
            .filter_map(|dir| Some((visible(&dir.path)?, dir.what == DirWhat::Made)));
        Changes {
            change: Change::Rows {
                rows: rows.collect(),
                names: values_changed(TAG_NAMES),
                taken: values_changed(TAKEN_IN),
                dirs: dirs.collect(),
            },
            rows: changed.files,
        }
    }
}

fn sorted(paths: impl Iterator<Item = LibPath>) -> Vec<LibPath> {
    let mut paths: Vec<LibPath> = paths.collect();
    paths.sort();
    paths
}

/// The most rows and entities a change names before it may be sent whole.
const MOST_KEYS: usize = 1024;

/// Every value of the set `key` on any entity.
fn values(view: &View, key: toshokan::Set<String>) -> BTreeSet<String> {
    let listed = view.values(key).values.into_iter();
    listed.map(|(value, _)| value).collect()
}

/// `path` as drawbar shows it: none with a name that starts with a dot.
fn visible(path: &RelPath) -> Option<LibPath> {
    let path = LibPath::parse(path.as_str())?;
    let hidden = path.components().any(|name| name.starts_with('.'));
    (!hidden).then_some(path)
}

#[cfg(test)]
mod tests {
    use toshokan::blocking::Library;
    use toshokan::env::{ExactNames, SeededRandom, TestClock};
    use toshokan::simulator::Machine;
    use toshokan::{Expect, Io, MemDisk, Root};

    use super::*;
    use crate::store::keys;

    /// Until a change joins them, the file two drawbars each tagged before either saw the
    /// other's tag shows the tags of both, and the entity joined into its own shows nothing.
    #[test]
    fn a_file_two_entities_hold_shows_the_tags_of_both_until_they_are_joined() {
        let folder = MemDisk::new();
        let at = RelPath::new("Grand.ne5p").unwrap();
        let bytes = b"grand".to_vec();
        let path = at.clone();
        let made = Io::Create {
            root: Root::Folder,
            path,
            bytes,
        };
        folder.perform(made).unwrap();
        let clock = TestClock::at(1_000);
        let open = |seed| {
            let random = Box::new(SeededRandom::new(seed));
            let env =
                super::super::keeper::env(Box::new(clock.clone()), random, Box::new(ExactNames));
            let machine = Machine {
                folder: folder.clone(),
                local: MemDisk::new(),
            };
            let (mut library, _) =
                Library::open(machine, keys::layout(), &keys::schema(), env).unwrap();
            library.rescan().unwrap();
            library
        };
        let (mut here, mut there) = (open(1), open(2));
        for (library, tag) in [(&mut here, "Live"), (&mut there, "Studio")] {
            let listed = library.view().listed(&at).unwrap();
            let expect = Expect::Listed {
                len: listed.len,
                modified: listed.modified,
            };
            let intent = library.intent("Tag").create(|e| {
                e.adopt(&at, expect).add(TAGS, tag.to_string());
            });
            intent.0.commit().unwrap();
        }
        here.refresh().unwrap();
        let view = here.view();
        let [joined] = view.joined() else {
            panic!("one entity joined: {:?}", view.joined());
        };
        let shown = Shown::of(&view);
        let rows: Vec<&Row> = shown.rows.values().collect();
        let [row] = rows[..] else {
            panic!("one row: {rows:?}");
        };
        let both = BTreeSet::from(["Live".to_string(), "Studio".to_string()]);
        assert_eq!((row.entity, &row.worn), (Some(joined.into), &both));
        assert_eq!(row.path, LibPath::parse("Grand.ne5p").unwrap());
    }
}
