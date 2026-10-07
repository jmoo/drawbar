//! Entries: what one line of a writer's log says.
//!
//! An entry is the JSON of a [`Line`]: an object with `prev`, `at` (an [`Hlc`]),
//! `kind`, and the members its kind defines. Unknown kinds, ops and members are
//! kept verbatim: the line itself is kept, what does not decode is carried as
//! [`Raw`], and an entry with members this build does not know says so.

use std::collections::BTreeMap;

use serde::de::DeserializeOwned;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use serde_json::value::RawValue;
use serde_json::Value;
use thiserror::Error as ThisError;

use crate::ids::{EntityId, EntryHash, Hlc, Identity, Nonce, WriterId};
use crate::line::{Line, LineError};
use crate::path::RelPath;
use crate::schema::Raw;

/// One verified, decoded line, kept with the line it was decoded from.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Entry {
    pub line: Line,
    pub at: Hlc,
    pub kind: EntryKind,
    /// Whether the line holds a member, at any depth, that [`Entry::kind`]
    /// leaves out. Member order and spacing do not count.
    pub unknown_members: bool,
}

#[derive(Clone, PartialEq, Eq, Debug)]
pub enum EntryKind {
    Genesis(Genesis),
    Intent(Logged),
    Settle(Settle),
    Bind(Bound),
    /// A kind this build does not know, or a known kind whose members do not
    /// decode, as the entry's whole JSON object: kept, merged by nothing, and
    /// reported.
    Unknown(Raw),
}

/// The first entry of a writer, with `prev` = [`EntryHash::ZERO`]. Its hash names
/// the writer's directory in the local root.
#[derive(Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub struct Genesis {
    pub writer: WriterId,
    /// Shown to other writers beside this writer's entries.
    pub label: String,
}

/// A committed intent: its facts and the file effects it made.
///
/// ⚠️ Its ops decode only straight from JSON text: not through `#[serde(flatten)]`
/// or an internally tagged enum, which buffer the [`Raw`] values an op keeps.
#[derive(Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub struct Logged {
    pub label: String,
    pub ops: Vec<Op>,
    /// Bytes the intent moved into this writer's trash.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub displaced: Vec<Displaced>,
    /// The entry this one compensates, for an undo or redo.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reverses: Option<EntryHash>,
}

/// One fact change. References name entry hashes: a write is identified by the
/// entry that logged it with its entity and key, so an intent writes each key of
/// an entity at most once.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Op {
    /// A write of the existence register: the entity exists.
    Create {
        entity: EntityId,
        replaces: Vec<EntryHash>,
    },
    /// A write of the existence register: the entity is deleted. `observed` names
    /// the field writes the deleting writer had seen.
    Delete {
        entity: EntityId,
        replaces: Vec<EntryHash>,
        observed: Vec<EntryHash>,
    },
    /// A register write replacing the writes it names; `None` clears.
    Write {
        entity: EntityId,
        key: String,
        value: Option<Raw>,
        replaces: Vec<EntryHash>,
    },
    /// A set add, tagged by its entry.
    Add {
        entity: EntityId,
        key: String,
        value: Raw,
    },
    /// A set remove of the adds of `value` that the entries `tags` made.
    Remove {
        entity: EntityId,
        key: String,
        value: Raw,
        tags: Vec<EntryHash>,
    },
    /// A write of the entity's file register by a file effect; `None` says it has
    /// no file.
    File {
        entity: EntityId,
        file: Option<FileFact>,
        replaces: Vec<EntryHash>,
    },
    /// A write of the entity's file register recording a binding a scan derived.
    /// Merged as [`Op::File`]; undo leaves it alone.
    Pin {
        entity: EntityId,
        file: FileFact,
        replaces: Vec<EntryHash>,
    },
    /// An op this build does not know, or a known op whose members do not decode,
    /// as its JSON object.
    Unknown(Raw),
}

