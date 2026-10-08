//! Snapshots: a writer's own entries folded, with every folded hash in chain order.

use std::collections::{BTreeMap, BTreeSet};

use serde::de::DeserializeOwned;
use serde::Serialize;
use thiserror::Error as ThisError;

use crate::ids::{EntryHash, Hlc, WriterId};
use crate::merge::Folded;
use crate::pack::{Bad, In, Pack, Unpack, Unpacked};
use crate::reader::MAX_FILE;
use crate::schema::Raw;

/// `snapshot-<nonce>.json` in a writer's directory: one JSON object, without a
/// final newline.
///
/// `folded` lists every entry the snapshot folds, from the genesis entry on, each
/// the successor of the one before it, so a reader can place an entry after any
/// folded one and tell a fork from any point.
#[derive(Clone, PartialEq, Debug)]
pub struct Snapshot {
    pub writer: WriterId,
    pub label: String,
    /// The reading of the last folded entry.
    pub at: Hlc,
    pub folded: Vec<EntryHash>,
    pub state: Folded,
    /// Members this build does not know, kept.
    pub unknown: BTreeMap<String, Raw>,
}

#[derive(ThisError, Clone, PartialEq, Eq, Debug)]
#[error("not a snapshot: {reason}")]
pub struct NotSnapshot {
    pub reason: String,
}

const KNOWN: [&str; 5] = ["writer", "label", "at", "folded", "state"];

#[derive(Serialize)]
struct Written<'a> {
    writer: WriterId,
    label: &'a str,
    at: Hlc,
    folded: &'a [EntryHash],
    state: &'a Folded,
    #[serde(flatten)]
    unknown: BTreeMap<&'a str, &'a Raw>,
}

impl Snapshot {
    /// Refuses bytes that are not a snapshot this build can place: not a JSON
    /// object with every member it needs, longer than a reader holds, or with an
    /// empty or repeating `folded` list.
    pub fn decode(bytes: &[u8]) -> Result<Self, NotSnapshot> {
        let refuse = |reason: String| NotSnapshot { reason };
        if bytes.len() as u64 > MAX_FILE {
            return Err(refuse(format!("longer than {MAX_FILE} bytes")));
        }
        let mut members: BTreeMap<String, Raw> =
            serde_json::from_slice(bytes).map_err(|error| refuse(error.to_string()))?;
        let mut take = |name: &str| {
            members
                .remove(name)
                .ok_or_else(|| refuse(format!("no `{name}`")))
        };
        let (writer, label, at, folded, state) = (
            take("writer")?,
            take("label")?,
            take("at")?,
            take("folded")?,
            take("state")?,
        );
        let snapshot = Self {
            writer: member("writer", &writer)?,
            label: member("label", &label)?,
            at: member("at", &at)?,
            folded: member("folded", &folded)?,
            state: member("state", &state)?,
            unknown: members,
        };
        let distinct: BTreeSet<_> = snapshot.folded.iter().collect();
        if snapshot.folded.is_empty() || distinct.len() != snapshot.folded.len() {
            return Err(refuse("`folded` is empty or repeats a hash".into()));
        }
        Ok(snapshot)
    }

    /// Members of [`Snapshot::unknown`] under a known name are left out.
    pub fn encode(&self) -> Vec<u8> {
        let unknown = self
            .unknown
            .iter()
            .filter(|(name, _)| !KNOWN.contains(&name.as_str()))
            .map(|(name, raw)| (name.as_str(), raw))
            .collect();
        serde_json::to_vec(&Written {
            writer: self.writer,
            label: &self.label,
            at: self.at,
            folded: &self.folded,
            state: &self.state,
            unknown,
        })
        .expect("a snapshot is JSON")
    }

    pub fn folds(&self, hash: EntryHash) -> bool {
        self.folded.contains(&hash)
    }

    /// The folded entry before `hash`: [`EntryHash::ZERO`] for the genesis entry,
    /// `None` when `hash` is not folded.
    pub fn predecessor(&self, hash: EntryHash) -> Option<EntryHash> {
        let at = self.folded.iter().position(|&folded| folded == hash)?;
        Some(
            at.checked_sub(1)
                .map_or(EntryHash::ZERO, |before| self.folded[before]),
        )
    }

    /// The last folded entry.
    pub fn head(&self) -> Option<EntryHash> {
        self.folded.last().copied()
    }

