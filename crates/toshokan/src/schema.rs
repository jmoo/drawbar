//! Typed keys and the fields they read.
//!
//! An app declares each key once, as a constant, and lists them in its [`Schema`]:
//!
//! ```
//! use toshokan::{Register, Schema, Set};
//!
//! const ORIGIN: Register<String> = Register::new("origin");
//! const TAGS: Set<String> = Set::new("tags");
//! let schema = Schema::of(&[ORIGIN.key(), TAGS.key()]).unwrap();
//! assert!(Schema::of(&[ORIGIN.key(), Set::<u8>::new("origin").key()]).is_err());
//! ```

use std::cmp::Ordering;
use std::collections::BTreeMap;
use std::fmt;
use std::hash::{Hash, Hasher};
use std::marker::PhantomData;

use serde::de::DeserializeOwned;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use serde_json::value::RawValue;

use crate::error::{Error, Result};
use crate::ids::{EntryHash, Hlc, WriterId};

/// What a key's values may be. The app owns the type; toshokan stores its JSON.
pub trait Value: Serialize + DeserializeOwned + Ord + Clone {}

impl<T: Serialize + DeserializeOwned + Ord + Clone> Value for T {}

/// A multi-value register: each write replaces the writes it observed, and
/// concurrent writes of different values all survive, in conflict.
pub struct Register<T> {
    name: &'static str,
    value: PhantomData<fn() -> T>,
}

/// An observed-remove set: a remove takes away the adds it observed, so a
/// concurrent add survives.
pub struct Set<T> {
    name: &'static str,
    value: PhantomData<fn() -> T>,
}

macro_rules! key_type {
    ($name:ident, $kind:expr) => {
        impl<T> $name<T> {
            pub const fn new(name: &'static str) -> Self {
                Self {
                    name,
                    value: PhantomData,
                }
            }

            pub const fn name(&self) -> &'static str {
                self.name
            }

            pub const fn key(&self) -> Key {
                Key {
                    name: self.name,
                    kind: $kind,
                }
            }
        }

        impl<T> Clone for $name<T> {
            fn clone(&self) -> Self {
                *self
            }
        }

        impl<T> Copy for $name<T> {}

        impl<T> fmt::Debug for $name<T> {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                write!(f, concat!(stringify!($name), "({:?})"), self.name)
            }
        }
    };
}

key_type!(Register, KeyKind::Register);
key_type!(Set, KeyKind::Set);

/// A typed key of either kind, as the lookups that read both take it.
pub trait Keyed: Copy {
    type Value: Value;

    fn key(&self) -> Key;
}

impl<T: Value> Keyed for Register<T> {
    type Value = T;

    fn key(&self) -> Key {
        Register::key(self)
    }
}

impl<T: Value> Keyed for Set<T> {
    type Value = T;

    fn key(&self) -> Key {
        Set::key(self)
    }
}

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub enum KeyKind {
    Register,
    Set,
}

/// A key without its value type, for listing in a [`Schema`].
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Key {
    pub name: &'static str,
    pub kind: KeyKind,
}

/// The keys an app declares. Facts under other names, which a newer writer may
/// have logged, are kept and reported as unreadable rather than dropped.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Schema {
    keys: BTreeMap<&'static str, KeyKind>,
}

impl Schema {
    /// Refuses a name declared twice, and an empty name.
    pub fn of(keys: &[Key]) -> Result<Self> {
        let mut declared = BTreeMap::new();
        for key in keys {
            if key.name.is_empty() {
                return Err(Error::InvalidKey { name: key.name });
            }
            if declared.insert(key.name, key.kind).is_some() {
                return Err(Error::DuplicateKey { name: key.name });
            }
        }
        Ok(Self { keys: declared })
    }

    pub fn kind(&self, name: &str) -> Option<KeyKind> {
        self.keys.get(name).copied()
    }

    pub fn keys(&self) -> impl Iterator<Item = Key> + '_ {
        self.keys.iter().map(|(&name, &kind)| Key { name, kind })
    }
}

