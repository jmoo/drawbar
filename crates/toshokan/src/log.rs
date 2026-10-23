//! Entries: what one line of a writer's log says.
//!
//! An entry is the JSON of a [`Line`]: an object with `prev`, `n` (its number in
//! its writer's chain), `at` (an [`Hlc`]), `kind`, and the members its kind
//! defines. Unknown kinds, ops and members are kept verbatim: the line itself is
//! kept, what does not decode is carried as [`Raw`], and an entry with members
//! this build does not know says so.

use std::borrow::Cow;
use std::collections::BTreeMap;

use serde::de::{self, DeserializeOwned, MapAccess, Visitor};
use serde::ser::{SerializeMap, SerializeTuple};
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
    /// [`Entry::UNNUMBERED`] for a line with no readable `n`.
    n: u64,
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

/// An entry another entry names: its writer, its number in that writer's chain,
/// and its hash. Written `["<writer>",<n>,"<hash>"]`.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub struct Ref {
    pub writer: WriterId,
    pub n: u64,
    pub hash: EntryHash,
}

/// One of a writer's own entries: its number and hash, written `[<n>,"<hash>"]`.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub struct Own {
    pub n: u64,
    pub hash: EntryHash,
}

/// A run of a forked writer's entries, as a dismissal or a take names it: from
/// entry `from` to `last` on `last`'s chain. A tip run is named by the tip's
/// head, and its first entry by number alone.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug, Serialize, Deserialize)]
pub struct Run {
    pub writer: WriterId,
    pub from: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub first: Option<EntryHash>,
    pub last: Own,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tip: Option<Own>,
    /// The first line of the file `last` was read in. A snapshot that
    /// certifies that file at least as far as `last` holds the run.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub segment: Option<Own>,
}

/// A fork point a writer knew of when it wrote: the writer whose history forked,
/// and the number of the entry it forked after. Written `["<writer>",<n>]`.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Debug)]
pub struct Forked {
    pub writer: WriterId,
    pub at: u64,
}

/// The field of a write a delete observed.
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Debug)]
pub enum Target {
    Register(String),
    /// A set's member, by its value.
    Member(String, Raw),
    File,
}

/// A field write a delete observed, which it kills.
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Debug)]
pub struct Observed {
    pub target: Target,
    pub at: Ref,
}

#[derive(Clone, PartialEq, Eq, Debug)]
pub enum EntryKind {
    Genesis(Genesis),
    Intent(Logged),
    /// Ops and displaced bytes of the intent before it that its line had no room
    /// for.
    More(More),
    Settle(Settle),
    Bind(Bound),
    /// A kind this build does not know, or a known kind whose members do not
    /// decode, as the entry's whole JSON object: kept, merged by nothing, and
    /// reported.
    Unknown(Raw),
}

/// The first entry of a writer, with `prev` = [`EntryHash::ZERO`] and `n` 0. Its
/// hash names the writer's directory in the local root.
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
    /// The fork points of the writers its ops name that its writer knew of.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub forks: Vec<Forked>,
    /// How many ops and displaced items continue in the [`More`] entries that
    /// follow it, which hold nothing else.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub more: u64,
}

fn is_zero(n: &u64) -> bool {
    *n == 0
}

/// The continuation of an intent: what of it the intent's line had no room for.
///
/// ⚠️ Its ops decode only straight from JSON text, as [`Logged`]'s do.
#[derive(Clone, PartialEq, Eq, Debug, Default, Serialize, Deserialize)]
pub struct More {
    pub ops: Vec<Op>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub displaced: Vec<Displaced>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub forks: Vec<Forked>,
    /// The entities the intent deletes whose fields its ops write: their writes
    /// die with the delete, as they would in the intent's own entry.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub deletes: Vec<EntityId>,
}

impl More {
    /// How many ops and displaced items it holds.
    pub fn len(&self) -> u64 {
        (self.ops.len() + self.displaced.len()) as u64
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

/// One fact change. A write is named by its writer, number and hash: a [`Ref`].
/// An intent writes each field of an entity at most once.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Op {
    /// A write of the existence register: the entity exists.
    Create {
        entity: EntityId,
        replaces: Vec<Ref>,
        take: Option<Run>,
    },
    /// A write of the existence register: the entity is deleted. `observed` names
    /// the field writes the deleting writer had seen.
    Delete {
        entity: EntityId,
        replaces: Vec<Ref>,
        observed: Vec<Observed>,
        take: Option<Run>,
    },
    /// A register write replacing the writes it names; `None` clears.
    Write {
        entity: EntityId,
        key: String,
        value: Option<Raw>,
        replaces: Vec<Ref>,
        take: Option<Run>,
    },
    /// A set add of `value`, replacing the adds of it the writer saw.
    Add {
        entity: EntityId,
        key: String,
        value: Raw,
        replaces: Vec<Ref>,
        take: Option<Run>,
    },
    /// A set remove of the adds of `value` that `tags` names.
    Remove {
        entity: EntityId,
        key: String,
        value: Raw,
        tags: Vec<Ref>,
        take: Option<Run>,
    },
    /// A write of the entity's file register by a file effect; `None` says it has
    /// no file.
    File {
        entity: EntityId,
        file: Option<FileFact>,
        replaces: Vec<Ref>,
        take: Option<Run>,
    },
    /// A write of the entity's file register recording a binding a scan derived,
    /// or the file of a run an intent took. Merged as [`Op::File`]; undo leaves
    /// it alone.
    Pin {
        entity: EntityId,
        file: FileFact,
        replaces: Vec<Ref>,
        take: Option<Run>,
    },
    /// A file effect on a file no entity is bound to: where it was, if anywhere,
    /// and where it is after the effect and what it holds, if anywhere. Facts are
    /// not changed; undo reads it.
    Path {
        from: Option<RelPath>,
        file: Option<FileFact>,
    },
    /// A file effect moved the tree at `from` to `to`, which it made. Facts are not
    /// changed; undo reads it to remove `to` once it moved its files back.
    Tree { from: RelPath, to: RelPath },
    /// A file effect made the directory `path` where `made`, or removed it, empty.
    /// Facts are not changed; undo reads it to do the opposite.
    Dir { path: RelPath, made: bool },
    /// The user dismissed these runs: no reader merges or shows them.
    Dismiss { runs: Vec<Run> },
    /// An op this build does not know, or a known op whose members do not decode,
    /// as its JSON object.
    Unknown(Raw),
}

impl Op {
    /// The entity the op changes; `None` for a dismissal and an op this build does
    /// not know.
    pub fn entity(&self) -> Option<EntityId> {
        match self {
            Self::Create { entity, .. }
            | Self::Delete { entity, .. }
            | Self::Write { entity, .. }
            | Self::Add { entity, .. }
            | Self::Remove { entity, .. }
            | Self::File { entity, .. }
            | Self::Pin { entity, .. } => Some(*entity),
            Self::Path { .. }
            | Self::Tree { .. }
            | Self::Dir { .. }
            | Self::Dismiss { .. }
            | Self::Unknown(_) => None,
        }
    }

    /// The run the op takes its change from.
    pub fn take(&self) -> Option<&Run> {
        match self {
            Self::Create { take, .. }
            | Self::Delete { take, .. }
            | Self::Write { take, .. }
            | Self::Add { take, .. }
            | Self::Remove { take, .. }
            | Self::File { take, .. }
            | Self::Pin { take, .. } => take.as_ref(),
            Self::Path { .. }
            | Self::Tree { .. }
            | Self::Dir { .. }
            | Self::Dismiss { .. }
            | Self::Unknown(_) => None,
        }
    }
}

/// Where an entity's file is and what it held when this writer last wrote or bound
/// it.
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Debug, Serialize, Deserialize)]
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

