//! File effects without atomic replace.
//!
//! Nothing in a user path is overwritten or deleted in place. A save stages its
//! bytes in `tmp/` and syncs them; every precondition is checked; a pending record
//! is written; displaced bytes are renamed into this writer's trash and the staged
//! file into place; the intent's entry is appended; the pending record is removed.
//! Each user path holds its old bytes, nothing or its new bytes, and while it holds
//! nothing a pending record names it. A source is never removed before its
//! destination is durable.
//!
//! A commit runs [`prepare`], [`apply`], the writer's append, then [`finish`].

#![expect(unused_variables, reason = "the skeleton's bodies are todo!()")]

use crate::env::Env;
use crate::error::{Refusal, Result};
use crate::ids::{EntityId, EntryHash, Nonce, WriterId};
use crate::io::{Capabilities, Task};
use crate::layout::Layout;
use crate::log::{Displaced, FileFact};
use crate::path::RelPath;
use crate::plan::{Expect, FileChange};
use crate::report::Outcome;
use crate::view::View;

/// One step of an intent's file effects, as its pending record lists it. Paths
/// are in the folder root.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum EffectStep {
    /// Requires `expect` at `path`.
    Check {
        path: RelPath,
        expect: Expect,
    },
    /// Moves the file at `path` into this writer's trash as `item`.
    ToTrash {
        path: RelPath,
        item: Nonce,
    },
    /// Moves the staged file `staged` to `path`.
    Place {
        staged: Nonce,
        path: RelPath,
    },
    Rename {
        from: RelPath,
        to: RelPath,
    },
    /// Moves trash item `item` back to `path`.
    FromTrash {
        item: Nonce,
        path: RelPath,
    },
    MakeDir {
        path: RelPath,
    },
    /// Removes a directory a move emptied.
    RemoveDir {
        path: RelPath,
    },
}

/// The file effects of one intent, resolved against a view.
#[derive(Clone, PartialEq, Eq, Debug, Default)]
pub struct EffectPlan {
    pub steps: Vec<EffectStep>,
    /// Bytes to stage before anything else, by staged name.
    pub staged: Vec<(Nonce, Vec<u8>)>,
    /// What each touched entity's file register will say once the steps are done.
    pub files: Vec<(EntityId, Option<FileFact>)>,
}

/// What [`apply`] did.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Applied {
    pub displaced: Vec<Displaced>,
    pub files: Vec<(EntityId, Option<FileFact>)>,
    pub outcome: Outcome,
}

/// Resolves `changes` into steps. `created` gives the ids of the plan's new
/// entities. Directory renames are used only where `capabilities` declares them.
pub fn resolve(
    changes: &[FileChange],
    created: &[EntityId],
    view: &View,
    layout: &Layout,
    capabilities: Capabilities,
    env: &mut Env,
) -> std::result::Result<EffectPlan, Refusal> {
    todo!()
}

/// Stages and syncs the plan's bytes, checks every precondition, then writes the
/// pending record `record` chained after `head`. A failed precondition removes
/// what was staged and refuses; nothing in a user path has changed.
pub fn prepare<'a>(
    layout: &'a Layout,
    writer: WriterId,
    head: EntryHash,
    record: Nonce,
    plan: &'a EffectPlan,
) -> Task<'a, Result<std::result::Result<(), Refusal>>> {
    todo!()
}

/// Carries out the steps in order, syncing each destination before its source is
/// removed. A step that fails stops the run with [`Outcome::Partial`], leaving the
/// pending record for recovery.
pub fn apply<'a>(
    layout: &'a Layout,
    writer: WriterId,
    plan: &'a EffectPlan,
) -> Task<'a, Result<Applied>> {
    todo!()
}

/// Removes the pending record once the intent's entry is durable.
pub fn finish(layout: &Layout, writer: WriterId, record: Nonce) -> Task<'static, Result<()>> {
    todo!()
}
