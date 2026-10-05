//! The desktop backend: a library is a directory, and commands run in order on a thread
//! of their own.

use std::fs::{self, File, OpenOptions};
use std::io::{self, Read as _, Seek as _, Write as _};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::Arc;
use std::thread::JoinHandle;

use eframe::egui;

use super::exec::{self, Children, Fs, Kind, Over, Staged, TEMP, TMP, WORKING};
use super::{Cmd, Event, Fingerprint, Stat};
use crate::ondisk::OnDisk;

/// The folder the default library is, inside drawbar's own data.
const LIBRARY: &str = "library";

/// The default library: `library` in drawbar's own data, beside eframe's store.
#[cfg(not(windows))]
pub fn default_root() -> Option<PathBuf> {
    beside_store()
}

/// The default library: `drawbar\library` in the local app data, or beside eframe's
/// store where `LOCALAPPDATA` names no absolute folder.
///
/// ⚠️ Not beside eframe's store, which is in the roaming app data: a domain profile copies
/// that to and from a server at every sign-in, and a piano library runs to hundreds of
/// megabytes. The local app data stays on this machine.
#[cfg(windows)]
pub fn default_root() -> Option<PathBuf> {
    std::env::var_os("LOCALAPPDATA")
        .map(PathBuf::from)
        .filter(|local| local.is_absolute())
        .map(|local| local.join(crate::APP).join(LIBRARY))
        .or_else(beside_store)
}

fn beside_store() -> Option<PathBuf> {
    Some(eframe::storage_dir(crate::APP)?.join(LIBRARY))
}

/// Make `root` ready to open as the library, or say why it cannot be. The default library,
/// `default`, is made where it is missing, as a first start makes it; any other folder
/// must already be there.
pub fn openable(root: &Path, default: Option<&Path>) -> Result<(), String> {
    if root.is_dir() {
        return Ok(());
    }
    if default == Some(root) && !root.exists() {
        return fs::create_dir_all(root)
            .map_err(|e| format!("{} could not be made: {e}", root.display()));
    }
    Err(format!(
        "{} is not a folder drawbar can open as the library.",
        root.display()
    ))
}

const LOCK: &str = ".drawbar/lock";

/// Where [`Fs::stage`] writes before [`Fs::place`] renames: `.drawbar/tmp/` for
/// the index's own files, and a hidden sibling in the same folder for a library file, so
/// the rename never crosses a volume. Each `attempt` after the first takes another name
/// the open's sweep still finds.
fn temp_for(path: &str, attempt: u32) -> String {
    let (parent, leaf) = match path.rsplit_once('/') {
        Some((parent, leaf)) => (Some(parent), leaf),
        None => (None, path),
    };
    let leaf = match attempt {
        0 => leaf.to_string(),
        n => format!("{leaf}.{n}"),
    };
    match (path.starts_with(".drawbar/"), parent) {
        (true, _) => format!("{TMP}/{leaf}"),
        (false, Some(parent)) => format!("{parent}/.{leaf}{TEMP}"),
        (false, None) => format!(".{leaf}{TEMP}"),
    }
}

/// How many names a temporary tries before a write gives up.
const TEMPS: u32 = 8;

/// A folder as a `file:` URL, or `None` for a path that is not absolute.
fn folder_url(dir: &Path) -> Option<String> {
    url::Url::from_directory_path(dir).ok().map(String::from)
}

/// The thread that owns one library's files.
pub struct Backend {
    root: PathBuf,
    tx: Option<Sender<Cmd>>,
    rx: Receiver<Event>,
    worker: Option<JoinHandle<()>>,
    /// Set once the library is let go, so a listing still running stops.
    stop: Arc<AtomicBool>,
    /// Commands that write ([`exec::writes`]) sent and not yet run through.
    unrun: Arc<AtomicUsize>,
}

