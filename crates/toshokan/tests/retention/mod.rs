//! Value-level checks of the retention design in `RETENTION.md`.
//!
//! Histories of three writers and a clone write one entity's existence, one
//! register and set members, compact at every point, and dismiss runs. Every
//! folder they leave, and every restore and dropped version of it, is read by
//! fresh and cached readers in every order, and compared with [`oracle::Oracle`],
//! which knows every entry and never compacts.
//!
//! A design is checked through [`Design`]: [`spec::Spec`] states `RETENTION.md`'s rules
//! over what a reader can see, and [`real::Real`] is toshokan.

#![allow(dead_code, reason = "each test binary uses its own part")]

use std::collections::BTreeSet;

pub mod check;
pub mod history;
pub mod oracle;
pub mod real;
pub mod spec;
pub mod world;

use world::{Dir, Dismissal, File, Hash, Seen, Shown, World};

/// What a writer has when it compacts.
pub struct Compaction<'a, S> {
    pub world: &'a World,
    pub dir: Dir,
    /// Its own chain, from its first entry to its head.
    pub chain: &'a [Hash],
    /// Its previous snapshot, as its cached view holds it.
    pub previous: Option<&'a S>,
    /// The folder now.
    pub files: &'a [File<S>],
    /// The segments of its directory its cached view records since its previous
    /// snapshot, present or not.
    pub recorded: &'a BTreeSet<usize>,
}

/// What a reader sees. A design reads only the lines of the segments in `seen`
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

/// A design under test.
pub trait Design {
    /// What a compaction writes.
    type Snapshot: Clone + std::fmt::Debug;
    /// What a reader remembers between reads.
    type Memory: Clone + Default + std::fmt::Debug;

    fn compact(&self, c: &Compaction<'_, Self::Snapshot>) -> Self::Snapshot;

    /// What a reader with `memory` shows of what it sees, and what it remembers.
    fn read(&self, r: &Reading<'_, Self::Snapshot>, memory: &Self::Memory)
        -> (Shown, Self::Memory);

    /// The grace step: drops what no file holds and nothing newer of its writer
    /// contradicts, then reads.
    fn grace(
        &self,
        r: &Reading<'_, Self::Snapshot>,
        memory: &Self::Memory,
    ) -> (Shown, Self::Memory);

    /// The entries the files `r` sees hold, by the oracle's knowledge; used only
    /// to state properties.
    fn holds(&self, r: &Reading<'_, Self::Snapshot>) -> BTreeSet<Hash>;

    /// The last entry a snapshot folds.
    fn head(&self, world: &World, s: &Self::Snapshot) -> Hash;

    /// The own writes of the tips among the snapshots `seen` shows: those no
    /// other one there certifies.
    fn tip_writes(
        &self,
        world: &World,
        files: &[File<Self::Snapshot>],
        seen: &[Seen],
    ) -> BTreeSet<Hash>;

    /// The runs a snapshot's dismissals name.
    fn dismissals(&self, world: &World, s: &Self::Snapshot) -> Vec<Dismissal>;

    /// The entries a reader remembers apart from its tips.
    fn remembered(&self, world: &World, memory: &Self::Memory) -> BTreeSet<Hash>;

    /// Whether a snapshot is within P9's bound.
    fn bounded(&self, s: &Self::Snapshot, bound: Bound) -> bool;
}
