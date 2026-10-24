//! The blocking driver: runs the core's operations on a [`Backend`] in a loop.

#[cfg(not(target_arch = "wasm32"))]
mod native;

#[cfg(not(target_arch = "wasm32"))]
pub use native::Native;

use std::collections::BTreeSet;

use crate::disk::MemDisk;
use crate::drafts::{Draft, Drafted};
use crate::env::Env;
use crate::error::Result;
use crate::ids::{EntityId, Identity, Nonce, WriterId};
use crate::intent::{Driver, Intent};
use crate::io::{Capabilities, Io, IoError, IoResult, Meta, Operation, Range, Reply, Root, Step};
use crate::layout::Layout;
use crate::library;
use crate::log::Settlement;
use crate::path::RelPath;
use crate::plan::{Splice, Splicing};
use crate::report::{
    Committed, Compacted, Emptied, HistoryItem, Opened, Orphan, Refreshed, Tidied, TrashItem,
    WriterInfo,
};
use crate::schema::Schema;
use crate::view::View;

/// Storage that performs requests as they come, the batched ones included.
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

/// What a saved file is filled from: the app writes it into this writer's staging
/// through [`Staging`], a chunk at a time, as the commit runs.
pub trait Source<B> {
    fn fill(self: Box<Self>, staging: &mut Staging<'_, B>) -> std::result::Result<(), IoError>;
}

impl<B: Backend> Source<B> for Vec<u8> {
    fn fill(self: Box<Self>, staging: &mut Staging<'_, B>) -> std::result::Result<(), IoError> {
        staging.write(0, *self)
    }
}

impl<B: Backend> Source<B> for Splice {
    fn fill(self: Box<Self>, staging: &mut Staging<'_, B>) -> std::result::Result<(), IoError> {
        let from = self.from.clone();
        for step in self.steps() {
            match step? {
                Splicing::Write { at, bytes } => staging.write(at, bytes)?,
                Splicing::Copy { at, range } => {
                    let bytes = staging.read(&from, range)?;
                    if bytes.len() as u64 != range.len {
                        return Err(IoError::SpliceRange);
                    }
                    staging.write(at, bytes)?;
                }
            }
        }
        Ok(())
    }
}

/// The staged file a [`Source`] fills, and the folder it may read.
pub struct Staging<'s, B> {
    backend: &'s mut B,
    root: Root,
    path: RelPath,
}

impl<B: Backend> Staging<'_, B> {
    /// Writes `bytes` at `offset` of the staged file.
    pub fn write(&mut self, offset: u64, bytes: Vec<u8>) -> std::result::Result<(), IoError> {
        self.backend
            .perform(Io::Write {
                root: self.root,
                path: self.path.clone(),
                offset,
                bytes,
            })
            .map(drop)
    }

    /// Reads `range` of the file at `path` in the folder, such as the one being
    /// rewritten.
    pub fn read(&mut self, path: &RelPath, range: Range) -> std::result::Result<Vec<u8>, IoError> {
        let read = Io::Read {
            root: self.root,
            path: path.clone(),
            range,
        };
        match self.backend.perform(read)? {
            Reply::Bytes(bytes) => Ok(bytes),
            _ => Err(IoError::Other("the backend gave the wrong reply".into())),
        }
    }

    /// What is at `path` in the folder now, such as the file a copy read, which may
    /// have changed while it was read.
    pub fn stat(&mut self, path: &RelPath) -> std::result::Result<Option<Meta>, IoError> {
        let stat = Io::Stat {
            root: self.root,
            path: path.clone(),
        };
        match self.backend.perform(stat)? {
            Reply::Stat(meta) => Ok(meta),
            _ => Err(IoError::Other("the backend gave the wrong reply".into())),
        }
    }
}

/// Runs `operation` to completion.
pub fn run<O: Operation, B: Backend>(backend: &mut B, operation: O) -> O::Output {
    run_with(backend, Vec::new(), operation)
}

