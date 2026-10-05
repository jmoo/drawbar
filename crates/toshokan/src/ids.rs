//! Who wrote what, and in which order.
//!
//! Every id is allocated by the writer that owns it, so writers never coordinate and
//! never collide. The crate draws no randomness: the caller supplies each [`WriterId`].

use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Deserializer, Serialize, Serializer};

use crate::error::{Error, Result};

/// One writer: an app installation, a device, a process that keeps its own log.
///
/// Written as 32 lowercase hexadecimal characters. Uppercase is refused, so each id
/// has exactly one text form.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct WriterId(u128);

impl WriterId {
    pub const fn from_u128(value: u128) -> Self {
        Self(value)
    }

    pub const fn to_u128(self) -> u128 {
        self.0
    }
}

impl fmt::Display for WriterId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:032x}", self.0)
    }
}

impl fmt::Debug for WriterId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "WriterId({self})")
    }
}

impl FromStr for WriterId {
    type Err = Error;

    fn from_str(text: &str) -> Result<Self> {
        let well_formed =
            text.len() == 32 && text.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'));
        if !well_formed {
            return Err(invalid("writer id", text));
        }
        u128::from_str_radix(text, 16)
            .map(Self)
            .map_err(|_| invalid("writer id", text))
    }
}

/// A decimal `u64` with no sign and no leading zeros, so each number has one text form.
pub(crate) fn canonical_u64(text: &str) -> Option<u64> {
    let canonical = !text.is_empty()
        && text.bytes().all(|b| b.is_ascii_digit())
        && (text == "0" || !text.starts_with('0'));
    canonical.then(|| text.parse().ok()).flatten()
}

fn parse_counter(what: &'static str, text: &str, whole: &str) -> Result<u64> {
    canonical_u64(text).ok_or_else(|| invalid(what, whole))
}

fn invalid(what: &'static str, text: &str) -> Error {
    Error::InvalidId {
        what,
        text: text.to_owned(),
    }
}

/// Ids a writer allocates from its own counter: `<writer>:<counter>`.
macro_rules! scoped_id {
    ($(#[$doc:meta])* $name:ident, $what:literal) => {
        $(#[$doc])*
        #[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
        pub struct $name {
            pub writer: WriterId,
            pub counter: u64,
        }

        impl $name {
            pub const fn new(writer: WriterId, counter: u64) -> Self {
                Self { writer, counter }
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                write!(f, "{}:{}", self.writer, self.counter)
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
                let (writer, counter) = text.split_once(':').ok_or_else(|| invalid($what, text))?;
                Ok(Self {
                    writer: writer.parse().map_err(|_| invalid($what, text))?,
                    counter: parse_counter($what, counter, text)?,
                })
            }
        }

        string_serde!($name);
    };
}

/// Serialize as the `Display` form and parse it back with `FromStr`.
macro_rules! string_serde {
    ($name:ident) => {
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
pub(crate) use string_serde;

string_serde!(WriterId);

scoped_id!(
    /// An entity: the subject of fields and sets, usually one file of the library.
    ///
    /// Ordered by writer, then counter.
    EntityId,
    "entity id"
);

scoped_id!(
    /// A group of entries a person did as one action, and undoes as one.
    ///
    /// Ordered by writer, then counter.
    IntentId,
    "intent id"
);

/// When an entry was written, as a Lamport timestamp made unique by its writer.
///
/// Ordered by `lamport`, then by `writer`; this order decides every last-writer-wins
/// register. Written as `<lamport>@<writer>`.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Version {
    pub lamport: u64,
    pub writer: WriterId,
}

impl Version {
    pub const fn new(lamport: u64, writer: WriterId) -> Self {
        Self { lamport, writer }
    }
}

impl fmt::Display for Version {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}@{}", self.lamport, self.writer)
    }
}

impl fmt::Debug for Version {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Version({self})")
    }
}

impl FromStr for Version {
    type Err = Error;

    fn from_str(text: &str) -> Result<Self> {
        let (lamport, writer) = text
            .split_once('@')
            .ok_or_else(|| invalid("version", text))?;
        Ok(Self {
            lamport: parse_counter("version", lamport, text)?,
            writer: writer.parse().map_err(|_| invalid("version", text))?,
        })
    }
}

string_serde!(Version);

#[cfg(test)]
mod tests {
    use super::*;

    const A: &str = "0123456789abcdef0123456789abcdef";

    #[test]
    fn a_writer_id_prints_as_32_lowercase_hex_digits() {
        assert_eq!(
            WriterId::from_u128(0xff).to_string(),
            format!("{:0>32}", "ff")
        );
        assert_eq!(A.parse::<WriterId>().unwrap().to_string(), A);
    }

    #[test]
    fn a_writer_id_refuses_every_other_spelling() {
        for text in [
            "",
            &A[1..],
            &format!("{A}0"),
            &A.to_uppercase(),
            &format!("+{}", &A[1..]),
            &format!("{}g", &A[1..]),
        ] {
            assert!(text.parse::<WriterId>().is_err(), "{text:?} parsed");
        }
    }

    #[test]
    fn ids_and_versions_round_trip_through_text() {
        let writer: WriterId = A.parse().unwrap();
        let entity = EntityId::new(writer, 42);
        let intent = IntentId::new(writer, 0);
        let version = Version::new(u64::MAX, writer);
        assert_eq!(entity.to_string(), format!("{A}:42"));
        assert_eq!(version.to_string(), format!("{}@{A}", u64::MAX));
        assert_eq!(entity.to_string().parse::<EntityId>().unwrap(), entity);
        assert_eq!(intent.to_string().parse::<IntentId>().unwrap(), intent);
        assert_eq!(version.to_string().parse::<Version>().unwrap(), version);
    }

    #[test]
    fn counters_refuse_signs_leading_zeros_and_overflow() {
        for counter in ["", "+1", "-1", "01", "18446744073709551616", "1 "] {
            let text = format!("{A}:{counter}");
            assert!(text.parse::<EntityId>().is_err(), "{text:?} parsed");
            let text = format!("{counter}@{A}");
            assert!(text.parse::<Version>().is_err(), "{text:?} parsed");
        }
    }

    #[test]
    fn versions_order_by_lamport_before_writer() {
        let low = WriterId::from_u128(1);
        let high = WriterId::from_u128(2);
        assert!(Version::new(1, high) < Version::new(2, low));
        assert!(Version::new(2, low) < Version::new(2, high));
    }

    #[test]
    fn ids_serialize_as_their_text_form() {
        let version = Version::new(7, A.parse().unwrap());
        let json = serde_json::to_string(&version).unwrap();
        assert_eq!(json, format!("\"7@{A}\""));
        assert_eq!(serde_json::from_str::<Version>(&json).unwrap(), version);
        assert!(serde_json::from_str::<Version>("\"7@\"").is_err());
    }
}
