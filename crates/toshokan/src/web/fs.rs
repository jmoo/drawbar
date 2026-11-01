//! The worker's side: requests performed on the browser's file system.

use std::cell::RefCell;
use std::collections::BTreeMap;

use js_sys::{Array, Function, Promise, Uint8Array};
use wasm_bindgen::closure::Closure;
use wasm_bindgen::{JsCast, JsValue};
use web_sys::{
    DedicatedWorkerGlobalScope, File, FileSystemCreateWritableOptions,
    FileSystemDirectoryHandle as Dir, FileSystemFileHandle, FileSystemGetDirectoryOptions,
    FileSystemGetFileOptions, FileSystemHandle, FileSystemHandleKind, FileSystemReadWriteOptions,
    FileSystemSyncAccessHandle, FileSystemWritableFileStream, WriteCommandType, WriteParams,
};

use super::{
    deferred, describe, failure, field, name_of, navigator, object, private_dir, wait, Folder,
};
use crate::asynch::{join, Fs};
use crate::disk::{FILLED_BY_DRIVERS, ROOT_ITSELF};
use crate::io::{Capabilities, Capability, DirEntry, Io, IoError, IoResult, Kind, Lock, Meta};
use crate::io::{Range, Reply};
use crate::path::RelPath;
use crate::Root;

/// The largest offset a browser addresses exactly: `Number.MAX_SAFE_INTEGER`.
const MAX_POSITION: u64 = (1 << 53) - 1;

/// Performs requests on a library's folder and local root. It runs in a
/// dedicated worker, the only place a private directory's files can be written.
pub struct Executor {
    folder: Tree,
    local: Tree,
    locks: Locks,
    /// A picked folder's file being filled by [`Io::Write`]s, kept open so each
    /// write does not copy the file again; closed before any request
    /// [`Io::keeps_open`] does not allow.
    ///
    /// ⚠️ Its writes land only at that close, so a close that fails fails the
    /// next request, whatever it asks, with an error that names the filled file.
    filling: RefCell<Option<Filling>>,
}

struct Filling {
    root: Root,
    path: RelPath,
    stream: FileSystemWritableFileStream,
}

/// One root: its top directory, how its files are written, and what it declares.
struct Tree {
    top: Dir,
    writes: Writes,
    capabilities: Capabilities,
    /// The directory at and under which no directory is removed or renamed:
    /// all of the local root, and toshokan's root in the folder once the library
    /// says which it is.
    kept: RefCell<Option<RelPath>>,
    /// The directories found there, by path. A browser can take milliseconds for
    /// each step of a path.
    // ⚠️ Firefox's handles follow a directory that moves, and fail once it is
    // removed, so only directories nothing moves or removes are kept.
    found: RefCell<BTreeMap<RelPath, Dir>>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Writes {
    /// Sync access handles, which write in place: the origin private file system.
    Handles,
    /// Writable streams, which write a copy and put it in place at close.
    Streams,
}

enum Entry {
    File(FileSystemFileHandle),
    Dir(Dir),
}

const DIRECTORY: Meta = Meta {
    kind: Kind::Directory,
    len: 0,
    modified: None,
};

impl Executor {
    /// An executor for `folder`, with the directory at `local` in the origin
    /// private file system as the local root, made if missing. Only in a
    /// dedicated worker.
    pub async fn new(folder: Folder, local: &RelPath) -> Result<Self, IoError> {
        if js_sys::global()
            .dyn_ref::<DedicatedWorkerGlobalScope>()
            .is_none()
        {
            return Err(IoError::Other(
                "a library's storage is written only from a dedicated worker".into(),
            ));
        }
        // Chromium's `flush()` waits for the disk, as its time shows; Firefox's
        // and WebKit's return at once, so they declare no sync. A sync covers a
        // file's contents only: see `Tree::sync`. `userAgentData` exists only in
        // Chromium.
        let chromium = field(&navigator(), "userAgentData").is_some();
        let private = Capabilities {
            append: true,
            rename_file: true,
            no_replace: false,
            rename_dir: false,
            fsync: chromium,
        };
        let folder = match folder {
            Folder::Private(path) => Tree::new(private_dir(&path).await?, Writes::Handles, private),
            Folder::Picked { dir, rename } => Tree::new(
                dir,
                Writes::Streams,
                Capabilities {
                    append: true,
                    rename_file: rename,
                    no_replace: false,
                    rename_dir: false,
                    fsync: false,
                },
            ),
        };
        let locks = Locks::new(local)?;
        let local = Tree::new(private_dir(local).await?, Writes::Handles, private);
        *local.kept.borrow_mut() = Some(RelPath::ROOT);
        Ok(Self {
            folder,
            local,
            locks,
            filling: RefCell::new(None),
        })
    }