impl Backend {
    pub fn start(ctx: &egui::Context, root: PathBuf) -> Backend {
        let (tx, commands) = channel::<Cmd>();
        let (answers, rx) = channel();
        let stop = Arc::new(AtomicBool::new(false));
        let unrun = Arc::new(AtomicUsize::new(0));
        let mut disk = Disk {
            root: root.clone(),
            prepared: false,
            lock: None,
            index: None,
            stop: stop.clone(),
            commands: Some(commands),
            held: None,
            taken: 0,
        };
        let ctx = ctx.clone();
        let ran = unrun.clone();
        let worker = std::thread::spawn(move || {
            while let Some(cmd) = disk.next() {
                exec::execute(&mut disk, cmd, &mut |event| {
                    let _ = answers.send(event);
                    ctx.request_repaint();
                });
                ran.fetch_sub(std::mem::take(&mut disk.taken), Ordering::Release);
            }
        });
        Backend {
            root,
            tx: Some(tx),
            rx,
            worker: Some(worker),
            stop,
            unrun,
        }
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Where the library is, as the user would look for it.
    pub fn label(&self) -> String {
        self.root.display().to_string()
    }

    /// The URL that shows the library's folder in the system's file manager.
    pub fn reveal(&self) -> Option<String> {
        folder_url(&self.root)
    }

    pub fn send(&mut self, cmd: Cmd) {
        if let Some(tx) = &self.tx {
            let writes = usize::from(exec::writes(&cmd));
            self.unrun.fetch_add(writes, Ordering::AcqRel);
            if tx.send(cmd).is_err() {
                self.unrun.fetch_sub(writes, Ordering::AcqRel);
            }
        }
    }

    /// Whether a command that writes has been sent and not run through yet.
    pub fn busy(&self) -> bool {
        self.unrun.load(Ordering::Acquire) != 0
    }

    pub fn try_recv(&mut self) -> Option<Event> {
        self.rx.try_recv().ok()
    }

    /// The next answer, waiting a while for it.
    pub fn recv(&mut self) -> Option<Event> {
        self.rx
            .recv_timeout(std::time::Duration::from_secs(10))
            .ok()
    }

    /// Run every command already sent, then let the library go. A listing in flight stops
    /// where it is, since nothing will read the rest of it.
    pub fn finish(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        self.tx = None;
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

impl Drop for Backend {
    fn drop(&mut self) {
        self.finish();
    }
}

/// A library's files on this machine's disk.
struct Disk {
    root: PathBuf,
    /// `.drawbar/` and its folders have been made, once, for this session.
    prepared: bool,
    /// The lock file, held open, and locked, while this library is written.
    lock: Option<File>,
    /// The index as this backend last read or wrote it: see [`Fs::last_index`].
    index: Option<(u64, u32)>,
    stop: Arc<AtomicBool>,
    /// The commands sent, in order. `None` in a test that runs commands one by one.
    commands: Option<Receiver<Cmd>>,
    /// A command a listing took and put back, to run next.
    held: Option<Cmd>,
    /// Commands that write taken from `commands` since the last one taken by
    /// [`Disk::next`] began. They have all run once it returns.
    taken: usize,
}

impl Disk {
    /// The next command to run, waiting for one, or `None` once the library is let go and
    /// every command sent has run.
    fn next(&mut self) -> Option<Cmd> {
        let cmd = self
            .held
            .take()
            .or_else(|| self.commands.as_ref()?.recv().ok());
        self.taken += usize::from(cmd.as_ref().is_some_and(exec::writes));
        cmd
    }

    /// Where `path` is on disk. A folder on the way to it that is a link is refused as
    /// [`io::ErrorKind::NotFound`], so nothing is read or written outside the library
    /// through one. The last name is left to each call: [`Fs::stat`] takes only a file.
    ///
    /// ⚠️ A folder made a link between this look and the call that follows is still
    /// followed. Only another program on this computer can do that, while drawbar works.
    fn locate(&self, path: &str) -> io::Result<PathBuf> {
        let mut at = self.root.clone();
        let mut parts = path.split('/').filter(|part| !part.is_empty()).peekable();
        let mut looking = true;
        while let Some(part) = parts.next() {
            // A drive letter or an alternate data stream on Windows.
            if cfg!(windows) && part.contains(':') {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    format!("{part:?} is not a name Windows holds"),
                ));
            }
            at.push(part);
            if !looking || parts.peek().is_none() {
                continue;
            }
            match fs::symlink_metadata(&at) {
                Ok(meta) if meta.file_type().is_symlink() => {
                    return Err(io::Error::new(
                        io::ErrorKind::NotFound,
                        format!("{part:?} is a link, which drawbar does not follow"),
                    ));
                }
                Ok(_) => {}
                // Nothing below a folder that is not there can be a link.
                Err(_) => looking = false,
            }
        }
        Ok(at)
    }

    /// Write the temporary for `path` with `write`, synced to the disk.
    fn staged(
        &self,
        path: &str,
        write: impl FnOnce(&mut File) -> io::Result<()>,
    ) -> io::Result<PathBuf> {
        let (temp, mut file) = self.fresh(path)?;
        match write(&mut file).and_then(|()| file.sync_all()) {
            Ok(()) => Ok(temp),
            Err(e) => {
                let _ = fs::remove_file(&temp);
                Err(e)
            }
        }
    }

    /// A new temporary for `path`, made where no entry is, so a link left at its name
    /// is never followed: a stale entry is removed first, which removes a link and not
    /// what it names, and a name taken again before the file is made gives way to the
    /// next.
    fn fresh(&self, path: &str) -> io::Result<(PathBuf, File)> {
        for attempt in 0..TEMPS {
            let temp = self.locate(&temp_for(path, attempt))?;
            match fs::remove_file(&temp) {
                Err(e) if e.kind() != io::ErrorKind::NotFound => return Err(e),
                _ => {}
            }
            match File::options().write(true).create_new(true).open(&temp) {
                Ok(file) => return Ok((temp, file)),
                Err(e) if e.kind() == io::ErrorKind::AlreadyExists => continue,
                Err(e) => return Err(e),
            }
        }
        Err(io::Error::new(
            io::ErrorKind::AlreadyExists,
            format!("no temporary name beside {path} stayed free"),
        ))
    }

    /// A temporary for `path` that shares the blocks of `source`, on a disk that can clone
    /// a file. `None` where it cannot, and nothing is made. A clone is made only where no
    /// entry is, so it never follows a link left at its name.
    #[cfg(target_vendor = "apple")]
    fn clone_of(&self, source: &File, path: &str) -> io::Result<Option<PathBuf>> {
        use rustix::fs::{fclonefileat, CloneFlags, CWD};

        for attempt in 0..TEMPS {
            let temp = self.locate(&temp_for(path, attempt))?;
            match fs::remove_file(&temp) {
                Err(e) if e.kind() != io::ErrorKind::NotFound => return Err(e),
                _ => {}
            }
            match fclonefileat(source, CWD, &temp, CloneFlags::NOFOLLOW) {
                Ok(()) => {
                    File::open(&temp)?.sync_all()?;
                    return Ok(Some(temp));
                }
                Err(rustix::io::Errno::EXIST) => continue,
                Err(_) => return Ok(None),
            }
        }
        Ok(None)
    }

    #[cfg(not(target_vendor = "apple"))]
    fn clone_of(&self, _: &File, _: &str) -> io::Result<Option<PathBuf>> {
        Ok(None)
    }

    /// Put the temporary `temp` at `path`: over whatever is there where `over` is set,
    /// and otherwise only where nothing is, as [`Fs::place`] says.
    fn put(&self, temp: PathBuf, path: &str, over: bool) -> io::Result<()> {
        let target = self.locate(path)?;
        let placed = match over {
            true => fs::rename(&temp, &target),
            // A hard link appears whole and refuses a name already taken, which a rename
            // would overwrite. A volume without links (FAT, some shares) falls back to the
            // check before the write and a rename.
            false => match fs::hard_link(&temp, &target) {
                Ok(()) => fs::remove_file(&temp),
                Err(e) if e.kind() == io::ErrorKind::AlreadyExists => Err(e),
                Err(_) => fs::rename(&temp, &target),
            },
        };
        if placed.is_err() {
            let _ = fs::remove_file(&temp);
        }
        placed?;
        sync_dir(parent(&target))
    }

    /// Refuse a new file where an entry already is.
    fn free(&self, path: &str) -> io::Result<()> {
        match fs::symlink_metadata(self.locate(path)?) {
            Ok(_) => Err(io::ErrorKind::AlreadyExists.into()),
            Err(_) => Ok(()),
        }
    }
}

/// The entries of one folder, by name. A name that is not Unicode cannot be named in the
/// index, so it is left out.
fn sorted(dir: &Path) -> io::Result<Vec<(String, fs::DirEntry)>> {
    let mut found = Vec::new();
    for entry in fs::read_dir(dir)? {
        let entry = entry?;
        if let Ok(name) = entry.file_name().into_string() {
            found.push((name, entry));
        }
    }
    found.sort_by(|a, b| a.0.cmp(&b.0));
    Ok(found)
}

/// What a listing says about one entry of a folder. `None` leaves it out: a link, which
/// is neither a file nor a folder here, so a loop of links is never walked, and an entry
/// gone since the folder was read.
fn kind(entry: &fs::DirEntry, name: &str) -> Option<Kind> {
    let unread = |e: io::Error| {
        let gone = e.kind() == io::ErrorKind::NotFound;
        (!gone && exec::opens(name)).then(|| Kind::Unread(e.to_string()))
    };
    let kind = match entry.file_type() {
        Ok(kind) => kind,
        Err(e) => return unread(e),
    };
    if kind.is_dir() {
        return Some(Kind::Dir);
    }
    if !kind.is_file() {
        return None;
    }
    if !exec::opens(name) {
        return Some(Kind::Other);
    }
    match entry.metadata() {
        Ok(meta) => Some(Kind::File(stat(&meta))),
        Err(e) => unread(e),
    }
}

fn stat(meta: &fs::Metadata) -> Stat {
    let modified = meta
        .modified()
        .ok()
        .and_then(|time| time.duration_since(std::time::UNIX_EPOCH).ok())
        .and_then(|since| u64::try_from(since.as_nanos()).ok());
    Stat {
        len: meta.len(),
        modified,
    }
}

/// Make a rename or a new entry in `dir` survive a power cut, not only a crash. Does
/// nothing off Unix, where a folder cannot be opened to sync.
pub fn sync_dir(dir: &Path) -> io::Result<()> {
    #[cfg(unix)]
    File::open(dir)?.sync_all()?;
    #[cfg(not(unix))]
    let _ = dir;
    Ok(())
}

/// Whether two paths reach one entry, as two spellings of a name do on a disk that
/// ignores case.
#[cfg(unix)]
fn one_entry(a: &Path, b: &Path) -> io::Result<bool> {
    use std::os::unix::fs::MetadataExt;
    let (a, b) = (fs::symlink_metadata(a)?, fs::symlink_metadata(b)?);
    Ok((a.dev(), a.ino()) == (b.dev(), b.ino()))
}

/// Whether two paths reach one entry: the system resolves each to the name the entry
/// has on disk.
#[cfg(not(unix))]
fn one_entry(a: &Path, b: &Path) -> io::Result<bool> {
    Ok(fs::canonicalize(a)? == fs::canonicalize(b)?)
}

fn parent(path: &Path) -> &Path {
    path.parent().unwrap_or(path)
}

impl Fs for Disk {
    fn stopped(&self) -> bool {
        self.stop.load(Ordering::Relaxed)
    }

