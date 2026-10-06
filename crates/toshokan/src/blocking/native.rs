use std::collections::BTreeMap;
use std::fs::{self, File, OpenOptions, TryLockError};
use std::io::{self, ErrorKind, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

use super::Backend;
use crate::io::{
    Capabilities, DirEntry, Io, IoError, IoResult, Kind, Lock, Meta, Range, Reply, Root,
};
use crate::path::RelPath;

/// The machine's file system through `std::fs`: the folder and the local root at
/// two directories, and locks as OS file locks on files in the local root.
///
/// Symbolic links and special files are not library files: listings leave them
/// out, and naming one fails. On Linux, Android and Apple systems a rename refuses
/// an existing destination atomically; elsewhere it checks first, so a file
/// another process creates between the check and the rename is replaced. On
/// Windows a directory's names cannot be flushed, so `fsync` is not declared, and
/// names containing `\` or `:` are refused.
pub struct Native {
    folder: PathBuf,
    local: PathBuf,
    locks: BTreeMap<RelPath, File>,
}

const CAPABILITIES: Capabilities = Capabilities {
    append: true,
    rename_file: true,
    rename_dir: true,
    fsync: cfg!(unix),
};

impl Native {
    /// Touches nothing until the first request.
    pub fn new(folder: impl Into<PathBuf>, local: impl Into<PathBuf>) -> Self {
        Self {
            folder: folder.into(),
            local: local.into(),
            locks: BTreeMap::new(),
        }
    }

    fn full(&self, root: Root, path: &RelPath) -> Result<PathBuf, IoError> {
        let mut full = match root {
            Root::Folder => self.folder.clone(),
            Root::Local => self.local.clone(),
        };
        for name in path.components() {
            check_native_name(name)?;
            full.push(name);
        }
        Ok(full)
    }

    fn stat(&self, root: Root, path: &RelPath) -> Result<Option<Meta>, IoError> {
        let full = self.full(root, path)?;
        let found = match path.is_root() {
            true => fs::metadata(&full),
            false => fs::symlink_metadata(&full),
        };
        match found {
            Ok(metadata) => convert(&metadata).map(Some),
            Err(error) if error.kind() == ErrorKind::NotFound => Ok(None),
            Err(error) => Err(io_error(error)),
        }
    }

    fn file(&self, root: Root, path: &RelPath) -> Result<PathBuf, IoError> {
        match self.stat(root, path)?.map(|meta| meta.kind) {
            Some(Kind::File) => self.full(root, path),
            Some(Kind::Directory) => Err(IoError::IsDirectory),
            None => Err(IoError::NotFound),
        }
    }

    fn list(&self, root: Root, dir: &RelPath) -> Result<Vec<DirEntry>, IoError> {
        let mut entries = Vec::new();
        for entry in fs::read_dir(self.full(root, dir)?).map_err(io_error)? {
            let entry = entry.map_err(io_error)?;
            let kind = entry.file_type().map_err(io_error)?;
            let kind = match (kind.is_file(), kind.is_dir()) {
                (true, _) => Kind::File,
                (_, true) => Kind::Directory,
                _ => continue,
            };
            let name = entry
                .file_name()
                .into_string()
                .map_err(|_| IoError::Other("a name is not valid UTF-8".into()))?;
            entries.push(DirEntry { name, kind });
        }
        entries.sort_by(|a, b| a.name.cmp(&b.name));
        Ok(entries)
    }

    fn read(&self, root: Root, path: &RelPath, range: Range) -> Result<Vec<u8>, IoError> {
        let mut file = File::open(self.file(root, path)?).map_err(io_error)?;
        file.seek(SeekFrom::Start(range.offset)).map_err(io_error)?;
        let mut bytes = Vec::new();
        file.take(range.len)
            .read_to_end(&mut bytes)
            .map_err(io_error)?;
        Ok(bytes)
    }

    fn create(&self, root: Root, path: &RelPath, bytes: &[u8]) -> Result<(), IoError> {
        refuse_root(path)?;
        let full = self.full(root, path)?;
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&full)
            .map_err(io_error)?;
        if let Err(error) = file.write_all(bytes) {
            drop(file);
            let _ = fs::remove_file(&full);
            return Err(io_error(error));
        }
        Ok(())
    }

    fn append(&self, root: Root, path: &RelPath, bytes: &[u8]) -> Result<(), IoError> {
        let mut file = OpenOptions::new()
            .append(true)
            .open(self.file(root, path)?)
            .map_err(io_error)?;
        let len = file.metadata().map_err(io_error)?.len();
        if let Err(error) = file.write_all(bytes) {
            let _ = file.set_len(len);
            return Err(io_error(error));
        }
        Ok(())
    }

    fn rename(&self, root: Root, from: &RelPath, to: &RelPath) -> Result<(), IoError> {
        refuse_root(from)?;
        refuse_root(to)?;
        if self.stat(root, from)?.is_none() {
            return Err(IoError::NotFound);
        }
        if self.stat(root, to)?.is_some() {
            return Err(IoError::AlreadyExists);
        }
        if to.starts_with(from) {
            return Err(IoError::IntoItself);
        }
        rename_new(&self.full(root, from)?, &self.full(root, to)?).map_err(io_error)
    }

    fn remove(&self, root: Root, path: &RelPath, kind: Kind) -> Result<(), IoError> {
        refuse_root(path)?;
        let found = self.stat(root, path)?.ok_or(IoError::NotFound)?.kind;
        let full = self.full(root, path)?;
        match (found, kind) {
            (Kind::File, Kind::File) => fs::remove_file(full).map_err(io_error),
            (Kind::Directory, Kind::Directory) => fs::remove_dir(full).map_err(io_error),
            (Kind::Directory, Kind::File) => Err(IoError::IsDirectory),
            (Kind::File, Kind::Directory) => Err(IoError::NotDirectory),
        }
    }

    fn make_dir(&self, root: Root, path: &RelPath) -> Result<(), IoError> {
        let mut walked = RelPath::ROOT;
        for name in path.components() {
            walked = walked.join(name).map_err(|_| IoError::NotFound)?;
            match self.stat(root, &walked)?.map(|meta| meta.kind) {
                Some(Kind::Directory) => continue,
                Some(Kind::File) => return Err(IoError::NotDirectory),
                None => {}
            }
            if let Err(error) = fs::create_dir(self.full(root, &walked)?) {
                let made_elsewhere = error.kind() == ErrorKind::AlreadyExists
                    && self.stat(root, &walked)?.map(|meta| meta.kind) == Some(Kind::Directory);
                if !made_elsewhere {
                    return Err(io_error(error));
                }
            }
        }
        Ok(())
    }

    fn sync(&self, root: Root, path: &RelPath) -> Result<(), IoError> {
        let full = self.full(root, path)?;
        if self.stat(root, path)?.is_none() {
            return Err(IoError::NotFound);
        }
        match CAPABILITIES.fsync {
            true => File::open(full)
                .and_then(|file| file.sync_all())
                .map_err(io_error),
            false => Ok(()),
        }
    }

    fn lock(&mut self, name: RelPath) -> Result<Lock, IoError> {
        if self.locks.contains_key(&name) {
            return Ok(Lock::Acquired);
        }
        let file = OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(false)
            .open(self.full(Root::Local, &name)?)
            .map_err(io_error)?;
        match file.try_lock() {
            Ok(()) => {
                self.locks.insert(name, file);
                Ok(Lock::Acquired)
            }
            Err(TryLockError::WouldBlock) => Ok(Lock::Held),
            Err(TryLockError::Error(error)) => Err(io_error(error)),
        }
    }
}