    pub fn capabilities(&self, root: Root) -> Capabilities {
        self.tree(root).capabilities
    }

    /// Takes `root`, toshokan's root in the folder, whose directories are never
    /// removed or renamed: their handles are kept from request to request.
    pub fn own(&self, root: &RelPath) {
        *self.folder.kept.borrow_mut() = Some(root.clone());
        self.folder.found.borrow_mut().clear();
    }

    pub async fn perform(&self, io: Io) -> IoResult {
        let tree = self.tree(io.root());
        let result = match self.land(&io).await {
            Ok(()) => self.attempt(tree, &io).await,
            Err(error) => Err(error),
        };
        // A kept handle may fail otherwise than as not found; the next request
        // finds the path again.
        if let Err(IoError::Other(_)) = result {
            tree.found.borrow_mut().clear();
        }
        result
    }

    /// Lands the stream filling a file unless `io` keeps it open.
    async fn land(&self, io: &Io) -> Result<(), IoError> {
        match self.filling.take() {
            Some(filling) if io.keeps_open(filling.root, &filling.path) => {
                *self.filling.borrow_mut() = Some(filling);
                Ok(())
            }
            Some(filling) => land(filling).await,
            None => Ok(()),
        }
    }

    /// Performs `io`, and once more from the top directory when it found
    /// nothing through a kept handle: a directory removed from outside, even one
    /// made again since, holds nothing through its old handle.
    async fn attempt(&self, tree: &Tree, io: &Io) -> IoResult {
        let kept = tree.through_found(io) && !self.continues_filling(io);
        let result = self.request(io).await;
        if !kept || !found_nothing(&result) {
            return result;
        }
        tree.found.borrow_mut().clear();
        self.request(io).await
    }

    /// Whether `io` writes on through the stream already filling its file, which
    /// another attempt would not see.
    fn continues_filling(&self, io: &Io) -> bool {
        let filling = self.filling.borrow();
        let open = filling.as_ref();
        matches!(io, Io::Write { .. }) && open.is_some_and(|f| io.keeps_open(f.root, &f.path))
    }

    async fn request(&self, io: &Io) -> IoResult {
        let done = |()| Reply::Done;
        match io {
            Io::List { root, dir } => self.tree(*root).list(dir).await.map(Reply::Listed),
            Io::Stat { root, path } => self.tree(*root).stat(path).await.map(Reply::Stat),
            Io::ListStat { root, dir } => {
                let listed = self.tree(*root).list_stat(dir).await;
                listed.map(Reply::ListedStat)
            }
            Io::Read { root, path, range } => {
                self.tree(*root).read(path, *range).await.map(Reply::Bytes)
            }
            Io::ReadMany { root, reads } => {
                let read = self.tree(*root).read_many(reads).await;
                Ok(Reply::ReadMany(read))
            }
            Io::Create { root, path, bytes } => {
                self.tree(*root).create(path, bytes).await.map(done)
            }
            Io::Append { root, path, bytes } => {
                let tree = self.tree(*root);
                tree.write(&tree.file(path).await?, None, bytes)
                    .await
                    .map(done)
            }
            Io::Write {
                root,
                path,
                offset,
                bytes,
            } => self
                .write(*root, path.clone(), *offset, bytes)
                .await
                .map(done),
            Io::Rename { root, from, to } => self.tree(*root).rename(from, to).await.map(done),
            Io::Remove { root, path } => self.tree(*root).remove(path).await.map(done),
            Io::RemoveDir { root, path } => self.tree(*root).remove_dir(path).await.map(done),
            Io::MakeDir { root, path } => self.tree(*root).make_dir(path).await.map(done),
            Io::Sync { root, path } => self.tree(*root).sync(path).await.map(done),
            Io::Lock { name } => self.locks.lock(name).await.map(Reply::Lock),
            Io::Unlock { name } => self.locks.unlock(name).await.map(|()| Reply::Done),
            Io::Fill { .. } => Err(IoError::Other(FILLED_BY_DRIVERS.into())),
        }
    }

