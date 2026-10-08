//! Entries: what one line of a writer's log says.
//!
//! An entry is the JSON of a [`Line`]: an object with `prev`, `at` (an [`Hlc`]),
//! `kind`, and the members its kind defines. Unknown kinds, ops and members are
//! kept verbatim: the line itself is kept, what does not decode is carried as
//! [`Raw`], and an entry with members this build does not know says so.

use std::borrow::Cow;
use std::collections::BTreeMap;

use serde::de::{self, DeserializeOwned, MapAccess, Visitor};
use serde::ser::SerializeMap;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use serde_json::value::RawValue;
use serde_json::Value;
use thiserror::Error as ThisError;

use crate::ids::{EntityId, EntryHash, Hlc, Identity, Nonce, WriterId};
use crate::line::{Line, LineError};
use crate::pack::{bad_variant, pack_struct, Bad, In, Pack, Unpack, Unpacked};
use crate::path::RelPath;
use crate::schema::Raw;

/// One verified, decoded line.
///
/// Held packed, as an install keeps entries for itself: its kind in a binary
/// encoding, and its JSON only when that is not what this build writes for the
/// kind, so most entries hold no text.
#[derive(Clone, PartialEq, Eq)]
pub struct Entry {
    hash: EntryHash,
    prev: EntryHash,
    at: Hlc,
    /// [`Flags`], the packed kind, then the JSON when [`Flags::TEXT`] says so.
    body: Box<[u8]>,
}

/// The first byte of an entry's packed body.
struct Flags;

impl Flags {
    /// The line holds a member, at any depth, that the kind leaves out.
    const UNKNOWN_MEMBERS: u8 = 1;
    /// The body ends with the line's JSON.
    const TEXT: u8 = 2;
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

/// An intent line holding exactly what this build writes of one: each member
/// once, none it does not know at any depth, and no member written as its
/// default, which this build would leave out. Its members are taken as they come,
/// so it reads as [`Entry::decode`] reads it, without unknown members. Any other
/// line fails, and takes the general path.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ExactLine<'a> {
    prev: EntryHash,
    at: Hlc,
    #[serde(borrow)]
    kind: &'a str,
    label: String,
    ops: Vec<ExactOp>,
    #[serde(default, deserialize_with = "nonempty")]
    displaced: Vec<ExactDisplaced>,
    #[serde(default, deserialize_with = "not_null")]
    reverses: Option<EntryHash>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ExactDisplaced {
    item: Nonce,
    from: RelPath,
    identity: Identity,
    len: u64,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ExactFact {
    path: RelPath,
    identity: Identity,
    len: u64,
    #[serde(default, deserialize_with = "not_null")]
    modified: Option<u64>,
}

impl From<ExactFact> for FileFact {
    fn from(fact: ExactFact) -> Self {
        let ExactFact {
            path,
            identity,
            len,
            modified,
        } = fact;
        Self {
            path,
            identity,
            len,
            modified,
        }
    }
}

/// A list that is not empty: an empty one this build leaves out.
fn nonempty<'de, D: Deserializer<'de>, T: Deserialize<'de>>(
    deserializer: D,
) -> Result<Vec<T>, D::Error> {
    let list = Vec::<T>::deserialize(deserializer)?;
    match list.is_empty() {
        true => Err(de::Error::custom("an empty list this build leaves out")),
        false => Ok(list),
    }
}

/// A member that is there and not `null`: a `null` this build leaves out.
fn not_null<'de, D: Deserializer<'de>, T: Deserialize<'de>>(
    deserializer: D,
) -> Result<Option<T>, D::Error> {
    T::deserialize(deserializer).map(Some)
}

impl ExactLine<'_> {
    /// The line's `prev`, `at` and intent, when it is one this build would write.
    fn read(json: &str) -> Option<(EntryHash, Hlc, Logged)> {
        let line: ExactLine = serde_json::from_str(json).ok()?;
        if line.kind != "intent" {
            return None;
        }
        let displaced = line.displaced.into_iter().map(|displaced| Displaced {
            item: displaced.item,
            from: displaced.from,
            identity: displaced.identity,
            len: displaced.len,
        });
        let mut ops: Vec<Op> = line.ops.into_iter().map(|op| op.0).collect();
        ops.shrink_to_fit();
        let logged = Logged {
            label: line.label,
            ops,
            displaced: displaced.collect(),
            reverses: line.reverses,
        };
        Some((line.prev, line.at, logged))
    }
}

