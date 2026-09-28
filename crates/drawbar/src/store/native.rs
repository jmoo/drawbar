//! The desktop backend: a library is a directory, and commands run in order on a thread
//! of their own.

use std::collections::VecDeque;
use std::fs::{self, File, OpenOptions};
use std::io::{self, Write as _};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::Arc;
use std::thread::JoinHandle;

use eframe::egui;

use super::exec::{self, Entry, Fs, Kind, MOST_ENTRIES, TEMP, TMP, WORKING};
use super::{Cmd, Event, Fingerprint, Stat};
use crate::ondisk::OnDisk;

/// The default library: `drawbar` in the user's Music folder, or in the home folder
/// where the system names no Music folder.
pub fn default_root() -> Option<PathBuf> {
    let home = std::env::home_dir()?;
    Some(music(&home).unwrap_or(home).join("drawbar"))
}

/// The user's Music folder: always `~/Music` on macOS and Windows.
///
/// ⚠️ A Windows Music folder the user moved elsewhere is not followed.
#[cfg(any(target_os = "macos", windows))]
fn music(home: &Path) -> Option<PathBuf> {
    Some(home.join("Music"))
}

/// The user's Music folder as `xdg-user-dirs` names it, which a minimal system may not.
#[cfg(not(any(target_os = "macos", windows)))]
fn music(home: &Path) -> Option<PathBuf> {
    let config = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .filter(|dir| dir.is_absolute())
        .unwrap_or_else(|| home.join(".config"));
    let text = fs::read_to_string(config.join("user-dirs.dirs")).ok()?;
    xdg_music(&text, home)
}

/// `XDG_MUSIC_DIR` from a `user-dirs.dirs` file: `"$HOME/…"` or an absolute path, in
/// quotes. A value naming the home folder itself means the user has no Music folder.
#[cfg_attr(any(target_os = "macos", windows), allow(dead_code))]
fn xdg_music(text: &str, home: &Path) -> Option<PathBuf> {
    let value = text
        .lines()
        .filter_map(|line| line.trim().strip_prefix("XDG_MUSIC_DIR="))
        .next_back()?
        .trim()
        .strip_prefix('"')?
        .strip_suffix('"')?;
    let dir = match value.strip_prefix("$HOME") {
        Some(rest) => home.join(rest.trim_start_matches('/')),
        None if value.starts_with('/') => PathBuf::from(value),
        None => return None,
    };
    (dir != home).then_some(dir)
}

const LOCK: &str = ".drawbar/lock";

/// Where [`Fs::replace`] and [`Fs::create`] write before the rename: `.drawbar/tmp/` for
/// the index's own files, and a hidden sibling in the same folder for a library file, so
/// the rename never crosses a volume.
fn temp_for(path: &str) -> String {
    let (parent, leaf) = match path.rsplit_once('/') {
        Some((parent, leaf)) => (Some(parent), leaf),
        None => (None, path),
    };
    match (path.starts_with(".drawbar/"), parent) {
        (true, _) => format!("{TMP}/{leaf}"),
        (false, Some(parent)) => format!("{parent}/.{leaf}{TEMP}"),
        (false, None) => format!(".{leaf}{TEMP}"),
    }
}

/// The thread that owns one library's files.
pub struct Backend {
    root: PathBuf,
    tx: Option<Sender<Cmd>>,
    rx: Receiver<Event>,
    worker: Option<JoinHandle<()>>,
}

