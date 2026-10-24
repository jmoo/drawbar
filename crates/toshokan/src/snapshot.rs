//! Snapshots: what a writer still contributes, and what lets a reader recognize
//! every entry it folds.

use std::collections::{BTreeMap, BTreeSet};

use serde::de::{DeserializeOwned, IgnoredAny};
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use thiserror::Error as ThisError;

use crate::contribution::Contribution;
use crate::ids::{EntryHash, Hlc, WriterId};
use crate::log::Own;
use crate::pack::{pack_struct, Bad, In, Pack, Unpack, Unpacked};
use crate::reader::MAX_FILE;
use crate::schema::Raw;

/// `snapshot-<nonce>.json` in a writer's directory: one JSON object, without a
/// final newline.
#[derive(Clone, PartialEq, Debug)]
pub struct Snapshot {
    pub writer: WriterId,
    pub label: String,
    /// The reading of the last folded entry.
    pub at: Hlc,
    /// The hash of the writer's genesis entry.
    pub genesis: EntryHash,
    /// The last folded entry.
    pub head: Own,
    pub certified: Certified,
    pub state: Contribution,
    /// Members this build does not know, kept.
    pub unknown: BTreeMap<String, Raw>,
}

/// How a snapshot certifies every entry it folds.
#[derive(Clone, PartialEq, Eq, Debug, Default, Serialize, Deserialize)]
pub struct Certified {
    /// Segments its chain was written to: each by its first line, and the last
    /// of its lines the snapshot folds.
    #[serde(default, skip_serializing_if = "BTreeSet::is_empty")]
    pub segments: BTreeSet<SegmentCert>,
    /// The heads of the snapshots of its directory on its chain.
    #[serde(default, skip_serializing_if = "BTreeSet::is_empty")]
    pub heads: BTreeSet<Own>,
    /// Every other folded entry, and the last entry each of its dismissals names
    /// on its chain.
    #[serde(default, skip_serializing_if = "BTreeSet::is_empty")]
    pub hashes: BTreeSet<Own>,
}

/// A segment a snapshot folds lines of: its first line, and the last line it
/// folds, by number and hash. Written `[[n, "<hash>"], [n, "<hash>"]]`.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
pub struct SegmentCert {
    pub first: Own,
    pub last: Own,
}

impl Serialize for SegmentCert {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        (self.first, self.last).serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for SegmentCert {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let (first, last) = Deserialize::deserialize(deserializer)?;
        Ok(Self { first, last })
    }
}

impl Certified {
    /// The entries these certificates name: every segment's ends, every head,
    /// every hash.
    pub fn named(&self) -> impl Iterator<Item = Own> + '_ {
        let ends = self
            .segments
            .iter()
            .flat_map(|cert| [cert.first, cert.last]);
        ends.chain(self.heads.iter().copied())
            .chain(self.hashes.iter().copied())
    }
}

#[derive(ThisError, Clone, PartialEq, Eq, Debug)]
#[error("not a snapshot: {reason}")]
pub struct NotSnapshot {
    pub reason: String,
    /// Whether the bytes, less a zero-filled tail, end inside a JSON value, as
    /// a file a crash cut short does.
    pub cut: bool,
}

const KNOWN: [&str; 8] = [
    "writer",
    "label",
    "at",
    "genesis",
    "n",
    "head",
    "certified",
    "state",
];

#[derive(Serialize)]
struct Written<'a> {
    writer: WriterId,
    label: &'a str,
    at: Hlc,
    genesis: EntryHash,
    n: u64,
    head: EntryHash,
    certified: &'a Certified,
    state: &'a Contribution,
    #[serde(flatten)]
    unknown: BTreeMap<&'a str, &'a Raw>,
}

