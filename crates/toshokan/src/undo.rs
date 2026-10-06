//! Undo and redo: compensating intents, per writer.
//!
//! Undo plans the intent that reverses this writer's latest intent not yet undone:
//! facts are set back by new writes, files come back from this writer's trash
//! under a precondition. It is refused, with a reason, where another writer has
//! changed the same thing since.

#![expect(unused_variables, reason = "the skeleton's bodies are todo!()")]

use crate::error::Refusal;
use crate::plan::Plan;
use crate::reader::WriterLog;
use crate::report::HistoryItem;
use crate::view::View;

/// This writer's intents, oldest first, with what is undone.
#[derive(Clone, PartialEq, Eq, Debug, Default)]
pub struct History {
    items: Vec<HistoryItem>,
}

impl History {
    /// From this writer's own log, including entries a snapshot folded.
    pub fn of(own: &WriterLog) -> Self {
        todo!()
    }

    pub fn items(&self) -> &[HistoryItem] {
        todo!()
    }

    /// The plan reversing the latest intent not undone, with
    /// [`Plan::reverses`] naming it.
    pub fn plan_undo(&self, own: &WriterLog, view: &View) -> Result<Plan, Refusal> {
        todo!()
    }

    /// The plan reapplying the latest undone intent, while nothing was committed
    /// since its undo.
    pub fn plan_redo(&self, own: &WriterLog, view: &View) -> Result<Plan, Refusal> {
        todo!()
    }
}