impl Op {
    /// The entity the op changes; `None` for an op this build does not know.
    pub fn entity(&self) -> Option<EntityId> {
        match self {
            Self::Create { entity, .. }
            | Self::Delete { entity, .. }
            | Self::Write { entity, .. }
            | Self::Add { entity, .. }
            | Self::Remove { entity, .. }
            | Self::File { entity, .. }
            | Self::Pin { entity, .. } => Some(*entity),
            Self::Unknown(_) => None,
        }
    }
}

/// Where an entity's file is and what it held when this writer last wrote or bound
/// it.
#[derive(Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub struct FileFact {
    pub path: RelPath,
    pub identity: Identity,
    pub len: u64,
    /// As the backend reported it; only equality means anything.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub modified: Option<u64>,
}

/// Bytes an intent moved from a library path into this writer's trash.
#[derive(Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub struct Displaced {
    pub item: Nonce,
    pub from: RelPath,
    pub identity: Identity,
    pub len: u64,
}

/// The moves a writer found and pinned as it committed, logged after the intent:
/// merged as an intent's ops are, and never undone.
#[derive(Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub struct Bound {
    pub ops: Vec<Op>,
}

/// This writer settled another writer's unfinished effect, with the user's consent.
#[derive(Clone, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub struct Settle {
    pub writer: WriterId,
    pub record: Nonce,
    pub outcome: Settlement,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Settlement {
    Finished,
    RolledBack,
    /// Left as it was, and no longer reported.
    Dismissed,
}

/// A verified line whose JSON is not an entry: not an object with a readable `at`.
#[derive(ThisError, Clone, PartialEq, Eq, Debug)]
#[error("entry {hash} is malformed: {reason}")]
pub struct Malformed {
    pub hash: EntryHash,
    pub reason: String,
}

#[derive(Deserialize)]
struct Head {
    at: Hlc,
    #[serde(default)]
    kind: Option<Raw>,
}

/// An intent line read in one pass, as [`Head`] and [`Logged`] read it; any
/// other line takes the general path.
#[derive(Deserialize)]
struct IntentLine<'a> {
    prev: EntryHash,
    at: Hlc,
    #[serde(borrow)]
    kind: Option<&'a str>,
    label: String,
    #[serde(borrow)]
    ops: Vec<&'a RawValue>,
    #[serde(default)]
    displaced: Vec<Displaced>,
    #[serde(default)]
    reverses: Option<EntryHash>,
}

impl IntentLine<'_> {
    /// The line's `prev`, `at` and intent, when it is one.
    fn read(json: &str) -> Option<(EntryHash, Hlc, Logged)> {
        let line: IntentLine = serde_json::from_str(json).ok()?;
        if line.kind != Some("intent") {
            return None;
        }
        let ops = line.ops.iter().map(|raw| {
            decode_op(raw.get())
                .unwrap_or_else(|| Op::Unknown(Raw::new(raw.get()).expect("read as JSON")))
        });
        let logged = Logged {
            label: line.label,
            ops: ops.collect(),
            displaced: line.displaced,
            reverses: line.reverses,
        };
        Some((line.prev, line.at, logged))
    }
}

#[derive(Serialize)]
struct Written<'a> {
    prev: EntryHash,
    at: Hlc,
    #[serde(flatten)]
    kind: &'a EntryKind,
}

impl Entry {
    /// Decodes a verified line. A known kind whose members do not decode becomes
    /// [`EntryKind::Unknown`], as does a genesis entry after another entry; an
    /// unknown op becomes [`Op::Unknown`]; unknown members are left out of the
    /// kind, survive in the line, and set [`Entry::unknown_members`].
    pub fn decode(line: Line) -> Result<Self, Malformed> {
        if let Some((_, at, logged)) = IntentLine::read(line.json()) {
            return Ok(Self::of(line, at, Some(EntryKind::Intent(logged))));
        }
        let json = line.json();
        let head: Head = serde_json::from_str(json).map_err(|error| Malformed {
            hash: line.hash(),
            reason: error.to_string(),
        })?;
        let name = head.kind.and_then(|kind| kind.decode::<String>().ok());
        let known = match name.as_deref() {
            Some("genesis") if line.prev() == EntryHash::ZERO => {
                members(json).map(EntryKind::Genesis)
            }
            Some("intent") => members(json).map(EntryKind::Intent),
            Some("settle") => members(json).map(EntryKind::Settle),
            Some("bind") => members(json).map(EntryKind::Bind),
            _ => None,
        };
        Ok(Self::of(line, head.at, known))
    }

