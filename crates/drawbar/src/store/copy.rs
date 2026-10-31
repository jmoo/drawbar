//! A copy of a library: a ZIP of its folder, as File ▸ Save a copy of the library saves
//! it, and opening one into a library that holds no files.
//!
//! A copy holds every file of the folder, toshokan's logs in `.drawbar/library/` among
//! them, so a drawbar that opens it reads the library's history as it reads a synced
//! copy's. It leaves out each install's own state, in `.drawbar/local/`, and the swap
//! files Chromium leaves.

use std::collections::BTreeSet;

use nord_usb::FileSource;
use toshokan::asynch::Fs;
use toshokan::io::{Io, IoError, Kind, Lock, Reply, Root};
use toshokan::RelPath;

use super::keys::{DIR, LOCAL};
use super::names::{inside, key};
use super::storage::{self, CHUNK};
use super::zip::{self, End, Stored, Unread};
use super::Outside;

/// The most entries a copy opened may hold.
const MOST_ENTRIES: u64 = 1_000_000;
/// The most bytes a copy's directory may take.
const MOST_DIRECTORY: u64 = 64 << 20;

/// The file that stands while a copy is being opened. A library that holds it opens
/// read-only, and the next copy opened into it first removes what the last one left.
pub const OPENING: &str = ".drawbar/opening-copy";

/// Whether the file or folder at `path` of a library's folder goes in a copy of it.
#[cfg(any(target_arch = "wasm32", test))]
pub fn copied(path: &str) -> bool {
    !own(path) && !path.ends_with(".crswap") && key(path) != key(OPENING)
}

/// How many windows of an install may have one library open, each holding a lock of
/// its own that [`hold_window`] takes.
const WINDOWS: usize = 16;

/// The lock, in an install's own state, that a window holds the `n`th of.
fn window(n: usize) -> RelPath {
    RelPath::new(&format!("window-{n}.lock")).expect("one component")
}

/// Take one of the locks a window holds while it has the library open, by which a copy
/// opens only into a library no other window has open, or say why none was taken: every
/// one is held, or a lock failed. The browser lets a window's locks go when the window
/// closes. ⚠️ A window takes it before it reads [`OPENING`] or writes anything, so a copy
/// that makes [`OPENING`] stand and then finds no lock held writes alone.
pub async fn hold_window<F: Fs>(fs: &F) -> Result<RelPath, String> {
    for name in (0..WINDOWS).map(window) {
        let lock = Io::Lock { name: name.clone() };
        match fs.perform(lock).await {
            Ok(Reply::Lock(Lock::Acquired)) => return Ok(name),
            Ok(_) => {}
            Err(e) => {
                let why = storage::failed(e);
                return Err(format!("taking this window's hold on the library: {why}"));
            }
        }
    }
    Err(format!(
        "drawbar has the library open in {WINDOWS} tabs or windows already: close one, \
         then open the library again"
    ))
}

/// Whether another window has the library open, by the locks [`hold_window`] takes.
async fn other_window<F: Fs>(fs: &F) -> Result<bool, IoError> {
    for name in (0..WINDOWS).map(window) {
        match fs.perform(Io::Lock { name: name.clone() }).await? {
            Reply::Lock(Lock::Held) => return Ok(true),
            _ => _ = fs.perform(Io::Unlock { name }).await?,
        }
    }
    Ok(false)
}

/// Whether a copy was being opened into the library on `fs` when the opening stopped.
pub async fn opening<F: Fs>(fs: &F) -> std::io::Result<bool> {
    Ok(storage::stat(fs, &opening_path()).await?.is_some())
}

fn opening_path() -> RelPath {
    RelPath::new(OPENING).expect("a library path")
}

/// Whether `path` is an install's own state, or holds it.
fn own(path: &str) -> bool {
    let local = format!("{DIR}/{LOCAL}");
    key(path) == key(&local) || inside(path, &local)
}

/// What a copy holds of `found`, the files and folders of a library's folder [`copied`]
/// lets in, each folder with `None`: every file and each folder that holds nothing, the
/// library's files before drawbar's own, so a copy opened only partway holds files
/// without their history, never history without its files.
#[cfg(any(target_arch = "wasm32", test))]
pub fn arranged<T>(mut found: Vec<(String, Option<T>)>) -> Vec<(String, Option<T>)> {
    let filled: BTreeSet<String> = found
        .iter()
        .filter_map(|(path, _)| Some(path.rsplit_once('/')?.0.to_string()))
        .collect();
    found.retain(|(path, file)| file.is_some() || !filled.contains(path));
    found.sort_by(|(a, _), (b, _)| (inside(a, DIR), a).cmp(&(inside(b, DIR), b)));
    found
}