    fn waiting(&mut self) -> Option<Cmd> {
        let cmd = self
            .held
            .take()
            .or_else(|| self.commands.as_ref()?.try_recv().ok());
        self.taken += usize::from(cmd.as_ref().is_some_and(exec::writes));
        cmd
    }

    fn hold(&mut self, cmd: Cmd) {
        debug_assert!(self.held.is_none(), "one command is put back at a time");
        self.taken = self.taken.saturating_sub(usize::from(exec::writes(&cmd)));
        self.held = Some(cmd);
    }

    async fn prepare(&mut self) -> io::Result<()> {
        if self.prepared {
            return Ok(());
        }
        for dir in [TMP, WORKING] {
            fs::create_dir_all(self.locate(dir)?)?;
        }
        self.prepared = true;
        Ok(())
    }

    async fn lock(&mut self) -> io::Result<bool> {
        // ⚠️ A second lock on its own handle would be refused by the one this already
        // holds.
        if self.lock.is_some() {
            return Ok(true);
        }
        let file = OpenOptions::new()
            .create(true)
            .truncate(false)
            .write(true)
            .open(self.locate(LOCK)?)?;
        match file.try_lock() {
            Ok(()) => {
                self.lock = Some(file);
                Ok(true)
            }
            Err(fs::TryLockError::WouldBlock) => Ok(false),
            Err(fs::TryLockError::Error(e)) => Err(e),
        }
    }