impl Snapshot {
    /// Refuses bytes that are not a snapshot this build can place: not a JSON
    /// object with every member it needs, longer than a reader holds, or with a
    /// certificate whose last line comes before its first.
    pub fn decode(bytes: &[u8]) -> Result<Self, NotSnapshot> {
        let refuse = |reason: String| NotSnapshot { reason, cut: false };
        if bytes.len() as u64 > MAX_FILE {
            return Err(refuse(format!("longer than {MAX_FILE} bytes")));
        }
        let mut members: BTreeMap<String, Raw> =
            serde_json::from_slice(bytes).map_err(|error| NotSnapshot {
                reason: error.to_string(),
                cut: ends_inside_a_value(bytes),
            })?;
        let mut take = |name: &str| {
            members
                .remove(name)
                .ok_or_else(|| refuse(format!("no `{name}`")))
        };
        let (writer, label, at, genesis, n, head, certified, state) = (
            take("writer")?,
            take("label")?,
            take("at")?,
            take("genesis")?,
            take("n")?,
            take("head")?,
            take("certified")?,
            take("state")?,
        );
        let snapshot = Self {
            writer: member("writer", &writer)?,
            label: member("label", &label)?,
            at: member("at", &at)?,
            genesis: member("genesis", &genesis)?,
            head: Own {
                n: member("n", &n)?,
                hash: member("head", &head)?,
            },
            certified: member("certified", &certified)?,
            state: member("state", &state)?,
            unknown: members,
        };
        snapshot
            .check()
            .map_err(|Bad(reason)| refuse(reason.into()))?;
        Ok(snapshot)
    }

    fn check(&self) -> Unpacked<()> {
        let backwards = self
            .certified
            .segments
            .iter()
            .any(|cert| cert.last.n < cert.first.n);
        let past = self.certified.named().any(|own| own.n > self.head.n);
        match (backwards, past) {
            (false, false) => Ok(()),
            (true, _) => Err(Bad("a certificate whose last line comes before its first")),
            (_, true) => Err(Bad("a certificate past the head")),
        }
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
            genesis: self.genesis,
            n: self.head.n,
            head: self.head.hash,
            certified: &self.certified,
            state: &self.state,
            unknown,
        })
        .expect("a snapshot is JSON")
    }

    /// Whether this snapshot certifies `own` by name: as its head, a segment's
    /// end, an earlier snapshot's head, or by hash.
    pub fn names(&self, own: Own) -> bool {
        self.head == own || self.certified.named().any(|named| named == own)
    }
}

pack_struct!(SegmentCert { first, last });

impl Pack for Certified {
    fn pack(&self, out: &mut Vec<u8>) {
        let segments: Vec<&SegmentCert> = self.segments.iter().collect();
        let heads: Vec<&Own> = self.heads.iter().collect();
        let hashes: Vec<&Own> = self.hashes.iter().collect();
        (segments, (heads, hashes)).pack(out);
    }
}

impl Unpack for Certified {
    fn unpack(input: &mut In<'_>) -> Unpacked<Self> {
        let (segments, (heads, hashes)): (Vec<SegmentCert>, (Vec<Own>, Vec<Own>)) =
            Unpack::unpack(input)?;
        Ok(Self {
            segments: segments.into_iter().collect(),
            heads: heads.into_iter().collect(),
            hashes: hashes.into_iter().collect(),
        })
    }
}

impl Pack for Snapshot {
    fn pack(&self, out: &mut Vec<u8>) {
        (self.writer, &self.label).pack(out);
        (self.at, (self.genesis, self.head)).pack(out);
        (&self.certified, (&self.state, &self.unknown)).pack(out);
    }
}

/// Refuses what [`Snapshot::decode`] refuses.
impl Unpack for Snapshot {
    fn unpack(input: &mut In<'_>) -> Unpacked<Self> {
        let (writer, label) = Unpack::unpack(input)?;
        let (at, (genesis, head)) = Unpack::unpack(input)?;
        let (certified, (state, unknown)) = Unpack::unpack(input)?;
        let snapshot = Self {
            writer,
            label,
            at,
            genesis,
            head,
            certified,
            state,
            unknown,
        };
        snapshot.check()?;
        Ok(snapshot)
    }
}