/// The name a copy saved on this day is given.
#[cfg(any(target_arch = "wasm32", test))]
pub fn file_name(year: u32, month: u32, day: u32) -> String {
    format!("drawbar library {year:04}-{month:02}-{day:02}.zip")
}

/// One entry of a copy, as it is opened.
#[derive(Debug, PartialEq, Eq)]
pub struct Restored {
    pub path: RelPath,
    pub folder: bool,
    pub entry: Stored,
    /// Where the next entry's header starts, or the directory, which its data must not
    /// pass.
    pub end: u64,
}

/// The entries of a copy whose central directory starts at `directory`, in the order
/// they are in it, or why it is not a copy drawbar could have saved.
pub fn restorable(mut entries: Vec<Stored>, directory: u64) -> Result<Vec<Restored>, String> {
    entries.sort_by_key(|entry| entry.at);
    let ends: Vec<u64> = entries
        .iter()
        .skip(1)
        .map(|entry| entry.at)
        .chain([directory])
        .collect();
    let mut seen = BTreeSet::new();
    let mut restored = Vec::with_capacity(entries.len());
    for (entry, end) in entries.into_iter().zip(ends) {
        let folder = entry.name.ends_with('/');
        let text = entry.name.strip_suffix('/').unwrap_or(&entry.name);
        let path = RelPath::new(text)
            .ok()
            .filter(|path| !path.is_root() && !text.contains('\\'))
            .ok_or_else(|| unlike(format!("it names a file outside the library, {text}")))?;
        if own(text) {
            return Err(unlike(format!("it holds {DIR}/{LOCAL}")));
        }
        if key(text) == key(OPENING) {
            return Err(unlike(format!("it holds {OPENING}")));
        }
        if !seen.insert(text.to_string()) {
            return Err(unlike(format!("it holds {text} twice")));
        }
        if folder && entry.len > 0 {
            return Err(unlike(format!("the folder {text} holds bytes")));
        }
        let least = zip::LOCAL + entry.name.len() as u64;
        let fits = entry
            .at
            .checked_add(least)
            .and_then(|at| at.checked_add(entry.len));
        if fits.is_none_or(|fits| fits > end) {
            return Err(unlike(format!("{text} overlaps what follows it")));
        }
        restored.push(Restored {
            path,
            folder,
            entry,
            end,
        });
    }
    Ok(restored)
}

/// Why an archive is refused, in the user's terms.
fn refused(unread: Unread) -> String {
    match unread {
        Unread::Compressed => format!(
            "its files are compressed, and a copy drawbar saves never is. Unzip it, and \
             open the folder with File ▸ {} in Chrome or Edge",
            crate::menu::OPEN_LIBRARY
        ),
        Unread::Malformed(why) => unlike(why.to_string()),
    }
}

fn unlike(why: String) -> String {
    format!("it is not a copy of a library drawbar saved: {why}")
}

/// Open the copy `from` into the library on `fs`, which must hold none of the user's
/// files and be open in no other window, and answer how many files it wrote. What the
/// library held of drawbar's own, its history among it, is replaced, and an install's
/// own state is kept. Every entry is written as it is, `.drawbar/library/` among them,
/// so this runs before the library opens. [`OPENING`] stands while it writes, so a
/// window that opens the library meanwhile opens it read-only: a copy that fails
/// partway leaves the library empty, and one stopped partway leaves it read-only until
/// a copy is opened again. drawbar's own files are not counted.
pub async fn restore<F: Fs>(fs: &F, from: &Outside) -> Result<usize, String> {
    let failed = |e: std::io::Error| e.to_string();
    let mut source = super::outside_source(from).map_err(|e| e.to_string())?;
    let resumed = opening(fs).await.map_err(failed)?;
    if !resumed {
        start_opening(fs).await?;
    }
    let entries = match ready_to_open(fs, &mut source, resumed).await {
        Ok(entries) => entries,
        Err(why) if resumed => return Err(why),
        Err(why) => {
            done_opening(fs).await.map_err(failed)?;
            return Err(why);
        }
    };
    let mut files = 0;
    for restored in &entries {
        if let Err(why) = write(fs, &mut source, restored).await {
            clear(fs, Clearing::Written(&entries))
                .await
                .map_err(failed)?;
            done_opening(fs).await.map_err(failed)?;
            return Err(why);
        }
        files += usize::from(!restored.folder && !inside(restored.path.as_str(), DIR));
    }
    done_opening(fs).await.map_err(failed)?;
    Ok(files)
}

