//! What a field or set holds.

use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Deserializer, Serialize, Serializer};

use crate::error::{Error, Result};
use crate::ids::{string_serde, EntityId};

/// The BLAKE3 hash of some bytes, which names them in the blob store.
///
/// Written as 64 lowercase hexadecimal characters.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct BlobId([u8; 32]);

impl BlobId {
    pub fn of(bytes: &[u8]) -> Self {
        Self(*blake3::hash(bytes).as_bytes())
    }

    pub const fn from_bytes(bytes: [u8; 32]) -> Self {
        Self(bytes)
    }

    pub const fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }
}

impl From<blake3::Hash> for BlobId {
    fn from(hash: blake3::Hash) -> Self {
        Self(*hash.as_bytes())
    }
}

impl fmt::Display for BlobId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.iter().try_for_each(|byte| write!(f, "{byte:02x}"))
    }
}

impl fmt::Debug for BlobId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "BlobId({self})")
    }
}

impl FromStr for BlobId {
    type Err = Error;

    fn from_str(text: &str) -> Result<Self> {
        let invalid = || Error::InvalidId {
            what: "blob id",
            text: text.to_owned(),
        };
        if text.len() != 64 {
            return Err(invalid());
        }
        let mut bytes = [0; 32];
        for (byte, pair) in bytes.iter_mut().zip(text.as_bytes().chunks_exact(2)) {
            *byte = (nibble(pair[0]).ok_or_else(invalid)? << 4)
                | nibble(pair[1]).ok_or_else(invalid)?;
        }
        Ok(Self(bytes))
    }
}

fn nibble(digit: u8) -> Option<u8> {
    match digit {
        b'0'..=b'9' => Some(digit - b'0'),
        b'a'..=b'f' => Some(digit - b'a' + 10),
        _ => None,
    }
}

string_serde!(BlobId);

/// A typed value. toshokan knows the type, never the meaning.
///
/// In JSON it is an object with one key naming the type: `{"text":"Brass"}`,
/// `{"int":-3}`, `{"bool":true}`, `{"ref":"<entity id>"}`, `{"blob":"<blob id>"}`.
/// The order of variants, then of contents, is the order of a set's members.
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Debug, Serialize, Deserialize)]
#[serde(rename_all = "lowercase", deny_unknown_fields)]
pub enum Value {
    Text(String),
    Int(i64),
    Bool(bool),
    /// A relation to another entity.
    Ref(EntityId),
    Blob(BlobId),
}

impl Value {
    pub fn as_blob(&self) -> Option<BlobId> {
        match self {
            Self::Blob(blob) => Some(*blob),
            Self::Text(_) | Self::Int(_) | Self::Bool(_) | Self::Ref(_) => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ids::WriterId;

    #[test]
    fn a_blob_id_is_the_blake3_hash_in_lowercase_hex() {
        // The BLAKE3 hash of the empty input, from the reference test vectors.
        let empty = "af1349b9f5f9a1a6a0404dea36dcc9499bcb25c9adc112b7cc9a93cae41f3262";
        assert_eq!(BlobId::of(b"").to_string(), empty);
        assert_eq!(empty.parse::<BlobId>().unwrap(), BlobId::of(b""));
    }

    #[test]
    fn a_blob_id_refuses_other_lengths_and_uppercase() {
        let hex = BlobId::of(b"x").to_string();
        for text in [&hex[1..], &format!("{hex}0"), &hex.to_uppercase()] {
            assert!(text.parse::<BlobId>().is_err(), "{text:?} parsed");
        }
    }

    #[test]
    fn values_are_tagged_by_type_in_json() {
        let writer = WriterId::from_u128(1);
        let blob = BlobId::of(b"");
        let cases = [
            (
                Value::Text("Brass".into()),
                r#"{"text":"Brass"}"#.to_owned(),
            ),
            (Value::Int(i64::MIN), format!(r#"{{"int":{}}}"#, i64::MIN)),
            (Value::Bool(true), r#"{"bool":true}"#.to_owned()),
            (
                Value::Ref(EntityId::new(writer, 3)),
                format!(r#"{{"ref":"{writer}:3"}}"#),
            ),
            (Value::Blob(blob), format!(r#"{{"blob":"{blob}"}}"#)),
        ];
        for (value, json) in cases {
            assert_eq!(serde_json::to_string(&value).unwrap(), json);
            assert_eq!(serde_json::from_str::<Value>(&json).unwrap(), value);
        }
    }

    #[test]
    fn a_value_with_an_unknown_type_does_not_decode() {
        assert!(serde_json::from_str::<Value>(r#"{"float":1.5}"#).is_err());
        assert!(serde_json::from_str::<Value>(r#"{"text":"a","int":1}"#).is_err());
    }
}
