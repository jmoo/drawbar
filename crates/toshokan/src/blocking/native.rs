use std::cell::Cell;
use std::collections::BTreeMap;
#[cfg(not(windows))]
use std::fs::TryLockError;
use std::fs::{self, File, OpenOptions};
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
/// out, and naming one fails, as does any request on a path through a
/// directory that is a link. What a root's renames promise is probed without
/// writing, from its volume's file system: by type on Linux and Android, by type
/// and the volume's capabilities on Apple systems, and by name on Windows. A
/// volume that keeps no journal of its names, such as FAT or exFAT, declares no
/// [`Capabilities::rename_file`] or [`Capabilities::rename_dir`], so that every
/// move copies, and one whose renames refuse an existing destination declares
/// [`Capabilities::no_replace`], as every other volume on Windows does. On
/// Windows names containing `\` or `:` are refused, and a request another
/// program holds the file of for a moment is tried again. A create or rename
/// through a file fails as [`IoError::NotDirectory`] everywhere; other requests
/// there fail as [`IoError::NotFound`] on Windows. The local root's
/// directory, and those it is in, are made where missing at its first change or
/// lock.
pub struct Native {
    folder: PathBuf,
    local: PathBuf,
    /// Whether the local root's directory is known to be there.
    local_made: bool,
    locks: BTreeMap<RelPath, File>,
    /// Each root's [`Renames`], once a probe could tell.
    renames: [Cell<Option<Renames>>; 2],
}

/// What a volume's renames promise across a power loss.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Renames {
    /// Atomic, and refusing an existing destination in the same step:
    /// [`Capabilities::no_replace`].
    Exclusive,
    /// Atomic; a rename may replace what another program made at the
    /// destination after the core's check.
    #[cfg_attr(windows, expect(dead_code, reason = "a Windows rename never replaces"))]
    Replacing,
    /// Not atomic: the volume keeps no journal of its names, and a crash can keep
    /// a rename's removal of the old name without the new one. No rename is
    /// declared, so every move copies.
    Torn,
}

impl Native {
    /// Touches nothing until the first request.
    pub fn new(folder: impl Into<PathBuf>, local: impl Into<PathBuf>) -> Self {
        Self {
            folder: folder.into(),
            local: local.into(),
            local_made: false,
            locks: BTreeMap::new(),
            renames: Default::default(),
        }
    }

    fn top(&self, root: Root) -> &Path {
        match root {
            Root::Folder => &self.folder,
            Root::Local => &self.local,
        }
    }

    /// What renames on the volume of `root` promise, probed at the nearest
    /// directory that exists, the root or one above it. A root no probe can tell
    /// about is taken to tear renames: a wrong guess then costs copies, not files.
    fn renames(&self, root: Root) -> Renames {
        let known = match root {
            Root::Folder => &self.renames[0],
            Root::Local => &self.renames[1],
        };
        if let Some(known) = known.get() {
            return known;
        }
        let made = self.top(root).ancestors().find(|dir| dir.exists());
        let probed = made.and_then(volume::probe);
        known.set(probed);
        probed.unwrap_or(Renames::Torn)
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

    /// `error`, which a request at `path` failed with, as Unix says it: a file in the
    /// way of a directory `path` is in, which Windows reports as nothing there, is
    /// [`IoError::NotDirectory`].
    fn in_the_way(&self, root: Root, path: &RelPath, error: IoError) -> IoError {
        if error != IoError::NotFound {
            return error;
        }
        let mut above = path.parent();
        while let Some(dir) = above.filter(|dir| !dir.is_root()) {
            if let Ok(Some(Meta {
                kind: Kind::File, ..
            })) = self.stat(root, &dir)
            {
                return IoError::NotDirectory;
            }
            above = dir.parent();
        }
        error
    }

    fn file(&self, root: Root, path: &RelPath) -> Result<PathBuf, IoError> {
        match self.stat(root, path)?.map(|meta| meta.kind) {
            Some(Kind::File) => self.full(root, path),
            Some(Kind::Directory) => Err(IoError::IsDirectory),
            None => Err(IoError::NotFound),
        }
    }

    /// The files and directories in `dir`, by name, each with its entry.
    fn entries(
        &self,
        root: Root,
        dir: &RelPath,
    ) -> Result<Vec<(String, Kind, fs::DirEntry)>, IoError> {
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
            entries.push((name, kind, entry));
        }
        entries.sort_by(|a, b| a.0.cmp(&b.0));
        Ok(entries)
    }

