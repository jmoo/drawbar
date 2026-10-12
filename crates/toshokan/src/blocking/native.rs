use std::cell::Cell;
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
/// out, and naming one fails. A root declares [`Capabilities::no_replace`] where
/// its volume's renames refuse an existing destination themselves, as probed
/// without writing: by file system type on Linux and Android, by the volume's
/// capabilities on Apple systems, and on every volume on Windows. Elsewhere a
/// rename may replace what another program made at the destination after the
/// core's check. On Windows a directory's names cannot be flushed, so `fsync` is
/// not declared, and names containing `\` or `:` are refused.
pub struct Native {
    folder: PathBuf,
    local: PathBuf,
    locks: BTreeMap<RelPath, File>,
    /// Each root's [`Capabilities::no_replace`], once a probe could tell.
    no_replace: [Cell<Option<bool>>; 2],
}

impl Native {
    /// Touches nothing until the first request.
    pub fn new(folder: impl Into<PathBuf>, local: impl Into<PathBuf>) -> Self {
        Self {
            folder: folder.into(),
            local: local.into(),
            locks: BTreeMap::new(),
            no_replace: Default::default(),
        }
    }

    fn top(&self, root: Root) -> &Path {
        match root {
            Root::Folder => &self.folder,
            Root::Local => &self.local,
        }
    }

    /// Whether renames on the volume of `root` refuse an existing destination
    /// themselves. A root that cannot be probed yet, such as one not made yet,
    /// is taken not to.
    fn no_replace(&self, root: Root) -> bool {
        let known = match root {
            Root::Folder => &self.no_replace[0],
            Root::Local => &self.no_replace[1],
        };
        if let Some(known) = known.get() {
            return known;
        }
        let probed = exclusive::probe(self.top(root));
        known.set(probed);
        probed.unwrap_or(false)
    }