/// An op as [`OpOut`] writes it: `op` first, then each member of its kind once,
/// and none other.
struct ExactOp(Op);

impl<'de> Deserialize<'de> for ExactOp {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        deserializer.deserialize_map(ExactOpVisitor)
    }
}

struct ExactOpVisitor;

/// The members of one op, each read at most once.
#[derive(Default)]
struct OpMembers {
    entity: Option<EntityId>,
    key: Option<String>,
    value: Option<Box<RawValue>>,
    replaces: Option<Vec<EntryHash>>,
    observed: Option<Vec<EntryHash>>,
    tags: Option<Vec<EntryHash>>,
    file: Option<ExactFact>,
}

/// Reads the next value into `slot`, refusing a second one.
fn once<'de, A: MapAccess<'de>, T: Deserialize<'de>>(
    map: &mut A,
    slot: &mut Option<T>,
) -> Result<(), A::Error> {
    if slot.is_some() {
        return Err(de::Error::custom("a member named twice"));
    }
    *slot = Some(map.next_value()?);
    Ok(())
}

impl<'de> Visitor<'de> for ExactOpVisitor {
    type Value = ExactOp;

    fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        f.write_str("an op as this build writes it")
    }

    fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<ExactOp, A::Error> {
        let missing = || de::Error::custom("a member missing");
        if map.next_key::<&str>()? != Some("op") {
            return Err(de::Error::custom("`op` is not first"));
        }
        let name: &str = map.next_value()?;
        let mut m = OpMembers::default();
        while let Some(member) = map.next_key::<&str>()? {
            match (name, member) {
                (_, "entity") => once(&mut map, &mut m.entity)?,
                ("write" | "add" | "remove", "key") => once(&mut map, &mut m.key)?,
                ("write" | "add" | "remove", "value") => once(&mut map, &mut m.value)?,
                ("create" | "delete" | "write" | "file" | "pin", "replaces") => {
                    once(&mut map, &mut m.replaces)?
                }
                ("delete", "observed") => once(&mut map, &mut m.observed)?,
                ("remove", "tags") => once(&mut map, &mut m.tags)?,
                ("file" | "pin", "file") => once(&mut map, &mut m.file)?,
                _ => return Err(de::Error::custom("a member this build does not write")),
            }
        }
        let entity = m.entity.ok_or_else(missing)?;
        let value = m.value.map(Raw::from);
        let file = m.file.map(FileFact::from);
        let op = match name {
            "create" => Op::Create {
                entity,
                replaces: m.replaces.ok_or_else(missing)?,
            },
            "delete" => Op::Delete {
                entity,
                replaces: m.replaces.ok_or_else(missing)?,
                observed: m.observed.ok_or_else(missing)?,
            },
            "write" => Op::Write {
                entity,
                key: m.key.ok_or_else(missing)?,
                value,
                replaces: m.replaces.ok_or_else(missing)?,
            },
            "add" => Op::Add {
                entity,
                key: m.key.ok_or_else(missing)?,
                value: value.ok_or_else(missing)?,
            },
            "remove" => Op::Remove {
                entity,
                key: m.key.ok_or_else(missing)?,
                value: value.ok_or_else(missing)?,
                tags: m.tags.ok_or_else(missing)?,
            },
            "file" => Op::File {
                entity,
                file,
                replaces: m.replaces.ok_or_else(missing)?,
            },
            "pin" => Op::Pin {
                entity,
                file: file.ok_or_else(missing)?,
                replaces: m.replaces.ok_or_else(missing)?,
            },
            _ => return Err(de::Error::custom("an op this build does not know")),
        };
        Ok(ExactOp(op))
    }
}