fn ends_inside_a_value(bytes: &[u8]) -> bool {
    let end = bytes
        .iter()
        .rposition(|&byte| byte != 0)
        .map_or(0, |at| at + 1);
    serde_json::from_slice::<IgnoredAny>(&bytes[..end]).is_err_and(|error| error.is_eof())
}

fn member<T: DeserializeOwned>(name: &str, raw: &Raw) -> Result<T, NotSnapshot> {
    raw.decode().map_err(|error| NotSnapshot {
        reason: format!("`{name}`: {error}"),
        cut: false,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hash(n: u128) -> EntryHash {
        EntryHash::from_u128(n)
    }

    fn own(n: u64) -> Own {
        Own {
            n,
            hash: hash(u128::from(n) + 100),
        }
    }

    fn snapshot() -> Snapshot {
        Snapshot {
            writer: WriterId::from_u128(1),
            label: "drawbar".into(),
            at: Hlc {
                wall_ms: 5,
                counter: 1,
            },
            genesis: own(0).hash,
            head: own(9),
            certified: Certified {
                segments: [SegmentCert {
                    first: own(0),
                    last: own(4),
                }]
                .into(),
                heads: [own(4)].into(),
                hashes: [own(7)].into(),
            },
            state: Contribution::default(),
            unknown: BTreeMap::new(),
        }
    }

    #[test]
    fn a_snapshot_reads_back_with_members_it_does_not_know() {
        let mut written = snapshot();
        written
            .unknown
            .insert("later".into(), Raw::new(r#"{"x":[1,2]}"#).unwrap());
        let read = Snapshot::decode(&written.encode()).unwrap();
        assert_eq!(read, written);
        assert_eq!(read.encode(), written.encode());
    }

    #[test]
    fn a_snapshot_keeps_the_members_a_newer_build_wrote_in_its_state() {
        let text = String::from_utf8(snapshot().encode()).unwrap();
        let newer = text.replacen(r#""state":{}"#, r#""state":{"also":[ true ]}"#, 1);
        let read = Snapshot::decode(newer.as_bytes()).unwrap();
        assert_eq!(read.state.later.len(), 1);
        let again = String::from_utf8(read.encode()).unwrap();
        assert!(again.contains(r#""also":[ true ]"#), "{again}");
        let packed = crate::pack::packed(&read);
        assert_eq!(crate::pack::unpacked::<Snapshot>(&packed), Ok(read));
    }

    #[test]
    fn a_snapshot_packs_and_unpacks_whole() {
        let mut written = snapshot();
        written
            .unknown
            .insert("later".into(), Raw::new(r#"{"x":[1,2]}"#).unwrap());
        let packed = crate::pack::packed(&written);
        assert_eq!(crate::pack::unpacked::<Snapshot>(&packed), Ok(written));
    }

    #[test]
    fn a_snapshot_names_its_head_its_certificates_ends_and_hashes() {
        let snapshot = snapshot();
        for n in [0, 4, 7, 9] {
            assert!(snapshot.names(own(n)), "{n}");
        }
        for n in [1, 5, 8] {
            assert!(!snapshot.names(own(n)), "{n}");
        }
    }

    #[test]
    fn what_cannot_be_placed_is_not_a_snapshot() {
        let good = String::from_utf8(snapshot().encode()).unwrap();
        let cases = [
            good.replace("\"label\"", "\"name\""),
            good.replace("[5,1]", "5"),
            good.replace("\"n\":9", "\"n\":3"),
            good.replace(r#""segments":[[[0,"#, r#""segments":[[[5,"#),
            "[]".into(),
            good[..good.len() - 1].into(),
        ];
        for text in cases {
            assert!(Snapshot::decode(text.as_bytes()).is_err(), "{text}");
        }
    }
}
