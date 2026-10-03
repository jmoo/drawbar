//! The library's index, `.drawbar/library.ron`: what the files themselves cannot say.
//!
//! One file, rewritten whole, so a write is one commit point. It is written by id and
//! never in directory order, so one library is always written as the same text.
//!
//! It holds a row only for an asset with something no file says, such as a tag, an
//! unsaved edit, or the slot it came off (see [`Row::indexed`]). Every other file is
//! known by its listing alone, so the index grows with what the user does, not with the
//! library.

use std::collections::{BTreeMap, BTreeSet};

use nord_usb::{Location, ObjectClass};
use serde::{Deserialize, Serialize};

use super::exec::opens;
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

/// One asset in the index. A field that holds nothing is not written.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Row {
    /// Where its file is. `None` for a view of a slot that holds an edit, kept only as a
    /// working copy until it becomes a file.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<LibPath>,
    /// Its name, for a row with no path to take one from. Empty for one with a path.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub name: String,
    /// What its file held when drawbar last read or wrote it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fingerprint: Option<Fingerprint>,
    #[serde(default, skip_serializing_if = "BTreeSet::is_empty")]
    pub tags: BTreeSet<u64>,
    /// Where it came from, where its path does not say: see [`Row::origin`].
    #[serde(default, skip_serializing_if = "Stored::is_implied")]
    pub origin: Stored,
    /// Its working copy, while it holds an edit not yet saved.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub working: Option<Working>,
}

/// A working copy, `working/<id>-<generation>`, and what it keeps.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Working {
    pub generation: u64,
    pub keeps: Keeps,
}

/// What a working copy keeps of an unsaved edit.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Keeps {
    /// The asset's bytes, whole.
    Bytes,
    /// An edit of the file the asset rests in, as [`crate::rewrite::Edit::working`]
    /// writes it.
    Edit,
}

impl Row {
    /// A row for an asset, keeping only what its path does not say.
    pub fn of(
        path: Option<LibPath>,
        name: &str,
        fingerprint: Option<Fingerprint>,
        tags: BTreeSet<u64>,
        origin: &Origin,
        working: Option<Working>,
    ) -> Row {
        let origin = match (origin, &path) {
            (Origin::File(_), Some(_)) | (Origin::Fresh, None) => Stored::Implied,
            (origin, _) => origin.into(),
        };
        Row {
            name: match path {
                Some(_) => String::new(),
                None => name.to_string(),
            },
            path,
            fingerprint,
            tags,
            origin,
            working,
        }
    }

    /// Whether this row holds something drawbar could not get back from the file alone:
    /// tags, an unsaved edit, or the slot it belongs to.
    pub fn precious(&self) -> bool {
        !self.tags.is_empty() || self.working.is_some() || self.origin.slot().is_some()
    }

    /// Whether the index keeps this row: it is [`Row::precious`], or its file is of a kind
    /// a listing passes over, which drawbar holds only because it made the file or was
    /// given it. The index holds no other row.
    pub fn indexed(&self) -> bool {
        let unopened = self.path.as_ref().is_some_and(|path| !opens(path.leaf()));
        self.precious() || unopened
    }

    /// Where the asset came from. [`Stored::Implied`] reads as the row's file where it
    /// has a path, and as made in drawbar where it has none.
    pub fn origin(&self) -> Origin {
        match (&self.origin, &self.path) {
            (Stored::Implied, Some(path)) => Origin::File(path.leaf().to_string()),
            (Stored::Implied, None) => Origin::Fresh,
            (stored, _) => stored.into(),
        }
    }
}

/// An [`Origin`] as the index writes it.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum Stored {
    /// Not written: read from its file, for a row with a path, and made in drawbar for
    /// one without.
    #[default]
    Implied,
    /// Made in drawbar, for a row with a path.
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
    fn is_implied(&self) -> bool {
        *self == Stored::Implied
    }

    fn slot(&self) -> Option<Location> {
        match self {
            Stored::Device { bank, slot, .. } | Stored::Rescued { bank, slot } => Some(Location {
                bank: *bank,
                slot: *slot,
            }),
            Stored::Implied | Stored::Fresh | Stored::File(_) => None,
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
            Stored::Implied | Stored::Fresh => Origin::Fresh,
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

pub fn write(sidecar: &Sidecar) -> Result<String, String> {
    ron::ser::to_string_pretty(sidecar, ron::ser::PrettyConfig::default())
        .map_err(|e| e.to_string())
}