    fn list(&self, root: Root, dir: &RelPath) -> Result<Vec<DirEntry>, IoError> {
        let entries = self.entries(root, dir)?.into_iter();
        Ok(entries
            .map(|(name, kind, _)| DirEntry { name, kind })
            .collect())
    }

    /// Each entry's metadata from its listing, so Windows, whose listing carries it,
    /// opens no file. ⚠️ There, the size and time of a file another program holds open
    /// can lag until it closes the file.
    fn list_stat(&self, root: Root, dir: &RelPath) -> Result<Vec<(String, Meta)>, IoError> {
        let mut found = Vec::new();
        for (name, _, entry) in self.entries(root, dir)? {
            match entry.metadata() {
                Ok(metadata) => found.push((name, convert(&metadata)?)),
                Err(error) if error.kind() == ErrorKind::NotFound => {}
                Err(error) => return Err(io_error(error)),
            }
        }
        Ok(found)
    }

    fn read(&self, root: Root, path: &RelPath, range: Range) -> Result<Vec<u8>, IoError> {
        let mut file = File::open(self.file(root, path)?).map_err(io_error)?;
        let len = file.metadata().map_err(io_error)?.len();
        let wanted = len.saturating_sub(range.offset).min(range.len);
        file.seek(SeekFrom::Start(range.offset)).map_err(io_error)?;
        // A read the size of what the file holds, rather than twice it as a growing
        // buffer would allocate.
        let mut bytes = Vec::with_capacity(usize::try_from(wanted).unwrap_or(0));
        file.take(range.len)
            .read_to_end(&mut bytes)
            .map_err(io_error)?;
        Ok(bytes)
    }

    fn create(&self, root: Root, path: &RelPath, bytes: &[u8]) -> Result<(), IoError> {
        refuse_root(path)?;
        let full = self.full(root, path)?;
        let open = || OpenOptions::new().write(true).create_new(true).open(&full);
        let mut file =
            patiently(open).map_err(|error| self.in_the_way(root, path, io_error(error)))?;
        if let Err(error) = file.write_all(bytes) {
            drop(file);
            let _ = fs::remove_file(&full);
            return Err(io_error(error));
        }
        Ok(())
    }

    fn append(&self, root: Root, path: &RelPath, bytes: &[u8]) -> Result<(), IoError> {
        let full = self.file(root, path)?;
        let open = || OpenOptions::new().append(true).open(&full);
        let mut file = patiently(open).map_err(io_error)?;
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
        let (source, destination) = (self.full(root, from)?, self.full(root, to)?);
        let renames = self.renames(root);
        patiently(|| match renames {
            Renames::Exclusive => volume::rename(&source, &destination),
            Renames::Replacing | Renames::Torn => fs::rename(&source, &destination),
        })
        .map_err(|error| self.in_the_way(root, to, io_error(error)))
    }

    fn write(&self, root: Root, path: &RelPath, offset: u64, bytes: &[u8]) -> Result<(), IoError> {
        let full = self.file(root, path)?;
        let mut file =
            patiently(|| OpenOptions::new().write(true).open(&full)).map_err(io_error)?;
        file.seek(SeekFrom::Start(offset)).map_err(io_error)?;
        file.write_all(bytes).map_err(io_error)
    }