    fn full(&self, root: Root, path: &RelPath) -> Result<PathBuf, IoError> {
        let mut full = self.top(root).to_path_buf();
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

    fn list_stat(&self, root: Root, dir: &RelPath) -> Result<Vec<(String, Meta)>, IoError> {
        let mut found = Vec::new();
        for entry in self.list(root, dir)? {
            let path = dir
                .join(&entry.name)
                .map_err(|error| IoError::Other(error.to_string()))?;
            if let Some(meta) = self.stat(root, &path)? {
                found.push((entry.name, meta));
            }
        }
        Ok(found)
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
        if to.starts_with(from) {
            return Err(IoError::IntoItself);
        }
        let (from, to) = (self.full(root, from)?, self.full(root, to)?);
        match self.no_replace(root) {
            true => exclusive::rename(&from, &to),
            false => fs::rename(&from, &to),
        }
        .map_err(io_error)
    }

    fn write(&self, root: Root, path: &RelPath, offset: u64, bytes: &[u8]) -> Result<(), IoError> {
        let mut file = OpenOptions::new()
            .write(true)
            .open(self.file(root, path)?)
            .map_err(io_error)?;
        file.seek(SeekFrom::Start(offset)).map_err(io_error)?;
        file.write_all(bytes).map_err(io_error)
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
        match FSYNC {
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

const FSYNC: bool = cfg!(unix);

impl Backend for Native {
    fn capabilities(&self, root: Root) -> Capabilities {
        Capabilities {
            append: true,
            rename_file: true,
            no_replace: self.no_replace(root),
            rename_dir: true,
            fsync: FSYNC,
        }
    }

    fn perform(&mut self, io: Io) -> IoResult {
        match io {
            Io::List { root, dir } => self.list(root, &dir).map(Reply::Listed),
            Io::Stat { root, path } => self.stat(root, &path).map(Reply::Stat),
            Io::ListStat { root, dir } => self.list_stat(root, &dir).map(Reply::ListedStat),
            Io::Read { root, path, range } => self.read(root, &path, range).map(Reply::Bytes),
            Io::ReadMany { root, reads } => Ok(Reply::ReadMany(
                reads
                    .iter()
                    .map(|(path, range)| self.read(root, path, *range))
                    .collect(),
            )),
            Io::Create { root, path, bytes } => self.create(root, &path, &bytes).map(done),
            Io::Append { root, path, bytes } => self.append(root, &path, &bytes).map(done),
            Io::Write {
                root,
                path,
                offset,
                bytes,
            } => self.write(root, &path, offset, &bytes).map(done),
            Io::Fill { .. } => Err(IoError::Other(crate::disk::FILLED_BY_DRIVERS.into())),
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

/// Renames that refuse an existing destination themselves, and whether a volume's
/// do.
#[cfg(any(target_os = "linux", target_os = "android"))]
mod exclusive {
    use std::io;
    use std::path::Path;

    /// The file systems whose renames take `RENAME_NOREPLACE`, by the magic
    /// numbers of `linux/magic.h`: ext2 to ext4, XFS, Btrfs, tmpfs, F2FS,
    /// bcachefs, overlayfs, FAT and exFAT. A volume not listed renames after
    /// checking the destination. A network file system's check is not atomic
    /// against other clients.
    const NO_REPLACE: [u32; 9] = [
        0xEF53,
        0x5846_5342,
        0x9123_683E,
        0x0102_1994,
        0xF2F5_2010,
        0xCA45_1A4E,
        0x794C_7630,
        0x4D44,
        0x2011_BAB0,
    ];

    pub fn probe(dir: &Path) -> Option<bool> {
        let volume = rustix::fs::statfs(dir).ok()?;
        #[allow(
            clippy::unnecessary_cast,
            clippy::cast_possible_truncation,
            clippy::cast_sign_loss,
            reason = "f_type's width varies by architecture; every magic fits 32 bits"
        )]
        let magic = volume.f_type as u32;
        Some(NO_REPLACE.contains(&magic))
    }

    pub fn rename(from: &Path, to: &Path) -> io::Result<()> {
        use rustix::fs::{renameat_with, RenameFlags, CWD};
        renameat_with(CWD, from, CWD, to, RenameFlags::NOREPLACE).map_err(io::Error::from)
    }
}

#[cfg(target_vendor = "apple")]
mod exclusive {
    use std::ffi::CString;
    use std::io;
    use std::path::Path;

    /// What `getattrlist` writes for `ATTR_VOL_CAPABILITIES`.
    #[repr(C)]
    struct Capabilities {
        length: u32,
        volume: libc::vol_capabilities_attr_t,
    }

    /// Whether the volume holding `dir` reports `VOL_CAP_INT_RENAME_EXCL`, which
    /// `renamex_np(2)` requires for `RENAME_EXCL`. Volume attributes are asked of
    /// the volume's mount point.
    pub fn probe(dir: &Path) -> Option<bool> {
        let volume = rustix::fs::statfs(dir).ok()?;
        let mount: Vec<u8> = volume
            .f_mntonname
            .iter()
            .take_while(|&&c| c != 0)
            .map(|&c| c.to_ne_bytes()[0])
            .collect();
        let mount = CString::new(mount).ok()?;
        let mut wanted = libc::attrlist {
            bitmapcount: libc::ATTR_BIT_MAP_COUNT,
            reserved: 0,
            commonattr: 0,
            volattr: libc::ATTR_VOL_INFO | libc::ATTR_VOL_CAPABILITIES,
            dirattr: 0,
            fileattr: 0,
            forkattr: 0,
        };
        let mut found = Capabilities {
            length: 0,
            volume: libc::vol_capabilities_attr_t {
                capabilities: [0; 4],
                valid: [0; 4],
            },
        };
        // SAFETY: `mount` is NUL-terminated, `wanted` is a valid attribute list
        // and `found` is a writable buffer of the size passed.
        let failed = unsafe {
            libc::getattrlist(
                mount.as_ptr(),
                std::ptr::from_mut(&mut wanted).cast(),
                std::ptr::from_mut(&mut found).cast(),
                std::mem::size_of::<Capabilities>(),
                0,
            )
        };
        if failed != 0 {
            return None;
        }
        let i = libc::VOL_CAPABILITIES_INTERFACES;
        let bit = libc::VOL_CAP_INT_RENAME_EXCL;
        Some(found.volume.valid[i] & found.volume.capabilities[i] & bit != 0)
    }

    pub fn rename(from: &Path, to: &Path) -> io::Result<()> {
        use rustix::fs::{renameat_with, RenameFlags, CWD};
        renameat_with(CWD, from, CWD, to, RenameFlags::NOREPLACE).map_err(io::Error::from)
    }
}

#[cfg(windows)]
mod exclusive {
    use std::io;
    use std::os::windows::ffi::OsStrExt;
    use std::path::Path;

    use windows_sys::Win32::Storage::FileSystem::MoveFileExW;

    /// `MoveFileExW` refuses an existing destination on every volume unless asked
    /// to replace it.
    pub fn probe(_: &Path) -> Option<bool> {
        Some(true)
    }

    pub fn rename(from: &Path, to: &Path) -> io::Result<()> {
        let wide =
            |path: &Path| -> Vec<u16> { path.as_os_str().encode_wide().chain([0]).collect() };
        let (from, to) = (wide(from), wide(to));
        // SAFETY: both paths are NUL-terminated UTF-16 that outlive the call.
        match unsafe { MoveFileExW(from.as_ptr(), to.as_ptr(), 0) } {
            0 => Err(io::Error::last_os_error()),
            _ => Ok(()),
        }
    }
}

#[cfg(not(any(
    target_os = "linux",
    target_os = "android",
    target_vendor = "apple",
    windows
)))]
mod exclusive {
    use std::io;
    use std::path::Path;

    pub fn probe(_: &Path) -> Option<bool> {
        Some(false)
    }

    pub fn rename(_: &Path, _: &Path) -> io::Result<()> {
        Err(io::ErrorKind::Unsupported.into())
    }
}