    fn tree(&self, root: Root) -> &Tree {
        match root {
            Root::Folder => &self.folder,
            Root::Local => &self.local,
        }
    }

    async fn write(
        &self,
        root: Root,
        path: RelPath,
        offset: u64,
        bytes: &[u8],
    ) -> Result<(), IoError> {
        let tree = self.tree(root);
        let at = position(offset)?;
        if tree.writes == Writes::Handles {
            return tree.write(&tree.file(&path).await?, Some(at), bytes).await;
        }
        let stream = match self.filling.take() {
            Some(filling) => filling.stream,
            None => open_stream(&tree.file(&path).await?).await?,
        };
        if let Err(error) = write_stream(&stream, at, bytes).await {
            let _ = wait(stream.abort()).await;
            return Err(error);
        }
        *self.filling.borrow_mut() = Some(Filling { root, path, stream });
        Ok(())
    }
}

impl Fs for Executor {
    fn capabilities(&self, root: Root) -> Capabilities {
        Executor::capabilities(self, root)
    }

    async fn perform(&self, io: Io) -> IoResult {
        Executor::perform(self, io).await
    }

    fn own(&self, root: &RelPath) {
        Executor::own(self, root);
    }
}

/// Whether `result` says something was not there.
fn found_nothing(result: &IoResult) -> bool {
    match result {
        Err(IoError::NotFound) | Ok(Reply::Stat(None)) => true,
        Ok(Reply::ReadMany(reads)) => reads.contains(&Err(IoError::NotFound)),
        _ => false,
    }
}

impl Tree {
    fn new(top: Dir, writes: Writes, capabilities: Capabilities) -> Self {
        Self {
            top,
            writes,
            capabilities,
            kept: RefCell::new(None),
            found: RefCell::new(BTreeMap::new()),
        }
    }

    /// Whether the handle of the directory at `path` is kept.
    fn keeps(&self, path: &RelPath) -> bool {
        let kept = self.kept.borrow();
        kept.as_ref().is_some_and(|kept| path.starts_with(kept))
    }

    /// Whether a request for `io` may start from a kept handle.
    fn through_found(&self, io: &Io) -> bool {
        let found = self.found.borrow();
        let through = |path: &RelPath| {
            std::iter::successors(Some(path.clone()), RelPath::parent)
                .any(|dir| found.contains_key(&dir))
        };
        match io {
            Io::Rename { from, to, .. } => through(from) || through(to),
            Io::ReadMany { reads, .. } => reads.iter().any(|(path, _)| through(path)),
            _ => through(io.path()),
        }
    }

    /// The directory at `path`, found from the nearest directory kept above it.
    async fn dir(&self, path: &RelPath) -> Result<Dir, IoError> {
        self.walk(path, None).await
    }

    /// The directory at `path`, each directory on the way made where missing
    /// when `make` says so. Making starts from the top directory, since a kept
    /// handle to a directory removed from outside makes nothing under it.
    async fn walk(
        &self,
        path: &RelPath,
        make: Option<&FileSystemGetDirectoryOptions>,
    ) -> Result<Dir, IoError> {
        let names: Vec<&str> = path.components().collect();
        let (at, mut dir) = match make {
            Some(_) => (0, self.top.clone()),
            None => self.nearest(&names),
        };
        let mut walked = RelPath::new(&names[..at].join("/")).unwrap_or_default();
        for name in &names[at..] {
            let next = match make {
                Some(make) => dir.get_directory_handle_with_options(name, make),
                None => dir.get_directory_handle(name),
            };
            dir = wait(next)
                .await
                .map_err(|error| failure(&error, IoError::NotDirectory))?
                .unchecked_into();
            walked = walked
                .join(name)
                .map_err(|error| IoError::Other(error.to_string()))?;
            if self.keeps(&walked) {
                self.found.borrow_mut().insert(walked.clone(), dir.clone());
            }
        }
        Ok(dir)
    }