    fn remove(&self, root: Root, path: &RelPath, kind: Kind) -> Result<(), IoError> {
        refuse_root(path)?;
        let found = self.stat(root, path)?.ok_or(IoError::NotFound)?.kind;
        let full = self.full(root, path)?;
        match (found, kind) {
            (Kind::File, Kind::File) => patiently(|| fs::remove_file(&full)).map_err(io_error),
            (Kind::Directory, Kind::Directory) => {
                patiently(|| fs::remove_dir(&full)).map_err(io_error)
            }
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
        let kind = self.stat(root, path)?.ok_or(IoError::NotFound)?.kind;
        patiently(|| flush(&full, kind)).map_err(io_error)
    }

    fn lock(&mut self, name: RelPath) -> Result<Lock, IoError> {
        if self.locks.contains_key(&name) {
            return Ok(Lock::Acquired);
        }
        let full = self.full(Root::Local, &name)?;
        let open = || {
            OpenOptions::new()
                .write(true)
                .create(true)
                .truncate(false)
                .open(&full)
        };
        let file = patiently(open).map_err(io_error)?;
        match try_lock(&file).map_err(io_error)? {
            true => {
                self.locks.insert(name, file);
                Ok(Lock::Acquired)
            }
            false => Ok(Lock::Held),
        }
    }
}

/// Takes the lock of `file` without waiting; `false` where another process holds it.
/// What the file holds stays readable and writable through other handles.
#[cfg(not(windows))]
fn try_lock(file: &File) -> io::Result<bool> {
    match file.try_lock() {
        Ok(()) => Ok(true),
        Err(TryLockError::WouldBlock) => Ok(false),
        Err(TryLockError::Error(error)) => Err(error),
    }
}

/// Takes the lock of `file` without waiting; `false` where another process holds it.
/// What the file holds stays readable and writable through other handles.
// A Windows lock bars every other handle from the bytes it covers, so this one
// covers a byte far past the end of any file.
#[cfg(windows)]
fn try_lock(file: &File) -> io::Result<bool> {
    use std::os::windows::io::AsRawHandle;
    use windows_sys::Win32::Foundation::ERROR_LOCK_VIOLATION;
    use windows_sys::Win32::Storage::FileSystem::{
        LockFileEx, LOCKFILE_EXCLUSIVE_LOCK, LOCKFILE_FAIL_IMMEDIATELY,
    };
    use windows_sys::Win32::System::IO::{OVERLAPPED, OVERLAPPED_0, OVERLAPPED_0_0};

    let mut at = OVERLAPPED {
        Internal: 0,
        InternalHigh: 0,
        Anonymous: OVERLAPPED_0 {
            Anonymous: OVERLAPPED_0_0 {
                Offset: 0,
                OffsetHigh: 0x7fff_ffff,
            },
        },
        hEvent: std::ptr::null_mut(),
    };
    let flags = LOCKFILE_EXCLUSIVE_LOCK | LOCKFILE_FAIL_IMMEDIATELY;
    // SAFETY: the handle is open for the call, and `at` outlives it: a lock asked to
    // fail at once completes before the call returns.
    let locked = unsafe { LockFileEx(file.as_raw_handle(), flags, 0, 1, 0, &mut at) };
    if locked != 0 {
        return Ok(true);
    }
    let error = io::Error::last_os_error();
    match error.raw_os_error() == Some(ERROR_LOCK_VIOLATION as i32) {
        true => Ok(false),
        false => Err(error),
    }
}

const FSYNC: bool = cfg!(any(unix, windows));

/// Makes what `path`, a `kind`, holds durable: a file's bytes, a directory's names.
#[cfg(unix)]
fn flush(path: &Path, _: Kind) -> io::Result<()> {
    File::open(path)?.sync_all()
}

/// Makes what `path`, a `kind`, holds durable: a file's bytes, a directory's names.
#[cfg(windows)]
fn flush(path: &Path, kind: Kind) -> io::Result<()> {
    use windows_sys::Win32::Foundation::{
        ERROR_ACCESS_DENIED, ERROR_INVALID_FUNCTION, ERROR_NOT_SUPPORTED,
    };

    const REFUSED: [u32; 3] = [
        ERROR_ACCESS_DENIED,
        ERROR_INVALID_FUNCTION,
        ERROR_NOT_SUPPORTED,
    ];
    match writable(path, kind).and_then(|file| file.sync_all()) {
        // A file's flush, and a rename written through, flush the volume's log
        // through every earlier change of names, so a volume that cannot flush a
        // directory keeps its names all the same. Reported by public documentation;
        // not confirmed on hardware.
        Err(error)
            if kind == Kind::Directory
                && error
                    .raw_os_error()
                    .is_some_and(|code| REFUSED.iter().any(|&refused| refused as i32 == code)) =>
        {
            Ok(())
        }
        flushed => flushed,
    }
}

/// `path`, a `kind`, opened so that it can be flushed: Windows flushes only through a
/// handle that may write, and opens a directory only for backup semantics.
#[cfg(windows)]
fn writable(path: &Path, kind: Kind) -> io::Result<File> {
    use std::os::windows::fs::OpenOptionsExt;
    use windows_sys::Win32::Storage::FileSystem::FILE_FLAG_BACKUP_SEMANTICS;

    let mut open = OpenOptions::new();
    open.write(true);
    if kind == Kind::Directory {
        open.custom_flags(FILE_FLAG_BACKUP_SEMANTICS);
    }
    open.open(path)
}

#[cfg(not(any(unix, windows)))]
fn flush(_: &Path, _: Kind) -> io::Result<()> {
    Ok(())
}

/// Runs `op` again, for about half a second, while another program holds the file
/// it names, as antivirus and indexing do for a moment after a file is written. A
/// refusal that lasts, as of a read-only folder, is said that much later.
#[cfg(windows)]
fn patiently<T>(mut op: impl FnMut() -> io::Result<T>) -> io::Result<T> {
    use windows_sys::Win32::Foundation::{
        ERROR_ACCESS_DENIED, ERROR_LOCK_VIOLATION, ERROR_SHARING_VIOLATION,
    };

    const HELD: [u32; 3] = [
        ERROR_SHARING_VIOLATION,
        ERROR_LOCK_VIOLATION,
        ERROR_ACCESS_DENIED,
    ];
    for wait in [10, 40, 150, 400] {
        match op() {
            Err(error)
                if error
                    .raw_os_error()
                    .is_some_and(|code| HELD.iter().any(|&held| held as i32 == code)) =>
            {
                std::thread::sleep(std::time::Duration::from_millis(wait));
            }
            done => return done,
        }
    }
    op()
}

#[cfg(not(windows))]
fn patiently<T>(mut op: impl FnMut() -> io::Result<T>) -> io::Result<T> {
    op()
}

impl Backend for Native {
    fn capabilities(&self, root: Root) -> Capabilities {
        let renames = self.renames(root);
        Capabilities {
            append: true,
            rename_file: renames != Renames::Torn,
            no_replace: renames == Renames::Exclusive,
            rename_dir: renames != Renames::Torn,
            fsync: FSYNC,
        }
    }

