//! The desktop backend: a library is a directory, and commands run in order on a thread
//! of their own: every change through toshokan, and reads through [`exec`].

use std::cell::RefCell;
use std::fs::{self, File};
use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::Arc;
use std::thread::JoinHandle;

use eframe::egui;
use toshokan::blocking::{Backend as _, Native};
use toshokan::env::{OsRandom, SystemClock};
use toshokan::io::{Capabilities, Io, IoResult, Root};

use super::counted::{Counted, Signs};
use super::exec::{self, Fs};
use super::keeper::{self, Keeper};
use super::{keys, names};
use super::{Cmd, Event, Fingerprint, LibPath, Stat};
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

/// What this install keeps of the library at `root` in drawbar's own data, never in the
/// library: toshokan's local root, and the slots' former occupants a write to the
/// instrument left. Named by the library's folder, so each library has its own. Where the
/// system names no folder for drawbar's data, it is in the system's temporary folder, and
/// lasts as long as that does.
///
/// ⚠️ Two drawbars running at once on one machine share it. Each writes as a writer of
/// its own, but the identities its scans read and what it let go of are kept in one file
/// each, and the one that writes last keeps its own.
pub fn local_root(root: &Path) -> PathBuf {
    let root = fs::canonicalize(root).unwrap_or_else(|_| root.to_path_buf());
    let hash = blake3::hash(root.as_os_str().as_encoded_bytes());
    let name: String = hash.as_bytes()[..16]
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect();
    let data =
        eframe::storage_dir(crate::APP).unwrap_or_else(|| std::env::temp_dir().join(crate::APP));
    data.join(LIBRARIES).join(name)
}

/// The folder of drawbar's own data that holds [`local_root`]s.
const LIBRARIES: &str = "libraries";

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

/// A folder as a `file:` URL, or `None` for a path that is not absolute.
fn folder_url(dir: &Path) -> Option<String> {
    url::Url::from_directory_path(dir).ok().map(String::from)
}

/// The thread that owns one library's files and facts.
pub struct Backend {
    root: PathBuf,
    local: PathBuf,
    tx: Option<Sender<Cmd>>,
    rx: Receiver<Event>,
    worker: Option<JoinHandle<()>>,
    /// Commands that write sent and not yet run through.
    unrun: Arc<AtomicUsize>,
    /// What the library's storage has done so far.
    signs: Arc<Signs>,
    /// Whether a command is running, and since when.
    life: Arc<Life>,
    /// How many requests the storage had answered when [`Backend::waiting`] last looked,
    /// and when.
    seen: std::cell::Cell<(usize, std::time::Instant)>,
    #[cfg(test)]
    fault: Arc<std::sync::Mutex<Option<super::Fault>>>,
    /// How far the library's clock runs ahead of the system's.
    #[cfg(test)]
    ahead: Arc<std::sync::atomic::AtomicU64>,
}

/// Whether the library's thread is running a command, and since when.
#[derive(Default)]
struct Life {
    running: std::sync::atomic::AtomicBool,
    since: std::sync::Mutex<Option<std::time::Instant>>,
}

impl Life {
    fn run(&self, running: bool) {
        *self.since.lock().expect("never poisoned") = Some(std::time::Instant::now());
        self.running.store(running, Ordering::Release);
    }
}

