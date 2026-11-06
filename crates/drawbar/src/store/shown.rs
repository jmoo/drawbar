//! What a library's view shows the app: each file drawbar lists, by the key the library
//! knows it by, and the tags. The backend that holds the library sends it whole once, as
//! the library opens, and after that only what each answer changed, so the app never
//! holds a view of its own.

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};
use toshokan::{EntityId, FileState, RelPath, View};

use super::keys::{Came, ORIGIN, TAGS, TAG_NAMES, TAKEN_IN};
use super::{Key, LibPath, Stat};

#[derive(Clone, Default, PartialEq, Debug, Serialize, Deserialize)]
pub struct Shown {
    /// Each file drawbar opens, with its path and what the last scan found there.
    pub files: BTreeMap<Key, (LibPath, Option<Stat>)>,
    /// Files drawbar does not open.
    pub others: BTreeMap<Key, LibPath>,
    /// Entities wearing tags whose file is nowhere, with where it was. One that says only
    /// where its file came from keeps nothing worth showing without the file.
    pub lost: BTreeMap<EntityId, LibPath>,
    /// Where the asset of each entity that says so came from.
    pub came: BTreeMap<EntityId, Came>,
    /// The tags each entity wears.
    pub worn: BTreeMap<EntityId, BTreeSet<String>>,
    /// The library's list of tags.
    pub names: BTreeSet<String>,
    /// The lists older drawbars kept that the library took in, by their hashes.
    pub taken: BTreeSet<String>,
}

/// What one key shows.
#[derive(Clone, Default, PartialEq, Debug, Serialize, Deserialize)]
pub struct Showing {
    file: Option<(LibPath, Option<Stat>)>,
    other: Option<LibPath>,
    lost: Option<LibPath>,
    came: Option<Came>,
    worn: Option<BTreeSet<String>>,
}

/// What an answer's view changed of what the app shows.
#[derive(Clone, PartialEq, Debug, Serialize, Deserialize)]
pub enum Change {
    /// Everything, as the library opens, once the app lost an answer, or where most
    /// of what it shows changed.
    Whole(Shown),
    /// What each key the view's changes reach shows now, and the list of tags and the
    /// lists taken in where they changed.
    Keys {
        keys: Vec<(Key, Showing)>,
        names: Option<BTreeSet<String>>,
        taken: Option<BTreeSet<String>>,
    },
}

impl Shown {
    /// What `view` shows, leaving out every path with a name that starts with a dot and
    /// every path that is no [`LibPath`].
    pub fn of(view: &View) -> Shown {
        let mut shown = Shown {
            names: values(view, TAG_NAMES),
            taken: values(view, TAKEN_IN),
            ..Shown::default()
        };
        for entity in view.entities() {
            let key = Key::Entity(entity.id());
            let showing = Showing::of(view, &key);
            shown.take(key, showing);
        }
        for path in view.unbound() {
            if let Some(at) = LibPath::parse(path.as_str()) {
                let key = Key::File(at);
                let showing = Showing::of(view, &key);
                shown.take(key, showing);
            }
        }
        shown
    }

    /// Take in `change`, and return the key of each file it changed; `None` where it
    /// changed everything.
    pub fn apply(&mut self, change: Change) -> Option<BTreeSet<Key>> {
        match change {
            Change::Whole(shown) => {
                *self = shown;
                None
            }
            Change::Keys { keys, names, taken } => {
                if let Some(names) = names {
                    self.names = names;
                }
                if let Some(taken) = taken {
                    self.taken = taken;
                }
                let mut reached = BTreeSet::new();
                for (key, showing) in keys {
                    self.forget(&key);
                    self.take(key.clone(), showing);
                    reached.insert(key);
                }
                Some(reached)
            }
        }
    }

    /// The files drawbar does not open, in path order.
    pub fn others(&self) -> Vec<LibPath> {
        let mut others: Vec<LibPath> = self.others.values().cloned().collect();
        others.sort();
        others
    }

    /// The entities wearing the tag `name`.
    pub fn wearing(&self, name: &str) -> Vec<EntityId> {
        let worn = self.worn.iter().filter(|(_, worn)| worn.contains(name));
        worn.map(|(entity, _)| *entity).collect()
    }

    fn forget(&mut self, key: &Key) {
        self.files.remove(key);
        self.others.remove(key);
        if let Some(entity) = key.entity() {
            self.lost.remove(&entity);
            self.came.remove(&entity);
            self.worn.remove(&entity);
        }
    }