    fn perform(&mut self, io: Io) -> IoResult {
        let local = io.root() == Root::Local && (io.mutates() || matches!(io, Io::Lock { .. }));
        if !local {
            return self.request(io);
        }
        if !self.local_made {
            fs::create_dir_all(&self.local).map_err(io_error)?;
            self.local_made = true;
        }
        let result = self.request(io.clone());
        if result != Err(IoError::NotFound) || self.local.is_dir() {
            return result;
        }
        fs::create_dir_all(&self.local).map_err(io_error)?;
        self.request(io)
    }
}

impl Native {
    /// Refuses a request on a path whose directories include a link, which would
    /// reach outside the root, as is a directory listed that is one. A link made
    /// between this look and the request is still followed.
    fn unlinked(&self, io: &Io) -> Result<(), IoError> {
        /// `path`, and how many of its names are directories a request on it passes.
        fn within(path: &RelPath) -> (&RelPath, usize) {
            (path, path.components().count().saturating_sub(1))
        }
        let paths: Vec<(&RelPath, usize)> = match io {
            Io::Read { path, .. }
            | Io::Create { path, .. }
            | Io::Append { path, .. }
            | Io::Write { path, .. }
            | Io::Remove { path, .. }
            | Io::RemoveDir { path, .. }
            | Io::Sync { path, .. }
            | Io::Stat { path, .. }
            | Io::MakeDir { path, .. } => vec![within(path)],
            Io::List { dir, .. } | Io::ListStat { dir, .. } => {
                vec![(dir, dir.components().count())]
            }
            Io::ReadMany { reads, .. } => reads.iter().map(|(path, _)| within(path)).collect(),
            Io::Rename { from, to, .. } => vec![within(from), within(to)],
            _ => Vec::new(),
        };
        let top = self.top(io.root());
        for (path, dirs) in paths {
            let mut at = top.to_path_buf();
            for name in path.components().take(dirs) {
                at.push(name);
                match fs::symlink_metadata(&at) {
                    Ok(meta) if meta.file_type().is_symlink() => {
                        return Err(IoError::Other(
                            "a directory on the way is a symbolic link".into(),
                        ));
                    }
                    Ok(_) => {}
                    Err(_) => break,
                }
            }
        }
        Ok(())
    }