impl Backend {
    /// The library at `root`, with this install's own state for it at `local`.
    pub fn start(ctx: &egui::Context, root: PathBuf, local: PathBuf) -> Backend {
        let (tx, commands) = channel::<Cmd>();
        let (answers, rx) = channel();
        let unrun = Arc::new(AtomicUsize::new(0));
        let disk = Disk { root: root.clone() };
        let (folder, kept) = (root.clone(), local.clone());
        let ctx = ctx.clone();
        let ran = unrun.clone();
        let signs = Arc::new(Signs::default());
        let counted = signs.clone();
        let life = Arc::new(Life::default());
        let living = life.clone();
        #[cfg(test)]
        let fault = Arc::new(std::sync::Mutex::new(None));
        #[cfg(test)]
        let faults = fault.clone();
        #[cfg(test)]
        let ahead = Arc::new(std::sync::atomic::AtomicU64::new(0));
        #[cfg(test)]
        let clock = Box::new(Ahead(ahead.clone()));
        #[cfg(not(test))]
        let clock = Box::new(SystemClock);
        let worker = std::thread::spawn(move || {
            // toshokan's library stays on the thread that opened it.
            let mut keeper = keeper(&folder, &kept, counted, clock);
            nord_usb::block_on(async {
                let mut landed = Vec::new();
                #[cfg(test)]
                let mut unread = None;
                while let Ok(cmd) = commands.recv() {
                    living.run(true);
                    #[cfg(test)]
                    let fault = faults.lock().expect("never poisoned").take();
                    #[cfg(test)]
                    match fault {
                        Some(super::Fault::Trap) => {
                            panic!("the library's thread was made to fault")
                        }
                        Some(super::Fault::Stall) => loop {
                            std::thread::park();
                        },
                        Some(super::Fault::Unread) => {
                            unread = Some(super::refusing::refused(&cmd));
                        }
                        Some(super::Fault::Next(after)) => {
                            *faults.lock().expect("never poisoned") = Some(*after);
                        }
                        None => {}
                    }
                    let wrote = cmd.writes();
                    match cmd {
                        Cmd::Read { files, room } => {
                            landed.push(Event::Read(exec::read_all(&disk, files, room).await));
                        }
                        cmd => keeper.run(cmd, &mut |event| landed.push(event)).await,
                    }
                    #[cfg(test)]
                    if let Some(refusing) = unread.take() {
                        landed = refusing.answers("its answers were made to be lost", false);
                    }
                    for event in landed.drain(..) {
                        let _ = answers.send(exec::rest_landed(&disk, event).await);
                        ctx.request_repaint();
                    }
                    ran.fetch_sub(usize::from(wrote), Ordering::Release);
                    living.run(false);
                }
                keeper.close().await;
            });
        });
        Backend {
            root,
            local,
            tx: Some(tx),
            rx,
            worker: Some(worker),
            unrun,
            signs,
            life,
            seen: std::cell::Cell::new((0, std::time::Instant::now())),
            #[cfg(test)]
            fault,
            #[cfg(test)]
            ahead,
        }
    }

    /// Move the library's clock `ms` further ahead of the system's.
    #[cfg(test)]
    pub fn pass(&self, ms: u64) {
        self.ahead.fetch_add(ms, Ordering::Relaxed);
    }

    /// Make the next command the library's thread takes fault or never finish, as a
    /// bug in it would.
    #[cfg(test)]
    pub fn fault(&self, fault: super::Fault) {
        *self.fault.lock().expect("never poisoned") = Some(fault);
    }

    /// Why the library's thread stopped, once it has without being let go: it answers
    /// nothing more.
    pub fn stopped(&self) -> Option<String> {
        let ended = self.worker.as_ref().is_some_and(JoinHandle::is_finished);
        (ended && self.tx.is_some()).then(|| "the library's thread stopped".to_string())
    }

    /// How long the command running now has gone without a sign of life: no answer, and
    /// no file found. `None` while none runs.
    pub fn waiting(&self) -> Option<std::time::Duration> {
        if !self.life.running.load(Ordering::Acquire) {
            return None;
        }
        let running = (*self.life.since.lock().expect("never poisoned"))?;
        let answered = self.signs.answered.load(Ordering::Relaxed);
        self.seen.set(match self.seen.get() {
            (_, at) if at < running => (answered, running),
            (before, _) if before != answered => (answered, std::time::Instant::now()),
            seen => seen,
        });
        Some(self.seen.get().1.elapsed())
    }

    /// Let the library go without waiting for its thread, which stopped or does not
    /// answer.
    pub fn abandon(&mut self) {
        self.tx = None;
        self.worker = None;
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    /// This install's own state for the library.
    pub fn local(&self) -> &Path {
        &self.local
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
            let writes = usize::from(cmd.writes());
            self.unrun.fetch_add(writes, Ordering::AcqRel);
            if tx.send(cmd).is_err() {
                self.unrun.fetch_sub(writes, Ordering::AcqRel);
            }
        }
    }

