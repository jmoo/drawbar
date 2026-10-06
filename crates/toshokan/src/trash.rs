//! A writer's trash: bytes its intents displaced, kept for undo until it empties
//! them. Emptying a trash is the only way bytes leave the folder, and only its
//! owner empties it.

#![expect(unused_variables, reason = "the skeleton's bodies are todo!()")]

use crate::error::Result;
use crate::ids::WriterId;
use crate::io::Task;
use crate::layout::Layout;
use crate::reader::WriterLog;
use crate::report::{Emptied, TrashItem};

/// What emptying keeps.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Policy {
    /// Items displaced longer ago than this are removed.
    pub max_age_ms: u64,
    /// Then the oldest items are removed until the rest fit.
    pub max_bytes: u64,
}

impl Default for Policy {
    /// Thirty days, and a gibibyte.
    fn default() -> Self {
        Self {
            max_age_ms: 30 * 24 * 60 * 60 * 1000,
            max_bytes: 1 << 30,
        }
    }
}

/// The items in `writer`'s trash, oldest first, each with the entry in `own` that
/// displaced it. Reads only.
pub fn list<'a>(
    layout: &'a Layout,
    writer: WriterId,
    own: &'a WriterLog,
) -> Task<'a, Result<Vec<TrashItem>>> {
    todo!()
}

/// Removes the items `policy` does not keep at wall time `now_ms`.
pub fn empty(
    layout: &Layout,
    writer: WriterId,
    items: Vec<TrashItem>,
    policy: Policy,
    now_ms: u64,
) -> Task<'static, Result<Emptied>> {
    todo!()
}
