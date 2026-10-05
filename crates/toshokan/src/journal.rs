//! The journal of effects in progress, and recovery from it at open.
//!
//! A writer records each multi-step effect under `journal/<writer>/` before its first
//! step and clears the record after its last, so a crash leaves a record of exactly
//! the effects it interrupted.

use crate::error::Result;
use crate::fs::{Fs, RelPath};
use crate::ids::IntentId;
use crate::layout::Layout;
use crate::log::LogWriter;
use crate::merge::State;

/// How recovery settled an interrupted effect.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Outcome {
    Finished,
    RolledBack,
}

/// An effect a crash interrupted, and how recovery settled it.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Recovered {
    pub intent: IntentId,
    /// The library paths the effect touched.
    pub paths: Vec<RelPath>,
    pub outcome: Outcome,
}

/// Finish or roll back each of this writer's journaled effects, then clear the
/// journal. Recovery is itself safe to interrupt and repeat.
pub async fn recover<F: Fs>(
    fs: &F,
    layout: &Layout,
    log: &mut LogWriter,
    state: &State,
) -> Result<Vec<Recovered>> {
    let _ = (fs, layout, log, state);
    todo!()
}
