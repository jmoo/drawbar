//! Value-level checks of the retention design in `RETENTION.md` (design R).
//!
//! Histories of three writers and a clone write one entity's existence, one
//! register and set members, compact at every point, and dismiss runs. Every
//! folder they leave, and every restore and dropped version of it, is read by
//! fresh and cached readers in every order, and compared with [`oracle::Oracle`],
//! which knows every entry and never compacts.
//!
//! A design is checked through [`Design`]: [`spec::Spec`] states design R's rules
//! over what a reader can see, and [`Real`] will be toshokan once it implements
//! them.

#![allow(dead_code, reason = "each test binary uses its own part")]

use std::collections::BTreeSet;

pub mod check;
pub mod history;
pub mod oracle;
pub mod spec;
pub mod world;

use world::{Dir, File, Hash, Seen, Shown, World};

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
}

/// toshokan, once it implements design R. Until then every check of it is
/// ignored.
#[derive(Clone, Copy, Debug, Default)]
pub struct Real;

impl Design for Real {
    type Snapshot = ();
    type Memory = ();

    fn compact(&self, _: &Compaction<'_, ()>) {
        unimplemented!("toshokan does not implement design R yet")
    }

    fn read(&self, _: &Reading<'_, ()>, _: &()) -> (Shown, ()) {
        unimplemented!("toshokan does not implement design R yet")
    }

    fn grace(&self, _: &Reading<'_, ()>, _: &()) -> (Shown, ()) {
        unimplemented!("toshokan does not implement design R yet")
    }

    fn holds(&self, _: &Reading<'_, ()>) -> BTreeSet<Hash> {
        unimplemented!("toshokan does not implement design R yet")
    }
}
