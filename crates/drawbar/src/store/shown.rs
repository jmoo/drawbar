//! What a library's view shows the app: each file drawbar lists, by the key the library
//! knows it by, and the tags. The backend that holds the library sends it whole once, as
//! the library opens, and after that only what each answer changed, so the app never
//! holds a view of its own.

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};
use toshokan::{EntityId, FileState, RelPath, View};

use super::keys::{Came, ORIGIN, TAGS, TAG_NAMES};
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
    /// What each key the view's changes reach shows now, and the list of tags where it
    /// changed.
    Keys {
        keys: Vec<(Key, Showing)>,
        names: Option<BTreeSet<String>>,
    },
}

impl Shown {
    /// What `view` shows, leaving out every path with a name that starts with a dot and
    /// every path that is no [`LibPath`].
    pub fn of(view: &View) -> Shown {
        let mut shown = Shown {
            names: names(view),
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
            Change::Keys { keys, names } => {
                if let Some(names) = names {
                    self.names = names;
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
        showing.came = toshokan::Field::shown(&shown.get(ORIGIN)).copied();
        let worn = shown.members(TAGS);
        let tagged = !worn.values.is_empty() || !worn.unreadable.is_empty();
        if !worn.values.is_empty() {
            showing.worn = Some(worn.values.into_iter().collect());
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
        let now = names(view);
        let names = (now != self::names(before)).then_some(now);
        let keys = keys.into_iter().map(|key| {
            let showing = Showing::of(view, &key);
            (key, showing)
        });
        Change::Keys {
            keys: keys.collect(),
            names,
        }
    }
}

/// The most keys a change names before it may be sent whole.
const MOST_KEYS: usize = 1024;

/// The library's list of tags.
fn names(view: &View) -> BTreeSet<String> {
    let listed = view.values(TAG_NAMES).values.into_iter();
    listed.map(|(name, _)| name).collect()
}

/// `path` as drawbar shows it: none with a name that starts with a dot.
fn visible(path: &RelPath) -> Option<LibPath> {
    let path = LibPath::parse(path.as_str())?;
    let hidden = path.components().any(|name| name.starts_with('.'));
    (!hidden).then_some(path)
}