impl Backend {
    pub fn start(ctx: &egui::Context, root: PathBuf) -> Backend {
        let (tx, commands) = channel::<Cmd>();
        let (answers, rx) = channel();
        let mut disk = Disk {
            root: root.clone(),
            prepared: false,
            lock: None,
        };
        let ctx = ctx.clone();
        let worker = std::thread::spawn(move || {
            for cmd in commands {
                if let Some(event) = exec::execute(&mut disk, cmd) {
                    let _ = answers.send(event);
                    ctx.request_repaint();
                }
            }
        });
        Backend {
            root,
            tx: Some(tx),
            rx,
            worker: Some(worker),
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
        Some(format!("file://{}", self.root.display()))
    }

    pub fn send(&mut self, cmd: Cmd) {
        if let Some(tx) = &self.tx {
            let _ = tx.send(cmd);
        }
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

    /// Run every command already sent, then let the library go.
    pub fn finish(&mut self) {
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
}

impl Disk {
    fn locate(&self, path: &str) -> io::Result<PathBuf> {
        let mut at = self.root.clone();
        for part in path.split('/').filter(|part| !part.is_empty()) {
            // A drive letter or an alternate data stream on Windows.
            if cfg!(windows) && part.contains(':') {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    format!("{part:?} is not a name Windows holds"),
                ));
            }
            at.push(part);
        }
        Ok(at)
    }

    /// Breadth first, so the top of a large tree is listed before the bound is reached.
    fn walk(&self) -> io::Result<Vec<Entry>> {
        let mut entries = Vec::new();
        let mut looked = 0;
        let mut folders = VecDeque::from([String::new()]);
        while let Some(prefix) = folders.pop_front() {
            if looked >= MOST_ENTRIES {
                entries.push(Entry {
                    path: prefix,
                    kind: Kind::Unwalked,
                });
                continue;
            }
            let found = match sorted(&self.locate(&prefix)?) {
                Ok(found) => found,
                // A folder inside that cannot be read is left unlisted, not the library.
                Err(_) if !prefix.is_empty() => {
                    entries.push(Entry {
                        path: prefix,
                        kind: Kind::Unwalked,
                    });
                    continue;
                }
                Err(e) => return Err(e),
            };
            let room = MOST_ENTRIES - looked;
            if found.len() > room {
                entries.push(Entry {
                    path: prefix.clone(),
                    kind: Kind::Unwalked,
                });
            }
            for (name, entry) in found.into_iter().take(room) {
                looked += 1;
                let path = match prefix.is_empty() {
                    true => name.clone(),
                    false => format!("{prefix}/{name}"),
                };
                let Some(kind) = kind(&entry, &name) else {
                    continue;
                };
                if matches!(kind, Kind::Dir) && !name.starts_with('.') {
                    folders.push_back(path.clone());
                }
                entries.push(Entry { path, kind });
            }
        }
        Ok(entries)
    }

    /// Write `bytes` to the temporary for `path`, synced to the disk.
    fn stage(&self, path: &str, bytes: &[u8]) -> io::Result<PathBuf> {
        let temp = self.locate(&temp_for(path))?;
        let wrote = File::create(&temp).and_then(|mut file| {
            file.write_all(bytes)?;
            file.sync_all()
        });
        match wrote {
            Ok(()) => Ok(temp),
            Err(e) => {
                let _ = fs::remove_file(&temp);
                Err(e)
            }
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

/// Make a rename or a new entry in `dir` survive a power cut, not only a crash.
fn sync_dir(dir: &Path) -> io::Result<()> {
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

    async fn list(&self) -> io::Result<Vec<Entry>> {
        match self.walk() {
            // The default library is made at its first write.
            Err(e) if e.kind() == io::ErrorKind::NotFound && !self.root.exists() => Ok(Vec::new()),
            walked => walked,
        }
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

    async fn stat(&self, path: &str) -> io::Result<Option<Stat>> {
        match fs::symlink_metadata(self.locate(path)?) {
            Ok(meta) => Ok(Some(stat(&meta))),
            Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(e),
        }
    }

    async fn create(&mut self, path: &str, bytes: &[u8]) -> io::Result<()> {
        let target = self.locate(path)?;
        if fs::symlink_metadata(&target).is_ok() {
            return Err(io::ErrorKind::AlreadyExists.into());
        }
        let temp = self.stage(path, bytes)?;
        // A hard link appears whole and refuses a name already taken, which a rename
        // would overwrite. A volume without links (FAT, some shares) falls back to the
        // check above and a rename.
        let placed = match fs::hard_link(&temp, &target) {
            Ok(()) => fs::remove_file(&temp),
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists => Err(e),
            Err(_) => fs::rename(&temp, &target),
        };
        if placed.is_err() {
            let _ = fs::remove_file(&temp);
        }
        placed?;
        sync_dir(parent(&target))
    }

    async fn replace(&mut self, path: &str, bytes: &[u8]) -> io::Result<()> {
        let target = self.locate(path)?;
        let temp = self.stage(path, bytes)?;
        if let Err(e) = fs::rename(&temp, &target) {
            let _ = fs::remove_file(&temp);
            return Err(e);
        }
        sync_dir(parent(&target))
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
        Ok(OnDisk::open(file, known.map(|print| print.crc))?.map(Arc::new))
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
        }
    }

    #[test]
    fn a_rename_that_changes_only_case_goes_through() {
        let root = Temp::new();
        fs::write(root.at("c3.ne5p"), b"lower").unwrap();
        let moved = exec::execute(
            &mut disk(&root),
            Cmd::Move {
                from: LibPath::root().join("c3.ne5p"),
                to: LibPath::root().join("C3.ne5p"),
            },
        );
        assert!(moved.is_none(), "{moved:?}");
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
        let moved = exec::execute(
            &mut disk(&root),
            Cmd::Move {
                from: LibPath::root().join("c3.ne5p"),
                to: LibPath::root().join("C3.ne5p"),
            },
        );
        assert!(matches!(moved, Some(Event::Failed(_))), "{moved:?}");
        assert_eq!(root.read("c3.ne5p"), b"lower");
        assert_eq!(root.read("C3.ne5p"), b"upper");
    }

    #[test]
    fn the_music_folder_is_read_from_user_dirs_as_xdg_writes_it() {
        let home = Path::new("/home/jo");
        let music = |text| xdg_music(text, home);
        assert_eq!(
            music("# written by xdg-user-dirs-update\nXDG_MUSIC_DIR=\"$HOME/Musik\"\n"),
            Some(PathBuf::from("/home/jo/Musik"))
        );
        assert_eq!(
            music("XDG_MUSIC_DIR=\"/srv/audio\""),
            Some(PathBuf::from("/srv/audio"))
        );
        assert_eq!(music("XDG_MUSIC_DIR=\"$HOME/\""), None, "no Music folder");
        assert_eq!(music("XDG_DOWNLOAD_DIR=\"$HOME/Downloads\""), None);
        assert_eq!(music("XDG_MUSIC_DIR=Music"), None, "not a path it writes");
    }
}