impl Serialize for Ref {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut tuple = serializer.serialize_tuple(3)?;
        tuple.serialize_element(&self.writer)?;
        tuple.serialize_element(&self.n)?;
        tuple.serialize_element(&self.hash)?;
        tuple.end()
    }
}

impl<'de> Deserialize<'de> for Ref {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let (writer, n, hash) = <(WriterId, u64, EntryHash)>::deserialize(deserializer)?;
        Ok(Self { writer, n, hash })
    }
}

impl Serialize for Own {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        (self.n, self.hash).serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for Own {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let (n, hash) = <(u64, EntryHash)>::deserialize(deserializer)?;
        Ok(Self { n, hash })
    }
}

impl Serialize for Forked {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        (self.writer, self.at).serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for Forked {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let (writer, at) = <(WriterId, u64)>::deserialize(deserializer)?;
        Ok(Self { writer, at })
    }
}

impl Ref {
    pub fn own(self) -> Own {
        Own {
            n: self.n,
            hash: self.hash,
        }
    }
}

impl Own {
    pub fn of(self, writer: WriterId) -> Ref {
        Ref {
            writer,
            n: self.n,
            hash: self.hash,
        }
    }
}

/// An observed write as JSON: its field's members, then `at`.
#[derive(Serialize)]
struct ObservedOut<'a> {
    #[serde(skip_serializing_if = "Option::is_none")]
    key: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    value: Option<&'a Raw>,
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    file: bool,
    at: Ref,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ObservedIn {
    #[serde(default)]
    key: Option<String>,
    #[serde(default, deserialize_with = "present")]
    value: Option<Raw>,
    #[serde(default, deserialize_with = "not_null")]
    file: Option<bool>,
    at: Ref,
}

impl Serialize for Observed {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let (key, value, file) = match &self.target {
            Target::Register(key) => (Some(key.as_str()), None, false),
            Target::Member(key, value) => (Some(key.as_str()), Some(value), false),
            Target::File => (None, None, true),
        };
        ObservedOut {
            key,
            value,
            file,
            at: self.at,
        }
        .serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for Observed {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let ObservedIn {
            key,
            value,
            file,
            at,
        } = ObservedIn::deserialize(deserializer)?;
        let target = match (key, value, file) {
            (Some(key), None, None) => Target::Register(key),
            (Some(key), Some(value), None) => Target::Member(key, value),
            (None, None, Some(true)) => Target::File,
            _ => return Err(de::Error::custom("an observed write names no one field")),
        };
        Ok(Self { target, at })
    }
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
    n: u64,
    at: Hlc,
    #[serde(default)]
    kind: Option<Raw>,
}

/// An intent line read in one pass, as [`Head`] and [`Logged`] read it; any
/// other line takes the general path.
#[derive(Deserialize)]
struct IntentLine<'a> {
    prev: EntryHash,
    n: u64,
    at: Hlc,
    #[serde(borrow)]
    kind: Option<&'a str>,
    #[serde(default)]
    label: Option<String>,
    #[serde(borrow)]
    ops: Vec<&'a RawValue>,
    #[serde(default)]
    displaced: Vec<Displaced>,
    #[serde(default)]
    forks: Vec<Forked>,
    #[serde(default)]
    more: u64,
    #[serde(default)]
    deletes: Vec<EntityId>,
}

impl IntentLine<'_> {
    /// The line's `prev`, `n`, `at` and intent or continuation, when it is one.
    fn read(json: &str) -> Option<(EntryHash, u64, Hlc, EntryKind)> {
        let line: IntentLine = serde_json::from_str(json).ok()?;
        let ops = line.ops.iter().map(|raw| {
            decode_op(raw.get())
                .unwrap_or_else(|| Op::Unknown(Raw::new(raw.get()).expect("read as JSON")))
        });
        let kind = continued(
            line.kind?,
            line.label,
            Continued {
                ops: ops.collect(),
                displaced: line.displaced,
                forks: line.forks,
                more: line.more,
                deletes: line.deletes,
            },
        )?;
        Some((line.prev, line.n, line.at, kind))
    }
}

/// The members an intent shares with its continuation, and those each has
/// alone.
struct Continued {
    ops: Vec<Op>,
    displaced: Vec<Displaced>,
    forks: Vec<Forked>,
    more: u64,
    deletes: Vec<EntityId>,
}