    /// One line, its newline included, verified as [`Line::parse`] verifies it and
    /// decoded as [`Entry::linked`] decodes it. An intent line is parsed once.
    pub fn parse(line: &[u8]) -> Result<Self, LineError> {
        let (json, hash) = crate::line::split(line)?;
        match IntentLine::read(json) {
            Some((prev, at, logged)) => {
                let line = Line::checked(prev, json.to_owned(), hash)?;
                Ok(Self::of(line, at, Some(EntryKind::Intent(logged))))
            }
            None => Line::parse(line).map(Self::linked),
        }
    }

    /// A line this install verified before and kept as its JSON and hash. A
    /// damaged copy fails its hash and is refused.
    pub fn kept(json: String, hash: EntryHash) -> Result<Self, LineError> {
        if let Some((prev, at, logged)) = IntentLine::read(&json) {
            let line = Line::checked(prev, json, hash)?;
            return Ok(Self::of(line, at, Some(EntryKind::Intent(logged))));
        }
        let line = Line::kept(json, hash)?;
        Raw::new(line.json()).map_err(|_| LineError::NoPrev)?;
        Ok(Self::linked(line))
    }

    /// A verified line as an entry. A line whose JSON is not an entry is still a
    /// link of the chain, so it is decoded as an unknown kind.
    pub fn linked(line: Line) -> Self {
        Self::decode(line.clone()).unwrap_or_else(|_| Self {
            at: Hlc::ZERO,
            kind: EntryKind::Unknown(Raw::new(line.json()).expect("a verified line holds JSON")),
            line,
            unknown_members: false,
        })
    }

    /// The entry of `line`, read at `at`, as `known` when its members decode.
    fn of(line: Line, at: Hlc, known: Option<EntryKind>) -> Self {
        let json = line.json();
        let unknown_members = known.as_ref().is_some_and(|kind| {
            let written = Written {
                prev: line.prev(),
                at,
                kind,
            };
            match serde_json::to_string(&written) {
                Ok(text) if text == json => false,
                Ok(_) => match (serde_json::from_str(json), serde_json::to_value(&written)) {
                    (Ok(read), Ok(written)) => holds_more(&read, &written),
                    _ => true,
                },
                Err(_) => true,
            }
        });
        let kind = known.unwrap_or_else(|| {
            EntryKind::Unknown(Raw::new(json).expect("a verified line holds a JSON object"))
        });
        Self {
            at,
            kind,
            line,
            unknown_members,
        }
    }

    /// The entry logging `kind` at `at` after `prev`, as [`Entry::decode`] reads it
    /// back. An unknown kind keeps every member of its object but `prev` and `at`.
    pub fn encode(prev: EntryHash, at: Hlc, kind: EntryKind) -> Result<Self, LineError> {
        let json = serde_json::to_string(&Written {
            prev,
            at,
            kind: &kind,
        })
        .map_err(|_| LineError::NoPrev)?;
        Self::decode(Line::seal(json)?).map_err(|_| LineError::NoPrev)
    }

    pub fn hash(&self) -> EntryHash {
        self.line.hash()
    }

    pub fn prev(&self) -> EntryHash {
        self.line.prev()
    }
}

fn members<T: DeserializeOwned>(json: &str) -> Option<T> {
    serde_json::from_str(json).ok()
}

/// Whether `read` has an object member, at any depth, that `written` lacks.
fn holds_more(read: &Value, written: &Value) -> bool {
    match read {
        Value::Object(read) => read.iter().any(|(name, member)| {
            written
                .get(name)
                .is_none_or(|written| holds_more(member, written))
        }),
        Value::Array(read) => written.as_array().is_none_or(|written| {
            read.len() != written.len()
                || read
                    .iter()
                    .zip(written)
                    .any(|(read, written)| holds_more(read, written))
        }),
        _ => false,
    }
}