    /// How many files the listings of the folder have found so far.
    pub fn listed(&self) -> usize {
        self.signs.listed.load(Ordering::Relaxed)
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

/// The keeper of the library in `folder`, with this install's own state for it in
/// `local`, counting into `signs` what its storage does.
/// The system's clock, run ahead by a shared amount.
#[cfg(test)]
struct Ahead(Arc<std::sync::atomic::AtomicU64>);

#[cfg(test)]
impl toshokan::Clock for Ahead {
    fn now_ms(&mut self) -> u64 {
        SystemClock.now_ms() + self.0.load(Ordering::Relaxed)
    }
}

pub(super) fn keeper(
    folder: &Path,
    local: &Path,
    signs: Arc<Signs>,
    clock: Box<dyn toshokan::Clock>,
) -> Keeper<Counted<Blocking>> {
    let names = Box::new(keys::Volume::probe(folder));
    let env = keeper::env(clock, Box::new(OsRandom::new()), names);
    let native = Blocking(RefCell::new(Native::new(folder, local)));
    Keeper::new(Counted::new(native, signs), env)
}

/// toshokan's native backend, as storage the async keeper drives.
pub(super) struct Blocking(RefCell<Native>);

impl toshokan::asynch::Fs for Blocking {
    fn capabilities(&self, root: Root) -> Capabilities {
        self.0.borrow().capabilities(root)
    }

    async fn perform(&self, io: Io) -> IoResult {
        self.0.borrow_mut().perform(io)
    }
}

/// A library's files on this machine's disk.
struct Disk {
    root: PathBuf,
}

/// Where the library path `path` is on disk under `root`. A folder on the way to it that
/// is a link is refused as [`io::ErrorKind::NotFound`], so nothing is read or written
/// outside the library through one. The last name is left to each caller.
///
/// ⚠️ A folder made a link between this look and the call that follows is still
/// followed. Only another program on this computer can do that, while drawbar works.
pub(super) fn locate(root: &Path, path: &str) -> io::Result<PathBuf> {
    let mut at = root.to_path_buf();
    let mut parts = path.split('/').filter(|part| !part.is_empty()).peekable();
    let mut looking = true;
    while let Some(part) = parts.next() {
        // ⚠️ A `:` names a drive or a stream and a `\` separates folders, so either
        // can reach outside the library.
        if let Some(why) = cfg!(windows)
            .then(|| names::windows_refusal(part))
            .flatten()
        {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                format!("{part:?} is not a name Windows holds, since {why}"),
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

impl Disk {
    fn locate(&self, path: &str) -> io::Result<PathBuf> {
        locate(&self.root, path)
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

impl Fs for Disk {
    async fn read(&self, path: &str) -> io::Result<Vec<u8>> {
        fs::read(self.locate(path)?)
    }

    async fn stat(&self, path: &str) -> io::Result<Option<Stat>> {
        match self.locate(path).and_then(fs::symlink_metadata) {
            Ok(meta) if meta.is_file() => Ok(Some(stat(&meta))),
            Ok(_) => Ok(None),
            Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(e),
        }
    }

    async fn rest(
        &self,
        path: &str,
        known: Option<Fingerprint>,
    ) -> io::Result<Option<Arc<OnDisk>>> {
        let file = File::open(self.locate(path)?)?;
        Ok(OnDisk::open(file, known.and_then(|print| print.crc))?.map(Arc::new))
    }

    async fn identity(&self, path: &LibPath) -> io::Result<Option<toshokan::Identity>> {
        let Ok(relative) = toshokan::RelPath::new(path.as_str()) else {
            return Ok(None);
        };
        let mut file = File::open(self.locate(path.as_str())?)?;
        super::keys::identity_of(&relative, &mut file).map(Some)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::Temp;

    fn disk(root: &Temp) -> Disk {
        Disk {
            root: root.0.clone(),
        }
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
