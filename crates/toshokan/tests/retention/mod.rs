//! Value-level checks of toshokan under the retention design in `RETENTION.md`.
//!
//! Histories of three writers and a clone write one entity's existence, one
//! register and set members, compact at every point, and dismiss runs. Every
//! folder they leave, and every restore and dropped version of it, is read by
//! toshokan ([`real::Real`]) along every delivery order, and compared with
//! [`oracle::Oracle`], which knows every entry and never compacts. A reader holds
//! nothing between reads: it derives what it shows from the files it sees.

#![allow(dead_code, reason = "each test binary uses its own part")]

use std::collections::BTreeSet;

pub mod check;
pub mod history;
pub mod oracle;
pub mod real;
pub mod world;

use world::{Dir, File, Hash, Seen, Shown, World};

/// What a writer has when it compacts.
pub struct Compaction<'a, S> {
    pub world: &'a World,
    pub dir: Dir,
    /// Its own chain, from its first entry to its head.
    pub chain: &'a [Hash],
    /// Its previous snapshot.
    pub previous: Option<&'a S>,
    /// The folder now.
    pub files: &'a [File<S>],
    /// The segments of its directory it has read since its previous snapshot,
    /// present or not.
    pub recorded: &'a BTreeSet<usize>,
}

/// What a reader sees. toshokan reads only the lines of the segments in `seen`
/// and the snapshots in `seen`; [`oracle::Oracle`] alone looks further into
/// `world`.
pub struct Reading<'a, S> {
    pub world: &'a World,
    pub files: &'a [File<S>],
    pub seen: &'a [Seen],
}

/// What P9 bounds a snapshot by: the fields written, the writers, the segment
/// files its directory ever held, and the compactions that followed a dropped
/// version.
#[derive(Clone, Copy, Debug)]
pub struct Bound {
    pub fields: usize,
    pub writers: usize,
    pub segments: usize,
    pub drops: usize,
}

/// How toshokan, or the oracle, makes and reads a history's folder.
pub trait Design {
    /// What a compaction writes.
    type Snapshot: Clone + std::fmt::Debug;

    fn compact(&self, c: &Compaction<'_, Self::Snapshot>) -> Self::Snapshot;

    /// What a reader shows of what it sees.
    fn read(&self, r: &Reading<'_, Self::Snapshot>) -> Shown;

    /// Whether a snapshot's certificates name `entry`.
    fn names(&self, world: &World, s: &Self::Snapshot, entry: Hash) -> bool;
}