#[derive(Serialize)]
struct Tagged<'a, M> {
    kind: &'static str,
    #[serde(flatten)]
    members: &'a M,
}

impl Serialize for EntryKind {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        match self {
            Self::Genesis(genesis) => Tagged {
                kind: "genesis",
                members: genesis,
            }
            .serialize(serializer),
            Self::Intent(logged) => Tagged {
                kind: "intent",
                members: logged,
            }
            .serialize(serializer),
            Self::Settle(settle) => Tagged {
                kind: "settle",
                members: settle,
            }
            .serialize(serializer),
            Self::Bind(bound) => Tagged {
                kind: "bind",
                members: bound,
            }
            .serialize(serializer),
            Self::Unknown(raw) => {
                let mut members: BTreeMap<String, Raw> =
                    raw.decode().map_err(serde::ser::Error::custom)?;
                members.retain(|name, _| name != "prev" && name != "at");
                members.serialize(serializer)
            }
        }
    }
}

#[derive(Serialize)]
#[serde(tag = "op", rename_all = "lowercase")]
enum OpOut<'a> {
    Create {
        entity: EntityId,
        replaces: &'a [EntryHash],
    },
    Delete {
        entity: EntityId,
        replaces: &'a [EntryHash],
        observed: &'a [EntryHash],
    },
    Write {
        entity: EntityId,
        key: &'a str,
        #[serde(skip_serializing_if = "Option::is_none")]
        value: Option<&'a Raw>,
        replaces: &'a [EntryHash],
    },
    Add {
        entity: EntityId,
        key: &'a str,
        value: &'a Raw,
    },
    Remove {
        entity: EntityId,
        key: &'a str,
        value: &'a Raw,
        tags: &'a [EntryHash],
    },
    File {
        entity: EntityId,
        #[serde(skip_serializing_if = "Option::is_none")]
        file: Option<&'a FileFact>,
        replaces: &'a [EntryHash],
    },
    Pin {
        entity: EntityId,
        file: &'a FileFact,
        replaces: &'a [EntryHash],
    },
}

/// A write without `value` clears; one with `"value":null` writes `null`.
impl Serialize for Op {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let out = match self {
            Self::Create { entity, replaces } => OpOut::Create {
                entity: *entity,
                replaces,
            },
            Self::Delete {
                entity,
                replaces,
                observed,
            } => OpOut::Delete {
                entity: *entity,
                replaces,
                observed,
            },
            Self::Write {
                entity,
                key,
                value,
                replaces,
            } => OpOut::Write {
                entity: *entity,
                key,
                value: value.as_ref(),
                replaces,
            },
            Self::Add { entity, key, value } => OpOut::Add {
                entity: *entity,
                key,
                value,
            },
            Self::Remove {
                entity,
                key,
                value,
                tags,
            } => OpOut::Remove {
                entity: *entity,
                key,
                value,
                tags,
            },
            Self::File {
                entity,
                file,
                replaces,
            } => OpOut::File {
                entity: *entity,
                file: file.as_ref(),
                replaces,
            },
            Self::Pin {
                entity,
                file,
                replaces,
            } => OpOut::Pin {
                entity: *entity,
                file,
                replaces,
            },
            Self::Unknown(raw) => return raw.serialize(serializer),
        };
        out.serialize(serializer)
    }
}

/// Never fails on an object: what does not decode is [`Op::Unknown`].
impl<'de> Deserialize<'de> for Op {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let raw = Raw::deserialize(deserializer)?;
        Ok(decode_op(raw.as_str()).unwrap_or(Op::Unknown(raw)))
    }
}

#[derive(Deserialize)]
struct OpName {
    op: String,
}

// Each variant's members include `op`, so that a repeated `op` member refuses
// the op however its name was found.