/// An entry as this build writes its line's JSON: `prev`, `at`, then its kind's
/// members.
struct Written<'a> {
    prev: EntryHash,
    at: Hlc,
    kind: &'a EntryKind,
}

impl Serialize for Written<'_> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut map = serializer.serialize_map(None)?;
        map.serialize_entry("prev", &self.prev)?;
        map.serialize_entry("at", &self.at)?;
        self.kind.members(&mut map)?;
        map.end()
    }
}

impl Entry {
    /// The entry of `line`, decoded as `kind` at `at`: what [`Entry::decode`]
    /// makes of a line, or anything a test needs.
    pub fn new(line: Line, at: Hlc, kind: EntryKind, unknown_members: bool) -> Self {
        let canonical = !unknown_members && writes_as(line.prev(), at, &kind, line.json());
        Self::assemble(line, at, &kind, unknown_members, canonical)
    }

    fn assemble(
        line: Line,
        at: Hlc,
        kind: &EntryKind,
        unknown_members: bool,
        canonical: bool,
    ) -> Self {
        let mut flags = 0;
        if unknown_members {
            flags |= Flags::UNKNOWN_MEMBERS;
        }
        if !canonical {
            flags |= Flags::TEXT;
        }
        let mut body = vec![flags];
        kind.pack(&mut body);
        if !canonical {
            line.json().pack(&mut body);
        }
        Self {
            hash: line.hash(),
            prev: line.prev(),
            at,
            body: body.into(),
        }
    }

    /// The JSON this build writes for `kind`; empty when it cannot be written.
    fn written(prev: EntryHash, at: Hlc, kind: &EntryKind) -> String {
        serde_json::to_string(&Written { prev, at, kind }).unwrap_or_default()
    }

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
        if let Some((prev, at, logged)) = ExactLine::read(json) {
            let line = Line::checked(prev, json.to_owned(), hash)?;
            return Ok(Self::exact(line, at, logged));
        }
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
        if let Some((prev, at, logged)) = ExactLine::read(&json) {
            let line = Line::checked(prev, json, hash)?;
            return Ok(Self::exact(line, at, logged));
        }
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
        Self::decode(line.clone()).unwrap_or_else(|_| {
            let raw = Raw::new(line.json()).expect("a verified line holds JSON");
            Self::assemble(line, Hlc::ZERO, &EntryKind::Unknown(raw), false, false)
        })
    }

    /// The entry of an intent line [`ExactLine`] read: it holds no member the
    /// intent leaves out.
    fn exact(line: Line, at: Hlc, logged: Logged) -> Self {
        Self::new(line, at, EntryKind::Intent(logged), false)
    }

    /// The entry of `line`, read at `at`, as `known` when its members decode.
    fn of(line: Line, at: Hlc, known: Option<EntryKind>) -> Self {
        let json = line.json();
        let Some(kind) = known else {
            let raw = Raw::new(json).expect("a verified line holds a JSON object");
            let kind = EntryKind::Unknown(raw);
            let canonical = writes_as(line.prev(), at, &kind, json);
            return Self::assemble(line, at, &kind, false, canonical);
        };
        let written = Written {
            prev: line.prev(),
            at,
            kind: &kind,
        };
        let (unknown_members, canonical) = match serde_json::to_string(&written) {
            Ok(text) if text == json => (false, true),
            Ok(_) => match (serde_json::from_str(json), serde_json::to_value(&written)) {
                (Ok(read), Ok(written)) => (holds_more(&read, &written), false),
                _ => (true, false),
            },
            Err(_) => (true, false),
        };
        Self::assemble(line, at, &kind, unknown_members, canonical)
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
        self.hash
    }

    pub fn prev(&self) -> EntryHash {
        self.prev
    }

    /// The entry's clock reading; [`Hlc::ZERO`] for a line that has none.
    pub fn at(&self) -> Hlc {
        self.at
    }

    /// Whether the line holds a member, at any depth, that [`Entry::kind`]
    /// leaves out. Member order and spacing do not count.
    pub fn unknown_members(&self) -> bool {
        self.body[0] & Flags::UNKNOWN_MEMBERS != 0
    }

    /// The kind, unpacked.
    pub fn kind(&self) -> EntryKind {
        let mut input = In::new(&self.body[1..]);
        EntryKind::unpack(&mut input).expect("an entry holds its packed kind")
    }

    /// An intent's label and the entry it reverses, without unpacking its ops.
    pub fn intent(&self) -> Option<(String, Option<EntryHash>)> {
        let mut input = In::new(&self.body[1..]);
        let summary = match input.byte() {
            Ok(INTENT) => <(String, Option<EntryHash>)>::unpack(&mut input),
            _ => return None,
        };
        Some(summary.expect("an entry holds its packed kind"))
    }

    /// The line's JSON, as it was read.
    pub fn json(&self) -> Cow<'_, str> {
        if self.body[0] & Flags::TEXT == 0 {
            return Cow::Owned(Self::written(self.prev, self.at, &self.kind()));
        }
        let mut input = In::new(&self.body[1..]);
        EntryKind::unpack(&mut input).expect("an entry holds its packed kind");
        Cow::Borrowed(input.str().expect("an entry holds its text"))
    }

    pub fn line(&self) -> Line {
        Line::verified(self.prev, self.json().into_owned(), self.hash)
    }

    /// The line, its newline included.
    pub fn to_bytes(&self) -> Vec<u8> {
        format!("{}\t{}\n", self.json(), self.hash).into_bytes()
    }

    /// The entry as packed in an install's cached view.
    pub(crate) fn pack_into(&self, out: &mut Vec<u8>) {
        (self.hash, self.prev).pack(out);
        self.at.pack(out);
        self.body.as_ref().len().pack(out);
        out.extend_from_slice(&self.body);
    }

    /// An entry [`Entry::pack_into`] packed, from bytes whose check says they
    /// are what this build packed: its body is taken as it is, unpacked only when
    /// its kind or text is asked for.
    pub(crate) fn unpack_from(input: &mut In<'_>) -> Unpacked<Self> {
        let (hash, prev) = <(EntryHash, EntryHash)>::unpack(input)?;
        let at = Hlc::unpack(input)?;
        let len = input.len()?;
        let body = input.take(len)?;
        match body {
            [flags, kind, ..]
                if flags & !(Flags::UNKNOWN_MEMBERS | Flags::TEXT) == 0 && *kind <= UNKNOWN => {}
            _ => return Err(Bad("an entry no packing writes")),
        }
        Ok(Self {
            hash,
            prev,
            at,
            body: body.into(),
        })
    }
}