    /// The nearest directory kept at or above the one `names` lead to, and how
    /// many of `names` lead to it.
    fn nearest(&self, names: &[&str]) -> (usize, Dir) {
        let found = self.found.borrow();
        for at in (1..=names.len()).rev() {
            let above = RelPath::new(&names[..at].join("/")).unwrap_or_default();
            if let Some(dir) = found.get(&above) {
                return (at, dir.clone());
            }
        }
        (0, self.top.clone())
    }

    /// The directory `path` would live in, and its name there.
    async fn parent<'p>(&self, path: &'p RelPath) -> Result<(Dir, &'p str), IoError> {
        let (Some(parent), Some(name)) = (path.parent(), path.name()) else {
            return Err(IoError::Other(ROOT_ITSELF.into()));
        };
        Ok((self.dir(&parent).await?, name))
    }

    async fn file(&self, path: &RelPath) -> Result<FileSystemFileHandle, IoError> {
        let (dir, name) = self.parent(path).await?;
        let file = wait(dir.get_file_handle(name)).await;
        let file = file.map_err(|error| failure(&error, IoError::IsDirectory))?;
        Ok(file.unchecked_into())
    }

    async fn list(&self, dir: &RelPath) -> Result<Vec<DirEntry>, IoError> {
        let listed = children(&self.dir(dir).await?).await?;
        let entries = listed.into_iter().map(|(name, entry)| DirEntry {
            name,
            kind: match entry {
                Entry::File(_) => Kind::File,
                Entry::Dir(_) => Kind::Directory,
            },
        });
        Ok(entries.collect())
    }

    async fn stat(&self, path: &RelPath) -> Result<Option<Meta>, IoError> {
        if path.is_root() {
            return Ok(Some(DIRECTORY));
        }
        let (dir, name) = match self.parent(path).await {
            Ok(place) => place,
            Err(IoError::NotFound) => return Ok(None),
            Err(error) => return Err(error),
        };
        match entry(&dir, name).await? {
            None => Ok(None),
            Some(Entry::Dir(_)) => Ok(Some(DIRECTORY)),
            Some(Entry::File(file)) => match snapshot(&file).await {
                Ok(file) => Ok(Some(meta(&file))),
                Err(IoError::NotFound) => Ok(None),
                Err(error) => Err(error),
            },
        }
    }

    /// Each entry's metadata is fetched at once, which takes 2.5 times less than
    /// one after another in Chromium and WebKit, by measurement.
    async fn list_stat(&self, dir: &RelPath) -> Result<Vec<(String, Meta)>, IoError> {
        let listed = children(&self.dir(dir).await?).await?;
        let metas = join(listed.iter().map(|(_, entry)| async move {
            match entry {
                Entry::Dir(_) => Ok(Some(DIRECTORY)),
                Entry::File(file) => match snapshot(file).await {
                    Ok(file) => Ok(Some(meta(&file))),
                    Err(IoError::NotFound) => Ok(None),
                    Err(error) => Err(error),
                },
            }
        }))
        .await;
        let mut found = Vec::new();
        for ((name, _), meta) in listed.into_iter().zip(metas) {
            if let Some(meta) = meta? {
                found.push((name, meta));
            }
        }
        Ok(found)
    }

