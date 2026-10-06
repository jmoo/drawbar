//! Identifiers and clocks.
//!
//! Every id is 128 bits written as 32 lowercase hexadecimal digits. Uppercase and
//! any other length are refused, so each id has exactly one text form.

use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Deserializer, Serialize, Serializer};

use crate::error::{Error, Result};

macro_rules! id128 {
    ($(#[$doc:meta])* $name:ident, $what:literal) => {
        $(#[$doc])*
        #[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
        pub struct $name(u128);

        impl $name {
            pub const fn from_u128(value: u128) -> Self {
                Self(value)
            }

            pub const fn to_u128(self) -> u128 {
                self.0
            }

            /// The big-endian bytes, whose hexadecimal is the text form.
            pub const fn to_bytes(self) -> [u8; 16] {
                self.0.to_be_bytes()
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                write!(f, "{:032x}", self.0)
            }
        }

        impl fmt::Debug for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                write!(f, concat!(stringify!($name), "({})"), self)
            }
        }

        impl FromStr for $name {
            type Err = Error;

            fn from_str(text: &str) -> Result<Self> {
                parse_hex128($what, text).map(Self)
            }
        }

        impl Serialize for $name {
            fn serialize<S: Serializer>(
                &self,
                serializer: S,
            ) -> std::result::Result<S::Ok, S::Error> {
                serializer.collect_str(self)
            }
        }

        impl<'de> Deserialize<'de> for $name {
            fn deserialize<D: Deserializer<'de>>(
                deserializer: D,
            ) -> std::result::Result<Self, D::Error> {
                let text = <std::borrow::Cow<'de, str>>::deserialize(deserializer)?;
                text.parse().map_err(serde::de::Error::custom)
            }
        }
    };
}

id128!(
    /// A writer: one running instance's log. Random, and never reused once its
    /// genesis entry is in the folder.
    WriterId,
    "writer id"
);

id128!(
    /// An entity: a thing facts are about, usually one of the user's files. Random.
    EntityId,
    "entity id"
);

id128!(
    /// The name of a log segment, `<name>.jsonl`. Random, so no two histories pick the
    /// same name. Names are advisory: readers place entries by chain, not by name.
    SegmentName,
    "segment name"
);

id128!(
    /// The random name of a snapshot, pending record, trash item or staged file.
    Nonce,
    "nonce"
);

id128!(
    /// An entry's hash: its id, the link the next entry names as `prev`, and its
    /// line's checksum.
    EntryHash,
    "entry hash"
);

id128!(
    /// What the app's identity function says a file's contents are. Equal
    /// identities are taken as equal contents.
    Identity,
    "identity"
);

impl EntryHash {
    /// The `prev` of a writer's genesis entry.
    pub const ZERO: Self = Self(0);

    /// The first 128 bits of BLAKE3 over `prev`'s 16 bytes followed by `json`.
    pub fn of(prev: EntryHash, json: &[u8]) -> Self {
        let mut hasher = blake3::Hasher::new();
        hasher.update(&prev.to_bytes());
        hasher.update(json);
        let digest = hasher.finalize();
        let mut first = [0; 16];
        first.copy_from_slice(&digest.as_bytes()[..16]);
        Self(u128::from_be_bytes(first))
    }
}

fn parse_hex128(what: &'static str, text: &str) -> Result<u128> {
    let well_formed =
        text.len() == 32 && text.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'));
    let invalid = || Error::InvalidId {
        what,
        text: text.chars().take(64).collect(),
    };
    if !well_formed {
        return Err(invalid());
    }
    u128::from_str_radix(text, 16).map_err(|_| invalid())
}

/// A hybrid logical clock reading: wall time in milliseconds since the Unix epoch,
/// and a counter that orders events within one millisecond. Orders entries for
/// display only; the writer id breaks ties.
///
/// Written as `[wall_ms, counter]`.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug, Default)]
pub struct Hlc {
    pub wall_ms: u64,
    pub counter: u32,
}

impl Hlc {
    pub const ZERO: Self = Self {
        wall_ms: 0,
        counter: 0,
    };

    /// The reading for a local event at wall time `now_ms`: after `self`, and at
    /// `now_ms` when the wall clock is ahead. A full counter moves on to the next
    /// millisecond. `None` only after the last reading there is.
    pub fn tick(self, now_ms: u64) -> Option<Self> {
        if now_ms > self.wall_ms {
            return Some(Self {
                wall_ms: now_ms,
                counter: 0,
            });
        }
        match self.counter.checked_add(1) {
            Some(counter) => Some(Self { counter, ..self }),
            None => Some(Self {
                wall_ms: self.wall_ms.checked_add(1)?,
                counter: 0,
            }),
        }
    }

    /// The reading after seeing `other`, so the next tick orders after it.
    pub fn observe(self, other: Hlc) -> Self {
        self.max(other)
    }
}

impl Serialize for Hlc {
    fn serialize<S: Serializer>(&self, serializer: S) -> std::result::Result<S::Ok, S::Error> {
        (self.wall_ms, self.counter).serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for Hlc {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> std::result::Result<Self, D::Error> {
        let (wall_ms, counter) = <(u64, u32)>::deserialize(deserializer)?;
        Ok(Self { wall_ms, counter })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_id_has_one_text_form() {
        let id = WriterId::from_u128(0xab);
        assert_eq!(id.to_string(), "000000000000000000000000000000ab");
        assert_eq!(id.to_string().parse::<WriterId>().unwrap(), id);
        for text in [
            "000000000000000000000000000000AB",
            "00000000000000000000000000000ab",
            "0000000000000000000000000000000ab",
            "+00000000000000000000000000000ab",
            "",
        ] {
            assert!(text.parse::<WriterId>().is_err(), "{text:?} accepted");
        }
        assert_eq!(serde_json::to_string(&id).unwrap(), format!("\"{id}\""));
        assert!(serde_json::from_str::<WriterId>("\"AB\"").is_err());
    }

    // BLAKE3 of 16 zero bytes then `{}`, first 16 bytes; computed with b3sum.
    #[test]
    fn an_entry_hash_covers_its_predecessor_and_json() {
        let genesis = EntryHash::of(EntryHash::ZERO, b"{}");
        assert_eq!(genesis.to_string(), "3ae46bcf71e48919d13408596da40d47");
        assert_ne!(EntryHash::of(genesis, b"{}"), genesis);
        assert_ne!(EntryHash::of(EntryHash::ZERO, b"{ }"), genesis);
    }

    #[test]
    fn a_clock_ticks_forward_and_follows_the_wall() {
        let start = Hlc {
            wall_ms: 10,
            counter: 3,
        };
        let cases = [
            (
                11,
                Some(Hlc {
                    wall_ms: 11,
                    counter: 0,
                }),
            ),
            (
                10,
                Some(Hlc {
                    wall_ms: 10,
                    counter: 4,
                }),
            ),
            (
                2,
                Some(Hlc {
                    wall_ms: 10,
                    counter: 4,
                }),
            ),
        ];
        for (now, expected) in cases {
            assert_eq!(start.tick(now), expected, "at {now}");
        }
        let full = Hlc {
            wall_ms: 10,
            counter: u32::MAX,
        };
        let next = Hlc {
            wall_ms: 11,
            counter: 0,
        };
        assert_eq!(full.tick(10), Some(next));
        let last = Hlc {
            wall_ms: u64::MAX,
            counter: u32::MAX,
        };
        assert_eq!(last.tick(10), None);
        assert_eq!(start.observe(full), full);
        assert_eq!(serde_json::to_string(&start).unwrap(), "[10,3]");
    }
}