    async fn unlock(&mut self) {
        self.lock = None;
    }

    fn last_index(&mut self) -> &mut Option<(u64, u32)> {
        &mut self.index
    }

    async fn children(
        &self,
        dir: &str,
        room: usize,
        known: &dyn Fn(&str) -> bool,
    ) -> io::Result<Children> {
        let found = match sorted(&self.locate(dir)?) {
            // The default library is made at its first write.
            Err(e)
                if e.kind() == io::ErrorKind::NotFound && dir.is_empty() && !self.root.exists() =>
            {
                return Ok((Vec::new(), false));
            }
            found => found?,
        };
        let more = found.len() > room;
        let children = found.into_iter().take(room).map(|(name, entry)| {
            let file = entry.file_type().is_ok_and(|kind| kind.is_file());
            let kind = (!(file && known(&name)))
                .then(|| kind(&entry, &name))
                .flatten();
            (name, kind)
        });
        Ok((children.collect(), more))
    }

    async fn names(&self, dir: &str) -> io::Result<Vec<String>> {
        let mut names = Vec::new();
        for entry in fs::read_dir(self.locate(dir)?)? {
            if let Ok(name) = entry?.file_name().into_string() {
                names.push(name);
            }
        }
        names.sort();
        Ok(names)
    }

    async fn read(&self, path: &str) -> io::Result<Vec<u8>> {
        fs::read(self.locate(path)?)
    }

    async fn crc(&self, path: &str) -> io::Result<u32> {
        crate::ondisk::crc_of(&mut File::open(self.locate(path)?)?)
    }

    async fn stat(&self, path: &str) -> io::Result<Option<Stat>> {
        match self.locate(path).and_then(fs::symlink_metadata) {
            Ok(meta) if meta.is_file() => Ok(Some(stat(&meta))),
            Ok(_) => Ok(None),
            Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(e),
        }
    }

    type Temp = PathBuf;

    /// A copy of the library's own file is a clone where the disk can make one, which
    /// shares the source's blocks rather than writing them again.
    async fn stage(&mut self, path: &str, what: Staged<'_>) -> io::Result<PathBuf> {
        match what {
            Staged::Bytes(bytes) => self.staged(path, |file| file.write_all(bytes)),
            Staged::Outside(from) => self.staged(path, |file| {
                io::copy(&mut File::open(from)?, file).map(|_| ())
            }),
            Staged::Part(part) => self.staged(path, |file| {
                let mut source = File::open(&part.from)?;
                source.seek(io::SeekFrom::Start(part.bytes.start))?;
                let len = part.bytes.end - part.bytes.start;
                let mut source = source.take(len);
                let mut crc = nord_format::crc::Crc32Stream::new();
                let (mut copied, mut buf) = (0, vec![0; 1 << 16]);
                loop {
                    let n = source.read(&mut buf)?;
                    if n == 0 {
                        break;
                    }
                    crc.update(&buf[..n]);
                    file.write_all(&buf[..n])?;
                    copied += n as u64;
                }
                match (copied, crc.value()) == (len, part.crc32) {
                    true => Ok(()),
                    false => Err(part.mismatch()),
                }
            }),
            Staged::Library(from) => {
                let mut source = File::open(self.locate(from)?)?;
                match self.clone_of(&source, path)? {
                    Some(temp) => Ok(temp),
                    None => self.staged(path, |file| io::copy(&mut source, file).map(|_| ())),
                }
            }
            Staged::Edited(from, edit) => self.staged(path, |file| edit.write(from, file)),
        }
    }

    async fn place(&mut self, temp: PathBuf, path: &str, over: Over) -> io::Result<()> {
        let over = over.replaces();
        if !over {
            if let Err(e) = self.free(path) {
                let _ = fs::remove_file(&temp);
                return Err(e);
            }
        }
        self.put(temp, path, over)
    }

    async fn discard(&mut self, temp: PathBuf) {
        let _ = fs::remove_file(temp);
    }

    async fn rename(&mut self, from: &str, to: &str) -> io::Result<()> {
        let (source, target) = (self.locate(from)?, self.locate(to)?);
        // On a disk that ignores case, a rename that only changes case finds the entry
        // itself at `to`.
        let taken = match fs::symlink_metadata(&target) {
            Ok(_) => !one_entry(&source, &target)?,
            Err(e) if e.kind() == io::ErrorKind::NotFound => false,
            Err(e) => return Err(e),
        };
        if taken {
            return Err(io::ErrorKind::AlreadyExists.into());
        }
        fs::rename(&source, &target)?;
        sync_dir(parent(&source))?;
        sync_dir(parent(&target))
    }