    /// Each of `reads`, as [`Tree::read`] reads it, finding each directory they
    /// name once: a browser can take milliseconds for each step of a path. The
    /// files of a directory kept that more than one read names are found by one
    /// listing of it, which Firefox gives in about the time of one step.
    async fn read_many(&self, reads: &[(RelPath, Range)]) -> Vec<Result<Vec<u8>, IoError>> {
        let mut named: BTreeMap<RelPath, usize> = BTreeMap::new();
        for parent in reads.iter().filter_map(|(path, _)| path.parent()) {
            *named.entry(parent).or_default() += 1;
        }
        let mut dirs: BTreeMap<RelPath, Result<Dir, IoError>> = BTreeMap::new();
        let mut listed: BTreeMap<RelPath, BTreeMap<String, FileSystemFileHandle>> = BTreeMap::new();
        for (parent, count) in named {
            let dir = self.dir(&parent).await;
            if let (Ok(found), true) = (&dir, count > 1 && self.keeps(&parent)) {
                if let Ok(entries) = children(found).await {
                    let files = entries.into_iter().filter_map(|(name, entry)| match entry {
                        Entry::File(file) => Some((name, file)),
                        Entry::Dir(_) => None,
                    });
                    listed.insert(parent.clone(), files.collect());
                }
            }
            dirs.insert(parent, dir);
        }
        let (dirs, listed) = (&dirs, &listed);
        join(reads.iter().map(|(path, range)| async move {
            let (Some(parent), Some(name)) = (path.parent(), path.name()) else {
                return Err(IoError::Other(ROOT_ITSELF.into()));
            };
            let dir = dirs[&parent].clone()?;
            let known = listed.get(&parent).and_then(|files| files.get(name));
            let file = match known {
                Some(file) => file.clone(),
                None => wait(dir.get_file_handle(name))
                    .await
                    .map_err(|error| failure(&error, IoError::IsDirectory))?
                    .unchecked_into(),
            };
            read_file(&file, *range).await
        }))
        .await
    }

    /// Through `getFile()`, which takes no lock, so another tab writing the file
    /// does not refuse it.
    async fn read(&self, path: &RelPath, range: Range) -> Result<Vec<u8>, IoError> {
        read_file(&self.file(path).await?, range).await
    }

    async fn create(&self, path: &RelPath, bytes: &[u8]) -> Result<(), IoError> {
        let (dir, name) = self.parent(path).await?;
        if entry(&dir, name).await?.is_some() {
            return Err(IoError::AlreadyExists);
        }
        let options = FileSystemGetFileOptions::new();
        options.set_create(true);
        let file: FileSystemFileHandle = wait(dir.get_file_handle_with_options(name, &options))
            .await
            .map_err(|error| failure(&error, IoError::AlreadyExists))?
            .unchecked_into();
        if bytes.is_empty() {
            return Ok(());
        }
        let written = self.write(&file, Some(0.0), bytes).await;
        if written.is_err() {
            let _ = wait(dir.remove_entry(name)).await;
        }
        written
    }

    /// Writes `bytes` at `at`, or at the end of the file where `at` is `None`.
    async fn write(
        &self,
        file: &FileSystemFileHandle,
        at: Option<f64>,
        bytes: &[u8],
    ) -> Result<(), IoError> {
        match self.writes {
            Writes::Handles => {
                let access = open_access(file).await?;
                let written = access
                    .get_size()
                    .and_then(|size| write_access(&access, at.unwrap_or(size), bytes));
                access.close();
                match written {
                    Ok(n) => whole(n, bytes.len()),
                    Err(error) => Err(failure(&error, IoError::IsDirectory)),
                }
            }
            Writes::Streams => {
                let at = match at {
                    Some(at) => at,
                    None => snapshot(file).await?.size(),
                };
                let stream = open_stream(file).await?;
                match write_stream(&stream, at, bytes).await {
                    Ok(()) => close(stream).await,
                    Err(error) => {
                        let _ = wait(stream.abort()).await;
                        Err(error)
                    }
                }
            }
        }
    }

