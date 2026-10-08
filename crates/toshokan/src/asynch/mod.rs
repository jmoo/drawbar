//! The async driver: runs the core's operations on an [`Fs`] in a loop. Futures
//! carry no `Send` bound, so a single-threaded browser backend can implement it.

use std::future::Future;
use std::pin::Pin;
use std::task::Poll;

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
use crate::plan::{Splice, Splicing};
use crate::report::{
    Committed, Compacted, Emptied, HistoryItem, Opened, Orphan, Refreshed, TrashItem, WriterInfo,
};
use crate::schema::Schema;
use crate::trash::Policy;
use crate::view::View;

/// Storage that performs requests asynchronously, the batched ones included;
/// storage that has no batched form answers those with [`fan_out`].
#[allow(async_fn_in_trait)]
pub trait Fs {
    fn capabilities(&self, root: Root) -> Capabilities;

    async fn perform(&self, io: Io) -> IoResult;

    /// Awaited at each [`Step::Pause`]. Storage sharing its thread with a user
    /// interface gives the thread back here once the core has held it long
    /// enough.
    async fn pause(&self) {}

    /// Told, as a library opens, its root in the folder: only toshokan writes
    /// there, and removes or renames no directory, so storage may keep the
    /// directories it finds there for later requests.
    fn own(&self, _root: &RelPath) {}
}

/// Answers `io` with `perform`, which takes only single requests:
/// [`Io::ListStat`] as an [`Io::List`] and then an [`Io::Stat`] of every name,
/// those issued together, and [`Io::ReadMany`] as its reads, issued together.
pub async fn fan_out<P, F>(io: Io, perform: P) -> IoResult
where
    P: Fn(Io) -> F,
    F: Future<Output = IoResult>,
{
    let wrong = || IoError::Other("the backend gave the wrong reply".into());
    match io {
        Io::ListStat { root, dir } => {
            let listed = perform(Io::List {
                root,
                dir: dir.clone(),
            });
            let Reply::Listed(entries) = listed.await? else {
                return Err(wrong());
            };
            let paths = entries.iter().map(|entry| dir.join(&entry.name));
            let paths: Vec<RelPath> = paths
                .collect::<Result<_>>()
                .map_err(|error| IoError::Other(error.to_string()))?;
            let stats = join(
                paths
                    .into_iter()
                    .map(|path| perform(Io::Stat { root, path })),
            );
            let mut found = Vec::new();
            for (entry, stat) in entries.into_iter().zip(stats.await) {
                match stat? {
                    Reply::Stat(Some(meta)) => found.push((entry.name, meta)),
                    Reply::Stat(None) => {}
                    _ => return Err(wrong()),
                }
            }
            Ok(Reply::ListedStat(found))
        }
        Io::ReadMany { root, reads } => {
            let reads = reads
                .into_iter()
                .map(|(path, range)| perform(Io::Read { root, path, range }));
            let read = join(reads).await.into_iter().map(|result| match result? {
                Reply::Bytes(bytes) => Ok(bytes),
                _ => Err(wrong()),
            });
            Ok(Reply::ReadMany(read.collect()))
        }
        io => perform(io).await,
    }
}