impl Backend for Native {
    fn capabilities(&self, _: Root) -> Capabilities {
        CAPABILITIES
    }

    fn perform(&mut self, io: Io) -> IoResult {
        match io {
            Io::List { root, dir } => self.list(root, &dir).map(Reply::Listed),
            Io::Stat { root, path } => self.stat(root, &path).map(Reply::Stat),
            Io::Read { root, path, range } => self.read(root, &path, range).map(Reply::Bytes),
            Io::Create { root, path, bytes } => self.create(root, &path, &bytes).map(done),
            Io::Append { root, path, bytes } => self.append(root, &path, &bytes).map(done),
            Io::Rename { root, from, to } => self.rename(root, &from, &to).map(done),
            Io::Remove { root, path } => self.remove(root, &path, Kind::File).map(done),
            Io::RemoveDir { root, path } => self.remove(root, &path, Kind::Directory).map(done),
            Io::MakeDir { root, path } => self.make_dir(root, &path).map(done),
            Io::Sync { root, path } => self.sync(root, &path).map(done),
            Io::Lock { name } => self.lock(name).map(Reply::Lock),
            Io::Unlock { name } => {
                self.locks.remove(&name);
                Ok(Reply::Done)
            }
        }
    }
}

fn done(_: ()) -> Reply {
    Reply::Done
}

