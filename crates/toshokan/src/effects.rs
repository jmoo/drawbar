//! Changing the user's files, with preconditions, so that nothing is overwritten.
//!
//! Every effect checks its precondition first and refuses with
//! [`crate::Error::Changed`] rather than overwrite. Displaced bytes enter the blob
//! store. Effects of more than one step are journaled, so recovery at the next open
//! can finish or roll back each one.

use crate::error::Result;
use crate::fs::{Fingerprint, Fs, RelPath};
use crate::ids::{EntityId, IntentId};
use crate::layout::Layout;
use crate::log::LogWriter;
use crate::merge::State;
use crate::value::BlobId;

/// What the writer expects to find at a path before changing it.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Precondition {
    Absent,
    /// The file still matches what the writer last read.
    Matches(Fingerprint),
}

/// Where new contents come from.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Source {
    Bytes(Vec<u8>),
    Blob(BlobId),
}

/// A change to the user's files.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Effect {
    /// Write the entity's file at `path`, displacing any old contents into the blob
    /// store.
    Save {
        entity: EntityId,
        path: RelPath,
        contents: Source,
        expect: Precondition,
    },
    /// Move the entity's file into the blob store.
    Delete {
        entity: EntityId,
        path: RelPath,
        expect: Precondition,
    },
    /// Move the entity's file to `to`, where nothing may be.
    Rename {
        entity: EntityId,
        from: RelPath,
        to: RelPath,
    },
    /// Move a directory and everything in it to `to`, where nothing may be.
    MoveTree { from: RelPath, to: RelPath },
}

/// What an effect did to the files.
#[derive(Clone, PartialEq, Eq, Debug, Default)]
pub struct Report {
    /// Files moved into the blob store, by the path they had.
    pub displaced: Vec<(RelPath, BlobId)>,
    /// Files moved, from and to.
    pub moved: Vec<(RelPath, RelPath)>,
}

/// Apply `effect` and append its entries to the log under `intent`.
pub async fn apply<F: Fs>(
    fs: &F,
    layout: &Layout,
    log: &mut LogWriter,
    state: &State,
    intent: IntentId,
    effect: &Effect,
) -> Result<Report> {
    let _ = (fs, layout, log, state, intent, effect);
    todo!()
}