#[derive(Deserialize)]
struct CreateIn {
    op: String,
    entity: EntityId,
    replaces: Vec<EntryHash>,
}

#[derive(Deserialize)]
struct DeleteIn {
    op: String,
    entity: EntityId,
    replaces: Vec<EntryHash>,
    observed: Vec<EntryHash>,
}

#[derive(Deserialize)]
struct WriteIn {
    op: String,
    entity: EntityId,
    key: String,
    #[serde(default, deserialize_with = "present")]
    value: Option<Raw>,
    replaces: Vec<EntryHash>,
}

#[derive(Deserialize)]
struct AddIn {
    op: String,
    entity: EntityId,
    key: String,
    value: Raw,
}

#[derive(Deserialize)]
struct RemoveIn {
    op: String,
    entity: EntityId,
    key: String,
    value: Raw,
    tags: Vec<EntryHash>,
}

#[derive(Deserialize)]
struct FileIn {
    op: String,
    entity: EntityId,
    #[serde(default)]
    file: Option<FileFact>,
    replaces: Vec<EntryHash>,
}

#[derive(Deserialize)]
struct PinIn {
    op: String,
    entity: EntityId,
    file: FileFact,
    replaces: Vec<EntryHash>,
}

/// A member that is there, even as `null`.
fn present<'de, D: Deserializer<'de>>(deserializer: D) -> Result<Option<Raw>, D::Error> {
    Raw::deserialize(deserializer).map(Some)
}

