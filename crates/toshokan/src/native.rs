//! The file system of the machine, through `std::fs`.

use std::fs::{self, File, OpenOptions};
use std::io::{self, ErrorKind, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

use crate::error::{Error, Result};
use crate::fs::{Capabilities, DirEntry, FileKind, Fs, Metadata, RelPath};

/// The library folder at `root` on the local file system.
///
/// Symbolic links and other special files are not library files: [`Fs::list`] leaves
/// them out, and naming one is an [`Error::Io`]. The library folder itself may be a
/// link.
///
/// **Limits.** On Linux, Android and Apple systems a rename refuses an existing
/// destination atomically. Elsewhere it checks for one first, so a file another
/// process creates between the check and the rename is replaced. On Windows,
/// [`Capabilities::fsync`] is not declared, because a directory's names cannot be
/// flushed, and names containing `\` or `:` are refused.
pub struct NativeFs {
    root: PathBuf,
}

impl NativeFs {
    /// Touches nothing until the first call.
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    fn full(&self, path: &RelPath) -> Result<PathBuf> {
        let mut full = self.root.clone();
        for name in path.components() {
            check_native_name(path, name)?;
            full.push(name);
        }
        Ok(full)
    }

    fn kind(&self, path: &RelPath) -> Result<Option<FileKind>> {
        Ok(self.stat(path)?.map(|metadata| metadata.kind))
    }

    fn stat(&self, path: &RelPath) -> Result<Option<Metadata>> {
        let full = self.full(path)?;
        let found = match path.is_root() {
            true => fs::metadata(&full),
            false => fs::symlink_metadata(&full),
        };
        match found {
            Ok(metadata) => convert(path, &metadata).map(Some),
            Err(error) if error.kind() == ErrorKind::NotFound => Ok(None),
            Err(error) => Err(io_error(path, error)),
        }
    }

    /// The full path of the existing file at `path`.
    fn file(&self, path: &RelPath) -> Result<PathBuf> {
        match self.kind(path)? {
            Some(FileKind::File) => self.full(path),
            Some(FileKind::Directory) => Err(Error::IsDirectory { path: path.clone() }),
            None => Err(Error::NotFound { path: path.clone() }),
        }
    }
}

#[cfg(windows)]
fn check_native_name(path: &RelPath, name: &str) -> Result<()> {
    match name.contains(['\\', ':']) {
        true => Err(Error::InvalidPath {
            path: path.as_str().to_owned(),
            reason: "a name contains `\\` or `:`, which Windows reads as a path",
        }),
        false => Ok(()),
    }
}

#[cfg(not(windows))]
fn check_native_name(_: &RelPath, _: &str) -> Result<()> {
    Ok(())
}

fn convert(path: &RelPath, metadata: &fs::Metadata) -> Result<Metadata> {
    let kind = metadata.file_type();
    if kind.is_dir() {
        return Ok(Metadata {
            kind: FileKind::Directory,
            len: 0,
            modified: None,
        });
    }
    if !kind.is_file() {
        return Err(Error::Io {
            path: path.clone(),
            source: io::Error::new(
                ErrorKind::InvalidInput,
                "a symbolic link or special file is not a library file",
            ),
        });
    }
    let modified = metadata
        .modified()
        .ok()
        .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
        .and_then(|since| u64::try_from(since.as_nanos()).ok());
    Ok(Metadata {
        kind: FileKind::File,
        len: metadata.len(),
        modified,
    })
}

fn io_error(path: &RelPath, source: io::Error) -> Error {
    let path = path.clone();
    match source.kind() {
        ErrorKind::NotFound => Error::NotFound { path },
        ErrorKind::AlreadyExists => Error::AlreadyExists { path },
        ErrorKind::IsADirectory => Error::IsDirectory { path },
        ErrorKind::NotADirectory => Error::NotDirectory { path },
        ErrorKind::DirectoryNotEmpty => Error::DirectoryNotEmpty { path },
        ErrorKind::StorageFull | ErrorKind::QuotaExceeded => Error::NoSpace { path },
        _ => Error::Io { path, source },
    }
}

fn refuse_root(path: &RelPath) -> Result<()> {
    match path.is_root() {
        true => Err(Error::InvalidPath {
            path: String::new(),
            reason: "the library folder itself cannot be created, moved or removed",
        }),
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

const CAPABILITIES: Capabilities = Capabilities {
    append: true,
    rename_file: true,
    rename_dir: true,
    hard_link: true,
    exclusive_create: true,
    fsync: cfg!(unix),
};

impl Fs for NativeFs {
    fn capabilities(&self) -> Capabilities {
        CAPABILITIES
    }

    async fn metadata(&self, path: &RelPath) -> Result<Option<Metadata>> {
        self.stat(path)
    }

    async fn list(&self, dir: &RelPath) -> Result<Vec<DirEntry>> {
        match self.kind(dir)? {
            Some(FileKind::Directory) => {}
            Some(FileKind::File) => return Err(Error::NotDirectory { path: dir.clone() }),
            None => return Err(Error::NotFound { path: dir.clone() }),
        }
        let mut entries = Vec::new();
        for entry in fs::read_dir(self.full(dir)?).map_err(|error| io_error(dir, error))? {
            let entry = entry.map_err(|error| io_error(dir, error))?;
            let kind = entry.file_type().map_err(|error| io_error(dir, error))?;
            let kind = match (kind.is_file(), kind.is_dir()) {
                (true, _) => FileKind::File,
                (_, true) => FileKind::Directory,
                _ => continue,
            };
            let name = entry.file_name().into_string().map_err(|name| {
                let name = name.to_string_lossy();
                Error::InvalidPath {
                    path: dir.join(&name).map_or(name.into_owned(), |p| p.to_string()),
                    reason: "a name is not valid UTF-8",
                }
            })?;
            entries.push(DirEntry { name, kind });
        }
        entries.sort_by(|a, b| a.name.cmp(&b.name));
        Ok(entries)
    }

    async fn read(&self, path: &RelPath) -> Result<Vec<u8>> {
        fs::read(self.file(path)?).map_err(|error| io_error(path, error))
    }

    async fn read_at(&self, path: &RelPath, offset: u64, len: usize) -> Result<Vec<u8>> {
        let full = self.file(path)?;
        let read = || {
            let mut file = File::open(full)?;
            file.seek(SeekFrom::Start(offset))?;
            let mut bytes = Vec::new();
            file.take(u64::try_from(len).unwrap_or(u64::MAX))
                .read_to_end(&mut bytes)?;
            Ok(bytes)
        };
        read().map_err(|error| io_error(path, error))
    }

    async fn create_dir_all(&self, dir: &RelPath) -> Result<()> {
        let mut walked = RelPath::ROOT;
        for name in dir.components() {
            walked = walked.join(name)?;
            match self.kind(&walked)? {
                Some(FileKind::Directory) => continue,
                Some(FileKind::File) => return Err(Error::NotDirectory { path: walked }),
                None => {}
            }
            if let Err(error) = fs::create_dir(self.full(&walked)?) {
                let made_elsewhere = error.kind() == ErrorKind::AlreadyExists
                    && self.kind(&walked)? == Some(FileKind::Directory);
                if !made_elsewhere {
                    return Err(io_error(&walked, error));
                }
            }
        }
        Ok(())
    }

    async fn create(&self, path: &RelPath, bytes: &[u8]) -> Result<()> {
        refuse_root(path)?;
        let full = self.full(path)?;
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&full)
            .map_err(|error| io_error(path, error))?;
        if let Err(error) = file.write_all(bytes) {
            drop(file);
            let _ = fs::remove_file(&full);
            return Err(io_error(path, error));
        }
        Ok(())
    }

    async fn append(&self, path: &RelPath, bytes: &[u8]) -> Result<()> {
        let full = self.file(path)?;
        let mut file = OpenOptions::new()
            .append(true)
            .open(full)
            .map_err(|error| io_error(path, error))?;
        let len = file
            .metadata()
            .map_err(|error| io_error(path, error))?
            .len();
        if let Err(error) = file.write_all(bytes) {
            let _ = file.set_len(len);
            return Err(io_error(path, error));
        }
        Ok(())
    }

    async fn rename(&self, from: &RelPath, to: &RelPath) -> Result<()> {
        refuse_root(from)?;
        refuse_root(to)?;
        if self.kind(from)?.is_none() {
            return Err(Error::NotFound { path: from.clone() });
        }
        if to == from {
            return Err(Error::AlreadyExists { path: to.clone() });
        }
        if to.starts_with(from) {
            return Err(Error::InvalidPath {
                path: to.as_str().to_owned(),
                reason: "a directory cannot move inside itself",
            });
        }
        rename_new(&self.full(from)?, &self.full(to)?).map_err(|error| io_error(to, error))
    }

    async fn hard_link(&self, from: &RelPath, to: &RelPath) -> Result<()> {
        refuse_root(to)?;
        fs::hard_link(self.file(from)?, self.full(to)?).map_err(|error| io_error(to, error))
    }

    async fn remove_file(&self, path: &RelPath) -> Result<()> {
        refuse_root(path)?;
        fs::remove_file(self.file(path)?).map_err(|error| io_error(path, error))
    }

    async fn remove_dir(&self, path: &RelPath) -> Result<()> {
        refuse_root(path)?;
        match self.kind(path)? {
            Some(FileKind::Directory) => {}
            Some(FileKind::File) => return Err(Error::NotDirectory { path: path.clone() }),
            None => return Err(Error::NotFound { path: path.clone() }),
        }
        fs::remove_dir(self.full(path)?).map_err(|error| io_error(path, error))
    }

    async fn sync(&self, path: &RelPath) -> Result<()> {
        if self.kind(path)?.is_none() {
            return Err(Error::NotFound { path: path.clone() });
        }
        match CAPABILITIES.fsync {
            true => File::open(self.full(path)?)
                .and_then(|file| file.sync_all())
                .map_err(|error| io_error(path, error)),
            false => Ok(()),
        }
    }
}