/// Make [`OPENING`] stand.
async fn start_opening<F: Fs>(fs: &F) -> Result<(), String> {
    let started = |e| format!("starting to open it: {}", storage::failed(e));
    let dir = Io::MakeDir {
        root: Root::Folder,
        path: RelPath::new(DIR).expect("a library path"),
    };
    fs.perform(dir).await.map_err(started)?;
    let mark = Io::Create {
        root: Root::Folder,
        path: opening_path(),
        bytes: Vec::new(),
    };
    fs.perform(mark).await.map_err(started)?;
    Ok(())
}

/// The copy's entries, once nothing but this opening writes in the library and nothing
/// stands where they go: no other window holds the library, and the folder holds only
/// drawbar's own files, or what an opening stopped partway left where `resumed`.
async fn ready_to_open<F: Fs>(
    fs: &F,
    source: &mut impl FileSource,
    resumed: bool,
) -> Result<Vec<Restored>, String> {
    let failed = |e: std::io::Error| e.to_string();
    match other_window(fs).await {
        Ok(false) => {}
        Ok(true) => {
            return Err(
                "drawbar has the library open in another tab or window of this browser: \
                 close it there, then open the copy"
                    .to_string(),
            )
        }
        Err(e) => return Err(format!("looking for other windows: {}", storage::failed(e))),
    }
    if let Some(path) = first_file(fs).await.map_err(failed)?.filter(|_| !resumed) {
        return Err(format!(
            "the library in this browser already holds files, such as {path}, and a copy \
             opens only into an empty one"
        ));
    }
    let entries = contents(source).await?;
    let earlier = match resumed {
        true => Clearing::All,
        false => Clearing::Drawbars,
    };
    clear(fs, earlier).await.map_err(failed)?;
    Ok(entries)
}

/// The first of the user's files in the folder, outside drawbar's own, if any.
async fn first_file<F: Fs>(fs: &F) -> std::io::Result<Option<RelPath>> {
    let mut waiting = vec![RelPath::ROOT];
    while let Some(dir) = waiting.pop() {
        for (name, meta) in storage::list(fs, Root::Folder, &dir).await? {
            let Ok(path) = dir.join(&name) else {
                continue;
            };
            match meta.kind {
                _ if key(path.as_str()) == key(DIR) => {}
                Kind::File => return Ok(Some(path)),
                Kind::Directory => waiting.push(path),
            }
        }
    }
    Ok(None)
}

/// What [`clear`] removes, of the folder but an install's own state and [`OPENING`].
enum Clearing<'a> {
    /// Every file and folder: what an opening stopped partway left.
    All,
    /// drawbar's own files and folders, which a copy replaces.
    Drawbars,
    /// drawbar's own, and the files and folders of `entries` an opening wrote, a folder
    /// only where it is left empty.
    Written(&'a [Restored]),
}