/// JSON kept verbatim: a value that does not decode as its declared type, or
/// anything else a newer writer wrote. Compared and ordered by its text.
#[derive(Clone)]
pub struct Raw(Box<RawValue>);

impl Raw {
    pub fn new(json: &str) -> serde_json::Result<Self> {
        RawValue::from_string(json.to_owned()).map(Self)
    }

    pub fn of<T: Serialize>(value: &T) -> serde_json::Result<Self> {
        serde_json::value::to_raw_value(value).map(Self)
    }

    pub fn decode<T: DeserializeOwned>(&self) -> serde_json::Result<T> {
        serde_json::from_str(self.0.get())
    }

    pub fn as_str(&self) -> &str {
        self.0.get()
    }
}

impl PartialEq for Raw {
    fn eq(&self, other: &Self) -> bool {
        self.as_str() == other.as_str()
    }
}

impl Eq for Raw {}

impl PartialOrd for Raw {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for Raw {
    fn cmp(&self, other: &Self) -> Ordering {
        self.as_str().cmp(other.as_str())
    }
}

impl Hash for Raw {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.as_str().hash(state);
    }
}

impl fmt::Debug for Raw {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Raw({})", self.as_str())
    }
}

impl Serialize for Raw {
    fn serialize<S: Serializer>(&self, serializer: S) -> std::result::Result<S::Ok, S::Error> {
        self.0.serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for Raw {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> std::result::Result<Self, D::Error> {
        Box::<RawValue>::deserialize(deserializer).map(Self)
    }
}

/// One surviving write of a register.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Written<T> {
    pub value: T,
    pub by: WriterId,
    pub at: Hlc,
    pub entry: EntryHash,
}

/// A register as a reader sees it.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Field<T> {
    Unset,
    Value(T),
    /// Concurrent writes of different values survive. `shown` is the value of the
    /// latest by [`Hlc`], then by writer id; a write naming every survivor resolves
    /// the conflict.
    Conflict {
        shown: T,
        all: Vec<Written<T>>,
    },
    /// The surviving value does not decode as the declared type.
    Unreadable(Raw),
}

impl<T> Field<T> {
    /// The value to display: the value, or the shown value of a conflict.
    pub fn shown(&self) -> Option<&T> {
        match self {
            Self::Value(value) | Self::Conflict { shown: value, .. } => Some(value),
            Self::Unset | Self::Unreadable(_) => None,
        }
    }
}

/// The members of a set as a reader sees them.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Members<T> {
    /// Sorted and without duplicates.
    pub values: Vec<T>,
    /// Members that do not decode as the declared type, sorted by text.
    pub unreadable: Vec<Raw>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_schema_refuses_a_name_declared_twice_or_empty() {
        const A: Register<u8> = Register::new("a");
        let schema = Schema::of(&[A.key(), Set::<u8>::new("b").key()]).unwrap();
        assert_eq!(schema.kind("a"), Some(KeyKind::Register));
        assert_eq!(schema.kind("b"), Some(KeyKind::Set));
        assert_eq!(schema.kind("c"), None);
        assert!(matches!(
            Schema::of(&[A.key(), Set::<u8>::new("a").key()]),
            Err(Error::DuplicateKey { name: "a" })
        ));
        assert!(matches!(
            Schema::of(&[Register::<u8>::new("").key()]),
            Err(Error::InvalidKey { name: "" })
        ));
    }

    #[test]
    fn raw_json_keeps_its_text() {
        let text = r#"{"b":1,"a":123456789012345678901234567890}"#;
        let raw = Raw::new(text).unwrap();
        assert_eq!(serde_json::to_string(&raw).unwrap(), text);
        assert_eq!(serde_json::from_str::<Raw>(text).unwrap(), raw);
        assert!(raw.decode::<u8>().is_err());
        assert_eq!(Raw::of(&7u8).unwrap().decode::<u8>().unwrap(), 7);
    }
}