    fn request(&mut self, io: Io) -> IoResult {
        self.unlinked(&io)?;
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
        ErrorKind::PermissionDenied => IoError::Denied,
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

/// What a volume's renames promise, and renames that refuse an existing
/// destination themselves.
#[cfg(any(target_os = "linux", target_os = "android"))]
mod volume {
    use std::io;
    use std::path::Path;

    use super::Renames;

    /// File systems by the magic number `statfs` reports, from `linux/magic.h`
    /// or the driver's source. A file system not listed is taken to rename
    /// atomically and replace, and a network file system's check is not atomic
    /// against other clients. ⚠️ ext2, and ext4 made without a journal, share
    /// ext4's number and are taken to rename atomically. A FUSE file system,
    /// such as ntfs-3g, exfat-fuse or sshfs, hides what it stores to, and is
    /// taken to tear renames.
    pub const VOLUMES: [(&str, u32, Renames); 14] = [
        ("ext2 to ext4", 0xEF53, Renames::Exclusive),
        ("XFS", 0x5846_5342, Renames::Exclusive),
        ("Btrfs", 0x9123_683E, Renames::Exclusive),
        ("tmpfs", 0x0102_1994, Renames::Exclusive),
        ("F2FS", 0xF2F5_2010, Renames::Exclusive),
        ("bcachefs", 0xCA45_1A4E, Renames::Exclusive),
        ("overlayfs", 0x794C_7630, Renames::Exclusive),
        ("FAT", 0x4D44, Renames::Torn),
        ("exFAT", 0x2011_BAB0, Renames::Torn),
        ("NTFS", 0x7366_746E, Renames::Torn),
        ("HFS", 0x4244, Renames::Torn),
        ("HFS+", 0x482B, Renames::Torn),
        ("UDF", 0x1501_3346, Renames::Torn),
        ("FUSE", 0x6573_5546, Renames::Torn),
    ];

    pub fn renames(magic: u32) -> Renames {
        VOLUMES
            .iter()
            .find(|(_, known, _)| *known == magic)
            .map_or(Renames::Replacing, |&(_, _, renames)| renames)
    }

    pub fn probe(dir: &Path) -> Option<Renames> {
        let volume = rustix::fs::statfs(dir).ok()?;
        #[allow(
            clippy::unnecessary_cast,
            clippy::cast_possible_truncation,
            clippy::cast_sign_loss,
            reason = "f_type's width varies by architecture; every magic fits 32 bits"
        )]
        let magic = volume.f_type as u32;
        Some(renames(magic))
    }