impl std::fmt::Debug for Entry {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Entry")
            .field("hash", &self.hash)
            .field("prev", &self.prev)
            .field("at", &self.at)
            .field("kind", &self.kind())
            .field("unknown_members", &self.unknown_members())
            .finish()
    }
}

/// The bytes naming each kind in a packed entry.
const GENESIS: u8 = 0;
const INTENT: u8 = 1;
const SETTLE: u8 = 2;
const BIND: u8 = 3;
const UNKNOWN: u8 = 4;

impl Pack for EntryKind {
    fn pack(&self, out: &mut Vec<u8>) {
        match self {
            Self::Genesis(genesis) => {
                out.push(GENESIS);
                genesis.pack(out);
            }
            Self::Intent(logged) => {
                out.push(INTENT);
                (&logged.label, &logged.reverses).pack(out);
                (&logged.displaced, &logged.ops).pack(out);
            }
            Self::Settle(settle) => {
                out.push(SETTLE);
                (settle.writer, settle.record).pack(out);
                settle.outcome.pack(out);
            }
            Self::Bind(bound) => {
                out.push(BIND);
                bound.ops.pack(out);
            }
            Self::Unknown(raw) => {
                out.push(UNKNOWN);
                raw.pack(out);
            }
        }
    }
}

impl Unpack for EntryKind {
    fn unpack(input: &mut In<'_>) -> Unpacked<Self> {
        Ok(match input.byte()? {
            GENESIS => Self::Genesis(Genesis::unpack(input)?),
            INTENT => {
                let (label, reverses) = Unpack::unpack(input)?;
                let (displaced, ops) = Unpack::unpack(input)?;
                Self::Intent(Logged {
                    label,
                    ops,
                    displaced,
                    reverses,
                })
            }
            SETTLE => {
                let (writer, record) = Unpack::unpack(input)?;
                Self::Settle(Settle {
                    writer,
                    record,
                    outcome: Settlement::unpack(input)?,
                })
            }
            BIND => Self::Bind(Bound {
                ops: Unpack::unpack(input)?,
            }),
            UNKNOWN => Self::Unknown(Raw::unpack(input)?),
            _ => return bad_variant(),
        })
    }
}