/// Runs `operation` to completion, filling the `n`th [`crate::plan::Content`] it
/// asks for from `sources[n]`.
pub fn run_with<'s, O: Operation, B: Backend>(
    backend: &mut B,
    sources: Vec<Box<dyn Source<B> + 's>>,
    mut operation: O,
) -> O::Output {
    let mut sources: Vec<_> = sources.into_iter().map(Some).collect();
    let mut result = None;
    loop {
        let io = match operation.resume(result.take()) {
            Step::Done(output) => return output,
            Step::Pause => continue,
            Step::Io(io) => io,
        };
        result = Some(match io {
            Io::Fill {
                root,
                path,
                content,
            } => match sources.get_mut(content.0).and_then(Option::take) {
                Some(source) => source
                    .fill(&mut Staging {
                        backend: &mut *backend,
                        root,
                        path,
                    })
                    .map(|()| Reply::Done),
                None => Err(IoError::Other(format!("no content {}", content.0))),
            },
            Io::Sync { root, .. } if !backend.capabilities(root).fsync => Ok(Reply::Done),
            io => backend.perform(io),
        });
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

    #[doc(hidden)]
    pub fn refolded(&self) -> crate::merge::Folded {
        self.core.refolded()
    }

    #[doc(hidden)]
    pub fn rebound(&mut self) -> [(crate::binding::Bindings, Vec<crate::log::Op>); 2] {
        self.core.rebound()
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
        Draft::new(self, Drafted::Entity(entity))
    }

    /// The draft of the file at `path`, which no entity is bound to.
    pub fn draft_file(&mut self, path: &RelPath) -> Draft<&mut Self> {
        Draft::new(self, Drafted::File(path.clone()))
    }

    pub fn undo(&mut self) -> Result<Committed> {
        run(&mut self.backend, self.core.undo())
    }

    pub fn redo(&mut self) -> Result<Committed> {
        run(&mut self.backend, self.core.redo())
    }

    pub fn settle(&mut self, orphan: Orphan, how: Settlement) -> Result<Committed> {
        run(&mut self.backend, self.core.settle(orphan, how))
    }

    /// Reads other writers' new entries, and scans only the files their facts
    /// moved.
    pub fn refresh(&mut self) -> Result<Refreshed> {
        run(&mut self.backend, self.core.refresh())
    }

    /// Refreshes and scans every library file, for what changed outside.
    pub fn rescan(&mut self) -> Result<Refreshed> {
        run(&mut self.backend, self.core.rescan())
    }

    /// Refreshes and scans the library files at `paths` and under them.
    pub fn rescan_paths(&mut self, paths: Vec<RelPath>) -> Result<Refreshed> {
        run(&mut self.backend, self.core.rescan_paths(paths))
    }

    pub fn discard_pooled_draft(&mut self, writer: WriterId, of: Drafted) -> Result<()> {
        run(
            &mut self.backend,
            self.core.discard_pooled_draft(writer, of),
        )
    }

    pub fn settle_interrupted(&mut self) -> Result<()> {
        run(&mut self.backend, self.core.settle_interrupted())
    }

    pub fn others(&mut self) -> Result<Vec<WriterInfo>> {
        run(&mut self.backend, self.core.others())
    }

    pub fn trash(&mut self) -> Result<Vec<TrashItem>> {
        run(&mut self.backend, self.core.trash())
    }

    pub fn empty_trash(&mut self, kept: BTreeSet<Nonce>) -> Result<Emptied> {
        run(&mut self.backend, self.core.empty_trash(kept))
    }

    /// Removes the pending records this writer's commits left once their entries
    /// were logged. An app calls it when idle; closing removes what is left.
    pub fn tidy(&mut self) -> Result<Tidied> {
        run(&mut self.backend, self.core.tidy())
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

impl<'l, B: Backend + 'l> Driver for &'l mut Library<B> {
    type Source = Box<dyn Source<B> + 'l>;

    fn bytes(bytes: Vec<u8>) -> Self::Source {
        Box::new(bytes)
    }

    fn entity_id(&mut self) -> EntityId {
        self.core.entity_id()
    }
}

impl<'l, B: Backend + 'l> Intent<&'l mut Library<B>> {
    /// Commits the intent. One whose file effects stop partway is logged as far as
    /// they got and fails with [`crate::Error::Partial`].
    pub fn commit(self) -> Result<Committed> {
        let (library, plan, sources) = self.into_parts();
        run_with(&mut library.backend, sources, library.core.commit(plan))
    }
}

impl<B: Backend> Draft<&mut Library<B>> {
    pub fn put(self, base: Identity, bytes: Vec<u8>) -> Result<()> {
        let (library, of) = self.into_parts();
        run(
            &mut library.backend,
            library.core.put_draft(of, base, bytes),
        )
    }

    pub fn discard(self) -> Result<()> {
        let (library, of) = self.into_parts();
        run(&mut library.backend, library.core.discard_draft(of))
    }
}
