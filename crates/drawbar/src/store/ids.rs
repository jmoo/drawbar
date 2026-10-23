//! Which workspace id stands for which file of the library this session.
//!
//! A workspace id lives for one session; the library knows a file by the entity bound to
//! it, or, for a file nothing is said about, by its path alone.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use toshokan::EntityId;

use super::LibPath;

/// How the library knows a file: by the entity bound to it, or by its path.
///
/// Serialized as one string, `e:<entity>` or `f:<path>`, so a map keyed by it serializes
/// wherever a map keyed by strings does.
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Debug)]
pub enum Key {
    Entity(EntityId),
    File(LibPath),
}

impl Serialize for Key {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let text = match self {
            Key::Entity(entity) => format!("e:{entity}"),
            Key::File(path) => format!("f:{path}"),
        };
        serializer.serialize_str(&text)
    }
}

impl<'de> Deserialize<'de> for Key {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Key, D::Error> {
        use serde::de::Error as _;

        let text = <std::borrow::Cow<'de, str>>::deserialize(deserializer)?;
        let wrong = || D::Error::custom(format!("{text:?} names no file of a library"));
        match text.split_once(':') {
            Some(("e", entity)) => entity.parse().map(Key::Entity).map_err(|_| wrong()),
            Some(("f", path)) => LibPath::parse(path).map(Key::File).ok_or_else(wrong),
            _ => Err(wrong()),
        }
    }
}

impl Key {
    pub fn entity(&self) -> Option<EntityId> {
        match self {
            Key::Entity(entity) => Some(*entity),
            Key::File(_) => None,
        }
    }
}

/// Workspace ids and keys, one to one.
#[derive(Default)]
pub struct Ids {
    keys: BTreeMap<u64, Key>,
    ids: BTreeMap<Key, u64>,
}

impl Ids {
    pub fn key(&self, id: u64) -> Option<&Key> {
        self.keys.get(&id)
    }

    pub fn id(&self, key: &Key) -> Option<u64> {
        self.ids.get(key).copied()
    }

    /// Every id with its key, by id.
    pub fn iter(&self) -> impl Iterator<Item = (u64, &Key)> {
        self.keys.iter().map(|(id, key)| (*id, key))
    }

    /// Give `id` the key `key`, which leaves whatever id or key either had before.
    pub fn insert(&mut self, id: u64, key: Key) {
        self.remove(id);
        if let Some(had) = self.ids.remove(&key) {
            self.keys.remove(&had);
        }
        self.keys.insert(id, key.clone());
        self.ids.insert(key, id);
    }

    pub fn remove(&mut self, id: u64) -> Option<Key> {
        let key = self.keys.remove(&id)?;
        self.ids.remove(&key);
        Some(key)
    }

    /// Follow a rename of `from` to `to` drawbar made: each file known by a path at or
    /// under `from` is known by where it went. An entity's file is followed by toshokan.
    pub fn moved(&mut self, from: &LibPath, to: &LibPath) {
        let moving: Vec<(u64, LibPath)> = self
            .keys
            .iter()
            .filter_map(|(id, key)| match key {
                Key::File(path) => Some((*id, path.moved(from, to)?)),
                Key::Entity(_) => None,
            })
            .collect();
        for (id, path) in moving {
            self.insert(id, Key::File(path));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn file(text: &str) -> Key {
        Key::File(LibPath::parse(text).unwrap())
    }

    #[test]
    fn an_id_and_a_key_stand_for_each_other_alone() {
        let mut ids = Ids::default();
        let entity = Key::Entity(EntityId::from_u128(7));
        ids.insert(1, file("a.ns3"));
        ids.insert(2, entity.clone());
        ids.insert(1, entity.clone());
        assert_eq!(ids.key(1), Some(&entity));
        assert_eq!(ids.key(2), None, "the key went to 1");
        assert_eq!(ids.id(&file("a.ns3")), None, "1 left its path");
        assert_eq!(ids.iter().count(), 1);
    }

    #[test]
    fn a_rename_drawbar_made_moves_the_paths_under_it_and_no_entity() {
        let mut ids = Ids::default();
        let entity = Key::Entity(EntityId::from_u128(7));
        ids.insert(1, file("Old/a.ns3"));
        ids.insert(2, file("Old/Deep/b.wav"));
        ids.insert(3, file("Older/c.ns3"));
        ids.insert(4, entity.clone());
        let (from, to) = (
            LibPath::parse("Old").unwrap(),
            LibPath::parse("New").unwrap(),
        );
        ids.moved(&from, &to);
        assert_eq!(ids.id(&file("New/a.ns3")), Some(1));
        assert_eq!(ids.id(&file("New/Deep/b.wav")), Some(2));
        assert_eq!(ids.id(&file("Older/c.ns3")), Some(3));
        assert_eq!(ids.id(&entity), Some(4));
    }
}