    fn take(&mut self, key: Key, showing: Showing) {
        let Showing {
            file,
            other,
            lost,
            came,
            worn,
        } = showing;
        if let Some(entity) = key.entity() {
            if let Some(lost) = lost {
                self.lost.insert(entity, lost);
            }
            if let Some(came) = came {
                self.came.insert(entity, came);
            }
            if let Some(worn) = worn {
                self.worn.insert(entity, worn);
            }
        }
        if let Some(other) = other {
            self.others.insert(key.clone(), other);
        }
        if let Some(file) = file {
            self.files.insert(key, file);
        }
    }
}

impl Showing {
    /// What `view` shows of `key`.
    fn of(view: &View, key: &Key) -> Showing {
        let mut showing = Showing::default();
        let entity = match key {
            Key::Entity(entity) => *entity,
            Key::File(path) => {
                let Ok(at) = RelPath::new(path.as_str()) else {
                    return showing;
                };
                if view.is_unbound(&at) {
                    showing.list(view, &at);
                }
                return showing;
            }
        };
        let Some(shown) = view.entity(entity) else {
            return showing;
        };
        // Until a change joins it into the file's entity, that entity shows its tags.
        if view.joined().iter().any(|joined| joined.entity == entity) {
            return showing;
        }
        showing.came = toshokan::Field::shown(&shown.get(ORIGIN)).copied();
        let joined = view.joined().iter().filter(|joined| joined.into == entity);
        let joined = joined.filter_map(|joined| view.entity(joined.entity));
        let (mut worn, mut unreadable) = (BTreeSet::new(), false);
        for members in std::iter::once(shown)
            .chain(joined)
            .map(|e| e.members(TAGS))
        {
            unreadable |= !members.unreadable.is_empty();
            worn.extend(members.values);
        }
        let tagged = !worn.is_empty() || unreadable;
        if !worn.is_empty() {
            showing.worn = Some(worn);
        }
        let Some(file) = shown.file() else {
            return showing;
        };
        match file.state {
            FileState::Missing if tagged => showing.lost = visible(&file.path),
            FileState::Missing => {}
            _ => showing.list(view, &file.path),
        }
        showing
    }

    fn list(&mut self, view: &View, at: &RelPath) {
        let Some(path) = visible(at) else {
            return;
        };
        if !super::opens(path.leaf()) {
            self.other = Some(path);
            return;
        }
        let stat = view.listed(at).map(|listed| Stat {
            len: listed.len,
            modified: listed.modified,
        });
        self.file = Some((path, stat));
    }
}

impl Change {
    /// What `view` changed since `before`, the view the app was last shown; everything
    /// where there was none, or where the change reaches most of what `view` shows, as
    /// the first listing of every file does.
    pub fn since(view: &View, before: Option<&View>) -> Change {
        let Some(before) = before else {
            return Change::Whole(Shown::of(view));
        };
        let changed = view.changed_since(before);
        let mut keys: BTreeSet<Key> = changed.entities.iter().copied().map(Key::Entity).collect();
        let joined = view.joined().iter().chain(before.joined());
        let joined = joined.filter(|joined| changed.entities.contains(&joined.entity));
        keys.extend(joined.map(|joined| Key::Entity(joined.into)));
        for path in &changed.files {
            if let Some(entity) = view.at(path).map(|entity| entity.id()) {
                keys.insert(Key::Entity(entity));
            }
            if let Some(at) = LibPath::parse(path.as_str()) {
                keys.insert(Key::File(at));
            }
        }
        let shown = || view.entities().len() + view.unbound().count();
        if keys.len() > MOST_KEYS && 2 * keys.len() > shown() {
            return Change::Whole(Shown::of(view));
        }
        let changed = |key| {
            let now = values(view, key);
            (now != values(before, key)).then_some(now)
        };
        let keys = keys.into_iter().map(|key| {
            let showing = Showing::of(view, &key);
            (key, showing)
        });
        Change::Keys {
            keys: keys.collect(),
            names: changed(TAG_NAMES),
            taken: changed(TAKEN_IN),
        }
    }
}

/// The most keys a change names before it may be sent whole.
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
        let both = BTreeSet::from(["Live".to_string(), "Studio".to_string()]);
        assert_eq!(shown.worn.get(&joined.into), Some(&both));
        assert_eq!(shown.worn.get(&joined.entity), None);
        assert_eq!(shown.lost, BTreeMap::new());
        let file = LibPath::parse("Grand.ne5p").unwrap();
        let files: Vec<&LibPath> = shown.files.values().map(|(path, _)| path).collect();
        assert_eq!(files, [&file]);
    }
}