/// Every future's output, in order, the futures polled together.
pub(crate) async fn join<T>(futures: impl Iterator<Item = impl Future<Output = T>>) -> Vec<T> {
    let mut running: Vec<_> = futures.map(Box::pin).collect();
    let mut outputs: Vec<Option<T>> = running.iter().map(|_| None).collect();
    std::future::poll_fn(|context| {
        let mut waiting = false;
        for (future, output) in running.iter_mut().zip(outputs.iter_mut()) {
            if output.is_some() {
                continue;
            }
            match future.as_mut().poll(context) {
                Poll::Ready(value) => *output = Some(value),
                Poll::Pending => waiting = true,
            }
        }
        match waiting {
            true => Poll::Pending,
            false => Poll::Ready(
                outputs
                    .iter_mut()
                    .map(|output| output.take().expect("every future is ready"))
                    .collect(),
            ),
        }
    })
    .await
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
                match step? {
                    Splicing::Write { at, bytes } => staging.write(at, bytes).await?,
                    Splicing::Copy { at, range } => {
                        let bytes = staging.read(&from, range).await?;
                        if bytes.len() as u64 != range.len {
                            return Err(IoError::SpliceRange);
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
            Step::Pause => {
                fs.pause().await;
                continue;
            }
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
            Io::Sync { root, .. } if !fs.capabilities(root).fsync => Ok(Reply::Done),
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
        fs.own(layout.root());
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
        Draft::new(self, entity)
    }

    pub async fn undo(&mut self) -> Result<Committed> {
        run(&self.fs, self.core.undo()).await
    }

    pub async fn redo(&mut self) -> Result<Committed> {
        run(&self.fs, self.core.redo()).await
    }

    pub async fn settle(&mut self, orphan: Orphan, how: Settlement) -> Result<Committed> {
        run(&self.fs, self.core.settle(orphan, how)).await
    }

    /// Reads other writers' new entries, and scans only the files their facts
    /// moved.
    pub async fn refresh(&mut self) -> Result<Refreshed> {
        run(&self.fs, self.core.refresh()).await
    }

    /// Refreshes and scans every library file, for what changed outside. Dropping
    /// the future stops the scan; the next rescan goes on where it stopped.
    pub async fn rescan(&mut self) -> Result<Refreshed> {
        run(&self.fs, self.core.rescan()).await
    }

    /// Refreshes and scans the library files at `paths` and under them, such as
    /// those a watcher saw change.
    pub async fn rescan_paths(&mut self, paths: Vec<RelPath>) -> Result<Refreshed> {
        run(&self.fs, self.core.rescan_paths(paths)).await
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

#[cfg(test)]
mod tests {
    use std::cell::Cell;
    use std::task::Context;

    use super::*;

    /// Ready on its second poll.
    struct Later(bool);

    impl Future for Later {
        type Output = ();

        fn poll(mut self: Pin<&mut Self>, context: &mut Context<'_>) -> Poll<()> {
            if self.0 {
                return Poll::Ready(());
            }
            self.0 = true;
            context.waker().wake_by_ref();
            Poll::Pending
        }
    }

    /// Pauses between requests: a stat, then `pauses` pauses, then a stat.
    struct Pausing {
        pauses: usize,
        asked: usize,
    }

    impl Operation for Pausing {
        type Output = usize;

        fn resume(&mut self, _: Option<IoResult>) -> Step<usize> {
            self.asked += 1;
            let stat = Io::Stat {
                root: Root::Folder,
                path: RelPath::new("a").unwrap(),
            };
            match self.asked {
                1 => Step::Io(stat),
                n if n <= 1 + self.pauses => Step::Pause,
                n if n == 2 + self.pauses => Step::Io(stat),
                _ => Step::Done(self.asked),
            }
        }
    }

    /// Storage that counts the pauses it is handed, and is never ready at once.
    struct Counting(MemDisk, Cell<usize>);

    impl Fs for Counting {
        fn capabilities(&self, root: Root) -> Capabilities {
            Fs::capabilities(&self.0, root)
        }

        async fn perform(&self, io: Io) -> IoResult {
            Fs::perform(&self.0, io).await
        }

        async fn pause(&self) {
            Later(false).await;
            self.1.set(self.1.get() + 1);
        }
    }

    #[test]
    fn the_async_driver_hands_each_pause_to_the_storage_and_resumes_after_it() {
        let fs = Counting(MemDisk::new(), Cell::new(0));
        let operation = Pausing {
            pauses: 3,
            asked: 0,
        };
        assert_eq!(pollster::block_on(run(&fs, operation)), 6);
        assert_eq!(fs.1.get(), 3);
    }

    #[test]
    fn a_batch_storage_cannot_take_is_issued_as_single_requests_together() {
        let disk = MemDisk::new();
        for name in ["d", "d/a", "d/b", "d/c"] {
            let path = RelPath::new(name).unwrap();
            let io = match name {
                "d" => Io::MakeDir {
                    root: Root::Folder,
                    path,
                },
                _ => Io::Create {
                    root: Root::Folder,
                    path,
                    bytes: name.as_bytes().to_vec(),
                },
            };
            disk.perform(io).unwrap();
        }
        let (running, most) = (Cell::new(0), Cell::new(0));
        let single = |io: Io| {
            let (disk, running, most) = (&disk, &running, &most);
            async move {
                running.set(running.get() + 1);
                most.set(most.get().max(running.get()));
                Later(false).await;
                running.set(running.get() - 1);
                disk.perform(io)
            }
        };
        let dir = RelPath::new("d").unwrap();
        let listed = Io::ListStat {
            root: Root::Folder,
            dir: dir.clone(),
        };
        let answered = pollster::block_on(fan_out(listed.clone(), single));
        assert_eq!(answered, disk.perform(listed));
        assert_eq!(most.get(), 3, "the three stats are in flight together");
        let reads = Io::ReadMany {
            root: Root::Folder,
            reads: ["d/a", "d/none", "d/c"]
                .map(|name| (RelPath::new(name).unwrap(), Range { offset: 2, len: 9 }))
                .into(),
        };
        let answered = pollster::block_on(fan_out(reads.clone(), single));
        assert_eq!(answered, disk.perform(reads));
        assert_eq!(
            answered,
            Ok(Reply::ReadMany(vec![
                Ok(b"a".to_vec()),
                Err(IoError::NotFound),
                Ok(b"c".to_vec())
            ]))
        );
    }
}