/// Remove what `clearing` names, deepest first, but `.drawbar` itself.
async fn clear<F: Fs>(fs: &F, clearing: Clearing<'_>) -> std::io::Result<()> {
    let mut written = BTreeSet::new();
    if let Clearing::Written(entries) = clearing {
        for entry in entries {
            let mut at = Some(entry.path.clone()).filter(|path| !path.is_root());
            while let Some(path) = at {
                at = path.parent().filter(|parent| !parent.is_root());
                written.insert(key(path.as_str()));
            }
        }
    }
    let cleared = |path: &str| match clearing {
        Clearing::All => true,
        Clearing::Drawbars | Clearing::Written(_) => {
            inside(path, DIR) || written.contains(&key(path))
        }
    };
    let mut found = Vec::new();
    let mut waiting = vec![RelPath::ROOT];
    while let Some(dir) = waiting.pop() {
        for (name, meta) in storage::list(fs, Root::Folder, &dir).await? {
            let Ok(path) = dir.join(&name) else {
                continue;
            };
            let text = path.as_str();
            if own(text) || key(text) == key(OPENING) {
                continue;
            }
            if meta.kind == Kind::Directory {
                waiting.push(path.clone());
            }
            if cleared(text) && key(text) != key(DIR) {
                found.push((path, meta.kind));
            }
        }
    }
    for (path, kind) in found.into_iter().rev() {
        match kind {
            Kind::File => storage::remove(fs, Root::Folder, &path).await?,
            Kind::Directory => {
                let io = Io::RemoveDir {
                    root: Root::Folder,
                    path,
                };
                match fs.perform(io).await {
                    Ok(_) | Err(IoError::NotFound) => {}
                    // A folder the copy holds that holds a file it does not.
                    Err(IoError::NotEmpty) if !matches!(clearing, Clearing::All) => {}
                    Err(e) => return Err(storage::failed(e)),
                }
            }
        }
    }
    Ok(())
}

/// Remove [`OPENING`], then `.drawbar` where nothing is left in it.
async fn done_opening<F: Fs>(fs: &F) -> std::io::Result<()> {
    storage::remove(fs, Root::Folder, &opening_path()).await?;
    let dir = Io::RemoveDir {
        root: Root::Folder,
        path: RelPath::new(DIR).expect("a library path"),
    };
    match fs.perform(dir).await {
        Ok(_) | Err(IoError::NotFound | IoError::NotEmpty) => Ok(()),
        Err(e) => Err(storage::failed(e)),
    }
}

/// The entries of the archive `source` holds, as a copy drawbar saved lays them out.
async fn contents(source: &mut impl FileSource) -> Result<Vec<Restored>, String> {
    let len = source.len();
    let short = || unlike("it ends early".to_string());
    let tail = len.checked_sub(zip::END).ok_or_else(short)?;
    let directory = match zip::end(&read(source, tail, zip::END).await?).map_err(refused)? {
        End::Directory(directory) => directory,
        End::Zip64 => {
            let at = tail.checked_sub(zip::LOCATOR).ok_or_else(short)?;
            let locator = read(source, at, zip::LOCATOR).await?;
            let record = zip::locator(&locator).map_err(refused)?;
            let record = read(source, record, zip::END64).await?;
            zip::end64(&record).map_err(refused)?
        }
    };
    if directory.count > MOST_ENTRIES || directory.len > MOST_DIRECTORY {
        return Err(format!(
            "it holds more than the {MOST_ENTRIES} files drawbar opens from a copy"
        ));
    }
    let listed = read(source, directory.offset, directory.len).await?;
    let entries = zip::directory(&listed, directory.count).map_err(refused)?;
    restorable(entries, directory.offset)
}

/// `len` bytes of `source` at `at`, where `len` is bounded by the caller.
async fn read(source: &mut impl FileSource, at: u64, len: u64) -> Result<Vec<u8>, String> {
    let mut bytes = vec![0; len as usize];
    match source.read_at(at, &mut bytes).await {
        Ok(()) => Ok(bytes),
        Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof => {
            Err(unlike("it ends early".to_string()))
        }
        Err(e) => Err(format!("reading it: {e}")),
    }
}

/// Write `restored` into the folder from `source`.
async fn write<F: Fs>(
    fs: &F,
    source: &mut impl FileSource,
    restored: &Restored,
) -> Result<(), String> {
    let Restored {
        path,
        folder,
        entry,
        ..
    } = restored;
    let failed = |e: toshokan::io::IoError| format!("writing {path}: {}", storage::failed(e));
    let dir = match folder {
        true => path.clone(),
        false => path.parent().expect("a file has a parent"),
    };
    let made = Io::MakeDir {
        root: Root::Folder,
        path: dir.clone(),
    };
    fs.perform(made).await.map_err(failed)?;
    if *folder {
        return Ok(());
    }
    let header = read(source, entry.at, zip::LOCAL + entry.name.len() as u64).await?;
    let data = zip::data_at(&header, entry).map_err(refused)?;
    if data
        .checked_add(entry.len)
        .is_none_or(|end| end > restored.end)
    {
        return Err(unlike(format!("{path} overlaps what follows it")));
    }
    let mut crc = nord_format::crc::Crc32Stream::new();
    let mut at = 0;
    loop {
        let len = CHUNK.min(entry.len - at);
        let mut bytes = vec![0; len as usize];
        source
            .read_at(data + at, &mut bytes)
            .await
            .map_err(|e| format!("reading {path} from the copy: {e}"))?;
        crc.update(&bytes);
        let (root, to) = (Root::Folder, path.clone());
        let io = match at {
            0 => Io::Create {
                root,
                path: to,
                bytes,
            },
            _ => Io::Append {
                root,
                path: to,
                bytes,
            },
        };
        fs.perform(io).await.map_err(failed)?;
        at += len;
        if at == entry.len {
            break;
        }
    }
    if crc.value() != entry.crc {
        return Err(format!(
            "the copy is damaged: {path} does not hold what it did"
        ));
    }
    storage::sync(fs, Root::Folder, path)
        .await
        .map_err(|e| format!("writing {path}: {e}"))?;
    storage::sync(fs, Root::Folder, &dir)
        .await
        .map_err(|e| format!("writing {path}: {e}"))
}

