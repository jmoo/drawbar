//! The library's index, `.drawbar/library.ron`: what the files themselves cannot say.
//!
//! One file, rewritten whole, so a write is one commit point. It is written by id and
//! never in directory order, so one library is always written as the same text.

use std::collections::{BTreeMap, BTreeSet};

use nord_usb::{Location, ObjectClass};
use serde::{Deserialize, Serialize};

use super::{Fingerprint, LibPath};
use crate::workspace::Origin;

/// The index version this build writes.
///
/// ⚠️ A library whose index carries a higher version opens read-only and is never
/// written, so a newer drawbar's index survives an older one being run over it.
pub const VERSION: u32 = 1;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Sidecar {
    pub version: u32,
    /// The id the next new asset takes.
    pub next_id: u64,
    /// The generation the next working copy is written under.
    #[serde(default)]
    pub next_generation: u64,
    /// Tag names, by tag id.
    #[serde(default)]
    pub tags: BTreeMap<u64, String>,
    /// Everything known about each asset, by workspace id.
    #[serde(default)]
    pub assets: BTreeMap<u64, Row>,
}

impl Default for Sidecar {
    fn default() -> Sidecar {
        Sidecar {
            version: VERSION,
            next_id: 1,
            next_generation: 1,
            tags: BTreeMap::new(),
            assets: BTreeMap::new(),
        }
    }
}

/// One asset in the index.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Row {
    /// Where its file is. `None` for a view of a slot that holds an edit, kept only as a
    /// working copy until it becomes a file.
    #[serde(default)]
    pub path: Option<LibPath>,
    /// Its name, for a row with no path to take one from.
    #[serde(default)]
    pub name: String,
    /// What its file held when drawbar last read or wrote it.
    #[serde(default)]
    pub fingerprint: Option<Fingerprint>,
    #[serde(default)]
    pub tags: BTreeSet<u64>,
    #[serde(default)]
    pub origin: Stored,
    /// The generation of its working copy, `working/<id>-<generation>`, while it holds
    /// an edit not yet saved.
    #[serde(default)]
    pub working: Option<u64>,
}

impl Row {
    /// Whether this row holds something drawbar could not get back from the file alone:
    /// tags, an unsaved edit, or the slot it belongs to.
    pub fn precious(&self) -> bool {
        !self.tags.is_empty() || self.working.is_some() || self.origin.slot().is_some()
    }
}

/// An [`Origin`] as the index writes it.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum Stored {
    #[default]
    Fresh,
    File(String),
    Device {
        class: u32,
        bank: u32,
        slot: u32,
    },
    Rescued {
        bank: u32,
        slot: u32,
    },
}

impl Stored {
    fn slot(&self) -> Option<Location> {
        match self {
            Stored::Device { bank, slot, .. } | Stored::Rescued { bank, slot } => Some(Location {
                bank: *bank,
                slot: *slot,
            }),
            Stored::Fresh | Stored::File(_) => None,
        }
    }
}

impl From<&Origin> for Stored {
    fn from(origin: &Origin) -> Stored {
        match origin {
            Origin::Fresh => Stored::Fresh,
            Origin::File(name) => Stored::File(name.clone()),
            Origin::Device { class, at } => Stored::Device {
                class: class.to_raw(),
                bank: at.bank,
                slot: at.slot,
            },
            Origin::Rescued { at } => Stored::Rescued {
                bank: at.bank,
                slot: at.slot,
            },
        }
    }
}

impl From<&Stored> for Origin {
    fn from(stored: &Stored) -> Origin {
        match stored {
            Stored::Fresh => Origin::Fresh,
            Stored::File(name) => Origin::File(name.clone()),
            Stored::Device { class, bank, slot } => Origin::Device {
                class: ObjectClass::from_raw(*class),
                at: Location {
                    bank: *bank,
                    slot: *slot,
                },
            },
            Stored::Rescued { bank, slot } => Origin::Rescued {
                at: Location {
                    bank: *bank,
                    slot: *slot,
                },
            },
        }
    }
}

/// What an index's text turned out to be.
#[derive(Debug, PartialEq)]
pub enum Read {
    Known(Sidecar),
    /// Written by a newer drawbar, under this version.
    Newer(u32),
    /// Not an index this build can read, and why.
    Unreadable(String),
}

/// Only the version, read first so an index from a newer build is recognized as that
/// even where its other fields would not parse.
#[derive(Deserialize)]
struct Probe {
    version: u32,
}

pub fn read(text: &str) -> Read {
    let version = match ron::from_str::<Probe>(text) {
        Ok(probe) => probe.version,
        Err(e) => return Read::Unreadable(e.to_string()),
    };
    if version > VERSION {
        return Read::Newer(version);
    }
    match ron::from_str::<Sidecar>(text) {
        Ok(sidecar) if sidecar.version == VERSION => Read::Known(sidecar),
        Ok(sidecar) => Read::Unreadable(format!(
            "version {} is not one this build reads",
            sidecar.version
        )),
        Err(e) => Read::Unreadable(e.to_string()),
    }
}

/// A fingerprint's CRC: `Some(…)` or `None`, or a bare number as an index written before
/// a fingerprint could lack one.
pub(super) fn crc<'de, D: serde::Deserializer<'de>>(at: D) -> Result<Option<u32>, D::Error> {
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum Crc {
        Bare(u32),
        Optional(Option<u32>),
    }
    Ok(match Crc::deserialize(at)? {
        Crc::Bare(crc) => Some(crc),
        Crc::Optional(crc) => crc,
    })
}

pub fn write(sidecar: &Sidecar) -> Result<String, String> {
    ron::ser::to_string_pretty(sidecar, ron::ser::PrettyConfig::default())
        .map_err(|e| e.to_string())
}
