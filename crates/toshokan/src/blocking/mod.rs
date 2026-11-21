//! The blocking driver: runs the core's operations on a [`Backend`] in a loop.

#[cfg(not(target_arch = "wasm32"))]
mod native;

#[cfg(not(target_arch = "wasm32"))]
pub use native::Native;

use std::collections::BTreeSet;

use crate::disk::MemDisk;
use crate::env::Env;
use crate::error::Result;
use crate::ids::{EntityId, EntryHash, Nonce, WriterId};
use crate::intent::{Driver, Intent};
use crate::io::{Capabilities, Io, IoError, IoResult, Meta, Operation, Range, Reply, Root, Step};
use crate::layout::Layout;
use crate::library;
use crate::log::Settlement;
use crate::path::RelPath;
use crate::plan::{Splice, Splicing};
use crate::report::{
    Committed, Compacted, Emptied, HistoryItem, Mode, Opened, Orphan, Refreshed, TrashItem,
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
    /// Opens the library in the folder as one writer of this install and reads the
    /// folder. Opening writes nothing in the folder and scans no library file: each
    /// entity's file is [`FileState::Unscanned`](crate::FileState::Unscanned) at the
    /// path its facts give until a [`rescan`](Self::rescan) finishes.
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
    pub fn owed(&self) -> usize {
        self.core.owed()
    }

    #[doc(hidden)]
    pub fn away(&self) -> usize {
        self.core.away()
    }

    #[doc(hidden)]
    pub fn rebound(&mut self) -> [(crate::binding::Bindings, Vec<crate::log::Op>); 2] {
        self.core.rebound()
    }

    /// This session's intents, for undo and redo.
    pub fn history(&self) -> &[HistoryItem] {
        self.core.history()
    }

    /// The entry of this session's latest intent, undo or redo, as one that finishes
    /// a change left unfinished logs it.
    pub fn last_logged(&self) -> Option<EntryHash> {
        self.core.last_logged()
    }

    /// The writer this instance writes as: none before its first write, and none
    /// once it stopped writing as one, until it writes again as another.
    pub fn writer_id(&self) -> Option<WriterId> {
        self.core.writer_id()
    }

    /// Whether the library may be written now, and why not.
    pub fn mode(&self) -> &Mode {
        self.core.mode()
    }

    /// The library paths a change of this writer's own still names while it waits
    /// to be finished by the next write or the next open.
    pub fn finishing(&self) -> Vec<RelPath> {
        self.core.finishing()
    }

    /// The files in other writers' directories, pending records among them, that
    /// the last read could not read, in order. What they hold is not shown, and no
    /// orphan is reported, until a read takes them in.
    pub fn unread(&self) -> Vec<RelPath> {
        self.core.unread()
    }

    /// An intent to build and commit. Its saves take bytes, or any [`Source`]
    /// boxed, such as a [`Splice`].
    pub fn intent(&mut self, label: &str) -> Intent<&mut Self> {
        Intent::new(self, label)
    }

    /// Undoes this session's latest intent not undone. Refused with
    /// [`Refusal::Nothing`](crate::Refusal::Nothing) when there is none,
    /// [`Refusal::ChangedSince`](crate::Refusal::ChangedSince) where another writer
    /// changed the same thing since, and [`Refusal::Emptied`](crate::Refusal::Emptied)
    /// when the bytes it would bring back have left the trash.
    pub fn undo(&mut self) -> Result<Committed> {
        run(&mut self.backend, self.core.undo())
    }

    /// Redoes this session's latest undo, while nothing was committed since. Refused
    /// as [`undo`](Self::undo) is.
    pub fn redo(&mut self) -> Result<Committed> {
        run(&mut self.backend, self.core.redo())
    }

    /// Lets go of what the next undo would reverse, as once its undo is refused: the
    /// next undo reaches this session's intent before it.
    pub fn forget_undo(&mut self) {
        self.core.forget_undo();
    }

    /// Settles another writer's unfinished effect, with the user's consent, in this
    /// writer's own log: finishes it, rolls it back or dismisses it, as `how` says.
    /// Effects that fail leave it open to settle again. Finishing or rolling back a
    /// directory move this folder cannot make is refused with
    /// [`Refusal::Unsupported`](crate::Refusal::Unsupported). A settlement is no step
    /// of undo.
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

    /// Settles now this writer's effects a crash interrupted, as
    /// [`Opened::settled`] reports them, rather than before its next write.
    pub fn settle_interrupted(&mut self) -> Result<()> {
        run(&mut self.backend, self.core.settle_interrupted())
    }

    /// Every writer with its last entry time, and whether another instance on this
    /// machine holds it.
    pub fn others(&mut self) -> Result<Vec<WriterInfo>> {
        run(&mut self.backend, self.core.others())
    }

    /// The bytes still in the trash of each writer this install wrote as, oldest
    /// first: this writer's, which undo restores and emptying removes, and those of
    /// the writers it retired, kept as they are.
    pub fn trash(&mut self) -> Result<Vec<TrashItem>> {
        run(&mut self.backend, self.core.trash())
    }

    /// Removes from this writer's trash every item but those of `kept` and those an
    /// unfinished effect may still move. An undo that needs a removed item is then
    /// refused with [`Refusal::Emptied`](crate::Refusal::Emptied).
    pub fn empty_trash(&mut self, kept: BTreeSet<Nonce>) -> Result<Emptied> {
        run(&mut self.backend, self.core.empty_trash(kept))
    }

    /// Compacts now, as tests and harnesses drive it; a writer compacts by itself.
    #[doc(hidden)]
    pub fn compact(&mut self) -> Result<Compacted> {
        run(&mut self.backend, self.core.compact())
    }

    /// Closes the library cleanly and returns the backend. A record of a finished
    /// effect it cannot remove stays for the next instance to find, and that failure
    /// is returned once the library is closed. A library dropped without closing
    /// leaves its segment open, as a crash does.
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
    /// Commits the intent. A refused intent fails with
    /// [`Error::Refused`](crate::Error::Refused) and changes nothing. One whose file
    /// effects fail logs nothing and fails; the next write finishes them first.
    pub fn commit(self) -> Result<Committed> {
        let (library, plan, sources) = self.into_parts();
        run_with(&mut library.backend, sources, library.core.commit(plan))
    }
}