impl Pack for Settlement {
    fn pack(&self, out: &mut Vec<u8>) {
        out.push(match self {
            Self::Finished => 0,
            Self::RolledBack => 1,
            Self::Dismissed => 2,
        });
    }
}

impl Unpack for Settlement {
    fn unpack(input: &mut In<'_>) -> Unpacked<Self> {
        match input.byte()? {
            0 => Ok(Self::Finished),
            1 => Ok(Self::RolledBack),
            2 => Ok(Self::Dismissed),
            _ => bad_variant(),
        }
    }
}

pack_struct!(Genesis { writer, label });
pack_struct!(Displaced {
    item,
    from,
    identity,
    len
});
pack_struct!(FileFact {
    path,
    identity,
    len,
    modified
});

impl Pack for Op {
    fn pack(&self, out: &mut Vec<u8>) {
        match self {
            Self::Create { entity, replaces } => {
                out.push(0);
                (entity, replaces).pack(out);
            }
            Self::Delete {
                entity,
                replaces,
                observed,
            } => {
                out.push(1);
                (entity, (replaces, observed)).pack(out);
            }
            Self::Write {
                entity,
                key,
                value,
                replaces,
            } => {
                out.push(2);
                ((entity, key), (value, replaces)).pack(out);
            }
            Self::Add { entity, key, value } => {
                out.push(3);
                ((entity, key), value).pack(out);
            }
            Self::Remove {
                entity,
                key,
                value,
                tags,
            } => {
                out.push(4);
                ((entity, key), (value, tags)).pack(out);
            }
            Self::File {
                entity,
                file,
                replaces,
            } => {
                out.push(5);
                (entity, (file, replaces)).pack(out);
            }
            Self::Pin {
                entity,
                file,
                replaces,
            } => {
                out.push(6);
                (entity, (file, replaces)).pack(out);
            }
            Self::Unknown(raw) => {
                out.push(7);
                raw.pack(out);
            }
        }
    }
}

impl Unpack for Op {
    fn unpack(input: &mut In<'_>) -> Unpacked<Self> {
        Ok(match input.byte()? {
            0 => {
                let (entity, replaces) = Unpack::unpack(input)?;
                Self::Create { entity, replaces }
            }
            1 => {
                let (entity, (replaces, observed)) = Unpack::unpack(input)?;
                Self::Delete {
                    entity,
                    replaces,
                    observed,
                }
            }
            2 => {
                let ((entity, key), (value, replaces)) = Unpack::unpack(input)?;
                Self::Write {
                    entity,
                    key,
                    value,
                    replaces,
                }
            }
            3 => {
                let ((entity, key), value) = Unpack::unpack(input)?;
                Self::Add { entity, key, value }
            }
            4 => {
                let ((entity, key), (value, tags)) = Unpack::unpack(input)?;
                Self::Remove {
                    entity,
                    key,
                    value,
                    tags,
                }
            }
            5 => {
                let (entity, (file, replaces)) = Unpack::unpack(input)?;
                Self::File {
                    entity,
                    file,
                    replaces,
                }
            }
            6 => {
                let (entity, (file, replaces)) = Unpack::unpack(input)?;
                Self::Pin {
                    entity,
                    file,
                    replaces,
                }
            }
            7 => Self::Unknown(Raw::unpack(input)?),
            _ => return bad_variant(),
        })
    }
}

