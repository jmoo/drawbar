//! Running a [`FileCmd`] against a library's files, the same way whichever backend
//! holds them.
//!
//! The crash-safety of every write rests on [`Fs::stage`] and [`Fs::place`]: a file is
//! written somewhere else first and appears at its path whole, from a hidden
//! `.<name>.drawbar-tmp` sibling. A browser folder that cannot move files is the one
//! exception: there a new file is made empty at its path before its contents land.

use std::io;
use std::sync::Arc;

use super::{Event, Failure, FileCmd, Fingerprint, Found, LibPath, Outside, Source, Stat};
use crate::ondisk::OnDisk;
use crate::rewrite::{self, Rewrite};

/// The most bytes of a library's files drawbar reads whole into memory, counting what it
/// holds whole already. A file left resting in place, read by range, is not counted.
pub const MOST_BYTES: u64 = 1 << 30;

/// Whether drawbar opens a file of this name, by its extension: a Nord file, a Sample
/// Editor project, a note, a MIDI file or a SysEx dump. Any other file is listed by its
/// name only.
pub fn opens(name: &str) -> bool {
    crate::browser::tagged(name).is_some()
}

/// A library's files, as one backend reaches them. Paths are relative to the root and
/// joined by `/`.
///
/// The desktop's calls finish before they return; a browser's storage answers only
/// later, so every call is a future.
pub trait Fs {
    /// The names in one folder.
    async fn names(&self, dir: &str) -> io::Result<Vec<String>>;
    async fn read(&self, path: &str) -> io::Result<Vec<u8>>;
    /// CRC-32 over the whole file at `path`, taken in one streaming pass that never holds
    /// it whole.
    async fn crc(&self, path: &str) -> io::Result<u32>;
    /// The file at `path`, or `None` where no file is: nothing, a folder, or a link.
    async fn stat(&self, path: &str) -> io::Result<Option<Stat>>;
    /// A temporary a write is staged in, until it is put at its path or let go.
    type Temp;
    /// Write `what` into a new temporary for `path`, flushed, never through anything
    /// already at the temporary's name. Nothing is left where it fails. A source that
    /// changed since it was read is refused with [`crate::rewrite::changed`].
    async fn stage(&mut self, path: &str, what: Staged<'_>) -> io::Result<Self::Temp>;
    /// Put a staged temporary at `path`, over what `over` allows: where it is
    /// [`Over::Nothing`], only where nothing is, refused as
    /// [`io::ErrorKind::AlreadyExists`]. A backend that copies the temporary rather than
    /// renaming it looks at an [`Over::Held`] file again just before the copy lands, and
    /// refuses one whose stat moved with [`moved`]. Afterwards, or after a crash at any
    /// point, the path holds the old contents or the new, never part of either, except
    /// that a copy to a new path leaves it empty. A temporary not put is let go.
    async fn place(&mut self, temp: Self::Temp, path: &str, over: Over) -> io::Result<()>;
    /// Let go of a staged temporary that is not to be put anywhere.
    async fn discard(&mut self, temp: Self::Temp);
    /// Rename a file or folder. Refused when another entry is at `to`.
    async fn rename(&mut self, from: &str, to: &str) -> io::Result<()>;
    async fn make_dir(&mut self, path: &str) -> io::Result<()>;
    async fn remove_file(&mut self, path: &str) -> io::Result<()>;
    /// Only an empty folder.
    async fn remove_dir(&mut self, path: &str) -> io::Result<()>;
    /// The file at `path` indexed and left on disk, to be read by range, where this
    /// backend reads files that way and the file is a piano or sample instrument. `None`
    /// has it read whole. `known` is its fingerprint where its [`Stat`] has not moved
    /// since, so its CRC need not be taken again.
    async fn rest(
        &self,
        _path: &str,
        _known: Option<Fingerprint>,
    ) -> io::Result<Option<Arc<OnDisk>>> {
        Ok(None)
    }
}

/// What a write stages before it puts the file at its path.
pub enum Staged<'a> {
    Bytes(&'a [u8]),
    /// A copy of a file outside the library, never held whole.
    Outside(&'a Outside),
    /// A copy of the bytes `range` of a file outside the library, never held whole.
    Part(&'a super::Part),
    /// A copy of the library's own file at this path, never held whole.
    Library(&'a str),
    /// The file an edit makes of a piano or sample instrument resting in the library,
    /// read by range and never held whole.
    Edited(&'a OnDisk, &'a Rewrite),
}

/// What a write may put its file over.
#[derive(Clone, Copy, Debug)]
pub enum Over {
    /// Nothing: the path must be free.
    Nothing,
    /// The file there, while it has this [`Stat`].
    Held(Stat),
}

impl Over {
    /// Whether anything may be at the path already.
    pub fn replaces(self) -> bool {
        !matches!(self, Over::Nothing)
    }
}

/// Write `what` to `path` through a temporary, over what `over` allows. A held file whose
/// stat moved by the time the temporary is written is refused with [`moved`], and nothing
/// is put over it.
///
/// ⚠️ A window remains between that last look and the rename, since no portable call
/// renames only over a file that is still the one looked at. It is one stat and one
/// rename long, after the whole write rather than before it. A backend that copies
/// instead looks again itself, just before the copy lands, which leaves a window as
/// long as that look.
async fn put(fs: &mut impl Fs, path: &str, what: Staged<'_>, over: Over) -> io::Result<()> {
    let temp = fs.stage(path, what).await?;
    if let Over::Held(held) = over {
        if !matches!(fs.stat(path).await, Ok(Some(now)) if now == held) {
            fs.discard(temp).await;
            return Err(moved());
        }
    }
    fs.place(temp, path, over).await
}

/// What a write may put its file over, where the file at `path` must still hold what
/// `expect` says: refused with [`Failure::Moved`] where it does not.
async fn held(fs: &impl Fs, path: &LibPath, expect: &Fingerprint) -> Result<Over, Failure> {
    let io = |e: io::Error| Failure::Io(e.to_string());
    if still(fs, path, expect).await.map_err(io)? != Some(true) {
        return Err(Failure::Moved);
    }
    let stat = fs.stat(path.as_str()).await.map_err(io)?;
    Ok(Over::Held(stat.ok_or(Failure::Moved)?))
}

/// Why a write refused to put its file over one whose stat moved while it was staged.
#[derive(Debug)]
struct Moved;

impl std::fmt::Display for Moved {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("the file changed on disk while its new contents were written")
    }
}

impl std::error::Error for Moved {}

pub fn moved() -> io::Error {
    io::Error::other(Moved)
}

/// A write refused for a target that is not the file it was to go over, or a name taken
/// first, as [`Failure::Moved`]; any other as why.
fn refusal(e: io::Error) -> Failure {
    let taken = e.kind() == io::ErrorKind::AlreadyExists;
    match taken || e.get_ref().is_some_and(|inner| inner.is::<Moved>()) {
        true => Failure::Moved,
        false => Failure::Io(e.to_string()),
    }
}

/// Run one command on a thread that may wait for it, as [`run`] does.
#[cfg(not(target_arch = "wasm32"))]
pub fn execute(fs: &mut impl Fs, cmd: FileCmd, answer: &mut impl FnMut(Event)) {
    nord_usb::block_on(run(fs, cmd, answer))
}

/// Run one command, handing its answer to `answer`. The commands that answer only on
/// failure say nothing when they succeed.
pub async fn run(fs: &mut impl Fs, cmd: FileCmd, answer: &mut impl FnMut(Event)) {
    let answered = match cmd {
        FileCmd::Read { files, room } => Some(Event::Read(read_all(fs, files, room).await)),
        FileCmd::Save {
            id,
            path,
            bytes,
            expect,
        } => {
            let result = save(fs, &path, &bytes, expect).await;
            Some(Event::Saved { id, path, result })
        }
        FileCmd::Rewrite {
            id,
            path,
            from,
            edit,
            expect,
        } => {
            let result = rewrite_over(fs, &path, &from, &edit, expect).await;
            Some(Event::Rewritten { id, path, result })
        }
        FileCmd::Move { from, to } => {
            let result = fs.rename(from.as_str(), to.as_str()).await;
            let result = result.map_err(|e| e.to_string());
            Some(Event::Moved { from, to, result })
        }
        FileCmd::Import {
            id,
            path,
            from,
            expect,
        } => {
            let result = import(fs, &path, &from, expect).await;
            Some(Event::Imported { id, path, result })
        }
        FileCmd::MakeDir(path) => fs
            .make_dir(path.as_str())
            .await
            .err()
            .map(|e| Event::Failed(format!("making the folder {path}: {e}"))),
        FileCmd::RemoveFile { path, expect } => remove(fs, &path, expect)
            .await
            .err()
            .map(|why| Event::Failed(format!("deleting {path}: {why}"))),
        FileCmd::RemoveDir(path) => remove_dir(fs, &path)
            .await
            .err()
            .map(|e| Event::Failed(format!("removing the folder {path}: {e}"))),
    };
    if let Some(event) = answered {
        answer(event);
    }
}

/// The files [`FileCmd::Read`] asks for, read in order until `room` runs out.
async fn read_all(
    fs: &impl Fs,
    files: Vec<(u64, LibPath, Option<Fingerprint>)>,
    mut room: u64,
) -> Vec<(u64, Result<Found, Failure>)> {
    let mut answers = Vec::new();
    for (id, path, print) in files {
        let answer = read_one(fs, path, print, room).await;
        if let Ok(found) = &answer {
            let read = found.bytes.as_ref().map_or(0, |bytes| bytes.len() as u64);
            room = room.saturating_sub(read);
        }
        answers.push((id, answer));
    }
    answers
}

async fn read_one(
    fs: &impl Fs,
    path: LibPath,
    print: Option<Fingerprint>,
    room: u64,
) -> Result<Found, Failure> {
    let io = |e: io::Error| match e.kind() {
        io::ErrorKind::NotFound => Failure::Moved,
        _ => Failure::Io(e.to_string()),
    };
    let stat = fs.stat(path.as_str()).await.map_err(io)?;
    let stat = stat.ok_or(Failure::Moved)?;
    let unmoved = print.filter(|print| print.stat() == stat);
    let file = fs.rest(path.as_str(), unmoved).await.map_err(io)?;
    let bytes = match file {
        Some(_) => None,
        None if stat.len > room => return Err(Failure::Room(stat.len)),
        None => Some(fs.read(path.as_str()).await.map_err(io)?),
    };
    Ok(Found {
        bytes,
        file,
        ..Found::unread(path, stat)
    })
}

/// Why a read [`Failure::Room`] refused did not run.
pub fn too_much() -> String {
    format!(
        "drawbar holds at most {} GiB of one library's files in memory, and the files open, \
         selected, in view, unsaved or waiting to be sent already come to that",
        MOST_BYTES >> 30
    )
}

/// Whether the file at `path` still holds what `expect` says, taking its CRC only when its
/// [`Stat`] moved to one of the same length, and `expect` has a CRC to compare.
async fn still(fs: &impl Fs, path: &LibPath, expect: &Fingerprint) -> io::Result<Option<bool>> {
    let Some(stat) = fs.stat(path.as_str()).await? else {
        return Ok(None);
    };
    if stat == expect.stat() {
        return Ok(Some(true));
    }
    let Some((len, crc)) = expect.contents().filter(|(len, _)| *len == stat.len) else {
        return Ok(Some(false));
    };
    Ok(Some((stat.len, fs.crc(path.as_str()).await?) == (len, crc)))
}

/// Write `bytes` to `path` through a temporary, over what `expect` allows.
async fn save(
    fs: &mut impl Fs,
    path: &LibPath,
    bytes: &[u8],
    expect: Option<Fingerprint>,
) -> Result<Fingerprint, Failure> {
    let io = |e: io::Error| Failure::Io(e.to_string());
    let over = match expect {
        None => Over::Nothing,
        Some(expect) => held(fs, path, &expect).await?,
    };
    put(fs, path.as_str(), Staged::Bytes(bytes), over)
        .await
        .map_err(refusal)?;
    let stat = fs
        .stat(path.as_str())
        .await
        .map_err(io)?
        .ok_or_else(|| Failure::Io("the file was gone as soon as it was written".into()))?;
    Ok(Fingerprint::of(stat, bytes))
}

/// Write the file `edit` makes of `from` over the file at `path`, which must still hold
/// what `expect` says, and find it there as a listing would.
async fn rewrite_over(
    fs: &mut impl Fs,
    path: &LibPath,
    from: &OnDisk,
    edit: &Rewrite,
    expect: Fingerprint,
) -> Result<Found, Failure> {
    let over = held(fs, path, &expect).await?;
    let wrote = put(fs, path.as_str(), Staged::Edited(from, edit), over).await;
    wrote.map_err(|e| match rewrite::is_changed(&e) {
        true => Failure::Moved,
        false => refusal(e),
    })?;
    landed(fs, path).await
}

/// The file just written at `path`, as a listing finds it: resting, where it is a piano
/// or sample instrument, and otherwise unread.
async fn landed(fs: &impl Fs, path: &LibPath) -> Result<Found, Failure> {
    let io = |e: io::Error| Failure::Io(e.to_string());
    let stat = fs
        .stat(path.as_str())
        .await
        .map_err(io)?
        .ok_or_else(|| Failure::Io("the file was gone as soon as it was written".into()))?;
    let file = fs.rest(path.as_str(), None).await.map_err(io)?;
    Ok(Found {
        file,
        ..Found::unread(path.clone(), stat)
    })
}

/// Copy `from` into the library at `path`, and find it there as a listing would:
/// resting, where it is a piano or sample instrument, and otherwise unread.
async fn import(
    fs: &mut impl Fs,
    path: &LibPath,
    from: &Source,
    expect: Option<Fingerprint>,
) -> Result<Found, Failure> {
    let io = |e: io::Error| Failure::Io(e.to_string());
    let changed = || Failure::Io("the file it copies changed on disk".to_string());
    if let Source::Library(source, held) = from {
        if still(fs, source, held).await.map_err(io)? != Some(true) {
            return Err(changed());
        }
    }
    let over = match expect {
        None => Over::Nothing,
        Some(expect) => held(fs, path, &expect).await?,
    };
    let what = match from {
        Source::Outside(file) => Staged::Outside(file),
        Source::Part(part) => Staged::Part(part),
        Source::Library(source, _) => Staged::Library(source.as_str()),
        Source::Edited(file, edit) => Staged::Edited(file, edit),
    };
    match put(fs, path.as_str(), what, over).await {
        Err(e) if rewrite::is_changed(&e) => return Err(changed()),
        copied => copied.map_err(refusal)?,
    }
    landed(fs, path).await
}

/// Remove a folder drawbar has emptied. macOS leaves a `.DS_Store` in any folder Finder
/// has shown, which would otherwise keep it from being removed.
async fn remove_dir(fs: &mut impl Fs, path: &LibPath) -> io::Result<()> {
    if fs.names(path.as_str()).await? == [".DS_Store"] {
        fs.remove_file(&format!("{path}/.DS_Store")).await?;
    }
    fs.remove_dir(path.as_str()).await
}

async fn remove(fs: &mut impl Fs, path: &LibPath, expect: Fingerprint) -> Result<(), String> {
    match still(fs, path, &expect).await.map_err(|e| e.to_string())? {
        None => Ok(()),
        Some(false) => Err("it changed on disk since drawbar read it, so it was left".into()),
        Some(true) => fs
            .remove_file(path.as_str())
            .await
            .map_err(|e| e.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::future::Future;

    use super::*;
    #[cfg(not(target_arch = "wasm32"))]
    use crate::testing::{on_disk, sample_bytes, Temp};

    /// A library whose files claim whatever size they are given, without taking it. The
    /// ones in `rests` are left in place, as the desktop leaves a sample instrument. A
    /// write staged for the path `meddles` names changes that file to the length it
    /// gives, as a program outside drawbar would while the write runs.
    struct Claimed {
        files: BTreeMap<String, u64>,
        meddles: Option<(String, u64)>,
        rests: BTreeMap<String, Arc<OnDisk>>,
    }

    impl Claimed {
        fn of(files: impl IntoIterator<Item = (String, u64)>) -> Claimed {
            Claimed {
                files: files.into_iter().collect(),
                meddles: None,
                rests: BTreeMap::new(),
            }
        }
    }

    fn stat(len: u64) -> Stat {
        Stat {
            len,
            modified: Some(1),
        }
    }

    fn refused() -> io::Error {
        io::ErrorKind::Unsupported.into()
    }

    impl Fs for Claimed {
        async fn names(&self, _: &str) -> io::Result<Vec<String>> {
            Ok(Vec::new())
        }
        async fn crc(&self, path: &str) -> io::Result<u32> {
            self.read(path)
                .await
                .map(|bytes| nord_format::crc::crc32(&bytes))
        }
        async fn read(&self, path: &str) -> io::Result<Vec<u8>> {
            Ok(path.as_bytes().to_vec())
        }
        async fn stat(&self, path: &str) -> io::Result<Option<Stat>> {
            Ok(self.files.get(path).map(|len| stat(*len)))
        }
        type Temp = (String, u64);

        async fn stage(&mut self, path: &str, what: Staged<'_>) -> io::Result<(String, u64)> {
            let Staged::Bytes(bytes) = what else {
                return Err(refused());
            };
            if let Some((at, len)) = self.meddles.take_if(|(at, _)| at == path) {
                self.files.insert(at, len);
            }
            Ok((path.to_string(), bytes.len() as u64))
        }

        async fn place(
            &mut self,
            (_, len): (String, u64),
            path: &str,
            over: Over,
        ) -> io::Result<()> {
            if !over.replaces() && self.files.contains_key(path) {
                return Err(io::ErrorKind::AlreadyExists.into());
            }
            self.files.insert(path.to_string(), len);
            Ok(())
        }

        async fn discard(&mut self, _: (String, u64)) {}

        async fn rename(&mut self, _: &str, _: &str) -> io::Result<()> {
            Err(refused())
        }
        async fn make_dir(&mut self, _: &str) -> io::Result<()> {
            Err(refused())
        }
        async fn remove_file(&mut self, _: &str) -> io::Result<()> {
            Err(refused())
        }
        async fn remove_dir(&mut self, _: &str) -> io::Result<()> {
            Err(refused())
        }
        async fn rest(
            &self,
            path: &str,
            _: Option<Fingerprint>,
        ) -> io::Result<Option<Arc<OnDisk>>> {
            Ok(self.rests.get(path).cloned())
        }
    }

    /// The answer of a future that never waits.
    fn now<T>(future: impl Future<Output = T>) -> T {
        let mut future = std::pin::pin!(future);
        let mut cx = std::task::Context::from_waker(std::task::Waker::noop());
        match future.as_mut().poll(&mut cx) {
            std::task::Poll::Ready(answer) => answer,
            std::task::Poll::Pending => panic!("the stand-in never waits"),
        }
    }

    fn path(text: &str) -> LibPath {
        LibPath::parse(text).unwrap()
    }

    /// A file changed outside drawbar while a save's new contents are written is not
    /// written over: the save is refused as moved, and what was written there stays.
    #[test]
    fn a_save_over_a_file_changed_while_it_was_staged_is_refused() {
        let mut fs = Claimed {
            meddles: Some(("Grand.ne5p".to_string(), 99)),
            ..Claimed::of([("Grand.ne5p".to_string(), 10)])
        };
        let expect = Fingerprint::unread(stat(10));
        let saved = now(save(&mut fs, &path("Grand.ne5p"), b"new", Some(expect)));
        assert_eq!(saved, Err(Failure::Moved));
        assert_eq!(fs.files["Grand.ne5p"], 99, "what was written there stays");
    }

    /// A read asked for leaves a sample instrument in its file, which takes none of the
    /// room, and refuses a file that does not fit what room is left.
    #[cfg(not(target_arch = "wasm32"))]
    #[test]
    fn a_read_leaves_an_instrument_in_place_and_stops_at_the_room_left() {
        let dir = Temp::new();
        let file = on_disk(&dir, "Marimba.nsmp", &sample_bytes());
        let fs = Claimed {
            rests: BTreeMap::from([("Marimba.nsmp".to_string(), file)]),
            ..Claimed::of([
                ("Marimba.nsmp".to_string(), MOST_BYTES),
                ("Small.ne5p".to_string(), 10),
                ("Large.ne5p".to_string(), 20),
            ])
        };
        let asked = ["Marimba.nsmp", "Small.ne5p", "Large.ne5p"]
            .into_iter()
            .enumerate()
            .map(|(id, at)| (id as u64, path(at), None))
            .collect();
        let read = now(read_all(&fs, asked, 25));
        let what: Vec<(u64, &str)> = read
            .iter()
            .map(|(id, answer)| {
                let held = match answer {
                    Ok(found) if found.file.is_some() => "resting",
                    Ok(found) if found.bytes.is_some() => "whole",
                    Ok(_) => "nothing",
                    Err(_) => "refused",
                };
                (*id, held)
            })
            .collect();
        assert_eq!(what, [(0, "resting"), (1, "whole"), (2, "refused")]);
    }
}