    /// A working copy is renamed over its target, or linked where nothing may be there,
    /// so nothing is written. Across volumes, or on one without links, nothing moves.
    async fn promote(&mut self, from: &str, path: &str, over: Over) -> io::Result<bool> {
        let (source, target) = (self.locate(from)?, self.locate(path)?);
        let moved = match over.replaces() {
            true => fs::rename(&source, &target),
            false => fs::hard_link(&source, &target),
        };
        match moved {
            Ok(()) => {}
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists => return Err(e),
            Err(_) => return Ok(false),
        }
        if !over.replaces() {
            let _ = fs::remove_file(&source);
        }
        sync_dir(parent(&target))?;
        sync_dir(parent(&source))?;
        Ok(true)
    }

    async fn make_dir(&mut self, path: &str) -> io::Result<()> {
        let target = self.locate(path)?;
        fs::create_dir_all(&target)?;
        sync_dir(parent(&target))
    }

    async fn remove_file(&mut self, path: &str) -> io::Result<()> {
        fs::remove_file(self.locate(path)?)
    }

    async fn remove_dir(&mut self, path: &str) -> io::Result<()> {
        fs::remove_dir(self.locate(path)?)
    }

    async fn rest(
        &self,
        path: &str,
        known: Option<Fingerprint>,
    ) -> io::Result<Option<Arc<OnDisk>>> {
        let file = File::open(self.locate(path)?)?;
        Ok(OnDisk::open(file, known.and_then(|print| print.crc))?.map(Arc::new))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::LibPath;
    use crate::testing::Temp;

    fn disk(root: &Temp) -> Disk {
        Disk {
            root: root.0.clone(),
            prepared: false,
            lock: None,
            index: None,
            stop: Arc::default(),
            commands: None,
            held: None,
            taken: 0,
        }
    }

    /// Run a command that answers at most once.
    fn execute(disk: &mut Disk, cmd: Cmd) -> Option<Event> {
        let mut answers = Vec::new();
        exec::execute(disk, cmd, &mut |event| answers.push(event));
        assert!(answers.len() <= 1, "{answers:?}");
        answers.pop()
    }

    #[cfg(unix)]
    #[test]
    fn a_folder_url_escapes_what_a_url_would_read_as_its_own() {
        assert_eq!(
            folder_url(Path::new("/Users/jo/Music/Jo's #1 kit?/ü")).as_deref(),
            Some("file:///Users/jo/Music/Jo's%20%231%20kit%3F/%C3%BC/")
        );
        assert_eq!(folder_url(Path::new("Music/drawbar")), None, "relative");
    }

    #[cfg(windows)]
    #[test]
    fn a_folder_url_on_windows_names_the_drive_and_escapes_the_rest() {
        assert_eq!(
            folder_url(Path::new(r"C:\Users\Jo Moore\Music\drawbar")).as_deref(),
            Some("file:///C:/Users/Jo%20Moore/Music/drawbar/")
        );
    }

    #[test]
    fn a_rename_that_changes_only_case_goes_through() {
        let root = Temp::new();
        fs::write(root.at("c3.ne5p"), b"lower").unwrap();
        let moved = execute(
            &mut disk(&root),
            Cmd::Move {
                from: LibPath::root().join("c3.ne5p"),
                to: LibPath::root().join("C3.ne5p"),
            },
        );
        assert!(
            matches!(moved, Some(Event::Moved { result: Ok(()), .. })),
            "{moved:?}"
        );
        assert_eq!(root.names(""), [".drawbar", "C3.ne5p"]);
        assert_eq!(root.read("C3.ne5p"), b"lower");
    }

    /// Only a disk that tells case apart can hold both names, so elsewhere this checks
    /// nothing.
    #[test]
    fn a_rename_onto_another_file_one_case_apart_is_refused() {
        let root = Temp::new();
        fs::write(root.at("c3.ne5p"), b"lower").unwrap();
        fs::write(root.at("C3.ne5p"), b"upper").unwrap();
        if root.names("").len() < 2 {
            return;
        }
        let moved = execute(
            &mut disk(&root),
            Cmd::Move {
                from: LibPath::root().join("c3.ne5p"),
                to: LibPath::root().join("C3.ne5p"),
            },
        );
        assert!(
            matches!(moved, Some(Event::Moved { result: Err(_), .. })),
            "{moved:?}"
        );
        assert_eq!(root.read("c3.ne5p"), b"lower");
        assert_eq!(root.read("C3.ne5p"), b"upper");
    }

    /// A file whose length or time moved since drawbar took them is deleted only where
    /// its CRC says it still holds what drawbar read. Without one, it is left.
    #[test]
    fn a_delete_over_a_file_whose_stat_moved_needs_its_crc() {
        let root = Temp::new();
        fs::write(root.at("c3.ne5p"), b"contents").unwrap();
        let stale = Stat {
            len: 8,
            modified: Some(1),
        };
        let path = LibPath::root().join("c3.ne5p");
        let remove = |crc| {
            execute(
                &mut disk(&root),
                Cmd::RemoveFile {
                    path: path.clone(),
                    expect: Fingerprint {
                        crc,
                        ..Fingerprint::unread(stale)
                    },
                },
            )
        };
        assert!(matches!(remove(None), Some(Event::Failed(_))));
        let other = Some(nord_format::crc::crc32(b"other st"));
        assert!(matches!(remove(other), Some(Event::Failed(_))));
        assert_eq!(root.read("c3.ne5p"), b"contents");

        assert!(remove(Some(nord_format::crc::crc32(b"contents"))).is_none());
        assert!(!root.at("c3.ne5p").exists());
    }

    /// A piano library the size a vendor ships, about 200 MB in 64 strokes of about 3 MB.
    fn large_piano() -> Vec<u8> {
        use nord_format::formats::npno::synthetic::{take, Build};
        use nord_format::formats::npno::Bank;

        Build {
            version: 0x464,
            channels: 1,
            takes: (21..85)
                .map(|root| take(root, Bank::Attack, 0, 3_000))
                .collect(),
            map: (21..85).map(|key| (key, key)).collect(),
        }
        .bytes()
        .expect("the builder lays out a library")
    }

    /// A plan over a piano library of a vendor's size is saved through its file: each
    /// kept stroke is read and written by its range, and nothing near the library's size
    /// is ever allocated.
    #[test]
    fn a_large_piano_plan_is_saved_with_no_buffer_near_its_size() {
        use crate::rewrite::Rewrite;

        let root = Temp::new();
        let bytes = large_piano();
        let len = bytes.len();
        assert!(len > 190_000_000, "{len} bytes is a vendor's size");
        fs::write(root.at("Grand.npno"), &bytes).unwrap();
        drop(bytes);
        let file = File::open(root.at("Grand.npno")).unwrap();
        let from = Arc::new(OnDisk::open(file, None).unwrap().unwrap());
        let crate::ondisk::Index::Piano(index) = &from.index else {
            panic!("a piano library")
        };
        let mut library = index.library().clone();
        library.set_name("Trimmed").unwrap();
        library.retain_strokes(|stroke| stroke.root % 2 == 0);
        let edit = Arc::new(Rewrite::Piano(library));
        let stat = stat(&fs::metadata(root.at("Grand.npno")).unwrap());

        let (answer, largest) = crate::testing::largest_allocation(|| {
            execute(
                &mut disk(&root),
                Cmd::Rewrite {
                    id: 1,
                    path: LibPath::root().join("Grand.npno"),
                    from,
                    edit,
                    expect: Fingerprint::unread(stat),
                    stale: None,
                },
            )
        });
        // One stroke's audio, about 3 MB, is the most held at once.
        assert!(
            (1 << 20..8 << 20).contains(&largest),
            "{largest} bytes held at once"
        );
        let Some(Event::Rewritten {
            result: Ok(found), ..
        }) = answer
        else {
            panic!("{answer:?}")
        };
        let saved = found.file.expect("the saved library rests in its file");
        let crate::ondisk::Index::Piano(index) = &saved.index else {
            panic!("a piano library")
        };
        assert_eq!(index.library().strokes().len(), 32);
        assert_eq!(index.library().name().0, "Trimmed");
        let info = nord_format::cbin::inspect(&mut File::open(root.at("Grand.npno")).unwrap());
        assert!(info.unwrap().checksum_ok);
        assert_eq!(
            root.names(""),
            [".drawbar", "Grand.npno"],
            "no copy is left"
        );
    }

    /// A piano library of a vendor's size is copied byte for byte, and copied with an
    /// edit written through it, with nothing near its size held at once.
    #[test]
    fn a_large_piano_is_copied_with_no_buffer_near_its_size() {
        use crate::rewrite::Rewrite;
        use crate::store::Source;

        let root = Temp::new();
        let bytes = large_piano();
        fs::write(root.at("Grand.npno"), &bytes).unwrap();
        let file = File::open(root.at("Grand.npno")).unwrap();
        let from = Arc::new(OnDisk::open(file, None).unwrap().unwrap());
        let crate::ondisk::Index::Piano(index) = &from.index else {
            panic!("a piano library")
        };
        let mut library = index.library().clone();
        library.set_name("Edited").unwrap();
        let stat = stat(&fs::metadata(root.at("Grand.npno")).unwrap());
        let import = |name: &str, from: Source| Cmd::Import {
            id: 1,
            path: LibPath::root().join(name),
            from,
            expect: None,
        };
        let copied = import(
            "Copy.npno",
            Source::Library(
                LibPath::root().join("Grand.npno"),
                Fingerprint::unread(stat),
            ),
        );
        let edited = import(
            "Edited.npno",
            Source::Edited(from, Arc::new(Rewrite::Piano(library))),
        );

        for (name, cmd) in [("Copy.npno", copied), ("Edited.npno", edited)] {
            let (answer, largest) =
                crate::testing::largest_allocation(|| execute(&mut disk(&root), cmd));
            assert!(largest < 8 << 20, "{name}: {largest} bytes held at once");
            let Some(Event::Imported {
                result: Ok(found), ..
            }) = answer
            else {
                panic!("{name}: {answer:?}")
            };
            assert!(found.file.is_some(), "{name} rests in its file");
        }
        assert!(root.read("Copy.npno") == bytes);
        let info = nord_format::cbin::inspect(&mut File::open(root.at("Edited.npno")).unwrap());
        assert!(info.unwrap().checksum_ok);
    }

    /// A file whose stat moved is told to hold what drawbar knew by its CRC, taken in one
    /// streaming pass: deleting a piano library of a vendor's size that way holds nothing
    /// near its size.
    #[test]
    fn a_file_whose_stat_moved_is_checked_by_a_streamed_crc() {
        let root = Temp::new();
        let bytes = large_piano();
        fs::write(root.at("Grand.npno"), &bytes).unwrap();
        let expect = Fingerprint {
            len: bytes.len() as u64,
            modified: Some(1),
            crc: Some(nord_format::crc::crc32(&bytes)),
        };
        drop(bytes);
        let remove = Cmd::RemoveFile {
            path: LibPath::root().join("Grand.npno"),
            expect,
        };

        let (answer, largest) =
            crate::testing::largest_allocation(|| execute(&mut disk(&root), remove));
        assert!(answer.is_none(), "{answer:?}");
        assert!(largest < 8 << 20, "{largest} bytes held at once");
        assert!(!root.at("Grand.npno").exists());
    }

    /// A link left at the name a write's temporary takes is never followed: the file it
    /// names, outside the library, keeps what it holds, and the write lands. The same
    /// holds for a copy of one of the library's files.
    #[cfg(unix)]
    #[test]
    fn a_link_at_a_temporary_name_is_never_written_through() {
        let (root, outside) = (Temp::new(), Temp::new());
        fs::write(outside.at("precious"), b"outside").unwrap();
        let plant = |leaf: &str| {
            std::os::unix::fs::symlink(outside.at("precious"), root.at(leaf)).unwrap();
        };
        fs::write(root.at("Grand.ne5p"), b"first").unwrap();
        plant(".Saved.ne5p.drawbar-tmp");
        plant(".Copy.ne5p.drawbar-tmp");

        let saved = execute(
            &mut disk(&root),
            Cmd::Save {
                id: 1,
                path: LibPath::root().join("Saved.ne5p"),
                bytes: b"saved".to_vec(),
                expect: None,
                stale: None,
                promote: None,
            },
        );
        assert!(
            matches!(saved, Some(Event::Saved { result: Ok(_), .. })),
            "{saved:?}"
        );
        let stat = stat(&fs::metadata(root.at("Grand.ne5p")).unwrap());
        let copied = execute(
            &mut disk(&root),
            Cmd::Import {
                id: 2,
                path: LibPath::root().join("Copy.ne5p"),
                from: crate::store::Source::Library(
                    LibPath::root().join("Grand.ne5p"),
                    Fingerprint::unread(stat),
                ),
                expect: None,
            },
        );
        assert!(
            matches!(copied, Some(Event::Imported { result: Ok(_), .. })),
            "{copied:?}"
        );

        assert_eq!(outside.read("precious"), b"outside");
        assert_eq!(root.read("Saved.ne5p"), b"saved");
        assert_eq!(root.read("Copy.ne5p"), b"first");
        assert_eq!(
            root.names(""),
            [".drawbar", "Copy.ne5p", "Grand.ne5p", "Saved.ne5p"]
        );
    }

    /// A folder of the library that is a link to one outside it is never written through:
    /// a save, a copy, a move, a new folder and a delete whose path passes through it are
    /// each refused, and the folder it names keeps what it holds.
    #[cfg(unix)]
    #[test]
    fn a_path_through_a_linked_folder_is_refused() {
        let (root, outside) = (Temp::new(), Temp::new());
        fs::write(outside.at("x.ne5p"), b"outside").unwrap();
        std::os::unix::fs::symlink(&outside.0, root.at("Linked")).unwrap();
        fs::write(root.at("a.ne5p"), b"inside").unwrap();
        let linked = |leaf: &str| LibPath::parse(&format!("Linked/{leaf}")).unwrap();
        let x = stat(&fs::metadata(outside.at("x.ne5p")).unwrap());
        let a = stat(&fs::metadata(root.at("a.ne5p")).unwrap());
        let mut disk = disk(&root);

        let saved = execute(
            &mut disk,
            Cmd::Save {
                id: 1,
                path: linked("new.ne5p"),
                bytes: b"saved".to_vec(),
                expect: None,
                stale: None,
                promote: None,
            },
        );
        assert!(
            matches!(saved, Some(Event::Saved { result: Err(_), .. })),
            "{saved:?}"
        );
        let over = execute(
            &mut disk,
            Cmd::Save {
                id: 2,
                path: linked("x.ne5p"),
                bytes: b"saved".to_vec(),
                expect: Some(Fingerprint::unread(x)),
                stale: None,
                promote: None,
            },
        );
        assert!(
            matches!(over, Some(Event::Saved { result: Err(_), .. })),
            "{over:?}"
        );
        let copied = execute(
            &mut disk,
            Cmd::Import {
                id: 3,
                path: linked("copy.ne5p"),
                from: crate::store::Source::Library(
                    LibPath::root().join("a.ne5p"),
                    Fingerprint::unread(a),
                ),
                expect: None,
            },
        );
        assert!(
            matches!(copied, Some(Event::Imported { result: Err(_), .. })),
            "{copied:?}"
        );
        let moved = execute(
            &mut disk,
            Cmd::Move {
                from: LibPath::root().join("a.ne5p"),
                to: linked("a.ne5p"),
            },
        );
        assert!(
            matches!(moved, Some(Event::Moved { result: Err(_), .. })),
            "{moved:?}"
        );
        let made = execute(&mut disk, Cmd::MakeDir(linked("Sub")));
        assert!(matches!(made, Some(Event::Failed(_))), "{made:?}");
        execute(
            &mut disk,
            Cmd::RemoveFile {
                path: linked("x.ne5p"),
                expect: Fingerprint::unread(x),
            },
        );

        assert_eq!(outside.names(""), ["x.ne5p"]);
        assert_eq!(outside.read("x.ne5p"), b"outside");
        assert_eq!(root.read("a.ne5p"), b"inside");
    }

    /// A file reached through a linked folder is not there, so a row of the index there
    /// reads as missing, and nothing outside the library is read.
    #[cfg(unix)]
    #[test]
    fn a_file_through_a_linked_folder_is_not_there() {
        let (root, outside) = (Temp::new(), Temp::new());
        fs::write(outside.at("x.ne5p"), b"outside").unwrap();
        std::os::unix::fs::symlink(&outside.0, root.at("Linked")).unwrap();
        let disk = disk(&root);
        let stat = nord_usb::block_on(disk.stat("Linked/x.ne5p"));
        assert!(matches!(stat, Ok(None)), "{stat:?}");
        assert!(nord_usb::block_on(disk.read("Linked/x.ne5p")).is_err());
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn the_default_library_on_macos_is_in_application_support() {
        let home = PathBuf::from(std::env::var_os("HOME").expect("HOME is set"));
        assert_eq!(
            default_root(),
            Some(home.join("Library/Application Support/drawbar/library"))
        );
    }

    /// The one test that sets `XDG_DATA_HOME`, so no other reads it while it moves.
    #[cfg(all(unix, not(target_os = "macos")))]
    #[test]
    fn the_default_library_elsewhere_on_unix_is_in_the_xdg_data_folder() {
        let data = Temp::new();
        let held = std::env::var_os("XDG_DATA_HOME");
        std::env::set_var("XDG_DATA_HOME", &data.0);
        let named = default_root();
        let storage = eframe::storage_dir(crate::APP);
        std::env::set_var("XDG_DATA_HOME", "relative/data");
        let relative = default_root();
        match held {
            Some(held) => std::env::set_var("XDG_DATA_HOME", held),
            None => std::env::remove_var("XDG_DATA_HOME"),
        }
        assert_eq!(named, Some(data.at("drawbar/library")));
        assert_eq!(
            named,
            storage.map(|dir| dir.join(LIBRARY)),
            "beside eframe's store"
        );
        let home = PathBuf::from(std::env::var_os("HOME").expect("HOME is set"));
        assert_eq!(
            relative,
            Some(home.join(".local/share/drawbar/library")),
            "a relative XDG_DATA_HOME is ignored"
        );
    }

    /// The one test that sets `LOCALAPPDATA`, so no other reads it while it moves.
    #[cfg(windows)]
    #[test]
    fn the_default_library_on_windows_is_in_the_local_app_data() {
        let local = Temp::new();
        let held = std::env::var_os("LOCALAPPDATA");
        std::env::set_var("LOCALAPPDATA", &local.0);
        let named = default_root();
        std::env::set_var("LOCALAPPDATA", "relative\\data");
        let relative = default_root();
        std::env::remove_var("LOCALAPPDATA");
        let unset = default_root();
        if let Some(held) = held {
            std::env::set_var("LOCALAPPDATA", held);
        }
        assert_eq!(named, Some(local.at("drawbar").join("library")));
        let roaming = eframe::storage_dir(crate::APP).expect("the system names one");
        assert_eq!(
            unset,
            Some(roaming.join(LIBRARY)),
            "without LOCALAPPDATA, beside eframe's store"
        );
        assert_eq!(relative, unset, "a relative LOCALAPPDATA is ignored");
    }

    /// The derived cache sits beside `app.ron`, one level above the default library, so
    /// the library never holds it.
    #[cfg(target_os = "macos")]
    #[test]
    fn the_default_library_is_a_folder_of_the_storage_eframe_keeps() {
        let storage = eframe::storage_dir(crate::APP).expect("the system names one");
        assert_eq!(default_root(), Some(storage.join(LIBRARY)));
    }

    /// The default library opens even where no first start made its folder, as when an
    /// earlier session opened only another library.
    #[test]
    fn the_default_library_is_made_where_it_is_missing() {
        let root = Temp::new();
        let default = root.at("drawbar/library");
        assert_eq!(openable(&default, Some(&default)), Ok(()));
        assert!(default.is_dir());
    }

    /// A library opened before and gone since is not made again, empty, in its place.
    #[test]
    fn a_missing_library_other_than_the_default_is_refused() {
        let root = Temp::new();
        let gone = root.at("gone");
        let why = openable(&gone, Some(&root.at("library"))).expect_err("refused");
        assert!(why.contains("is not a folder drawbar can open"), "{why}");
        assert!(!gone.exists());
    }

    /// A file where the default library would be is left alone.
    #[test]
    fn a_file_where_the_default_library_would_be_is_refused() {
        let root = Temp::new();
        let default = root.at("library");
        fs::write(&default, b"not a folder").unwrap();
        assert!(openable(&default, Some(&default)).is_err());
        assert_eq!(fs::read(&default).unwrap(), b"not a folder");
    }
}