/// Whether this build writes `kind` read at `at` after `prev` as `json`, byte for
/// byte; compared as it is written.
fn writes_as(prev: EntryHash, at: Hlc, kind: &EntryKind, json: &str) -> bool {
    struct Same<'a>(&'a [u8]);
    impl std::io::Write for Same<'_> {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            match self.0.strip_prefix(bytes) {
                Some(rest) => {
                    self.0 = rest;
                    Ok(bytes.len())
                }
                None => Err(std::io::ErrorKind::InvalidData.into()),
            }
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    let mut same = Same(json.as_bytes());
    let written = serde_json::to_writer(&mut same, &Written { prev, at, kind });
    written.is_ok() && same.0.is_empty()
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

impl EntryKind {
    /// Writes `kind` and the members of this kind into `map`, in the order this
    /// build writes them. An unknown kind writes every member of its object but
    /// `prev` and `at`, in order of name.
    fn members<M: SerializeMap>(&self, map: &mut M) -> Result<(), M::Error> {
        match self {
            Self::Genesis(Genesis { writer, label }) => {
                map.serialize_entry("kind", "genesis")?;
                map.serialize_entry("writer", writer)?;
                map.serialize_entry("label", label)
            }
            Self::Intent(Logged {
                label,
                ops,
                displaced,
                reverses,
            }) => {
                map.serialize_entry("kind", "intent")?;
                map.serialize_entry("label", label)?;
                map.serialize_entry("ops", ops)?;
                if !displaced.is_empty() {
                    map.serialize_entry("displaced", displaced)?;
                }
                match reverses {
                    Some(reverses) => map.serialize_entry("reverses", reverses),
                    None => Ok(()),
                }
            }
            Self::Settle(Settle {
                writer,
                record,
                outcome,
            }) => {
                map.serialize_entry("kind", "settle")?;
                map.serialize_entry("writer", writer)?;
                map.serialize_entry("record", record)?;
                map.serialize_entry("outcome", outcome)
            }
            Self::Bind(Bound { ops }) => {
                map.serialize_entry("kind", "bind")?;
                map.serialize_entry("ops", ops)
            }
            Self::Unknown(raw) => {
                let members: BTreeMap<String, Raw> =
                    raw.decode().map_err(serde::ser::Error::custom)?;
                let mut kept = members
                    .iter()
                    .filter(|(name, _)| *name != "prev" && *name != "at");
                kept.try_for_each(|(name, value)| map.serialize_entry(name, value))
            }
        }
    }
}