#[cfg(windows)]
fn check_native_name(name: &str) -> Result<(), IoError> {
    match name.contains(['\\', ':']) {
        true => Err(IoError::Other(
            "a name contains `\\` or `:`, which Windows reads as a path".into(),
        )),
        false => Ok(()),
    }
}

#[cfg(not(windows))]
fn check_native_name(_: &str) -> Result<(), IoError> {
    Ok(())
}

fn convert(metadata: &fs::Metadata) -> Result<Meta, IoError> {
    let kind = metadata.file_type();
    if kind.is_dir() {
        return Ok(Meta {
            kind: Kind::Directory,
            len: 0,
            modified: None,
        });
    }
    if !kind.is_file() {
        return Err(IoError::Other(
            "a symbolic link or special file is not a library file".into(),
        ));
    }
    let modified = metadata
        .modified()
        .ok()
        .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
        .and_then(|since| u64::try_from(since.as_nanos()).ok());
    Ok(Meta {
        kind: Kind::File,
        len: metadata.len(),
        modified,
    })
}

fn io_error(error: io::Error) -> IoError {
    match error.kind() {
        ErrorKind::NotFound => IoError::NotFound,
        ErrorKind::AlreadyExists => IoError::AlreadyExists,
        ErrorKind::IsADirectory => IoError::IsDirectory,
        ErrorKind::NotADirectory => IoError::NotDirectory,
        ErrorKind::DirectoryNotEmpty => IoError::NotEmpty,
        ErrorKind::StorageFull | ErrorKind::QuotaExceeded => IoError::NoSpace,
        _ => IoError::Other(error.to_string()),
    }
}

fn refuse_root(path: &RelPath) -> Result<(), IoError> {
    match path.is_root() {
        true => Err(IoError::Other(
            "the root itself cannot be created, moved or removed".into(),
        )),
        false => Ok(()),
    }
}

#[cfg(any(target_os = "linux", target_os = "android", target_vendor = "apple"))]
fn rename_new(from: &Path, to: &Path) -> io::Result<()> {
    use rustix::fs::{renameat_with, RenameFlags, CWD};
    renameat_with(CWD, from, CWD, to, RenameFlags::NOREPLACE).map_err(io::Error::from)
}

/// ⚠️ The check and the rename are separate steps.
#[cfg(not(any(target_os = "linux", target_os = "android", target_vendor = "apple")))]
fn rename_new(from: &Path, to: &Path) -> io::Result<()> {
    match fs::symlink_metadata(to) {
        Ok(_) => Err(ErrorKind::AlreadyExists.into()),
        Err(error) if error.kind() == ErrorKind::NotFound => fs::rename(from, to),
        Err(error) => Err(error),
    }
}