/// The name an op written by [`OpOut`] starts with, read without parsing it.
fn leading_name(json: &str) -> Option<&str> {
    let rest = json.strip_prefix(r#"{"op":""#)?;
    let (name, rest) = rest.split_once('"')?;
    let plain = name.bytes().all(|b| b.is_ascii_lowercase());
    (plain && rest.starts_with(',')).then_some(name)
}

/// The members of op `name`, when `json` decodes as them and names it.
fn op_members<T: DeserializeOwned>(json: &str, name: &str, op: impl Fn(&T) -> &str) -> Option<T> {
    members(json).filter(|members| op(members) == name)
}

fn decode_op(json: &str) -> Option<Op> {
    let name = match leading_name(json) {
        Some(name) => name.to_owned(),
        None => members::<OpName>(json)?.op,
    };
    let name = name.as_str();
    match name {
        "create" => op_members(json, name, |m: &CreateIn| &m.op).map(
            |CreateIn {
                 entity, replaces, ..
             }| Op::Create { entity, replaces },
        ),
        "delete" => op_members(json, name, |m: &DeleteIn| &m.op).map(
            |DeleteIn {
                 entity,
                 replaces,
                 observed,
                 ..
             }| Op::Delete {
                entity,
                replaces,
                observed,
            },
        ),
        "write" => op_members(json, name, |m: &WriteIn| &m.op).map(
            |WriteIn {
                 entity,
                 key,
                 value,
                 replaces,
                 ..
             }| Op::Write {
                entity,
                key,
                value,
                replaces,
            },
        ),
        "add" => op_members(json, name, |m: &AddIn| &m.op).map(
            |AddIn {
                 entity, key, value, ..
             }| Op::Add { entity, key, value },
        ),
        "remove" => op_members(json, name, |m: &RemoveIn| &m.op).map(
            |RemoveIn {
                 entity,
                 key,
                 value,
                 tags,
                 ..
             }| Op::Remove {
                entity,
                key,
                value,
                tags,
            },
        ),
        "file" => op_members(json, name, |m: &FileIn| &m.op).map(
            |FileIn {
                 entity,
                 file,
                 replaces,
                 ..
             }| Op::File {
                entity,
                file,
                replaces,
            },
        ),
        "pin" => op_members(json, name, |m: &PinIn| &m.op).map(
            |PinIn {
                 entity,
                 file,
                 replaces,
                 ..
             }| Op::Pin {
                entity,
                file,
                replaces,
            },
        ),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hash(n: u128) -> EntryHash {
        EntryHash::from_u128(n)
    }

    fn at(wall_ms: u64) -> Hlc {
        Hlc {
            wall_ms,
            counter: 0,
        }
    }

    fn raw(json: &str) -> Raw {
        Raw::new(json).unwrap()
    }

    fn every_op() -> Vec<Op> {
        let entity = EntityId::from_u128(7);
        vec![
            Op::Create {
                entity,
                replaces: vec![hash(1)],
            },
            Op::Delete {
                entity,
                replaces: vec![hash(1)],
                observed: vec![hash(2), hash(3)],
            },
            Op::Write {
                entity,
                key: "origin".into(),
                value: Some(raw(r#"{"b":1,"a":123456789012345678901234567890}"#)),
                replaces: vec![],
            },
            Op::Write {
                entity,
                key: "origin".into(),
                value: Some(raw("null")),
                replaces: vec![hash(4)],
            },
            Op::Write {
                entity,
                key: "origin".into(),
                value: None,
                replaces: vec![hash(5)],
            },
            Op::Add {
                entity,
                key: "tags".into(),
                value: raw(r#""Sunday""#),
            },
            Op::Remove {
                entity,
                key: "tags".into(),
                value: raw(r#""Sunday""#),
                tags: vec![hash(6)],
            },
            Op::File {
                entity,
                file: Some(FileFact {
                    path: RelPath::new("a/b.npno").unwrap(),
                    identity: Identity::from_u128(9),
                    len: 10,
                    modified: Some(11),
                }),
                replaces: vec![],
            },
            Op::File {
                entity,
                file: None,
                replaces: vec![hash(8)],
            },
            Op::Pin {
                entity,
                file: FileFact {
                    path: RelPath::new("c.npno").unwrap(),
                    identity: Identity::from_u128(12),
                    len: 13,
                    modified: None,
                },
                replaces: vec![hash(10)],
            },
        ]
    }

    #[test]
    fn every_kind_and_op_reads_back_as_written() {
        let kinds = [
            EntryKind::Genesis(Genesis {
                writer: WriterId::from_u128(1),
                label: "drawbar".into(),
            }),
            EntryKind::Intent(Logged {
                label: "Import".into(),
                ops: every_op(),
                displaced: vec![Displaced {
                    item: Nonce::from_u128(3),
                    from: RelPath::new("x.npno").unwrap(),
                    identity: Identity::from_u128(4),
                    len: 5,
                }],
                reverses: Some(hash(2)),
            }),
            EntryKind::Settle(Settle {
                writer: WriterId::from_u128(2),
                record: Nonce::from_u128(3),
                outcome: Settlement::RolledBack,
            }),
            EntryKind::Bind(Bound {
                ops: every_op()
                    .into_iter()
                    .filter(|op| matches!(op, Op::Pin { .. }))
                    .collect(),
            }),
        ];
        for kind in kinds {
            let prev = match kind {
                EntryKind::Genesis(_) => EntryHash::ZERO,
                _ => hash(1),
            };
            let entry = Entry::encode(prev, at(5), kind.clone()).unwrap();
            assert_eq!(entry.kind, kind);
            assert_eq!(entry.prev(), prev);
            let read = Entry::decode(Line::parse(&entry.line.to_bytes()).unwrap()).unwrap();
            assert_eq!(read, entry);
        }
    }

    // The members of each kind and op are the format SPEC.md specifies.
    #[test]
    fn an_intent_is_written_as_specified() {
        let entity = EntityId::from_u128(0xe);
        let kind = EntryKind::Intent(Logged {
            label: "Tag".into(),
            ops: vec![
                Op::Add {
                    entity,
                    key: "tags".into(),
                    value: raw(r#""x""#),
                },
                Op::Write {
                    entity,
                    key: "origin".into(),
                    value: None,
                    replaces: vec![hash(1)],
                },
            ],
            displaced: vec![],
            reverses: None,
        });
        let entry = Entry::encode(hash(2), at(3), kind).unwrap();
        let e = "0000000000000000000000000000000e";
        let expected = format!(
            concat!(
                r#"{{"prev":"00000000000000000000000000000002","at":[3,0],"kind":"intent","#,
                r#""label":"Tag","ops":[{{"op":"add","entity":"{e}","key":"tags","value":"x"}},"#,
                r#"{{"op":"write","entity":"{e}","key":"origin","replaces":["#,
                r#""00000000000000000000000000000001"]}}]}}"#
            ),
            e = e
        );
        assert_eq!(entry.line.json(), expected);
    }

    #[test]
    fn unknown_kinds_ops_and_members_survive() {
        let prev = hash(1);
        let json = format!(
            concat!(
                r#"{{"prev":"{prev}","at":[1,2],"kind":"intent","label":"L","later":[1,2],"#,
                r#""ops":[{{"op":"rename","entity":"x"}},{{"op":"add","entity":"0000000000000000000000000000000e","key":"k"}}]}}"#
            ),
            prev = prev
        );
        let entry = Entry::decode(Line::seal(json.clone()).unwrap()).unwrap();
        let EntryKind::Intent(logged) = &entry.kind else {
            panic!("{:?}", entry.kind)
        };
        assert_eq!(
            logged.ops,
            [
                Op::Unknown(raw(r#"{"op":"rename","entity":"x"}"#)),
                Op::Unknown(raw(
                    r#"{"op":"add","entity":"0000000000000000000000000000000e","key":"k"}"#
                )),
            ]
        );
        assert_eq!(entry.line.json(), json);

        let future = format!(r#"{{"prev":"{prev}","at":[1,2],"kind":"merge","into":"y"}}"#);
        let entry = Entry::decode(Line::seal(future.clone()).unwrap()).unwrap();
        assert_eq!(entry.kind, EntryKind::Unknown(raw(&future)));
        let again = Entry::encode(hash(9), at(4), entry.kind).unwrap();
        assert_eq!(
            again.line.json(),
            r#"{"prev":"00000000000000000000000000000009","at":[4,0],"into":"y","kind":"merge"}"#
        );
    }

    #[test]
    fn only_members_a_build_leaves_out_mark_an_entry_extended() {
        let prev = hash(1);
        let e = "0000000000000000000000000000000e";
        let op = format!(r#"{{"op":"add","entity":"{e}","key":"tags","value":{{"b":1,"a":[2]}}}}"#);
        let reordered = format!(
            r#"{{ "kind" : "intent", "ops" : [ {op} ], "label" : "L", "at" : [1, 2], "prev" : "{prev}" }}"#
        );
        let entry = Entry::decode(Line::seal(reordered.clone()).unwrap()).unwrap();
        assert!(
            matches!(entry.kind, EntryKind::Intent(_)),
            "{:?}",
            entry.kind
        );
        assert!(!entry.unknown_members, "{reordered}");

        let nested = format!(
            r#"{{"prev":"{prev}","at":[1,2],"kind":"intent","label":"L","ops":[{}]}}"#,
            op.replace(r#""op":"add""#, r#""op":"add","since":3"#)
        );
        let entry = Entry::decode(Line::seal(nested.clone()).unwrap()).unwrap();
        assert!(
            matches!(entry.kind, EntryKind::Intent(_)),
            "{:?}",
            entry.kind
        );
        assert!(entry.unknown_members, "{nested}");
    }

    #[test]
    fn a_known_kind_that_does_not_decode_is_unknown() {
        let prev = hash(1);
        for json in [
            format!(r#"{{"prev":"{prev}","at":[1,0],"kind":"settle","writer":"x"}}"#),
            format!(
                r#"{{"prev":"{prev}","at":[1,0],"kind":"genesis","writer":"{prev}","label":"L"}}"#
            ),
            format!(r#"{{"prev":"{prev}","at":[1,0]}}"#),
            format!(r#"{{"prev":"{prev}","at":[1,0],"kind":7}}"#),
        ] {
            let entry = Entry::decode(Line::seal(json.clone()).unwrap()).unwrap();
            assert_eq!(entry.kind, EntryKind::Unknown(raw(&json)), "{json}");
        }
    }

    #[test]
    fn an_entry_without_a_readable_clock_is_malformed() {
        let prev = hash(1);
        for json in [
            format!(r#"{{"prev":"{prev}"}}"#),
            format!(r#"{{"prev":"{prev}","at":"soon"}}"#),
            format!(r#"{{"prev":"{prev}","at":[1]}}"#),
            format!(r#"{{"prev":"{prev}","at":"soon","kind":"intent","label":"L","ops":[]}}"#),
        ] {
            let line = Line::seal(json.clone()).unwrap();
            let hash = line.hash();
            assert!(
                matches!(Entry::decode(line), Err(Malformed { hash: h, .. }) if h == hash),
                "{json}"
            );
        }
    }

    #[test]
    fn a_line_read_in_one_pass_reads_as_it_does_parsed_then_decoded() {
        let prev = hash(1);
        let e = "0000000000000000000000000000000e";
        let add = format!(r#"{{"op":"add","entity":"{e}","key":"tags","value":"x"}}"#);
        let jsons = [
            format!(r#"{{"prev":"{prev}","at":[1,0],"kind":"intent","label":"L","ops":[{add}]}}"#),
            format!(r#"{{"prev":"{prev}","at":[1,0],"kind":"intent","label":"L","ops":[],"x":1}}"#),
            format!(r#"{{"kind":"intent","ops":[{add}],"label":"L","at":[1,0],"prev":"{prev}"}}"#),
            format!(r#"{{"prev":"{prev}","at":[1,0],"kind":"intent","label":"L","ops":[7,{{}}]}}"#),
            format!(
                r#"{{"prev":"{prev}","at":[1,0],"kind":"settle","writer":"{e}","record":"{e}","outcome":"dismissed"}}"#
            ),
            format!(r#"{{"prev":"{prev}","at":[1,0],"kind":"merge"}}"#),
            format!(r#"{{"prev":"{prev}","at":"soon","kind":"intent","label":"L","ops":[]}}"#),
            format!(
                r#"{{"prev":"{prev}","prev":"{e}","at":[1,0],"kind":"intent","label":"L","ops":[]}}"#
            ),
            format!(r#"{{"prev":"{prev}","at":[1,0],"kind":"intent","label":"L","ops":[]"#),
        ];
        for json in jsons {
            for hash in [EntryHash::of(prev, json.as_bytes()), hash(2)] {
                let bytes = format!("{json}\t{hash}\n");
                let once = Entry::parse(bytes.as_bytes());
                let twice = Line::parse(bytes.as_bytes()).map(Entry::linked);
                assert_eq!(once, twice, "{json}");
                if let Ok(entry) = &once {
                    assert_eq!(
                        Entry::kept(json.clone(), hash).as_ref(),
                        Ok(entry),
                        "{json}"
                    );
                }
            }
        }
    }

    #[test]
    fn an_op_naming_itself_twice_is_unknown() {
        let e = "0000000000000000000000000000000e";
        for op in [
            format!(r#"{{"op":"add","entity":"{e}","key":"k","value":1,"op":"write"}}"#),
            format!(r#"{{"entity":"{e}","op":"add","key":"k","value":1,"op":"add"}}"#),
        ] {
            let json = format!(
                r#"{{"prev":"{}","at":[1,0],"kind":"intent","label":"L","ops":[{op}]}}"#,
                hash(1)
            );
            let entry = Entry::decode(Line::seal(json).unwrap()).unwrap();
            let EntryKind::Intent(logged) = &entry.kind else {
                panic!("{:?}", entry.kind)
            };
            assert_eq!(logged.ops, [Op::Unknown(raw(&op))], "{op}");
        }
    }

    #[test]
    fn an_entry_too_long_for_a_line_is_refused() {
        let kind = EntryKind::Genesis(Genesis {
            writer: WriterId::from_u128(1),
            label: "x".repeat(crate::line::MAX_LINE),
        });
        assert_eq!(
            Entry::encode(EntryHash::ZERO, at(1), kind),
            Err(LineError::TooLong)
        );
    }
}
