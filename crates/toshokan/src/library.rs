//! The library as one instance holds it: the pure core both drivers run.
//!
//! Only [`Library::commit`] (and undo, redo and settling, which commit),
//! [`Library::empty_trash`] and [`Library::compact`] write the folder. Opening,
//! viewing, refreshing and scanning never do; they write only in the local root.

#![expect(
    dead_code,
    unused_variables,
    reason = "the skeleton's bodies are todo!()"
)]

use crate::env::Env;
use crate::error::{Invalid, Result};
use crate::ids::{EntityId, Identity};
use crate::io::{Capabilities, Task};
use crate::layout::Layout;
use crate::log::Settlement;
use crate::plan::Plan;
use crate::reader::Reader;
use crate::report::{
    Change, Committed, Compacted, Emptied, HistoryItem, Mode, Opened, Orphan, TrashItem, WriterInfo,
};
use crate::schema::Schema;
use crate::trash::Policy;
use crate::undo::History;
use crate::view::View;
use crate::writer::Writer;

pub struct Library {
    layout: Layout,
    schema: Schema,
    env: Env,
    capabilities: Capabilities,
    reader: Reader,
    writer: Option<Writer>,
    view: View,
    history: History,
    mode: Mode,
}

impl Library {
    /// Reads the cached view and the folder, claims a writer from the pool,
    /// assesses recovery, checks drafts and scans the library's files.
    /// `capabilities` are the folder's.
    pub fn open(
        layout: Layout,
        schema: Schema,
        env: Env,
        capabilities: Capabilities,
    ) -> Task<'static, Result<(Library, Opened)>> {
        todo!()
    }

    pub fn view(&self) -> View {
        todo!()
    }

    pub fn history(&self) -> &[HistoryItem] {
        todo!()
    }

    /// Checks every precondition against the current view and the files, then
    /// logs the intent and carries out its file effects. Before its first write a
    /// new writer is created; before any write this writer's interrupted effects
    /// are settled. A refusal is [`crate::Error::Refused`] and changes nothing.
    pub fn commit(
        &mut self,
        plan: std::result::Result<Plan, Invalid>,
    ) -> Task<'_, Result<Committed>> {
        todo!()
    }

    pub fn undo(&mut self) -> Task<'_, Result<Committed>> {
        todo!()
    }

    pub fn redo(&mut self) -> Task<'_, Result<Committed>> {
        todo!()
    }

    /// Settles another writer's unfinished effect, with the user's consent.
    pub fn settle(&mut self, orphan: Orphan, how: Settlement) -> Task<'_, Result<()>> {
        todo!()
    }

    /// Reads other writers' new entries and rescans: what changed since the last
    /// view, attributed to its writer, or to nobody for outside changes.
    pub fn refresh(&mut self) -> Task<'_, Result<Vec<Change>>> {
        todo!()
    }

    /// Every writer with its last entry time, and whether another instance on this
    /// machine holds it.
    pub fn others(&mut self) -> Task<'_, Result<Vec<WriterInfo>>> {
        todo!()
    }

    pub fn trash(&mut self) -> Task<'_, Result<Vec<TrashItem>>> {
        todo!()
    }

    pub fn empty_trash(&mut self, policy: Policy) -> Task<'_, Result<Emptied>> {
        todo!()
    }

    pub fn compact(&mut self) -> Task<'_, Result<Compacted>> {
        todo!()
    }

    /// Keeps an unsaved edit of `entity` over a file holding `base`.
    pub fn put_draft(
        &mut self,
        entity: EntityId,
        base: Identity,
        bytes: Vec<u8>,
    ) -> Task<'_, Result<()>> {
        todo!()
    }

    pub fn discard_draft(&mut self, entity: EntityId) -> Task<'_, Result<()>> {
        todo!()
    }

    /// Seals the open segment, writes the cached view and releases the writer's
    /// lock. A library dropped without closing leaves its segment open, as a crash
    /// does.
    pub fn close(&mut self) -> Task<'_, Result<()>> {
        todo!()
    }
}
