//! The browser's files, in a folder of its private storage: the open library's own state
//! for this install, or the browser's library's where none is open. They are written by
//! toshokan's storage worker, since the handles that write in place exist only in a
//! worker, and read back by slices of the `File` the browser gives for them.

use std::cell::RefCell;
use std::io;
use std::rc::Rc;

use nord_usb::{FileSink, FileSource};
use toshokan::asynch::Fs as _;
use toshokan::io::{Io, IoError, Reply, Root};
use toshokan::web::{private_dir, Folder, Worker};
use toshokan::RelPath;
use web_sys::FileSystemFileHandle;

use super::{all_taken, numbered, partial, NAMES};
use crate::store::settle;

/// Where new files go, and the worker that writes there, started with the first.
#[derive(Clone, Default)]
pub struct Scratch {
    dir: Rc<RefCell<Option<RelPath>>>,
    writer: Rc<RefCell<Option<Writing>>>,
}

/// The worker that writes new files, and the folder it writes them in.
type Writing = (RelPath, Rc<Worker>);

impl Scratch {
    /// Make new files in `dir` of the private storage from now on: the open library's
    /// own state for this install, or with `None`, the browser's library's.
    pub fn keep_in(&self, dir: Option<RelPath>) {
        *self.dir.borrow_mut() = dir;
    }

    fn dir(&self) -> RelPath {
        let dir = self.dir.borrow().clone();
        dir.unwrap_or_else(|| {
            crate::store::Root::Private
                .local()
                .join(crate::store::RESCUED)
                .expect("a folder's name is one component")
        })
    }

    async fn writer(&self, dir: &RelPath) -> io::Result<Rc<Worker>> {
        if let Some((held, writer)) = &*self.writer.borrow() {
            if held == dir {
                return Ok(writer.clone());
            }
        }
        let started = Worker::start(Folder::Private(dir.clone()), dir).await;
        let writer = Rc::new(started.map_err(failed)?);
        *self.writer.borrow_mut() = Some((dir.clone(), writer.clone()));
        Ok(writer)
    }

    /// A new file to be named `name`, never over one already there, under its partial
    /// name until [`Kept::finish`].
    pub async fn create(&self, name: &str) -> io::Result<Kept> {
        let dir = self.dir();
        let writer = self.writer(&dir).await?;
        for attempt in 0..NAMES {
            let named = numbered(name, attempt);
            let leaf = partial(&named);
            if !free(&writer, &named).await? || !free(&writer, &leaf).await? {
                continue;
            }
            let path = at(&leaf)?;
            ask(
                &writer,
                Io::Create {
                    root: Root::Folder,
                    path,
                    bytes: Vec::new(),
                },
            )
            .await?;
            return Ok(Kept {
                writer,
                dir,
                leaf,
                named,
            });
        }
        Err(all_taken(name))
    }
}

fn failed(error: IoError) -> io::Error {
    match error {
        IoError::NotFound => io::ErrorKind::NotFound.into(),
        error => io::Error::other(error.to_string()),
    }
}

async fn ask(writer: &Worker, io: Io) -> io::Result<Reply> {
    writer.perform(io).await.map_err(failed)
}

fn at(leaf: &str) -> io::Result<RelPath> {
    RelPath::new(leaf).map_err(|e| io::Error::other(e.to_string()))
}

/// Whether no file is called `leaf` in the folder.
async fn free(writer: &Worker, leaf: &str) -> io::Result<bool> {
    let path = at(leaf)?;
    match ask(
        writer,
        Io::Stat {
            root: Root::Folder,
            path,
        },
    )
    .await?
    {
        Reply::Stat(found) => Ok(found.is_none()),
        _ => Err(io::Error::other("the storage gave the wrong reply")),
    }
}

/// One file made by [`Scratch::create`].
pub struct Kept {
    writer: Rc<Worker>,
    /// The folder of the private storage it is in.
    dir: RelPath,
    /// Its partial name until [`Kept::finish`], then its own.
    leaf: String,
    /// The name [`Kept::finish`] gives it.
    named: String,
}

impl Kept {
    /// Where the file is, in words a person can follow to it.
    pub fn place(&self) -> String {
        format!(
            "{}/{} in this browser's storage for drawbar",
            self.dir, self.leaf
        )
    }

    /// Make what was written durable.
    pub async fn close(&mut self) -> io::Result<()> {
        if !self.writer.capabilities(Root::Folder).fsync {
            return Ok(());
        }
        let path = at(&self.leaf)?;
        ask(
            &self.writer,
            Io::Sync {
                root: Root::Folder,
                path,
            },
        )
        .await
        .map(drop)
    }

    /// Give the file, closed, its own name.
    pub async fn finish(&mut self) -> io::Result<()> {
        let (from, to) = (at(&self.leaf)?, at(&self.named)?);
        ask(
            &self.writer,
            Io::Rename {
                root: Root::Folder,
                from,
                to,
            },
        )
        .await?;
        self.leaf = self.named.clone();
        Ok(())
    }

    async fn file(&self) -> io::Result<web_sys::File> {
        let dir = private_dir(&self.dir).await.map_err(failed)?;
        let handle: FileSystemFileHandle = settle(dir.get_file_handle(&self.leaf)).await?;
        settle(handle.get_file()).await
    }

    /// The file, read from its start. It must be closed first.
    pub async fn source(&self) -> io::Result<impl FileSource> {
        Ok(crate::ondisk::Sliced::new(self.file().await?))
    }

    /// The file, as one to copy into the library from outside it. It must be closed
    /// first.
    pub async fn outside(&self) -> io::Result<crate::store::Outside> {
        self.file().await
    }

    /// Delete the file.
    pub async fn remove(self) -> io::Result<()> {
        let path = at(&self.leaf)?;
        ask(
            &self.writer,
            Io::Remove {
                root: Root::Folder,
                path,
            },
        )
        .await
        .map(drop)
    }
}

impl FileSink for Kept {
    async fn write_at(&mut self, offset: u64, buf: &[u8]) -> io::Result<()> {
        let path = at(&self.leaf)?;
        let write = Io::Write {
            root: Root::Folder,
            path,
            offset,
            bytes: buf.to_vec(),
        };
        ask(&self.writer, write).await.map(drop)
    }
}
