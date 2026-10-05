//! Finding the library's files and binding them to entities.
//!
//! A scan lists every file outside toshokan's root, fingerprints it, and binds it to
//! an entity: by the entity's `path` field first, then by fingerprint, which follows a
//! file renamed outside the app. Hashes are computed only where length and time
//! cannot decide. A scan writes nothing, and what it finds is never logged.

use std::collections::BTreeMap;

use crate::error::Result;
use crate::fs::{Fingerprint, Fs, RelPath};
use crate::ids::EntityId;
use crate::layout::Layout;
use crate::merge::State;

/// An entity whose file was found at another path.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Move {
    pub entity: EntityId,
    pub from: RelPath,
    pub to: RelPath,
}

/// What a scan found, each list in order.
#[derive(Clone, PartialEq, Eq, Debug, Default)]
pub struct Scan {
    /// Every library file with its fingerprint.
    pub files: BTreeMap<RelPath, Fingerprint>,
    /// The entity each bound file belongs to.
    pub bound: BTreeMap<RelPath, EntityId>,
    /// Files no entity claims.
    pub arrivals: Vec<RelPath>,
    /// Existing entities whose file was found nowhere.
    pub departures: Vec<EntityId>,
    /// Entities whose file was found at another path.
    pub moves: Vec<Move>,
    /// Entities whose file is at its path with contents other than toshokan last
    /// wrote or bound.
    pub changed: Vec<EntityId>,
}

pub async fn scan<F: Fs>(fs: &F, layout: &Layout, state: &State) -> Result<Scan> {
    let _ = (fs, layout, state);
    todo!()
}
