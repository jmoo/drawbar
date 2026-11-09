//! Which workspace id stands for which row of the library's view this session, and the
//! keys unsaved edits are kept by across sessions.
//!
//! A workspace id and a FileId each live for one session; an unsaved edit is kept by the
//! entity bound to its file, or, for a file nothing is said about, by its path alone.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use toshokan::{EntityId, FileId};

use super::LibPath;

/// What an unsaved edit is kept by: the entity bound to its file, or its path, or, for
/// an unsaved copy with no file, the path it was last at and a number of its own.
///
/// Serialized as one string, `e:<entity>`, `f:<path>` or `c:<number>:<path>`, the
/// number in 32 hex digits, so a map keyed by it serializes wherever a map keyed by
/// strings does.
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Debug)]
pub enum Key {
    Entity(EntityId),
    File(LibPath),
    Copy(LibPath, u128),
}

impl Serialize for Key {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let text = match self {
            Key::Entity(entity) => format!("e:{entity}"),
            Key::File(path) => format!("f:{path}"),
            Key::Copy(path, number) => format!("c:{number:032x}:{path}"),
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
            Some(("c", copy)) => {
                let (number, path) = copy.split_once(':').ok_or_else(wrong)?;
                let number = u128::from_str_radix(number, 16).map_err(|_| wrong())?;
                let path = LibPath::parse(path).ok_or_else(wrong)?;
                Ok(Key::Copy(path, number))
            }
            _ => Err(wrong()),
        }
    }
}

/// Workspace ids and the rows of the library's view, one to one.
#[derive(Default)]
pub struct Ids {
    files: BTreeMap<u64, FileId>,
    ids: BTreeMap<FileId, u64>,
}

impl Ids {
    pub fn file(&self, id: u64) -> Option<FileId> {
        self.files.get(&id).copied()
    }

    pub fn id(&self, file: FileId) -> Option<u64> {
        self.ids.get(&file).copied()
    }

    /// Every id with its row, by id.
    pub fn iter(&self) -> impl Iterator<Item = (u64, FileId)> + '_ {
        self.files.iter().map(|(id, file)| (*id, *file))
    }

    /// Give `id` the row `file`, which leaves whatever row or id either had before, and
    /// return the id that row leaves.
    pub fn insert(&mut self, id: u64, file: FileId) -> Option<u64> {
        self.remove(id);
        let had = self.ids.insert(file, id);
        had.inspect(|had| _ = self.files.remove(had));
        self.files.insert(id, file);
        had
    }

    pub fn remove(&mut self, id: u64) -> Option<FileId> {
        let file = self.files.remove(&id)?;
        self.ids.remove(&file);
        Some(file)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn file(n: u64) -> FileId {
        serde_json::from_str(&n.to_string()).unwrap()
    }

    #[test]
    fn a_key_reads_back_as_it_was_written() {
        let path = LibPath::parse("Live/a: b.ne5p").unwrap();
        let keys = [
            Key::Entity(EntityId::from_u128(7)),
            Key::File(path.clone()),
            Key::Copy(path, u128::MAX - 1),
        ];
        for key in keys {
            let text = serde_json::to_string(&key).unwrap();
            assert_eq!(serde_json::from_str::<Key>(&text).unwrap(), key, "{text}");
        }
        assert!(serde_json::from_str::<Key>(r#""c:12:a""#).is_ok());
        assert!(serde_json::from_str::<Key>(r#""c:zz:a""#).is_err());
    }

    #[test]
    fn an_id_and_a_row_stand_for_each_other_alone() {
        let mut ids = Ids::default();
        assert_eq!(ids.insert(1, file(10)), None);
        assert_eq!(ids.insert(2, file(20)), None);
        assert_eq!(ids.insert(1, file(20)), Some(2), "the row leaves 2");
        assert_eq!(ids.file(1), Some(file(20)));
        assert_eq!(ids.file(2), None, "the row went to 1");
        assert_eq!(ids.id(file(10)), None, "1 left its row");
        assert_eq!(ids.iter().count(), 1);
        assert_eq!(
            ids.insert(1, file(20)),
            None,
            "a row an id has leaves no other"
        );
    }
}
