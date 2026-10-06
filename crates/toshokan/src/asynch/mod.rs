//! The async driver: runs the core's operations on an [`Fs`] in a loop. Futures
//! carry no `Send` bound, so a single-threaded browser backend can implement it.

use std::future::Future;
use std::pin::Pin;

use crate::disk::MemDisk;
use crate::drafts::Draft;
use crate::env::Env;
use crate::error::Result;
use crate::ids::{EntityId, Identity};
use crate::intent::{Driver, Intent};
use crate::io::{Capabilities, Io, IoError, IoResult, Operation, Range, Reply, Root, Step};
use crate::layout::Layout;
use crate::library;
use crate::log::Settlement;
use crate::path::RelPath;
use crate::plan::{Splice, Splicing, SHRANK};
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

/// A future a [`Source`] returns.
pub type Filling<'s> = Pin<Box<dyn Future<Output = std::result::Result<(), IoError>> + 's>>;

/// What a saved file is filled from: the app writes it into this writer's staging
/// through [`Staging`], a chunk at a time, as the commit runs.
pub trait Source<F> {
    fn fill<'s>(self: Box<Self>, staging: Staging<'s, F>) -> Filling<'s>
    where
        Self: 's;
}

impl<F: Fs> Source<F> for Vec<u8> {
    fn fill<'s>(self: Box<Self>, staging: Staging<'s, F>) -> Filling<'s>
    where
        Self: 's,
    {
        Box::pin(async move { staging.write(0, *self).await })
    }
}

impl<F: Fs> Source<F> for Splice {
    fn fill<'s>(self: Box<Self>, staging: Staging<'s, F>) -> Filling<'s>
    where
        Self: 's,
    {
        Box::pin(async move {
            let from = self.from.clone();
            for step in self.steps() {
                match step {
                    Splicing::Write { at, bytes } => staging.write(at, bytes).await?,
                    Splicing::Copy { at, range } => {
                        let bytes = staging.read(&from, range).await?;
                        if bytes.len() as u64 != range.len {
                            return Err(IoError::Other(SHRANK.into()));
                        }
                        staging.write(at, bytes).await?;
                    }
                }
            }
            Ok(())
        })
    }
}

/// The staged file a [`Source`] fills, and the folder it may read.
pub struct Staging<'s, F> {
    fs: &'s F,
    root: Root,
    path: RelPath,
}

impl<F: Fs> Staging<'_, F> {
    /// Writes `bytes` at `offset` of the staged file.
    pub async fn write(&self, offset: u64, bytes: Vec<u8>) -> std::result::Result<(), IoError> {
        let write = Io::Write {
            root: self.root,
            path: self.path.clone(),
            offset,
            bytes,
        };
        self.fs.perform(write).await.map(drop)
    }

    /// Reads `range` of the file at `path` in the folder, such as the one being
    /// rewritten.
    pub async fn read(
        &self,
        path: &RelPath,
        range: Range,
    ) -> std::result::Result<Vec<u8>, IoError> {
        let read = Io::Read {
            root: self.root,
            path: path.clone(),
            range,
        };
        match self.fs.perform(read).await? {
            Reply::Bytes(bytes) => Ok(bytes),
            _ => Err(IoError::Other("the backend gave the wrong reply".into())),
        }
    }
}

/// Runs `operation` to completion.
pub async fn run<O: Operation, F: Fs>(fs: &F, operation: O) -> O::Output {
    run_with(fs, Vec::new(), operation).await
}

/// Runs `operation` to completion, filling the `n`th [`crate::plan::Content`] it
/// asks for from `sources[n]`.
pub async fn run_with<'s, O: Operation, F: Fs>(
    fs: &F,
    sources: Vec<Box<dyn Source<F> + 's>>,
    mut operation: O,
) -> O::Output {
    let mut sources: Vec<_> = sources.into_iter().map(Some).collect();
    let mut result = None;
    loop {
        let io = match operation.resume(result.take()) {
            Step::Done(output) => return output,
            Step::Io(io) => io,
        };
        result = Some(match io {
            Io::Fill {
                root,
                path,
                content,
            } => match sources.get_mut(content.0).and_then(Option::take) {
                Some(source) => source
                    .fill(Staging { fs, root, path })
                    .await
                    .map(|()| Reply::Done),
                None => Err(IoError::Other(format!("no content {}", content.0))),
            },
            io => fs.perform(io).await,
        });
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

    /// An intent to build and commit. Its saves take bytes, or any [`Source`]
    /// boxed, such as a [`Splice`].
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

impl<'l, F: Fs + 'l> Driver for &'l mut Library<F> {
    type Source = Box<dyn Source<F> + 'l>;

    fn bytes(bytes: Vec<u8>) -> Self::Source {
        Box::new(bytes)
    }

    fn entity_id(&mut self) -> EntityId {
        self.core.entity_id()
    }
}

impl<'l, F: Fs + 'l> Intent<&'l mut Library<F>> {
    /// Commits the intent. One whose file effects stop partway is logged as far as
    /// they got and fails with [`crate::Error::Partial`].
    pub async fn commit(self) -> Result<Committed> {
        let (library, plan, sources) = self.into_parts();
        run_with(&library.fs, sources, library.core.commit(plan)).await
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