    async fn rename(&self, from: &RelPath, to: &RelPath) -> Result<(), IoError> {
        let (from_dir, from_name) = self.parent(from).await?;
        let moving = entry(&from_dir, from_name)
            .await?
            .ok_or(IoError::NotFound)?;
        let capability = match moving {
            Entry::File(_) => Capability::RenameFile,
            Entry::Dir(_) => Capability::RenameDir,
        };
        if !self.capabilities.has(capability) {
            return Err(IoError::Unsupported(capability));
        }
        let (to_dir, to_name) = self.parent(to).await?;
        match (entry(&to_dir, to_name).await?, &moving) {
            (None, _) => {}
            (Some(Entry::File(_)), Entry::File(_)) if !self.capabilities.no_replace => {}
            (Some(_), _) => return Err(IoError::AlreadyExists),
        }
        if to.starts_with(from) {
            return Err(IoError::IntoItself);
        }
        let handle: &FileSystemHandle = match &moving {
            Entry::File(file) => file,
            Entry::Dir(dir) => dir,
        };
        let refused = || IoError::Unsupported(capability);
        let failed = |error: JsValue| match name_of(&error).as_str() {
            "NotSupportedError" => refused(),
            _ => failure(&error, IoError::AlreadyExists),
        };
        let call: Function = field(handle, "move")
            .and_then(|call| call.dyn_into().ok())
            .ok_or_else(refused)?;
        let moved = call
            .call2(handle, &to_dir, &JsValue::from_str(to_name))
            .map_err(failed)?;
        let moved: Promise = moved.dyn_into().map_err(|_| refused())?;
        wait(moved).await.map_err(failed)?;
        Ok(())
    }

    async fn remove(&self, path: &RelPath) -> Result<(), IoError> {
        let (dir, name) = self.parent(path).await?;
        match entry(&dir, name).await? {
            None => Err(IoError::NotFound),
            Some(Entry::Dir(_)) => Err(IoError::IsDirectory),
            Some(Entry::File(_)) => wait(dir.remove_entry(name))
                .await
                .map(drop)
                .map_err(|error| failure(&error, IoError::IsDirectory)),
        }
    }

    /// A directory is checked empty first: WebKit refuses a full one with an
    /// error it gives for other failures too.
    async fn remove_dir(&self, path: &RelPath) -> Result<(), IoError> {
        let (dir, name) = self.parent(path).await?;
        match entry(&dir, name).await? {
            None => Err(IoError::NotFound),
            Some(Entry::File(_)) => Err(IoError::NotDirectory),
            Some(Entry::Dir(removed)) if !children(&removed).await?.is_empty() => {
                Err(IoError::NotEmpty)
            }
            Some(Entry::Dir(_)) => wait(dir.remove_entry(name))
                .await
                .map(drop)
                .map_err(|error| match name_of(&error).as_str() {
                    "InvalidModificationError" => IoError::NotEmpty,
                    _ => failure(&error, IoError::NotDirectory),
                }),
        }
    }

    async fn make_dir(&self, path: &RelPath) -> Result<(), IoError> {
        let options = FileSystemGetDirectoryOptions::new();
        options.set_create(true);
        self.walk(path, Some(&options)).await.map(drop)
    }

    /// Flushes a file through a sync access handle. A directory's names cannot
    /// be synced: the browser keeps them in its own database, as durable as it
    /// makes them. A file a stream fills is landed before any sync of it reaches
    /// here, since a sync does not keep the stream open ([`Io::keeps_open`]), and
    /// closing the stream is what makes its bytes durable.
    async fn sync(&self, path: &RelPath) -> Result<(), IoError> {
        if path.is_root() {
            return Ok(());
        }
        let (dir, name) = self.parent(path).await?;
        let file = match entry(&dir, name).await? {
            None => return Err(IoError::NotFound),
            Some(Entry::Dir(_)) => return Ok(()),
            Some(Entry::File(file)) => file,
        };
        if self.writes == Writes::Streams || !self.capabilities.fsync {
            return Ok(());
        }
        let access = open_access(&file).await?;
        let flushed = access.flush();
        access.close();
        flushed.map_err(|error| failure(&error, IoError::IsDirectory))
    }
}

/// What is at `name` in `dir`.
async fn entry(dir: &Dir, name: &str) -> Result<Option<Entry>, IoError> {
    let error = match wait(dir.get_file_handle(name)).await {
        Ok(file) => return Ok(Some(Entry::File(file.unchecked_into()))),
        Err(error) => error,
    };
    match name_of(&error).as_str() {
        "NotFoundError" => Ok(None),
        "TypeMismatchError" => match wait(dir.get_directory_handle(name)).await {
            Ok(dir) => Ok(Some(Entry::Dir(dir.unchecked_into()))),
            Err(error) if name_of(&error) == "NotFoundError" => Ok(None),
            Err(error) => Err(failure(&error, IoError::NotDirectory)),
        },
        _ => Err(failure(&error, IoError::IsDirectory)),
    }
}