    /// Each folded entry with its predecessor, in chain order.
    pub fn pairs(&self) -> impl Iterator<Item = (EntryHash, EntryHash)> + '_ {
        let prevs = std::iter::once(EntryHash::ZERO).chain(self.folded.iter().copied());
        self.folded.iter().copied().zip(prevs)
    }

    /// Whether this snapshot folds every entry `other` does, in the same order.
    pub fn extends(&self, other: &Snapshot) -> bool {
        self.folded.starts_with(&other.folded)
    }
}

impl Pack for Snapshot {
    fn pack(&self, out: &mut Vec<u8>) {
        (self.writer, &self.label).pack(out);
        (self.at, &self.folded).pack(out);
        (&self.state, &self.unknown).pack(out);
    }
}

/// Refuses what [`Snapshot::decode`] refuses.
impl Unpack for Snapshot {
    fn unpack(input: &mut In<'_>) -> Unpacked<Self> {
        let (writer, label) = Unpack::unpack(input)?;
        let (at, folded): (Hlc, Vec<EntryHash>) = Unpack::unpack(input)?;
        let (state, unknown) = Unpack::unpack(input)?;
        let distinct: BTreeSet<_> = folded.iter().collect();
        if folded.is_empty() || distinct.len() != folded.len() {
            return Err(Bad("`folded` is empty or repeats a hash"));
        }
        Ok(Self {
            writer,
            label,
            at,
            folded,
            state,
            unknown,
        })
    }
}

fn member<T: DeserializeOwned>(name: &str, raw: &Raw) -> Result<T, NotSnapshot> {
    raw.decode().map_err(|error| NotSnapshot {
        reason: format!("`{name}`: {error}"),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn snapshot(folded: &[u128]) -> Snapshot {
        Snapshot {
            writer: WriterId::from_u128(1),
            label: "drawbar".into(),
            at: Hlc {
                wall_ms: 5,
                counter: 1,
            },
            folded: folded.iter().copied().map(EntryHash::from_u128).collect(),
            state: Folded::default(),
            unknown: BTreeMap::new(),
        }
    }

    fn hash(n: u128) -> EntryHash {
        EntryHash::from_u128(n)
    }

    #[test]
    fn a_snapshot_reads_back_with_members_it_does_not_know() {
        let mut written = snapshot(&[1, 2, 3]);
        written
            .unknown
            .insert("later".into(), Raw::new(r#"{"x":[1,2]}"#).unwrap());
        let read = Snapshot::decode(&written.encode()).unwrap();
        assert_eq!(read, written);
        assert_eq!(read.encode(), written.encode());
    }

    #[test]
    fn a_snapshot_packs_and_unpacks_whole() {
        let mut written = snapshot(&[1, 2, 3]);
        written
            .unknown
            .insert("later".into(), Raw::new(r#"{"x":[1,2]}"#).unwrap());
        let packed = crate::pack::packed(&written);
        assert_eq!(crate::pack::unpacked::<Snapshot>(&packed), Ok(written));
        let repeated = crate::pack::packed(&snapshot(&[1, 1]));
        assert!(crate::pack::unpacked::<Snapshot>(&repeated).is_err());
    }

    #[test]
    fn folded_entries_know_their_predecessors() {
        let snapshot = snapshot(&[1, 2, 3]);
        assert_eq!(snapshot.predecessor(hash(1)), Some(EntryHash::ZERO));
        assert_eq!(snapshot.predecessor(hash(3)), Some(hash(2)));
        assert_eq!(snapshot.predecessor(hash(4)), None);
        assert_eq!(snapshot.head(), Some(hash(3)));
        assert_eq!(
            snapshot.pairs().collect::<Vec<_>>(),
            [
                (hash(1), EntryHash::ZERO),
                (hash(2), hash(1)),
                (hash(3), hash(2))
            ]
        );
        assert!(snapshot.extends(&self::snapshot(&[1, 2])));
        assert!(!self::snapshot(&[1, 2]).extends(&snapshot));
        assert!(!snapshot.extends(&self::snapshot(&[1, 4])));
    }

    #[test]
    fn what_cannot_be_placed_is_not_a_snapshot() {
        let good = String::from_utf8(snapshot(&[1, 2]).encode()).unwrap();
        let one = format!("\"{}\"", hash(1));
        let cases = [
            good.replace(&format!("[{one},\"{}\"]", hash(2)), "[]"),
            good.replace(&format!("\"{}\"]", hash(2)), &format!("{one}]")),
            good.replace("\"label\"", "\"name\""),
            good.replace("[5,1]", "5"),
            "[]".into(),
            good[..good.len() - 1].into(),
        ];
        for text in cases {
            assert!(Snapshot::decode(text.as_bytes()).is_err(), "{text}");
        }
    }
}