    pub fn rename(from: &Path, to: &Path) -> io::Result<()> {
        use rustix::fs::{renameat_with, RenameFlags, CWD};
        renameat_with(CWD, from, CWD, to, RenameFlags::NOREPLACE).map_err(io::Error::from)
    }
}

#[cfg(target_vendor = "apple")]
mod volume {
    use std::ffi::{CStr, CString};
    use std::io;
    use std::path::Path;

    use super::Renames;

    /// File systems by the type `statfs` names, whose renames are torn whatever
    /// the volume reports: exFAT, FAT, NTFS through Apple's driver, Paragon's
    /// and Tuxera's, UDF, and FUSE file systems, which hide what they store to.
    /// exFAT and FAT volumes report no `RENAME_EXCL`, and no journal.
    pub const TORN: [&str; 8] = [
        "exfat",
        "msdos",
        "ntfs",
        "ufsd_NTFS",
        "tuxera_ntfs",
        "udf",
        "macfuse",
        "osxfuse",
    ];

    /// What renames on a volume of type `name` promise, where its capabilities
    /// include `VOL_CAP_INT_RENAME_EXCL` or not.
    pub fn renames(name: &str, exclusive: bool) -> Renames {
        match (TORN.contains(&name), exclusive) {
            (true, _) => Renames::Torn,
            (false, true) => Renames::Exclusive,
            (false, false) => Renames::Replacing,
        }
    }

    /// What `getattrlist` writes for `ATTR_VOL_CAPABILITIES`.
    #[repr(C)]
    struct Capabilities {
        length: u32,
        volume: libc::vol_capabilities_attr_t,
    }

    /// By the type of the volume holding `dir`, and whether it reports
    /// `VOL_CAP_INT_RENAME_EXCL`, which `renamex_np(2)` requires for
    /// `RENAME_EXCL`. Volume attributes are asked of the volume's mount point.
    pub fn probe(dir: &Path) -> Option<Renames> {
        let volume = rustix::fs::statfs(dir).ok()?;
        let name = c_text(&volume.f_fstypename)?;
        let mount = CString::new(c_text(&volume.f_mntonname)?).ok()?;
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
        let exclusive = found.volume.valid[i] & found.volume.capabilities[i] & bit != 0;
        Some(renames(&name, exclusive))
    }

    /// The text of a NUL-terminated field `statfs` fills.
    fn c_text(field: &[libc::c_char]) -> Option<String> {
        let bytes: Vec<u8> = field.iter().map(|&c| c.to_ne_bytes()[0]).collect();
        let text = CStr::from_bytes_until_nul(&bytes).ok()?;
        text.to_str().ok().map(str::to_owned)
    }

    pub fn rename(from: &Path, to: &Path) -> io::Result<()> {
        use rustix::fs::{renameat_with, RenameFlags, CWD};
        renameat_with(CWD, from, CWD, to, RenameFlags::NOREPLACE).map_err(io::Error::from)
    }
}

#[cfg(windows)]
mod volume {
    use std::fs::OpenOptions;
    use std::io;
    use std::os::windows::ffi::OsStrExt;
    use std::os::windows::fs::OpenOptionsExt;
    use std::os::windows::io::AsRawHandle;
    use std::path::Path;

    use super::Renames;

    use windows_sys::Win32::Storage::FileSystem::{
        GetVolumeInformationByHandleW, MoveFileExW, FILE_FLAG_BACKUP_SEMANTICS,
        MOVEFILE_WRITE_THROUGH,
    };

    /// File systems by the name Windows gives them, whose renames are torn.
    pub const TORN: [&str; 4] = ["FAT", "FAT32", "exFAT", "UDF"];

    /// What renames on a volume whose file system Windows names `name` promise.
    /// `MoveFileExW` refuses an existing destination on every volume unless asked
    /// to replace it.
    pub fn renames(name: &str) -> Renames {
        match TORN.iter().any(|torn| torn.eq_ignore_ascii_case(name)) {
            true => Renames::Torn,
            false => Renames::Exclusive,
        }
    }

