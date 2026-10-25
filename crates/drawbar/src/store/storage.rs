//! What the keeper asks of a library's storage itself, outside toshokan's intents:
//! listings and reads of the folder, and this install's own files in the local root.

use std::io;

use toshokan::asynch::Fs;
use toshokan::io::{IoError, Kind, Meta, Range, Reply, Root};
use toshokan::{Identify as _, Identity, Io, RelPath};

use super::keys::Contents;
use super::Stat;

/// How much of a file one read asks for.
pub const CHUNK: u64 = 4 << 20;

/// `error` as the app tells it.
pub fn failed(error: IoError) -> io::Error {
    match error {
        IoError::NotFound => io::ErrorKind::NotFound.into(),
        IoError::AlreadyExists => io::ErrorKind::AlreadyExists.into(),
        IoError::Denied => io::Error::new(io::ErrorKind::PermissionDenied, error.to_string()),
        error => io::Error::other(error.to_string()),
    }
}

async fn ask<F: Fs>(fs: &F, io: Io) -> io::Result<Reply> {
    fs.perform(io).await.map_err(failed)
}

fn wrong() -> io::Error {
    io::Error::other("the storage gave the wrong reply")
}

/// What is at `path`, file or directory.
pub async fn meta<F: Fs>(fs: &F, root: Root, path: &RelPath) -> io::Result<Option<Meta>> {
    let path = path.clone();
    match ask(fs, Io::Stat { root, path }).await? {
        Reply::Stat(meta) => Ok(meta),
        _ => Err(wrong()),
    }
}

/// The file at `path` in the folder, as a listing finds it; `None` where no file is.
pub async fn stat<F: Fs>(fs: &F, path: &RelPath) -> io::Result<Option<Stat>> {
    Ok(file(meta(fs, Root::Folder, path).await?))
}

/// What a stat found, where it found a file.
pub fn file(meta: Option<Meta>) -> Option<Stat> {
    meta.filter(|meta| meta.kind == Kind::File)
        .map(|meta| Stat {
            len: meta.len,
            modified: meta.modified,
        })
}

/// At most `len` bytes of the file at `path` from `offset`.
pub async fn read<F: Fs>(fs: &F, root: Root, path: &RelPath, range: Range) -> io::Result<Vec<u8>> {
    let path = path.clone();
    match ask(fs, Io::Read { root, path, range }).await? {
        Reply::Bytes(bytes) => Ok(bytes),
        _ => Err(wrong()),
    }
}

/// The entries of the directory `dir`, each with what a stat finds.
pub async fn list<F: Fs>(fs: &F, root: Root, dir: &RelPath) -> io::Result<Vec<(String, Meta)>> {
    let dir = dir.clone();
    match ask(fs, Io::ListStat { root, dir }).await? {
        Reply::ListedStat(entries) => Ok(entries),
        _ => Err(wrong()),
    }
}

/// The identity of the folder's file at `path`, `len` bytes long, read through `read` by
/// the ranges the library takes it by.
pub async fn identity(
    path: &RelPath,
    len: u64,
    mut read: impl AsyncFnMut(Range) -> io::Result<Vec<u8>>,
) -> io::Result<Identity> {
    let mut digest = Contents.digest(path, len);
    for range in Contents.ranges(path, len) {
        let mut at = range.offset;
        let end = range.offset + range.len;
        while at < end {
            let len = CHUNK.min(end - at);
            let bytes = read(Range { offset: at, len }).await?;
            if bytes.len() as u64 != len {
                return Err(io::Error::other("the file changed while it was read"));
            }
            digest.update(&bytes);
            at += len;
        }
    }
    Ok(digest.finish())
}

/// Remove the file at `path`; none there is success.
pub async fn remove<F: Fs>(fs: &F, root: Root, path: &RelPath) -> io::Result<()> {
    let path = path.clone();
    match fs.perform(Io::Remove { root, path }).await {
        Ok(_) | Err(IoError::NotFound) => Ok(()),
        Err(error) => Err(failed(error)),
    }
}

async fn sync<F: Fs>(fs: &F, root: Root, path: &RelPath) -> io::Result<()> {
    if !fs.capabilities(root).fsync {
        return Ok(());
    }
    let path = path.clone();
    ask(fs, Io::Sync { root, path }).await.map(drop)
}

/// The name [`create`] writes a file under before it moves it to its own.
pub const STAGED: &str = ".next";

/// Keep `bytes` as a new file at `path`, where nothing is: written whole under a name
/// [`STAGED`] ends, then moved to its own, so the file is whole wherever it is found.
pub async fn create<F: Fs>(fs: &F, root: Root, path: &RelPath, bytes: Vec<u8>) -> io::Result<()> {
    let dir = path.parent().expect("a file has a parent");
    let name = path.name().expect("a file has a name");
    let staged = dir
        .join(&format!("{name}{STAGED}"))
        .expect("a name with a suffix is one component");
    let made = Io::MakeDir {
        root,
        path: dir.clone(),
    };
    ask(fs, made).await?;
    let create = Io::Create {
        root,
        path: staged.clone(),
        bytes,
    };
    ask(fs, create).await?;
    sync(fs, root, &staged).await?;
    let rename = Io::Rename {
        root,
        from: staged,
        to: path.clone(),
    };
    ask(fs, rename).await?;
    sync(fs, root, &dir).await
}

/// The whole file at `path`; `None` where no file is.
pub async fn read_whole<F: Fs>(fs: &F, root: Root, path: &RelPath) -> io::Result<Option<Vec<u8>>> {
    let Some(meta) = meta(fs, root, path).await? else {
        return Ok(None);
    };
    if meta.kind != Kind::File {
        return Ok(None);
    }
    let mut bytes = Vec::new();
    while (bytes.len() as u64) < meta.len {
        let offset = bytes.len() as u64;
        let len = CHUNK.min(meta.len - offset);
        let read = read(fs, root, path, Range { offset, len }).await?;
        if read.is_empty() {
            break;
        }
        bytes.extend_from_slice(&read);
    }
    Ok(Some(bytes))
}
