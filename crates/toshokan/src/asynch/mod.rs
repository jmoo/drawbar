//! The async driver: runs the core's operations on an [`Fs`] in a loop. Futures
//! carry no `Send` bound, so a single-threaded browser backend can implement it.

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
    Committed, Compacted, Emptied, HistoryItem, Opened, Orphan, Refreshed, TrashItem, WriterInfo,
};
use crate::schema::Schema;
use crate::trash::Policy;
use crate::view::View;

/// Storage that performs requests asynchronously.
#[allow(async_fn_in_trait)]
pub trait Fs {
    fn capabilities(&self, root: Root) -> Capabilities;

    async fn perform(&self, io: Io) -> IoResult;
}

impl Fs for MemDisk {
    fn capabilities(&self, root: Root) -> Capabilities {
        MemDisk::capabilities(self, root)
    }

    async fn perform(&self, io: Io) -> IoResult {
        MemDisk::perform(self, io)
    }
}

/// Runs `operation` to completion.
pub async fn run<O: Operation>(fs: &impl Fs, mut operation: O) -> O::Output {
    let mut result = None;
    loop {
        match operation.resume(result.take()) {
            Step::Done(output) => return output,
            Step::Io(io) => result = Some(fs.perform(io).await),
        }
    }
}

/// A library open as one writer, on an async backend.
pub struct Library<F> {
    fs: F,
    core: library::Library,
}

impl<F: Fs> Library<F> {
    pub async fn open(fs: F, layout: Layout, schema: &Schema, env: Env) -> Result<(Self, Opened)> {
        let capabilities = fs.capabilities(Root::Folder);
        let open = library::Library::open(layout, schema.clone(), env, capabilities);
        let (core, opened) = run(&fs, open).await?;
        Ok((Self { fs, core }, opened))
    }

    pub fn fs(&self) -> &F {
        &self.fs
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

    pub async fn undo(&mut self) -> Result<Committed> {
        run(&self.fs, self.core.undo()).await
    }

    pub async fn redo(&mut self) -> Result<Committed> {
        run(&self.fs, self.core.redo()).await
    }

    pub async fn settle(&mut self, orphan: Orphan, how: Settlement) -> Result<()> {
        run(&self.fs, self.core.settle(orphan, how)).await
    }

    pub async fn refresh(&mut self) -> Result<Refreshed> {
        run(&self.fs, self.core.refresh()).await
    }

    /// Stops showing what [`Opened::removed`] reports, on every open of this
    /// install, while the folder lacks it. Writes only in the local root.
    pub async fn let_go(&mut self) -> Result<()> {
        run(&self.fs, self.core.let_go()).await
    }

    /// Republishes what [`Opened::removed`] reports as an intent labeled `label`,
    /// then lets it go: the folder holds again what this install showed.
    pub async fn adopt(&mut self, label: &str) -> Result<Committed> {
        run(&self.fs, self.core.adopt(label)).await
    }

    pub async fn others(&mut self) -> Result<Vec<WriterInfo>> {
        run(&self.fs, self.core.others()).await
    }

    pub async fn trash(&mut self) -> Result<Vec<TrashItem>> {
        run(&self.fs, self.core.trash()).await
    }

    pub async fn empty_trash(&mut self, policy: Policy) -> Result<Emptied> {
        run(&self.fs, self.core.empty_trash(policy)).await
    }

    pub async fn compact(&mut self) -> Result<Compacted> {
        run(&self.fs, self.core.compact()).await
    }

    /// Closes the library cleanly and returns the backend.
    pub async fn close(mut self) -> Result<F> {
        run(&self.fs, self.core.close()).await?;
        Ok(self.fs)
    }
}

impl<F: Fs> Intent<&mut Library<F>> {
    pub async fn commit(self) -> Result<Committed> {
        let (library, plan) = self.into_parts();
        run(&library.fs, library.core.commit(plan)).await
    }
}

impl<F: Fs> Draft<&mut Library<F>> {
    pub async fn put(self, base: Identity, bytes: Vec<u8>) -> Result<()> {
        let (library, entity) = self.into_parts();
        run(&library.fs, library.core.put_draft(entity, base, bytes)).await
    }

    pub async fn discard(self) -> Result<()> {
        let (library, entity) = self.into_parts();
        run(&library.fs, library.core.discard_draft(entity)).await
    }
}