/// A directory's entries, sorted by name.
async fn children(dir: &Dir) -> Result<Vec<(String, Entry)>, IoError> {
    let iterator = dir.values();
    let mut found = Vec::new();
    loop {
        let next = iterator
            .next()
            .map_err(|error| IoError::Other(describe(&error)))?;
        let step = wait(next)
            .await
            .map_err(|error| failure(&error, IoError::NotDirectory))?;
        if field(&step, "done").and_then(|done| done.as_bool()) == Some(true) {
            break;
        }
        let Some(handle) = field(&step, "value") else {
            continue;
        };
        let handle: FileSystemHandle = handle.unchecked_into();
        let name = handle.name();
        let entry = match handle.kind() {
            FileSystemHandleKind::File => Entry::File(handle.unchecked_into()),
            FileSystemHandleKind::Directory => Entry::Dir(handle.unchecked_into()),
            _ => continue,
        };
        found.push((name, entry));
    }
    found.sort_by(|a, b| a.0.cmp(&b.0));
    Ok(found)
}

/// `range` of `file`, as much of it as the file holds.
async fn read_file(file: &FileSystemFileHandle, range: Range) -> Result<Vec<u8>, IoError> {
    let file = snapshot(file).await?;
    let size = file.size() as u64;
    let start = range.offset.min(size);
    let end = range.offset.saturating_add(range.len).min(size);
    if start == end {
        return Ok(Vec::new());
    }
    let slice = file
        .slice_with_f64_and_f64(start as f64, end as f64)
        .map_err(|error| failure(&error, IoError::IsDirectory))?;
    let buffer = wait(slice.array_buffer())
        .await
        .map_err(|error| failure(&error, IoError::IsDirectory))?;
    Ok(Uint8Array::new(&buffer).to_vec())
}

async fn snapshot(file: &FileSystemFileHandle) -> Result<File, IoError> {
    let file = wait(file.get_file()).await;
    Ok(file
        .map_err(|error| failure(&error, IoError::IsDirectory))?
        .unchecked_into())
}

/// A file's metadata. Browsers keep the time of a change in milliseconds, so
/// two changes within one millisecond that keep the length look alike.
fn meta(file: &File) -> Meta {
    let modified = file.last_modified();
    Meta {
        kind: Kind::File,
        len: file.size() as u64,
        modified: (modified.is_finite() && modified >= 0.0)
            .then(|| (modified as u64).saturating_mul(1_000_000)),
    }
}

fn position(offset: u64) -> Result<f64, IoError> {
    match offset <= MAX_POSITION {
        true => Ok(offset as f64),
        false => Err(IoError::Other(format!(
            "offset {offset} is past what a browser addresses"
        ))),
    }
}

/// What a sync access handle's write of `len` bytes that wrote `n` means.
// Firefox meets the origin's quota by writing fewer bytes than asked, where the
// standard throws `QuotaExceededError`, as Chromium does. Measured.
fn whole(n: f64, len: usize) -> Result<(), IoError> {
    match n == len as f64 {
        true => Ok(()),
        false => Err(IoError::NoSpace),
    }
}

async fn open_access(file: &FileSystemFileHandle) -> Result<FileSystemSyncAccessHandle, IoError> {
    let access = wait(file.create_sync_access_handle()).await;
    Ok(access
        .map_err(|error| failure(&error, IoError::IsDirectory))?
        .unchecked_into())
}

fn write_access(
    access: &FileSystemSyncAccessHandle,
    at: f64,
    bytes: &[u8],
) -> Result<f64, JsValue> {
    let options = FileSystemReadWriteOptions::new();
    options.set_at(at);
    access.write_with_u8_array_and_options(bytes, &options)
}

async fn open_stream(file: &FileSystemFileHandle) -> Result<FileSystemWritableFileStream, IoError> {
    let options = FileSystemCreateWritableOptions::new();
    options.set_keep_existing_data(true);
    let stream = wait(file.create_writable_with_options(&options)).await;
    Ok(stream
        .map_err(|error| failure(&error, IoError::IsDirectory))?
        .unchecked_into())
}