    pub fn probe(dir: &Path) -> Option<Renames> {
        // A directory opens only for backup semantics.
        let dir = OpenOptions::new()
            .read(true)
            .custom_flags(FILE_FLAG_BACKUP_SEMANTICS)
            .open(dir)
            .ok()?;
        let mut name = [0u16; 64];
        // SAFETY: the handle is open for the call, and `name` is a writable
        // buffer of the length passed; the other outputs are not asked for.
        let found = unsafe {
            GetVolumeInformationByHandleW(
                dir.as_raw_handle(),
                std::ptr::null_mut(),
                0,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                name.as_mut_ptr(),
                u32::try_from(name.len()).unwrap_or(0),
            )
        };
        if found == 0 {
            return None;
        }
        let len = name.iter().position(|&c| c == 0).unwrap_or(name.len());
        Some(renames(&String::from_utf16(&name[..len]).ok()?))
    }

    /// Renames `from` to `to`, refusing an existing `to`, and returns once the
    /// rename is on the disk.
    pub fn rename(from: &Path, to: &Path) -> io::Result<()> {
        let (from, to) = (verbatim(from)?, verbatim(to)?);
        // SAFETY: both paths are NUL-terminated UTF-16 that outlive the call.
        match unsafe { MoveFileExW(from.as_ptr(), to.as_ptr(), MOVEFILE_WRITE_THROUGH) } {
            0 => Err(io::Error::last_os_error()),
            _ => Ok(()),
        }
    }

    /// `path` as Windows reads it past `MAX_PATH`, as `std::fs` passes every path:
    /// absolute, under the verbatim prefix, NUL-terminated.
    pub fn verbatim(path: &Path) -> io::Result<Vec<u16>> {
        let absolute: Vec<u16> = std::path::absolute(path)?
            .as_os_str()
            .encode_wide()
            .collect();
        let wide = |text: &str| text.encode_utf16().collect::<Vec<u16>>();
        let prefixed =
            if absolute.starts_with(&wide(r"\\?\")) || absolute.starts_with(&wide(r"\\.\")) {
                absolute
            } else if let Some(share) = absolute.strip_prefix(wide(r"\\").as_slice()) {
                [wide(r"\\?\UNC\").as_slice(), share].concat()
            } else {
                [wide(r"\\?\").as_slice(), &absolute].concat()
            };
        Ok(prefixed.into_iter().chain([0]).collect())
    }
}

#[cfg(not(any(
    target_os = "linux",
    target_os = "android",
    target_vendor = "apple",
    windows
)))]
mod volume {
    use std::io;
    use std::path::Path;

    use super::Renames;

    pub fn probe(_: &Path) -> Option<Renames> {
        Some(Renames::Replacing)
    }