/// The copy of the library in `folder` that File ▸ Save a copy of the library saves, as
/// the browser lays it out from the files.
#[cfg(test)]
pub(super) fn saved(folder: &std::path::Path) -> Vec<u8> {
    let mut found = Vec::new();
    let mut waiting = vec![String::new()];
    while let Some(at) = waiting.pop() {
        for entry in std::fs::read_dir(folder.join(&at)).unwrap() {
            let entry = entry.unwrap();
            let name = entry.file_name().into_string().unwrap();
            let path = match at.is_empty() {
                true => name,
                false => format!("{at}/{name}"),
            };
            if !copied(&path) {
                continue;
            }
            match entry.file_type().unwrap().is_dir() {
                true => {
                    waiting.push(path.clone());
                    found.push((path, None));
                }
                false => found.push((path, Some(std::fs::read(entry.path()).unwrap()))),
            }
        }
    }
    let found = arranged(found);
    let entries: Vec<zip::Entry> = found
        .iter()
        .map(|(path, bytes)| match bytes {
            Some(bytes) => zip::Entry {
                name: path.clone(),
                len: bytes.len() as u64,
                crc: nord_format::crc::crc32(bytes),
                modified: None,
            },
            None => zip::Entry {
                name: format!("{path}/"),
                len: 0,
                crc: 0,
                modified: None,
            },
        })
        .collect();
    let mut out = Vec::new();
    for piece in zip::layout(&entries).unwrap() {
        match piece {
            zip::Piece::Bytes(bytes) => out.extend(bytes),
            zip::Piece::Data(at) => out.extend(found[at].1.as_deref().unwrap()),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Each window of an install holds a lock of its own while it has the library open,
    /// and one past them all opens nothing until another lets its lock go.
    #[test]
    fn a_window_past_every_lock_held_is_refused_until_one_is_let_go() {
        let disk = toshokan::MemDisk::new();
        let windows: Vec<toshokan::MemDisk> = (0..WINDOWS).map(|_| disk.process()).collect();
        let held: BTreeSet<RelPath> = windows
            .iter()
            .map(|window| nord_usb::block_on(hold_window(window)).expect("a lock"))
            .collect();
        assert_eq!(held.len(), WINDOWS, "each window holds its own");
        let last = disk.process();
        let why = nord_usb::block_on(hold_window(&last)).expect_err("every lock is held");
        assert!(why.contains("16 tabs or windows"), "{why}");
        let freed = window(3);
        windows[3]
            .perform(Io::Unlock {
                name: freed.clone(),
            })
            .unwrap();
        assert_eq!(nord_usb::block_on(hold_window(&last)), Ok(freed));
    }

    fn stored(name: &str, at: u64, len: u64) -> Stored {
        Stored {
            name: name.to_string(),
            len,
            crc: 0,
            at,
        }
    }

    /// One entry named `name`, alone in a copy.
    fn alone(name: &str) -> Result<Vec<Restored>, String> {
        restorable(vec![stored(name, 0, 0)], 1000)
    }

    #[test]
    fn a_copy_leaves_out_each_installs_own_state_and_swap_files() {
        for left in [
            ".drawbar/local",
            ".drawbar/local/private/head.json",
            "a.ne5p.crswap",
        ] {
            assert!(!copied(left), "{left}");
        }
        for kept in [
            ".drawbar",
            ".drawbar/library/writers.2/w/a.txt",
            ".drawbar/library.ron",
            ".drawbar/locally.txt",
            "Set/Grand.ne5p",
        ] {
            assert!(copied(kept), "{kept}");
        }
    }

    #[test]
    fn a_copy_holds_empty_folders_and_the_librarys_files_before_drawbars() {
        let found = vec![
            (".drawbar".to_string(), None),
            (".drawbar/library".to_string(), None),
            (".drawbar/library/a.txt".to_string(), Some(())),
            ("Set".to_string(), None),
            ("Set/b.ne5p".to_string(), Some(())),
            ("Empty".to_string(), None),
            ("z.ne5p".to_string(), Some(())),
        ];
        let arranged: Vec<String> = arranged(found).into_iter().map(|(path, _)| path).collect();
        assert_eq!(
            arranged,
            ["Empty", "Set/b.ne5p", "z.ne5p", ".drawbar/library/a.txt"]
        );
    }

    #[test]
    fn a_copy_is_named_for_the_day_it_was_saved() {
        assert_eq!(file_name(2026, 3, 9), "drawbar library 2026-03-09.zip");
    }

    #[test]
    fn a_copy_opens_its_files_folders_and_drawbars_history() {
        let entries = vec![
            stored("Set/", 0, 0),
            stored("Set/Grand.ne5p", 40, 5),
            stored(".drawbar/library/writers.2/w/a.txt", 100, 3),
        ];
        let restored = restorable(entries, 200).unwrap();
        let said: Vec<(&str, bool, u64)> = restored
            .iter()
            .map(|restored| (restored.path.as_str(), restored.folder, restored.end))
            .collect();
        assert_eq!(
            said,
            [
                ("Set", true, 40),
                ("Set/Grand.ne5p", false, 100),
                (".drawbar/library/writers.2/w/a.txt", false, 200),
            ]
        );
    }

    #[test]
    fn a_copy_naming_a_path_outside_the_library_is_refused() {
        for name in [
            "../a.ne5p",
            "/a.ne5p",
            "Set/../../a.ne5p",
            "Set\\a.ne5p",
            "a//b",
            "/",
        ] {
            let refused = alone(name).unwrap_err();
            assert!(refused.contains("outside the library"), "{name}: {refused}");
        }
    }

    #[test]
    fn a_copy_holding_an_installs_own_state_is_refused() {
        for name in [
            ".drawbar/local/private/head.json",
            ".Drawbar/LOCAL/x",
            ".drawbar/local/",
        ] {
            let refused = alone(name).unwrap_err();
            assert!(refused.contains(".drawbar/local"), "{name}: {refused}");
        }
    }

    #[test]
    fn a_copy_holding_a_path_twice_is_refused() {
        let entries = vec![stored("a.ne5p", 0, 0), stored("a.ne5p", 40, 0)];
        let refused = restorable(entries, 100).unwrap_err();
        assert!(refused.contains("twice"), "{refused}");
    }

    /// The browser's own storage tells names apart by case, as a library there may.
    #[test]
    fn names_that_differ_only_by_case_are_two_files() {
        let entries = vec![stored("a.ne5p", 0, 0), stored("A.ne5p", 40, 0)];
        assert_eq!(restorable(entries, 100).unwrap().len(), 2);
    }

    #[test]
    fn a_folder_holding_bytes_is_refused() {
        let refused = restorable(vec![stored("Set/", 0, 4)], 100).unwrap_err();
        assert!(refused.contains("holds bytes"), "{refused}");
    }

    #[test]
    fn an_entry_reaching_past_the_next_is_refused() {
        let entries = vec![stored("a", 0, 20), stored("b", 40, 0)];
        let refused = restorable(entries, 100).unwrap_err();
        assert!(refused.contains("a overlaps"), "{refused}");
        let refused = restorable(vec![stored("a", 0, u64::MAX)], 100).unwrap_err();
        assert!(refused.contains("a overlaps"), "{refused}");
    }

    #[test]
    fn a_compressed_copy_says_how_to_open_it_instead() {
        let refused = refused(Unread::Compressed);
        assert!(refused.contains("compressed"), "{refused}");
        assert!(refused.contains(crate::menu::OPEN_LIBRARY), "{refused}");
    }
}