async fn write_stream(
    stream: &FileSystemWritableFileStream,
    at: f64,
    bytes: &[u8],
) -> Result<(), IoError> {
    let params = WriteParams::new(WriteCommandType::Write);
    params.set_position(Some(at));
    params.set_data(&Uint8Array::from(bytes));
    let writing = stream
        .write_with_write_params(&params)
        .map_err(|error| failure(&error, IoError::IsDirectory))?;
    wait(writing)
        .await
        .map(drop)
        .map_err(|error| failure(&error, IoError::IsDirectory))
}

/// Closes the stream filling a file, failing as [`IoError::undelivered`].
async fn land(filling: Filling) -> Result<(), IoError> {
    let Filling { path, stream, .. } = filling;
    close(stream)
        .await
        .map_err(|error| IoError::undelivered(&path, &error))
}

/// Puts what a stream wrote in place: the point at which it lands.
async fn close(stream: FileSystemWritableFileStream) -> Result<(), IoError> {
    wait(stream.close())
        .await
        .map(drop)
        .map_err(|error| failure(&error, IoError::IsDirectory))
}

/// Web Locks held by this worker, so by this process: the browser releases them
/// when the worker ends.
struct Locks {
    /// `navigator.locks`, called by name: web-sys binds it only with unstable APIs.
    manager: JsValue,
    /// Names this install's locks apart from another local root's.
    prefix: String,
    /// Each held lock's release, and the request that settles once the browser
    /// has let the lock go.
    held: RefCell<BTreeMap<RelPath, (Function, Promise)>>,
}

impl Locks {
    fn new(local: &RelPath) -> Result<Self, IoError> {
        let manager = field(&navigator(), "locks")
            .ok_or_else(|| IoError::Other("this browser has no Web Locks".into()))?;
        Ok(Self {
            manager,
            prefix: format!("toshokan:/{local}"),
            held: RefCell::default(),
        })
    }

    async fn lock(&self, name: &RelPath) -> Result<Lock, IoError> {
        if self.held.borrow().contains_key(name) {
            return Ok(Lock::Acquired);
        }
        let (answered, answer) = deferred();
        let granted = Closure::once_into_js(move |lock: JsValue| -> Promise {
            if lock.is_null() {
                let _ = answer.call1(&JsValue::NULL, &JsValue::NULL);
                return Promise::resolve(&JsValue::UNDEFINED);
            }
            let (holding, release) = deferred();
            let _ = answer.call1(&JsValue::NULL, &release);
            holding
        });
        let other = |error: JsValue| IoError::Other(describe(&error));
        let request: Function = field(&self.manager, "request")
            .ok_or_else(|| IoError::Other("this browser cannot request a lock".into()))?
            .unchecked_into();
        let options = object(&[("ifAvailable", true.into())]);
        let requested: Promise = request
            .call3(
                &self.manager,
                &JsValue::from_str(&format!("{}/{name}", self.prefix)),
                &options,
                &granted,
            )
            .map_err(other)?
            .unchecked_into();
        let first = Promise::race(&Array::of2(&answered, &requested));
        let answer = wait(first).await.map_err(other)?;
        match answer.dyn_into::<Function>() {
            Ok(release) => {
                self.held
                    .borrow_mut()
                    .insert(name.clone(), (release, requested));
                Ok(Lock::Acquired)
            }
            Err(_) => Ok(Lock::Held),
        }
    }

    /// Lets the lock `name` go, and returns once the browser has, so that
    /// another process asking next finds it free.
    async fn unlock(&self, name: &RelPath) -> Result<(), IoError> {
        let Some((release, requested)) = self.held.borrow_mut().remove(name) else {
            return Ok(());
        };
        let _ = release.call0(&JsValue::NULL);
        wait(requested)
            .await
            .map(drop)
            .map_err(|error| IoError::Other(describe(&error)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_write_cut_short_is_out_of_space() {
        assert_eq!(whole(5.0, 5), Ok(()));
        assert_eq!(whole(0.0, 131_072), Err(IoError::NoSpace));
        assert_eq!(whole(4096.0, 131_072), Err(IoError::NoSpace));
    }
}
