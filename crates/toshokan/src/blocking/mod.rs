//! The blocking driver: runs the core's operations on a [`Backend`] in a loop.

#[cfg(not(target_arch = "wasm32"))]
mod native;

#[cfg(not(target_arch = "wasm32"))]
pub use native::Native;

use crate::disk::MemDisk;
use crate::drafts::Draft;
use crate::env::Env;
use crate::error::Result;
use crate::ids::{EntityId, Identity};
use crate::intent::Intent;
use crate::io::{Capabilities, Io, IoResult, Operation, Root, Step};
use crate::layout::Layout;
use crate::library;
use crate::log::Settlement;
use crate::report::{
    Change, Committed, Compacted, Emptied, HistoryItem, Opened, Orphan, TrashItem, WriterInfo,
};
use crate::schema::Schema;
use crate::trash::Policy;
use crate::view::View;

/// Storage that performs requests as they come.
pub trait Backend {
    fn capabilities(&self, root: Root) -> Capabilities;

    fn perform(&mut self, io: Io) -> IoResult;
}

impl Backend for MemDisk {
    fn capabilities(&self, root: Root) -> Capabilities {
        MemDisk::capabilities(self, root)
    }

    fn perform(&mut self, io: Io) -> IoResult {
        MemDisk::perform(self, io)
    }
}

/// Runs `operation` to completion.
pub fn run<O: Operation>(backend: &mut impl Backend, mut operation: O) -> O::Output {
    let mut result = None;
    loop {
        match operation.resume(result.take()) {
            Step::Done(output) => return output,
            Step::Io(io) => result = Some(backend.perform(io)),
        }
    }
}

/// A library open as one writer, on a blocking backend.
pub struct Library<B> {
    backend: B,
    core: library::Library,
}

impl<B: Backend> Library<B> {
    pub fn open(
        mut backend: B,
        layout: Layout,
        schema: &Schema,
        env: Env,
    ) -> Result<(Self, Opened)> {
        let capabilities = backend.capabilities(Root::Folder);
        let open = library::Library::open(layout, schema.clone(), env, capabilities);
        let (core, opened) = run(&mut backend, open)?;
        Ok((Self { backend, core }, opened))
    }

    pub fn backend(&self) -> &B {
        &self.backend
    }

    pub fn view(&self) -> View {
        self.core.view()
    }

    pub fn history(&self) -> &[HistoryItem] {
        self.core.history()
    }

    pub fn intent(&mut self, label: &str) -> Intent<&mut Self> {
        Intent::new(self, label)
    }

    pub fn draft(&mut self, entity: EntityId) -> Draft<&mut Self> {
        Draft::new(self, entity)
    }

    pub fn undo(&mut self) -> Result<Committed> {
        run(&mut self.backend, self.core.undo())
    }

    pub fn redo(&mut self) -> Result<Committed> {
        run(&mut self.backend, self.core.redo())
    }

    pub fn settle(&mut self, orphan: Orphan, how: Settlement) -> Result<()> {
        run(&mut self.backend, self.core.settle(orphan, how))
    }

    pub fn refresh(&mut self) -> Result<Vec<Change>> {
        run(&mut self.backend, self.core.refresh())
    }

    pub fn others(&mut self) -> Result<Vec<WriterInfo>> {
        run(&mut self.backend, self.core.others())
    }

    pub fn trash(&mut self) -> Result<Vec<TrashItem>> {
        run(&mut self.backend, self.core.trash())
    }

    pub fn empty_trash(&mut self, policy: Policy) -> Result<Emptied> {
        run(&mut self.backend, self.core.empty_trash(policy))
    }

    pub fn compact(&mut self) -> Result<Compacted> {
        run(&mut self.backend, self.core.compact())
    }

    /// Closes the library cleanly and returns the backend.
    pub fn close(mut self) -> Result<B> {
        run(&mut self.backend, self.core.close())?;
        Ok(self.backend)
    }
}

impl<B: Backend> Intent<&mut Library<B>> {
    pub fn commit(self) -> Result<Committed> {
        let (library, plan) = self.into_parts();
        run(&mut library.backend, library.core.commit(plan))
    }
}

impl<B: Backend> Draft<&mut Library<B>> {
    pub fn put(self, base: Identity, bytes: Vec<u8>) -> Result<()> {
        let (library, entity) = self.into_parts();
        run(
            &mut library.backend,
            library.core.put_draft(entity, base, bytes),
        )
    }

    pub fn discard(self) -> Result<()> {
        let (library, entity) = self.into_parts();
        run(&mut library.backend, library.core.discard_draft(entity))
    }
}
