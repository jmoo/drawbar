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
/// out, and naming one fails, as does reading or changing a path through a
/// directory that is a link. What a root's renames promise is probed without
/// writing, from its volume's file system: by type on Linux and Android, by type
/// and the volume's capabilities on Apple systems, and by name on Windows. A
/// volume that keeps no journal of its names, such as FAT or exFAT, declares no
/// [`Capabilities::rename_file`] or [`Capabilities::rename_dir`], so that every
/// move copies, and one whose renames refuse an existing destination declares
/// [`Capabilities::no_replace`], as every other volume on Windows does. On
/// Windows a directory's names cannot be flushed, so `fsync` is not declared,
/// and names containing `\` or `:` are refused. The local root's directory, and
/// those it is in, are made where missing at its first change or lock.
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
        match self.renames(root) {
            Renames::Exclusive => volume::rename(&from, &to),
            Renames::Replacing | Renames::Torn => fs::rename(&from, &to),
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

const FSYNC: bool = cfg!(unix);

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
    /// Refuses a read or a change of a path whose directories include a link, which
    /// would reach outside the root. A link made between this look and the request is
    /// still followed.
    fn unlinked(&self, io: &Io) -> Result<(), IoError> {
        let paths: Vec<&RelPath> = match io {
            Io::Read { path, .. }
            | Io::Create { path, .. }
            | Io::Append { path, .. }
            | Io::Write { path, .. }
            | Io::Remove { path, .. }
            | Io::RemoveDir { path, .. }
            | Io::Sync { path, .. } => vec![path],
            Io::ReadMany { reads, .. } => reads.iter().map(|(path, _)| path).collect(),
            Io::Rename { from, to, .. } => vec![from, to],
            _ => Vec::new(),
        };
        let top = self.top(io.root());
        for path in paths {
            let mut at = top.to_path_buf();
            let mut dirs = path.components().peekable();
            while let Some(name) = dirs.next() {
                if dirs.peek().is_none() {
                    break;
                }
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