/// An intent labeled `label`, or, with no label and nothing more, its
/// continuation, as `kind` names it.
fn continued(kind: &str, label: Option<String>, members: Continued) -> Option<EntryKind> {
    let Continued {
        ops,
        displaced,
        forks,
        more,
        deletes,
    } = members;
    match (kind, label) {
        ("intent", Some(label)) if deletes.is_empty() => Some(EntryKind::Intent(Logged {
            label,
            ops,
            displaced,
            forks,
            more,
        })),
        ("more", None) if more == 0 => Some(EntryKind::More(More {
            ops,
            displaced,
            forks,
            deletes,
        })),
        _ => None,
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
    n: u64,
    at: Hlc,
    #[serde(borrow)]
    kind: &'a str,
    #[serde(default, deserialize_with = "not_null")]
    label: Option<String>,
    ops: Vec<ExactOp>,
    #[serde(default, deserialize_with = "nonempty")]
    displaced: Vec<ExactDisplaced>,
    #[serde(default, deserialize_with = "nonempty")]
    forks: Vec<Forked>,
    #[serde(default, deserialize_with = "nonzero")]
    more: u64,
    #[serde(default, deserialize_with = "nonempty")]
    deletes: Vec<EntityId>,
}

/// A number that is not 0: a 0 this build leaves out.
fn nonzero<'de, D: Deserializer<'de>>(deserializer: D) -> Result<u64, D::Error> {
    match u64::deserialize(deserializer)? {
        0 => Err(de::Error::custom("a 0 this build leaves out")),
        n => Ok(n),
    }
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

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ExactRun {
    writer: WriterId,
    from: u64,
    #[serde(default, deserialize_with = "not_null")]
    first: Option<EntryHash>,
    last: Own,
    #[serde(default, deserialize_with = "not_null")]
    tip: Option<Own>,
    #[serde(default, deserialize_with = "not_null")]
    segment: Option<Own>,
}

impl From<ExactRun> for Run {
    fn from(run: ExactRun) -> Self {
        let ExactRun {
            writer,
            from,
            first,
            last,
            tip,
            segment,
        } = run;
        Self {
            writer,
            from,
            first,
            last,
            tip,
            segment,
        }
    }
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
    /// The line's `prev`, `n`, `at` and intent or continuation, when it is one
    /// this build would write.
    fn read(json: &str) -> Option<(EntryHash, u64, Hlc, EntryKind)> {
        let line: ExactLine = serde_json::from_str(json).ok()?;
        let displaced = line.displaced.into_iter().map(|displaced| Displaced {
            item: displaced.item,
            from: displaced.from,
            identity: displaced.identity,
            len: displaced.len,
        });
        let mut ops: Vec<Op> = line.ops.into_iter().map(|op| op.0).collect();
        ops.shrink_to_fit();
        let kind = continued(
            line.kind,
            line.label,
            Continued {
                ops,
                displaced: displaced.collect(),
                forks: line.forks,
                more: line.more,
                deletes: line.deletes,
            },
        )?;
        Some((line.prev, line.n, line.at, kind))
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
    replaces: Option<Vec<Ref>>,
    observed: Option<Vec<ExactObserved>>,
    tags: Option<Vec<Ref>>,
    file: Option<ExactFact>,
    from: Option<RelPath>,
    to: Option<RelPath>,
    path: Option<RelPath>,
    made: Option<bool>,
    take: Option<ExactRun>,
    runs: Option<Vec<ExactRun>>,
}

/// An observed write as [`ObservedOut`] writes it.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ExactObserved {
    #[serde(default, deserialize_with = "not_null")]
    key: Option<String>,
    #[serde(default)]
    value: Option<Box<RawValue>>,
    #[serde(default, deserialize_with = "not_null")]
    file: Option<bool>,
    at: Ref,
}

impl ExactObserved {
    fn read(self) -> Option<Observed> {
        let target = match (self.key, self.value, self.file) {
            (Some(key), None, None) => Target::Register(key),
            (Some(key), Some(value), None) => Target::Member(key, Raw::from(value)),
            (None, None, Some(true)) => Target::File,
            _ => return None,
        };
        Some(Observed {
            target,
            at: self.at,
        })
    }
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
                ("dismiss", "runs") => once(&mut map, &mut m.runs)?,
                ("dismiss", _) => {
                    return Err(de::Error::custom("a member this build does not write"))
                }
                (_, "entity") => once(&mut map, &mut m.entity)?,
                ("write" | "add" | "remove", "key") => once(&mut map, &mut m.key)?,
                ("write" | "add" | "remove", "value") => once(&mut map, &mut m.value)?,
                ("create" | "delete" | "write" | "add" | "file" | "pin", "replaces") => {
                    once(&mut map, &mut m.replaces)?
                }
                ("delete", "observed") => once(&mut map, &mut m.observed)?,
                ("remove", "tags") => once(&mut map, &mut m.tags)?,
                ("file" | "pin" | "path", "file") => once(&mut map, &mut m.file)?,
                ("path" | "tree", "from") => once(&mut map, &mut m.from)?,
                ("tree", "to") => once(&mut map, &mut m.to)?,
                ("dir", "path") => once(&mut map, &mut m.path)?,
                ("dir", "made") => once(&mut map, &mut m.made)?,
                ("create" | "delete" | "write" | "add" | "remove" | "file" | "pin", "take") => {
                    once(&mut map, &mut m.take)?
                }
                _ => return Err(de::Error::custom("a member this build does not write")),
            }
        }
        let take = m.take.map(Run::from);
        if name == "dismiss" {
            let runs = m.runs.ok_or_else(missing)?;
            return Ok(ExactOp(Op::Dismiss {
                runs: runs.into_iter().map(Run::from).collect(),
            }));
        }
        let value = m.value.map(Raw::from);
        let file = m.file.map(FileFact::from);
        match name {
            "path" => return Ok(ExactOp(Op::Path { from: m.from, file })),
            "tree" => {
                let (from, to) = (m.from.ok_or_else(missing)?, m.to.ok_or_else(missing)?);
                return Ok(ExactOp(Op::Tree { from, to }));
            }
            "dir" => {
                let (path, made) = (m.path.ok_or_else(missing)?, m.made.ok_or_else(missing)?);
                return Ok(ExactOp(Op::Dir { path, made }));
            }
            _ => {}
        }
        let entity = m.entity.ok_or_else(missing)?;
        let op = match name {
            "create" => Op::Create {
                entity,
                replaces: m.replaces.ok_or_else(missing)?,
                take,
            },
            "delete" => Op::Delete {
                entity,
                replaces: m.replaces.ok_or_else(missing)?,
                observed: m
                    .observed
                    .ok_or_else(missing)?
                    .into_iter()
                    .map(|observed| observed.read().ok_or_else(missing))
                    .collect::<Result<_, _>>()?,
                take,
            },
            "write" => Op::Write {
                entity,
                key: m.key.ok_or_else(missing)?,
                value,
                replaces: m.replaces.ok_or_else(missing)?,
                take,
            },
            "add" => Op::Add {
                entity,
                key: m.key.ok_or_else(missing)?,
                value: value.ok_or_else(missing)?,
                replaces: m.replaces.ok_or_else(missing)?,
                take,
            },
            "remove" => Op::Remove {
                entity,
                key: m.key.ok_or_else(missing)?,
                value: value.ok_or_else(missing)?,
                tags: m.tags.ok_or_else(missing)?,
                take,
            },
            "file" => Op::File {
                entity,
                file,
                replaces: m.replaces.ok_or_else(missing)?,
                take,
            },
            "pin" => Op::Pin {
                entity,
                file: file.ok_or_else(missing)?,
                replaces: m.replaces.ok_or_else(missing)?,
                take,
            },
            _ => return Err(de::Error::custom("an op this build does not know")),
        };
        Ok(ExactOp(op))
    }
}

/// An entry as this build writes its line's JSON: `prev`, `n`, `at`, then its
/// kind's members.
struct Written<'a> {
    prev: EntryHash,
    n: u64,
    at: Hlc,
    kind: &'a EntryKind,
}

impl Serialize for Written<'_> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut map = serializer.serialize_map(None)?;
        map.serialize_entry("prev", &self.prev)?;
        map.serialize_entry("n", &self.n)?;
        map.serialize_entry("at", &self.at)?;
        self.kind.members(&mut map)?;
        map.end()
    }
}

impl Entry {
    /// The number of a line with no readable `n`: such an entry links the chain
    /// but is never placed.
    pub const UNNUMBERED: u64 = u64::MAX;

    /// The entry of `line`, decoded as `kind` numbered `n` at `at`: what
    /// [`Entry::decode`] makes of a line, or anything a test needs.
    pub fn new(line: Line, n: u64, at: Hlc, kind: EntryKind, unknown_members: bool) -> Self {
        let canonical = !unknown_members && writes_as(line.prev(), n, at, &kind, line.json());
        Self::assemble(line, n, at, &kind, unknown_members, canonical)
    }