impl Serialize for EntryKind {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut map = serializer.serialize_map(None)?;
        self.members(&mut map)?;
        map.end()
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
            assert_eq!(entry.kind(), kind);
            assert_eq!(entry.prev(), prev);
            let read = Entry::decode(Line::parse(&entry.to_bytes()).unwrap()).unwrap();
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
        assert_eq!(entry.json(), expected);
    }

    // The members of each kind, in the order SPEC.md gives them.
    #[test]
    fn every_kind_is_written_as_specified() {
        let id = |n: u128| format!("{n:032x}");
        let written = |prev: EntryHash, kind: EntryKind| {
            Entry::encode(prev, at(5), kind)
                .unwrap()
                .json()
                .into_owned()
        };
        let genesis = EntryKind::Genesis(Genesis {
            writer: WriterId::from_u128(1),
            label: "drawbar".into(),
        });
        assert_eq!(
            written(EntryHash::ZERO, genesis),
            format!(
                r#"{{"prev":"{}","at":[5,0],"kind":"genesis","writer":"{}","label":"drawbar"}}"#,
                id(0),
                id(1)
            )
        );
        let save = EntryKind::Intent(Logged {
            label: "Save".into(),
            ops: Vec::new(),
            displaced: vec![Displaced {
                item: Nonce::from_u128(3),
                from: RelPath::new("x.npno").unwrap(),
                identity: Identity::from_u128(4),
                len: 5,
            }],
            reverses: Some(hash(2)),
        });
        assert_eq!(
            written(hash(1), save),
            format!(
                concat!(
                    r#"{{"prev":"{}","at":[5,0],"kind":"intent","label":"Save","ops":[],"#,
                    r#""displaced":[{{"item":"{}","from":"x.npno","identity":"{}","len":5}}],"#,
                    r#""reverses":"{}"}}"#
                ),
                id(1),
                id(3),
                id(4),
                id(2)
            )
        );
        let settle = EntryKind::Settle(Settle {
            writer: WriterId::from_u128(2),
            record: Nonce::from_u128(3),
            outcome: Settlement::RolledBack,
        });
        assert_eq!(
            written(hash(1), settle),
            format!(
                r#"{{"prev":"{}","at":[5,0],"kind":"settle","writer":"{}","record":"{}","outcome":"rolled-back"}}"#,
                id(1),
                id(2),
                id(3)
            )
        );
        let bind = EntryKind::Bind(Bound {
            ops: vec![Op::Pin {
                entity: EntityId::from_u128(7),
                file: FileFact {
                    path: RelPath::new("c.npno").unwrap(),
                    identity: Identity::from_u128(12),
                    len: 13,
                    modified: None,
                },
                replaces: vec![hash(10)],
            }],
        });
        assert_eq!(
            written(hash(1), bind),
            format!(
                concat!(
                    r#"{{"prev":"{}","at":[5,0],"kind":"bind","ops":[{{"op":"pin","entity":"{}","#,
                    r#""file":{{"path":"c.npno","identity":"{}","len":13}},"replaces":["{}"]}}]}}"#
                ),
                id(1),
                id(7),
                id(12),
                id(10)
            )
        );
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
        let EntryKind::Intent(logged) = &entry.kind() else {
            panic!("{:?}", entry.kind())
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
        assert_eq!(entry.json(), json);

        let future = format!(r#"{{"prev":"{prev}","at":[1,2],"kind":"merge","into":"y"}}"#);
        let entry = Entry::decode(Line::seal(future.clone()).unwrap()).unwrap();
        assert_eq!(entry.kind(), EntryKind::Unknown(raw(&future)));
        let again = Entry::encode(hash(9), at(4), entry.kind()).unwrap();
        assert_eq!(
            again.json(),
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
            matches!(entry.kind(), EntryKind::Intent(_)),
            "{:?}",
            entry.kind()
        );
        assert!(!entry.unknown_members(), "{reordered}");

        let nested = format!(
            r#"{{"prev":"{prev}","at":[1,2],"kind":"intent","label":"L","ops":[{}]}}"#,
            op.replace(r#""op":"add""#, r#""op":"add","since":3"#)
        );
        let entry = Entry::decode(Line::seal(nested.clone()).unwrap()).unwrap();
        assert!(
            matches!(entry.kind(), EntryKind::Intent(_)),
            "{:?}",
            entry.kind()
        );
        assert!(entry.unknown_members(), "{nested}");
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
            assert_eq!(entry.kind(), EntryKind::Unknown(raw(&json)), "{json}");
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

    /// Variations of `json`, an intent this build wrote: spacing, members it
    /// does not know at every depth, members written as their defaults, members
    /// named twice, and text that is no longer JSON.
    fn variations(json: &str, random: &mut crate::env::SeededRandom) -> Vec<String> {
        use crate::env::Random;
        let mut pick = |n: usize| (random.next_u128() % n as u128) as usize;
        let objects: Vec<usize> = json.match_indices('{').map(|(at, _)| at + 1).collect();
        let commas: Vec<usize> = json.match_indices(',').map(|(at, _)| at).collect();
        let end = json.len() - 1;
        let insert = |at: usize, text: &str| format!("{}{text}{}", &json[..at], &json[at..]);
        let label = json.find(r#""label""#).expect("an intent");
        let mut found = vec![
            json.replace(',', " , ").replace(':', ": "),
            insert(objects[pick(objects.len())], r#""later":[1],"#),
            insert(end, r#","displaced":[]"#),
            insert(end, r#","reverses":null"#),
            insert(end, r#","label":"again""#),
            insert(label, r#""prev":"00000000000000000000000000000001","#),
            insert(commas[pick(commas.len())], "]"),
            json.replacen(r#""ops":[{"op""#, r#""ops":[{"o\u0070""#, 1),
            json.replacen(r#""op":"add","#, r#""op":"add","op":"add","#, 1),
            json.replacen(r#","replaces""#, r#","file":null,"replaces""#, 1),
            json.replacen(r#""len":10"#, r#""len":10,"modified":null"#, 1),
            json.replacen(r#""len":10"#, r#""len":10,"size":2"#, 1),
        ];
        found.retain(|variation| variation != json);
        found
    }

    #[test]
    fn a_line_read_in_one_pass_reads_as_the_general_path_reads_it() {
        let mut random = crate::env::SeededRandom::new(11);
        let ops = every_op();
        for round in 0..300 {
            use crate::env::Random;
            let mut chosen: Vec<Op> = ops
                .iter()
                .filter(|_| random.next_u128().is_multiple_of(2))
                .cloned()
                .collect();
            let turn = (round % ops.len()).min(chosen.len());
            chosen.rotate_left(turn);
            let kind = EntryKind::Intent(Logged {
                label: format!("round {round}"),
                ops: chosen,
                displaced: match round % 3 {
                    0 => vec![Displaced {
                        item: Nonce::from_u128(3),
                        from: RelPath::new("x.npno").unwrap(),
                        identity: Identity::from_u128(4),
                        len: 5,
                    }],
                    _ => Vec::new(),
                },
                reverses: (round % 4 == 0).then(|| hash(2)),
            });
            let written = Entry::encode(hash(1), at(round as u64), kind).unwrap();
            let json = written.json().into_owned();
            assert!(ExactLine::read(&json).is_some(), "read in one pass: {json}");
            for text in std::iter::once(json.clone()).chain(variations(&json, &mut random)) {
                let bytes = format!("{text}\t{}\n", EntryHash::of(hash(1), text.as_bytes()));
                let once = Entry::parse(bytes.as_bytes());
                let general = Line::parse(bytes.as_bytes()).map(Entry::linked);
                assert_eq!(once, general, "round {round}: {text}");
                if let Ok(entry) = once {
                    assert_eq!(
                        entry.to_bytes(),
                        bytes.as_bytes(),
                        "round {round}: verbatim"
                    );
                    let mut packed = Vec::new();
                    entry.pack_into(&mut packed);
                    let unpacked = Entry::unpack_from(&mut In::new(&packed));
                    assert_eq!(unpacked.as_ref(), Ok(&entry), "round {round}: {text}");
                }
            }
        }
    }

    #[test]
    fn an_entry_keeps_its_text_only_when_this_build_would_write_another() {
        let kind = EntryKind::Intent(Logged {
            label: "Tag".into(),
            ops: every_op(),
            displaced: Vec::new(),
            reverses: None,
        });
        let written = Entry::encode(hash(1), at(3), kind).unwrap();
        let spaced = written.json().replacen(':', ": ", 1);
        let read = Entry::decode(Line::seal(spaced.clone()).unwrap()).unwrap();
        assert_eq!(read.kind(), written.kind());
        assert_eq!(read.json(), spaced);
        let text = |entry: &Entry| entry.body[0] & Flags::TEXT != 0;
        assert!(!text(&written) && text(&read));
        assert!(read.body.len() > written.body.len() + spaced.len());
    }

    #[test]
    fn a_cut_or_mislabeled_packed_entry_is_refused() {
        let entry =
            Entry::encode(hash(1), at(3), EntryKind::Bind(Bound { ops: every_op() })).unwrap();
        let mut packed = Vec::new();
        entry.pack_into(&mut packed);
        for cut in 0..packed.len() {
            assert!(
                Entry::unpack_from(&mut In::new(&packed[..cut])).is_err(),
                "cut at {cut}"
            );
        }
        let flags = packed.len() - entry.body.len();
        for (at, bit) in [(flags, 4), (flags + 1, 8)] {
            let mut mislabeled = packed.clone();
            mislabeled[at] |= bit;
            let read = Entry::unpack_from(&mut In::new(&mislabeled));
            assert_eq!(read, Err(Bad("an entry no packing writes")), "byte {at}");
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
            let EntryKind::Intent(logged) = &entry.kind() else {
                panic!("{:?}", entry.kind())
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
