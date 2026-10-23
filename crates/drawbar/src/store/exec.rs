//! Reading a library's files for [`super::Cmd::Read`], the same way whichever backend
//! holds them. Every change to the files is an [`super::Intent`], which toshokan writes.

use std::io;
use std::sync::Arc;

use super::{Failure, Fingerprint, Found, LibPath, Stat};
use crate::ondisk::OnDisk;

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
    async fn read(&self, path: &str) -> io::Result<Vec<u8>>;
    /// The file at `path`, or `None` where no file is: nothing, a folder, or a link.
    async fn stat(&self, path: &str) -> io::Result<Option<Stat>>;
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
    /// The identity of the file at `path`, which is left resting, where this backend
    /// can take it without reading the file whole.
    async fn identity(&self, _path: &LibPath) -> io::Result<Option<toshokan::Identity>> {
        Ok(None)
    }
}

/// The files [`super::Cmd::Read`] asks for, read in order until `room` runs out.
pub async fn read_all(
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
    let (bytes, identity) = match file {
        Some(_) => (None, fs.identity(&path).await.map_err(io)?),
        None if stat.len > room => return Err(Failure::Room(stat.len)),
        None => {
            let bytes = fs.read(path.as_str()).await.map_err(io)?;
            let identity = super::keys::identity_of_bytes(&path, &bytes);
            (Some(bytes), identity)
        }
    };
    Ok(Found {
        bytes,
        file,
        identity,
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

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::future::Future;

    use super::*;
    #[cfg(not(target_arch = "wasm32"))]
    use crate::testing::{on_disk, sample_bytes, Temp};

    /// A library whose files claim whatever size they are given, without taking it. The
    /// ones in `rests` are left in place, as the desktop leaves a sample instrument.
    struct Claimed {
        files: BTreeMap<String, u64>,
        rests: BTreeMap<String, Arc<OnDisk>>,
    }

    impl Claimed {
        fn of(files: impl IntoIterator<Item = (String, u64)>) -> Claimed {
            Claimed {
                files: files.into_iter().collect(),
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

    impl Fs for Claimed {
        async fn read(&self, path: &str) -> io::Result<Vec<u8>> {
            Ok(path.as_bytes().to_vec())
        }
        async fn stat(&self, path: &str) -> io::Result<Option<Stat>> {
            Ok(self.files.get(path).map(|len| stat(*len)))
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