    pub fn rename(_: &Path, _: &Path) -> io::Result<()> {
        Err(io::ErrorKind::Unsupported.into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(any(target_os = "linux", target_os = "android"))]
    #[test]
    fn volumes_without_a_journal_of_their_names_move_by_copying_on_linux() {
        // The magic numbers of `linux/magic.h`, and ntfs3's own.
        for (name, magic) in [
            ("FAT", 0x4D44),
            ("exFAT", 0x2011_BAB0),
            ("ntfs3", 0x7366_746E),
            ("HFS", 0x4244),
            ("FUSE", 0x6573_5546),
        ] {
            assert_eq!(volume::renames(magic), Renames::Torn, "{name}");
        }
        for (name, magic) in [
            ("ext4", 0xEF53),
            ("XFS", 0x5846_5342),
            ("Btrfs", 0x9123_683E),
        ] {
            assert_eq!(volume::renames(magic), Renames::Exclusive, "{name}");
        }
        assert_eq!(volume::renames(0x6969), Renames::Replacing, "NFS");
    }

    #[cfg(any(target_os = "linux", target_os = "android"))]
    #[test]
    fn each_linux_file_system_is_listed_once() {
        let magics: std::collections::BTreeSet<u32> =
            volume::VOLUMES.iter().map(|&(_, magic, _)| magic).collect();
        assert_eq!(magics.len(), volume::VOLUMES.len());
    }

    #[cfg(target_vendor = "apple")]
    #[test]
    fn exfat_and_fat_volumes_move_by_copying_whatever_they_report_on_apple_systems() {
        for name in ["exfat", "msdos"] {
            assert_eq!(volume::renames(name, true), Renames::Torn, "{name}");
            assert_eq!(volume::renames(name, false), Renames::Torn, "{name}");
        }
        assert_eq!(volume::renames("apfs", true), Renames::Exclusive);
        assert_eq!(volume::renames("smbfs", false), Renames::Replacing);
    }

    #[cfg(windows)]
    #[test]
    fn a_verbatim_path_keeps_its_drive_or_its_share() {
        let text = |path: &str| {
            let wide = volume::verbatim(Path::new(path)).unwrap();
            String::from_utf16(&wide[..wide.len() - 1]).unwrap()
        };
        assert_eq!(text(r"C:\a\b.npno"), r"\\?\C:\a\b.npno");
        assert_eq!(text(r"\\host\share\a"), r"\\?\UNC\host\share\a");
        assert_eq!(text(r"\\?\C:\a"), r"\\?\C:\a");
    }

    #[cfg(windows)]
    #[test]
    fn a_rename_past_max_path_is_made_and_refuses_what_is_there() {
        let deep = std::env::temp_dir()
            .join(format!("toshokan-long-{}", std::process::id()))
            .join("d".repeat(120))
            .join("e".repeat(120));
        fs::create_dir_all(&deep).unwrap();
        let (from, to) = (deep.join("a.npno"), deep.join("b.npno"));
        assert!(to.as_os_str().len() > 260, "{}", to.display());
        fs::write(&from, b"a").unwrap();
        volume::rename(&from, &to).unwrap();
        assert_eq!(fs::read(&to).unwrap(), b"a");
        fs::write(&from, b"again").unwrap();
        let refused = volume::rename(&from, &to).unwrap_err();
        assert_eq!(io_error(refused), IoError::AlreadyExists);
        let _ = fs::remove_dir_all(deep.ancestors().nth(2).unwrap());
    }

    #[cfg(windows)]
    #[test]
    fn a_file_and_a_directory_are_flushed_on_windows() {
        let dir = std::env::temp_dir().join(format!("toshokan-sync-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("a.npno"), b"a").unwrap();
        for (path, kind) in [
            (dir.join("a.npno"), Kind::File),
            (dir.clone(), Kind::Directory),
        ] {
            let file = writable(&path, kind).unwrap_or_else(|e| panic!("{kind:?}: {e}"));
            file.sync_all().unwrap_or_else(|e| panic!("{kind:?}: {e}"));
        }
        let _ = fs::remove_dir_all(&dir);
    }

    #[cfg(windows)]
    #[test]
    fn fat_and_exfat_volumes_move_by_copying_on_windows() {
        for name in ["FAT", "FAT32", "exFAT"] {
            assert_eq!(volume::renames(name), Renames::Torn, "{name}");
        }
        for name in ["NTFS", "ReFS"] {
            assert_eq!(volume::renames(name), Renames::Exclusive, "{name}");
        }
    }

    #[test]
    fn a_root_no_probe_can_tell_about_declares_no_rename() {
        let native = Native::new("", "");
        assert!(!native.capabilities(Root::Folder).rename_file);
    }

    #[test]
    fn a_root_not_made_yet_is_probed_where_it_will_be() {
        let made = std::env::temp_dir();
        let native = Native::new(made.join("not/made/yet"), &made);
        assert_eq!(native.renames(Root::Folder), native.renames(Root::Local));
    }

    #[test]
    fn a_root_whose_renames_are_torn_declares_no_rename() {
        let native = Native::new("folder", "local");
        native.renames[0].set(Some(Renames::Torn));
        let declared = native.capabilities(Root::Folder);
        assert!(!declared.rename_file && !declared.rename_dir && !declared.no_replace);
        assert!(declared.append && declared.fsync == FSYNC);
    }
}