    fn assemble(
        line: Line,
        n: u64,
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
            n,
            at,
            body: body.into(),
        }
    }

    /// The JSON this build writes for `kind`; empty when it cannot be written.
    fn written(prev: EntryHash, n: u64, at: Hlc, kind: &EntryKind) -> String {
        serde_json::to_string(&Written { prev, n, at, kind }).unwrap_or_default()
    }

    /// Decodes a verified line. A known kind whose members do not decode becomes
    /// [`EntryKind::Unknown`], as does a genesis entry after another entry or
    /// numbered other than 0; an unknown op becomes [`Op::Unknown`]; unknown
    /// members are left out of the kind, survive in the line, and set
    /// [`Entry::unknown_members`].
    pub fn decode(line: Line) -> Result<Self, Malformed> {
        if let Some((_, n, at, kind)) = IntentLine::read(line.json()) {
            return Ok(Self::of(line, n, at, Some(kind)));
        }
        let json = line.json();
        let head: Head = serde_json::from_str(json).map_err(|error| Malformed {
            hash: line.hash(),
            reason: error.to_string(),
        })?;
        let name = head.kind.and_then(|kind| kind.decode::<String>().ok());
        let known = match name.as_deref() {
            Some("genesis") if line.prev() == EntryHash::ZERO && head.n == 0 => {
                members(json).map(EntryKind::Genesis)
            }
            Some("intent") => members(json).map(EntryKind::Intent),
            Some("more") => members(json).map(EntryKind::More),
            Some("settle") => members(json).map(EntryKind::Settle),
            Some("bind") => members(json).map(EntryKind::Bind),
            _ => None,
        };
        Ok(Self::of(line, head.n, head.at, known))
    }

    /// One line, its newline included, verified as [`Line::parse`] verifies it and
    /// decoded as [`Entry::linked`] decodes it. An intent line is parsed once.
    pub fn parse(line: &[u8]) -> Result<Self, LineError> {
        let (json, hash) = crate::line::split(line)?;
        if let Some((prev, n, at, kind)) = ExactLine::read(json) {
            let line = Line::checked(prev, json.to_owned(), hash)?;
            return Ok(Self::exact(line, n, at, kind));
        }
        match IntentLine::read(json) {
            Some((prev, n, at, kind)) => {
                let line = Line::checked(prev, json.to_owned(), hash)?;
                Ok(Self::of(line, n, at, Some(kind)))
            }
            None => Line::parse(line).map(Self::linked),
        }
    }

    /// A line this install verified before and kept as its JSON and hash. A
    /// damaged copy fails its hash and is refused.
    pub fn kept(json: String, hash: EntryHash) -> Result<Self, LineError> {
        if let Some((prev, n, at, kind)) = ExactLine::read(&json) {
            let line = Line::checked(prev, json, hash)?;
            return Ok(Self::exact(line, n, at, kind));
        }
        if let Some((prev, n, at, kind)) = IntentLine::read(&json) {
            let line = Line::checked(prev, json, hash)?;
            return Ok(Self::of(line, n, at, Some(kind)));
        }
        let line = Line::kept(json, hash)?;
        Raw::new(line.json()).map_err(|_| LineError::NoPrev)?;
        Ok(Self::linked(line))
    }

    /// A verified line as an entry. A line whose JSON is not an entry is still a
    /// link of the chain, so it is decoded as an unknown kind, unnumbered.
    pub fn linked(line: Line) -> Self {
        Self::decode(line.clone()).unwrap_or_else(|_| {
            let raw = Raw::new(line.json()).expect("a verified line holds JSON");
            let kind = EntryKind::Unknown(raw);
            Self::assemble(line, Self::UNNUMBERED, Hlc::ZERO, &kind, false, false)
        })
    }

    /// The entry of an intent or continuation line [`ExactLine`] read: it holds
    /// no member the kind leaves out.
    fn exact(line: Line, n: u64, at: Hlc, kind: EntryKind) -> Self {
        Self::new(line, n, at, kind, false)
    }

    /// The entry of `line`, numbered `n` and read at `at`, as `known` when its
    /// members decode.
    fn of(line: Line, n: u64, at: Hlc, known: Option<EntryKind>) -> Self {
        let json = line.json();
        let Some(kind) = known else {
            let raw = Raw::new(json).expect("a verified line holds a JSON object");
            let kind = EntryKind::Unknown(raw);
            let canonical = writes_as(line.prev(), n, at, &kind, json);
            return Self::assemble(line, n, at, &kind, false, canonical);
        };
        let written = Written {
            prev: line.prev(),
            n,
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
        Self::assemble(line, n, at, &kind, unknown_members, canonical)
    }

    /// The entry logging `kind` numbered `n` at `at` after `prev`, as
    /// [`Entry::decode`] reads it back. An unknown kind keeps every member of its
    /// object but `prev`, `n` and `at`.
    pub fn encode(prev: EntryHash, n: u64, at: Hlc, kind: EntryKind) -> Result<Self, LineError> {
        let json = serde_json::to_string(&Written {
            prev,
            n,
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

    /// The entry's number in its writer's chain: 0 for the genesis entry, its
    /// predecessor's plus one after; [`Entry::UNNUMBERED`] for a line without one.
    pub fn n(&self) -> u64 {
        self.n
    }

    /// The entry's number and hash.
    pub fn own(&self) -> Own {
        Own {
            n: self.n,
            hash: self.hash,
        }
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

    /// An intent's label, without unpacking its ops.
    pub fn intent(&self) -> Option<String> {
        let mut input = In::new(&self.body[1..]);
        let label = match input.byte() {
            Ok(INTENT) => String::unpack(&mut input),
            _ => return None,
        };
        Some(label.expect("an entry holds its packed kind"))
    }

    /// The line's JSON, as it was read.
    pub fn json(&self) -> Cow<'_, str> {
        if self.body[0] & Flags::TEXT == 0 {
            return Cow::Owned(Self::written(self.prev, self.n, self.at, &self.kind()));
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
        self.n.pack(out);
        self.at.pack(out);
        self.body.as_ref().len().pack(out);
        out.extend_from_slice(&self.body);
    }

    /// An entry [`Entry::pack_into`] packed, from bytes whose check says they
    /// are what this build packed: its body is taken as it is, unpacked only when
    /// its kind or text is asked for.
    pub(crate) fn unpack_from(input: &mut In<'_>) -> Unpacked<Self> {
        let (hash, prev) = <(EntryHash, EntryHash)>::unpack(input)?;
        let n = u64::unpack(input)?;
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
            n,
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
            .field("n", &self.n)
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
const MORE: u8 = 5;

impl Pack for EntryKind {
    fn pack(&self, out: &mut Vec<u8>) {
        match self {
            Self::Genesis(genesis) => {
                out.push(GENESIS);
                genesis.pack(out);
            }
            Self::Intent(logged) => {
                out.push(INTENT);
                logged.label.pack(out);
                (&logged.displaced, &logged.ops).pack(out);
                (&logged.forks, logged.more).pack(out);
            }
            Self::More(more) => {
                out.push(MORE);
                (&more.displaced, &more.ops).pack(out);
                (&more.forks, &more.deletes).pack(out);
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
                let label = Unpack::unpack(input)?;
                let (displaced, ops) = Unpack::unpack(input)?;
                let (forks, more) = Unpack::unpack(input)?;
                Self::Intent(Logged {
                    label,
                    ops,
                    displaced,
                    forks,
                    more,
                })
            }
            MORE => {
                let (displaced, ops) = Unpack::unpack(input)?;
                let (forks, deletes) = Unpack::unpack(input)?;
                Self::More(More {
                    ops,
                    displaced,
                    forks,
                    deletes,
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
pack_struct!(Ref { writer, n, hash });
pack_struct!(Own { n, hash });
pack_struct!(Forked { writer, at });
pack_struct!(Run {
    writer,
    from,
    first,
    last,
    tip,
    segment
});
pack_struct!(Observed { target, at });

impl Pack for Target {
    fn pack(&self, out: &mut Vec<u8>) {
        match self {
            Self::Register(key) => {
                out.push(0);
                key.pack(out);
            }
            Self::Member(key, value) => {
                out.push(1);
                (key, value).pack(out);
            }
            Self::File => out.push(2),
        }
    }
}

impl Unpack for Target {
    fn unpack(input: &mut In<'_>) -> Unpacked<Self> {
        Ok(match input.byte()? {
            0 => Self::Register(Unpack::unpack(input)?),
            1 => {
                let (key, value) = Unpack::unpack(input)?;
                Self::Member(key, value)
            }
            2 => Self::File,
            _ => return bad_variant(),
        })
    }
}

impl Pack for Op {
    fn pack(&self, out: &mut Vec<u8>) {
        match self {
            Self::Create {
                entity,
                replaces,
                take,
            } => {
                out.push(0);
                (entity, (replaces, take)).pack(out);
            }
            Self::Delete {
                entity,
                replaces,
                observed,
                take,
            } => {
                out.push(1);
                ((entity, replaces), (observed, take)).pack(out);
            }
            Self::Write {
                entity,
                key,
                value,
                replaces,
                take,
            } => {
                out.push(2);
                ((entity, key), (value, (replaces, take))).pack(out);
            }
            Self::Add {
                entity,
                key,
                value,
                replaces,
                take,
            } => {
                out.push(3);
                ((entity, key), (value, (replaces, take))).pack(out);
            }
            Self::Remove {
                entity,
                key,
                value,
                tags,
                take,
            } => {
                out.push(4);
                ((entity, key), (value, (tags, take))).pack(out);
            }
            Self::File {
                entity,
                file,
                replaces,
                take,
            } => {
                out.push(5);
                (entity, (file, (replaces, take))).pack(out);
            }
            Self::Pin {
                entity,
                file,
                replaces,
                take,
            } => {
                out.push(6);
                (entity, (file, (replaces, take))).pack(out);
            }
            Self::Dismiss { runs } => {
                out.push(7);
                runs.pack(out);
            }
            Self::Unknown(raw) => {
                out.push(8);
                raw.pack(out);
            }
            Self::Path { from, file } => {
                out.push(9);
                (from, file).pack(out);
            }
            Self::Tree { from, to } => {
                out.push(10);
                (from, to).pack(out);
            }
            Self::Dir { path, made } => {
                out.push(11);
                (path, made).pack(out);
            }
        }
    }
}

impl Unpack for Op {
    fn unpack(input: &mut In<'_>) -> Unpacked<Self> {
        Ok(match input.byte()? {
            0 => {
                let (entity, (replaces, take)) = Unpack::unpack(input)?;
                Self::Create {
                    entity,
                    replaces,
                    take,
                }
            }
            1 => {
                let ((entity, replaces), (observed, take)) = Unpack::unpack(input)?;
                Self::Delete {
                    entity,
                    replaces,
                    observed,
                    take,
                }
            }
            2 => {
                let ((entity, key), (value, (replaces, take))) = Unpack::unpack(input)?;
                Self::Write {
                    entity,
                    key,
                    value,
                    replaces,
                    take,
                }
            }
            3 => {
                let ((entity, key), (value, (replaces, take))) = Unpack::unpack(input)?;
                Self::Add {
                    entity,
                    key,
                    value,
                    replaces,
                    take,
                }
            }
            4 => {
                let ((entity, key), (value, (tags, take))) = Unpack::unpack(input)?;
                Self::Remove {
                    entity,
                    key,
                    value,
                    tags,
                    take,
                }
            }
            5 => {
                let (entity, (file, (replaces, take))) = Unpack::unpack(input)?;
                Self::File {
                    entity,
                    file,
                    replaces,
                    take,
                }
            }
            6 => {
                let (entity, (file, (replaces, take))) = Unpack::unpack(input)?;
                Self::Pin {
                    entity,
                    file,
                    replaces,
                    take,
                }
            }
            7 => Self::Dismiss {
                runs: Unpack::unpack(input)?,
            },
            8 => Self::Unknown(Raw::unpack(input)?),
            9 => {
                let (from, file) = Unpack::unpack(input)?;
                Self::Path { from, file }
            }
            10 => {
                let (from, to) = Unpack::unpack(input)?;
                Self::Tree { from, to }
            }
            11 => {
                let (path, made) = Unpack::unpack(input)?;
                Self::Dir { path, made }
            }
            _ => return bad_variant(),
        })
    }
}

/// Whether this build writes `kind` read at `at`, numbered `n`, after `prev` as
/// `json`, byte for byte; compared as it is written.
fn writes_as(prev: EntryHash, n: u64, at: Hlc, kind: &EntryKind, json: &str) -> bool {
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
    let written = serde_json::to_writer(&mut same, &Written { prev, n, at, kind });
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
    /// `prev`, `n` and `at`, in order of name.
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
                forks,
                more,
            }) => {
                map.serialize_entry("kind", "intent")?;
                map.serialize_entry("label", label)?;
                map.serialize_entry("ops", ops)?;
                if !displaced.is_empty() {
                    map.serialize_entry("displaced", displaced)?;
                }
                if !forks.is_empty() {
                    map.serialize_entry("forks", forks)?;
                }
                if *more != 0 {
                    map.serialize_entry("more", more)?;
                }
                Ok(())
            }
            Self::More(More {
                ops,
                displaced,
                forks,
                deletes,
            }) => {
                map.serialize_entry("kind", "more")?;
                map.serialize_entry("ops", ops)?;
                if !displaced.is_empty() {
                    map.serialize_entry("displaced", displaced)?;
                }
                if !forks.is_empty() {
                    map.serialize_entry("forks", forks)?;
                }
                if !deletes.is_empty() {
                    map.serialize_entry("deletes", deletes)?;
                }
                Ok(())
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
                    .filter(|(name, _)| !matches!(name.as_str(), "prev" | "n" | "at"));
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
        replaces: &'a [Ref],
        #[serde(skip_serializing_if = "Option::is_none")]
        take: Option<&'a Run>,
    },
    Delete {
        entity: EntityId,
        replaces: &'a [Ref],
        observed: &'a [Observed],
        #[serde(skip_serializing_if = "Option::is_none")]
        take: Option<&'a Run>,
    },
    Write {
        entity: EntityId,
        key: &'a str,
        #[serde(skip_serializing_if = "Option::is_none")]
        value: Option<&'a Raw>,
        replaces: &'a [Ref],
        #[serde(skip_serializing_if = "Option::is_none")]
        take: Option<&'a Run>,
    },
    Add {
        entity: EntityId,
        key: &'a str,
        value: &'a Raw,
        replaces: &'a [Ref],
        #[serde(skip_serializing_if = "Option::is_none")]
        take: Option<&'a Run>,
    },
    Remove {
        entity: EntityId,
        key: &'a str,
        value: &'a Raw,
        tags: &'a [Ref],
        #[serde(skip_serializing_if = "Option::is_none")]
        take: Option<&'a Run>,
    },
    File {
        entity: EntityId,
        #[serde(skip_serializing_if = "Option::is_none")]
        file: Option<&'a FileFact>,
        replaces: &'a [Ref],
        #[serde(skip_serializing_if = "Option::is_none")]
        take: Option<&'a Run>,
    },
    Pin {
        entity: EntityId,
        file: &'a FileFact,
        replaces: &'a [Ref],
        #[serde(skip_serializing_if = "Option::is_none")]
        take: Option<&'a Run>,
    },
    Dismiss {
        runs: &'a [Run],
    },
    Path {
        #[serde(skip_serializing_if = "Option::is_none")]
        from: Option<&'a RelPath>,
        #[serde(skip_serializing_if = "Option::is_none")]
        file: Option<&'a FileFact>,
    },
    Tree {
        from: &'a RelPath,
        to: &'a RelPath,
    },
    Dir {
        path: &'a RelPath,
        made: bool,
    },
}

/// A write without `value` clears; one with `"value":null` writes `null`.
impl Serialize for Op {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let out = match self {
            Self::Create {
                entity,
                replaces,
                take,
            } => OpOut::Create {
                entity: *entity,
                replaces,
                take: take.as_ref(),
            },
            Self::Delete {
                entity,
                replaces,
                observed,
                take,
            } => OpOut::Delete {
                entity: *entity,
                replaces,
                observed,
                take: take.as_ref(),
            },
            Self::Write {
                entity,
                key,
                value,
                replaces,
                take,
            } => OpOut::Write {
                entity: *entity,
                key,
                value: value.as_ref(),
                replaces,
                take: take.as_ref(),
            },
            Self::Add {
                entity,
                key,
                value,
                replaces,
                take,
            } => OpOut::Add {
                entity: *entity,
                key,
                value,
                replaces,
                take: take.as_ref(),
            },
            Self::Remove {
                entity,
                key,
                value,
                tags,
                take,
            } => OpOut::Remove {
                entity: *entity,
                key,
                value,
                tags,
                take: take.as_ref(),
            },
            Self::File {
                entity,
                file,
                replaces,
                take,
            } => OpOut::File {
                entity: *entity,
                file: file.as_ref(),
                replaces,
                take: take.as_ref(),
            },
            Self::Pin {
                entity,
                file,
                replaces,
                take,
            } => OpOut::Pin {
                entity: *entity,
                file,
                replaces,
                take: take.as_ref(),
            },
            Self::Path { from, file } => OpOut::Path {
                from: from.as_ref(),
                file: file.as_ref(),
            },
            Self::Tree { from, to } => OpOut::Tree { from, to },
            Self::Dir { path, made } => OpOut::Dir { path, made: *made },
            Self::Dismiss { runs } => OpOut::Dismiss { runs },
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
    replaces: Vec<Ref>,
    #[serde(default)]
    take: Option<Run>,
}

#[derive(Deserialize)]
struct DeleteIn {
    op: String,
    entity: EntityId,
    replaces: Vec<Ref>,
    observed: Vec<Observed>,
    #[serde(default)]
    take: Option<Run>,
}

#[derive(Deserialize)]
struct WriteIn {
    op: String,
    entity: EntityId,
    key: String,
    #[serde(default, deserialize_with = "present")]
    value: Option<Raw>,
    replaces: Vec<Ref>,
    #[serde(default)]
    take: Option<Run>,
}

#[derive(Deserialize)]
struct AddIn {
    op: String,
    entity: EntityId,
    key: String,
    value: Raw,
    replaces: Vec<Ref>,
    #[serde(default)]
    take: Option<Run>,
}

#[derive(Deserialize)]
struct RemoveIn {
    op: String,
    entity: EntityId,
    key: String,
    value: Raw,
    tags: Vec<Ref>,
    #[serde(default)]
    take: Option<Run>,
}

#[derive(Deserialize)]
struct FileIn {
    op: String,
    entity: EntityId,
    #[serde(default)]
    file: Option<FileFact>,
    replaces: Vec<Ref>,
    #[serde(default)]
    take: Option<Run>,
}

#[derive(Deserialize)]
struct PathIn {
    op: String,
    #[serde(default)]
    from: Option<RelPath>,
    #[serde(default)]
    file: Option<FileFact>,
}

#[derive(Deserialize)]
struct TreeIn {
    op: String,
    from: RelPath,
    to: RelPath,
}

#[derive(Deserialize)]
struct DirIn {
    op: String,
    path: RelPath,
    made: bool,
}

#[derive(Deserialize)]
struct PinIn {
    op: String,
    entity: EntityId,
    file: FileFact,
    replaces: Vec<Ref>,
    #[serde(default)]
    take: Option<Run>,
}

#[derive(Deserialize)]
struct DismissIn {
    op: String,
    runs: Vec<Run>,
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
                 entity,
                 replaces,
                 take,
                 ..
             }| Op::Create {
                entity,
                replaces,
                take,
            },
        ),
        "delete" => op_members(json, name, |m: &DeleteIn| &m.op).map(
            |DeleteIn {
                 entity,
                 replaces,
                 observed,
                 take,
                 ..
             }| Op::Delete {
                entity,
                replaces,
                observed,
                take,
            },
        ),
        "write" => op_members(json, name, |m: &WriteIn| &m.op).map(
            |WriteIn {
                 entity,
                 key,
                 value,
                 replaces,
                 take,
                 ..
             }| Op::Write {
                entity,
                key,
                value,
                replaces,
                take,
            },
        ),
        "add" => op_members(json, name, |m: &AddIn| &m.op).map(
            |AddIn {
                 entity,
                 key,
                 value,
                 replaces,
                 take,
                 ..
             }| Op::Add {
                entity,
                key,
                value,
                replaces,
                take,
            },
        ),
        "remove" => op_members(json, name, |m: &RemoveIn| &m.op).map(
            |RemoveIn {
                 entity,
                 key,
                 value,
                 tags,
                 take,
                 ..
             }| Op::Remove {
                entity,
                key,
                value,
                tags,
                take,
            },
        ),
        "file" => op_members(json, name, |m: &FileIn| &m.op).map(
            |FileIn {
                 entity,
                 file,
                 replaces,
                 take,
                 ..
             }| Op::File {
                entity,
                file,
                replaces,
                take,
            },
        ),
        "pin" => op_members(json, name, |m: &PinIn| &m.op).map(
            |PinIn {
                 entity,
                 file,
                 replaces,
                 take,
                 ..
             }| Op::Pin {
                entity,
                file,
                replaces,
                take,
            },
        ),
        "path" => op_members(json, name, |m: &PathIn| &m.op)
            .map(|PathIn { from, file, .. }| Op::Path { from, file }),
        "tree" => op_members(json, name, |m: &TreeIn| &m.op)
            .map(|TreeIn { from, to, .. }| Op::Tree { from, to }),
        "dir" => op_members(json, name, |m: &DirIn| &m.op)
            .map(|DirIn { path, made, .. }| Op::Dir { path, made }),
        "dismiss" => op_members(json, name, |m: &DismissIn| &m.op)
            .map(|DismissIn { runs, .. }| Op::Dismiss { runs }),
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

    fn r(n: u128) -> Ref {
        Ref {
            writer: WriterId::from_u128(0xb),
            n: n as u64,
            hash: hash(n),
        }
    }

    fn run() -> Run {
        Run {
            writer: WriterId::from_u128(0xb),
            from: 3,
            first: None,
            last: Own {
                n: 4,
                hash: hash(4),
            },
            segment: Some(Own {
                n: 2,
                hash: hash(2),
            }),
            tip: Some(Own {
                n: 4,
                hash: hash(4),
            }),
        }
    }

    fn every_op() -> Vec<Op> {
        let entity = EntityId::from_u128(7);
        vec![
            Op::Create {
                entity,
                replaces: vec![r(1)],
                take: None,
            },
            Op::Delete {
                entity,
                replaces: vec![r(1)],
                observed: vec![
                    Observed {
                        target: Target::Register("origin".into()),
                        at: r(2),
                    },
                    Observed {
                        target: Target::Member("tags".into(), raw(r#""x""#)),
                        at: r(3),
                    },
                    Observed {
                        target: Target::File,
                        at: r(5),
                    },
                ],
                take: Some(run()),
            },
            Op::Write {
                entity,
                key: "origin".into(),
                value: Some(raw(r#"{"b":1,"a":123456789012345678901234567890}"#)),
                replaces: vec![],
                take: None,
            },
            Op::Write {
                entity,
                key: "origin".into(),
                value: Some(raw("null")),
                replaces: vec![r(4)],
                take: Some(Run {
                    first: Some(hash(3)),
                    tip: None,
                    ..run()
                }),
            },
            Op::Write {
                entity,
                key: "origin".into(),
                value: None,
                replaces: vec![r(5)],
                take: None,
            },
            Op::Add {
                entity,
                key: "tags".into(),
                value: raw(r#""Sunday""#),
                replaces: vec![r(2)],
                take: None,
            },
            Op::Remove {
                entity,
                key: "tags".into(),
                value: raw(r#""Sunday""#),
                tags: vec![r(6)],
                take: None,
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
                take: None,
            },
            Op::File {
                entity,
                file: None,
                replaces: vec![r(8)],
                take: None,
            },
            Op::Pin {
                entity,
                file: FileFact {
                    path: RelPath::new("c.npno").unwrap(),
                    identity: Identity::from_u128(12),
                    len: 13,
                    modified: None,
                },
                replaces: vec![r(10)],
                take: None,
            },
            Op::Path {
                from: Some(RelPath::new("d.wav").unwrap()),
                file: None,
            },
            Op::Tree {
                from: RelPath::new("old").unwrap(),
                to: RelPath::new("new/old").unwrap(),
            },
            Op::Dir {
                path: RelPath::new("Gig").unwrap(),
                made: true,
            },
            Op::Pin {
                entity,
                file: FileFact {
                    path: RelPath::new("d.npno").unwrap(),
                    identity: Identity::from_u128(14),
                    len: 15,
                    modified: Some(16),
                },
                replaces: vec![],
                take: Some(run()),
            },
            Op::Dismiss { runs: vec![run()] },
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
                forks: vec![Forked {
                    writer: WriterId::from_u128(0xb),
                    at: 2,
                }],
                more: 7,
            }),
            EntryKind::More(More {
                ops: every_op(),
                displaced: vec![Displaced {
                    item: Nonce::from_u128(5),
                    from: RelPath::new("y.npno").unwrap(),
                    identity: Identity::from_u128(6),
                    len: 7,
                }],
                forks: vec![Forked {
                    writer: WriterId::from_u128(0xb),
                    at: 2,
                }],
                deletes: vec![EntityId::from_u128(8)],
            }),
            EntryKind::More(More {
                ops: every_op(),
                ..More::default()
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
            let (prev, n) = match kind {
                EntryKind::Genesis(_) => (EntryHash::ZERO, 0),
                _ => (hash(1), 2),
            };
            let entry = Entry::encode(prev, n, at(5), kind.clone()).unwrap();
            assert_eq!(entry.kind(), kind);
            assert_eq!((entry.prev(), entry.n()), (prev, n));
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
                    replaces: vec![],
                    take: None,
                },
                Op::Write {
                    entity,
                    key: "origin".into(),
                    value: None,
                    replaces: vec![r(1)],
                    take: None,
                },
            ],
            displaced: vec![],
            forks: vec![],
            more: 0,
        });
        let entry = Entry::encode(hash(2), 3, at(3), kind).unwrap();
        let e = "0000000000000000000000000000000e";
        let w = "0000000000000000000000000000000b";
        let expected = format!(
            concat!(
                r#"{{"prev":"00000000000000000000000000000002","n":3,"at":[3,0],"kind":"intent","#,
                r#""label":"Tag","ops":[{{"op":"add","entity":"{e}","key":"tags","value":"x","replaces":[]}},"#,
                r#"{{"op":"write","entity":"{e}","key":"origin","replaces":[["{w}",1,"#,
                r#""00000000000000000000000000000001"]]}}]}}"#
            ),
            e = e,
            w = w
        );
        assert_eq!(entry.json(), expected);
    }

    // The members of each kind, in the order SPEC.md gives them.
    #[test]
    fn every_kind_is_written_as_specified() {
        let id = |n: u128| format!("{n:032x}");
        let written = |prev: EntryHash, n: u64, kind: EntryKind| {
            Entry::encode(prev, n, at(5), kind)
                .unwrap()
                .json()
                .into_owned()
        };
        let genesis = EntryKind::Genesis(Genesis {
            writer: WriterId::from_u128(1),
            label: "drawbar".into(),
        });
        assert_eq!(
            written(EntryHash::ZERO, 0, genesis),
            format!(
                r#"{{"prev":"{}","n":0,"at":[5,0],"kind":"genesis","writer":"{}","label":"drawbar"}}"#,
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
            forks: vec![Forked {
                writer: WriterId::from_u128(2),
                at: 6,
            }],
            more: 0,
        });
        assert_eq!(
            written(hash(1), 1, save),
            format!(
                concat!(
                    r#"{{"prev":"{}","n":1,"at":[5,0],"kind":"intent","label":"Save","ops":[],"#,
                    r#""displaced":[{{"item":"{}","from":"x.npno","identity":"{}","len":5}}],"#,
                    r#""forks":[["{}",6]]}}"#
                ),
                id(1),
                id(3),
                id(4),
                id(2)
            )
        );
        let moved = EntryKind::Intent(Logged {
            label: "Move".into(),
            ops: Vec::new(),
            displaced: Vec::new(),
            forks: Vec::new(),
            more: 2,
        });
        assert_eq!(
            written(hash(1), 1, moved),
            format!(
                r#"{{"prev":"{}","n":1,"at":[5,0],"kind":"intent","label":"Move","ops":[],"more":2}}"#,
                id(1)
            )
        );
        let more = EntryKind::More(More {
            ops: vec![Op::File {
                entity: EntityId::from_u128(7),
                file: None,
                replaces: Vec::new(),
                take: None,
            }],
            displaced: vec![Displaced {
                item: Nonce::from_u128(3),
                from: RelPath::new("x.npno").unwrap(),
                identity: Identity::from_u128(4),
                len: 5,
            }],
            forks: Vec::new(),
            deletes: vec![EntityId::from_u128(7)],
        });
        assert_eq!(
            written(hash(1), 2, more),
            format!(
                concat!(
                    r#"{{"prev":"{}","n":2,"at":[5,0],"kind":"more","#,
                    r#""ops":[{{"op":"file","entity":"{}","replaces":[]}}],"#,
                    r#""displaced":[{{"item":"{}","from":"x.npno","identity":"{}","len":5}}],"#,
                    r#""deletes":["{}"]}}"#
                ),
                id(1),
                id(7),
                id(3),
                id(4),
                id(7)
            )
        );
        let settle = EntryKind::Settle(Settle {
            writer: WriterId::from_u128(2),
            record: Nonce::from_u128(3),
            outcome: Settlement::RolledBack,
        });
        assert_eq!(
            written(hash(1), 1, settle),
            format!(
                r#"{{"prev":"{}","n":1,"at":[5,0],"kind":"settle","writer":"{}","record":"{}","outcome":"rolled-back"}}"#,
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
                replaces: vec![r(10)],
                take: None,
            }],
        });
        assert_eq!(
            written(hash(1), 1, bind),
            format!(
                concat!(
                    r#"{{"prev":"{}","n":1,"at":[5,0],"kind":"bind","ops":[{{"op":"pin","entity":"{}","#,
                    r#""file":{{"path":"c.npno","identity":"{}","len":13}},"replaces":[["{}",10,"{}"]]}}]}}"#
                ),
                id(1),
                id(7),
                id(12),
                id(0xb),
                id(10)
            )
        );
        let dismiss = EntryKind::Intent(Logged {
            label: "Keep".into(),
            ops: vec![Op::Dismiss { runs: vec![run()] }],
            displaced: vec![],
            forks: vec![],
            more: 0,
        });
        assert_eq!(
            written(hash(1), 1, dismiss),
            format!(
                concat!(
                    r#"{{"prev":"{}","n":1,"at":[5,0],"kind":"intent","label":"Keep","#,
                    r#""ops":[{{"op":"dismiss","runs":[{{"writer":"{}","from":3,"last":[4,"{}"],"tip":[4,"{}"],"#,
                    r#""segment":[2,"{}"]}}]}}]}}"#
                ),
                id(1),
                id(0xb),
                id(4),
                id(4),
                id(2)
            )
        );
    }

    #[test]
    fn unknown_kinds_ops_and_members_survive() {
        let prev = hash(1);
        let json = format!(
            concat!(
                r#"{{"prev":"{prev}","n":2,"at":[1,2],"kind":"intent","label":"L","later":[1,2],"#,
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

        let future = format!(r#"{{"prev":"{prev}","n":2,"at":[1,2],"kind":"merge","into":"y"}}"#);
        let entry = Entry::decode(Line::seal(future.clone()).unwrap()).unwrap();
        assert_eq!(entry.kind(), EntryKind::Unknown(raw(&future)));
        let again = Entry::encode(hash(9), 4, at(4), entry.kind()).unwrap();
        assert_eq!(
            again.json(),
            r#"{"prev":"00000000000000000000000000000009","n":4,"at":[4,0],"into":"y","kind":"merge"}"#
        );
    }

    #[test]
    fn only_members_a_build_leaves_out_mark_an_entry_extended() {
        let prev = hash(1);
        let e = "0000000000000000000000000000000e";
        let op = format!(
            r#"{{"op":"add","entity":"{e}","key":"tags","value":{{"b":1,"a":[2]}},"replaces":[]}}"#
        );
        let reordered = format!(
            r#"{{ "kind" : "intent", "ops" : [ {op} ], "label" : "L", "at" : [1, 2], "n": 2, "prev" : "{prev}" }}"#
        );
        let entry = Entry::decode(Line::seal(reordered.clone()).unwrap()).unwrap();
        assert!(
            matches!(entry.kind(), EntryKind::Intent(_)),
            "{:?}",
            entry.kind()
        );
        assert!(!entry.unknown_members(), "{reordered}");

        let nested = format!(
            r#"{{"prev":"{prev}","n":2,"at":[1,2],"kind":"intent","label":"L","ops":[{}]}}"#,
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
            format!(r#"{{"prev":"{prev}","n":1,"at":[1,0],"kind":"settle","writer":"x"}}"#),
            format!(
                r#"{{"prev":"{prev}","n":1,"at":[1,0],"kind":"genesis","writer":"{prev}","label":"L"}}"#
            ),
            format!(r#"{{"prev":"{prev}","n":1,"at":[1,0]}}"#),
            format!(r#"{{"prev":"{prev}","n":1,"at":[1,0],"kind":7}}"#),
        ] {
            let entry = Entry::decode(Line::seal(json.clone()).unwrap()).unwrap();
            assert_eq!(entry.kind(), EntryKind::Unknown(raw(&json)), "{json}");
        }
        let zero = EntryHash::ZERO;
        let numbered = format!(
            r#"{{"prev":"{zero}","n":1,"at":[1,0],"kind":"genesis","writer":"{prev}","label":"L"}}"#
        );
        let entry = Entry::decode(Line::seal(numbered.clone()).unwrap()).unwrap();
        assert_eq!(entry.kind(), EntryKind::Unknown(raw(&numbered)));
    }

    #[test]
    fn an_entry_without_a_readable_clock_or_number_is_malformed() {
        let prev = hash(1);
        for json in [
            format!(r#"{{"prev":"{prev}","n":1}}"#),
            format!(r#"{{"prev":"{prev}","n":1,"at":"soon"}}"#),
            format!(r#"{{"prev":"{prev}","n":1,"at":[1]}}"#),
            format!(r#"{{"prev":"{prev}","at":[1,0],"kind":"intent","label":"L","ops":[]}}"#),
            format!(
                r#"{{"prev":"{prev}","n":-1,"at":[1,0],"kind":"intent","label":"L","ops":[]}}"#
            ),
        ] {
            let line = Line::seal(json.clone()).unwrap();
            let hash = line.hash();
            assert!(
                matches!(Entry::decode(line.clone()), Err(Malformed { hash: h, .. }) if h == hash),
                "{json}"
            );
            assert_eq!(Entry::linked(line).n(), Entry::UNNUMBERED, "{json}");
        }
    }

    #[test]
    fn a_line_read_in_one_pass_reads_as_it_does_parsed_then_decoded() {
        let prev = hash(1);
        let e = "0000000000000000000000000000000e";
        let add =
            format!(r#"{{"op":"add","entity":"{e}","key":"tags","value":"x","replaces":[]}}"#);
        let jsons = [
            format!(
                r#"{{"prev":"{prev}","n":1,"at":[1,0],"kind":"intent","label":"L","ops":[{add}]}}"#
            ),
            format!(
                r#"{{"prev":"{prev}","n":1,"at":[1,0],"kind":"intent","label":"L","ops":[],"x":1}}"#
            ),
            format!(
                r#"{{"kind":"intent","ops":[{add}],"label":"L","at":[1,0],"n":1,"prev":"{prev}"}}"#
            ),
            format!(
                r#"{{"prev":"{prev}","n":1,"at":[1,0],"kind":"intent","label":"L","ops":[7,{{}}]}}"#
            ),
            format!(
                r#"{{"prev":"{prev}","n":1,"at":[1,0],"kind":"settle","writer":"{e}","record":"{e}","outcome":"dismissed"}}"#
            ),
            format!(r#"{{"prev":"{prev}","n":1,"at":[1,0],"kind":"merge"}}"#),
            format!(
                r#"{{"prev":"{prev}","n":1,"at":"soon","kind":"intent","label":"L","ops":[]}}"#
            ),
            format!(
                r#"{{"prev":"{prev}","prev":"{e}","n":1,"at":[1,0],"kind":"intent","label":"L","ops":[]}}"#
            ),
            format!(r#"{{"prev":"{prev}","n":1,"at":[1,0],"kind":"intent","label":"L","ops":[]"#),
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
            insert(end, r#","forks":[]"#),
            insert(end, r#","label":"again""#),
            insert(label, r#""prev":"00000000000000000000000000000001","#),
            insert(commas[pick(commas.len())], "]"),
            json.replacen(r#""ops":[{"op""#, r#""ops":[{"o\u0070""#, 1),
            json.replacen(r#""op":"add","#, r#""op":"add","op":"add","#, 1),
            json.replacen(r#","replaces""#, r#","file":null,"replaces""#, 1),
            json.replacen(r#","replaces""#, r#","take":null,"replaces""#, 1),
            json.replacen(r#""len":10"#, r#""len":10,"modified":null"#, 1),
            json.replacen(r#""len":10"#, r#""len":10,"size":2"#, 1),
            json.replacen(r#""from":3"#, r#""from":3,"first":null"#, 1),
            json.replacen(r#""at":["#, r#""at":[[0],"#, 1),
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
                forks: match round % 4 {
                    0 => vec![Forked {
                        writer: WriterId::from_u128(0xb),
                        at: 1,
                    }],
                    _ => Vec::new(),
                },
                more: 0,
            });
            let written = Entry::encode(hash(1), round as u64 + 1, at(round as u64), kind).unwrap();
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
            forks: Vec::new(),
            more: 0,
        });
        let written = Entry::encode(hash(1), 2, at(3), kind).unwrap();
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
        let entry = Entry::encode(
            hash(1),
            2,
            at(3),
            EntryKind::Bind(Bound { ops: every_op() }),
        )
        .unwrap();
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
            format!(
                r#"{{"op":"add","entity":"{e}","key":"k","value":1,"replaces":[],"op":"write"}}"#
            ),
            format!(
                r#"{{"entity":"{e}","op":"add","key":"k","value":1,"replaces":[],"op":"add"}}"#
            ),
        ] {
            let json = format!(
                r#"{{"prev":"{}","n":1,"at":[1,0],"kind":"intent","label":"L","ops":[{op}]}}"#,
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
    fn an_observed_write_naming_no_one_field_makes_its_delete_unknown() {
        let e = "0000000000000000000000000000000e";
        let at = r#"["0000000000000000000000000000000b",1,"00000000000000000000000000000001"]"#;
        for observed in [
            format!(r#"{{"at":{at}}}"#),
            format!(r#"{{"key":"k","file":true,"at":{at}}}"#),
            format!(r#"{{"value":1,"at":{at}}}"#),
            format!(r#"{{"file":false,"at":{at}}}"#),
        ] {
            let op = format!(
                r#"{{"op":"delete","entity":"{e}","replaces":[],"observed":[{observed}]}}"#
            );
            assert_eq!(decode_op(&op), None, "{op}");
        }
    }

    #[test]
    fn an_entry_too_long_for_a_line_is_refused() {
        let kind = EntryKind::Genesis(Genesis {
            writer: WriterId::from_u128(1),
            label: "x".repeat(crate::line::MAX_LINE),
        });
        assert_eq!(
            Entry::encode(EntryHash::ZERO, 0, at(1), kind),
            Err(LineError::TooLong)
        );
    }
}
