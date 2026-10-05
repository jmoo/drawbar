//! The browser's files, under `.drawbar/tmp/` of its private storage: written through a
//! `library-writer.js` of their own, since the handles that write in place exist only in
//! a worker, and read back by slices of the `File` the browser gives for them.

use std::cell::RefCell;
use std::io;
use std::rc::Rc;

use web_sys::{FileSystemDirectoryHandle, FileSystemFileHandle, FileSystemGetDirectoryOptions};

use nord_usb::{FileSink, FileSource};

use super::{all_taken, numbered, partial, NAMES};
use crate::store::{buffer, move_to, private_root, settle, Writer, TMP};

/// The writer new files are written through, started with the first.
#[derive(Clone, Default)]
pub struct Scratch {
    writer: Rc<RefCell<Option<Rc<Writer>>>>,
}

impl Scratch {
    fn writer(&self) -> io::Result<Rc<Writer>> {
        let mut held = self.writer.borrow_mut();
        if held.is_none() {
            *held = Some(Rc::new(Writer::start()?));
        }
        Ok(held.clone().expect("started above"))
    }

    /// A new file to be named `name` under `.drawbar/tmp/`, never over one already there,
    /// under its partial name until [`Kept::finish`].
    pub async fn create(&self, name: &str) -> io::Result<Kept> {
        let dir = tmp().await?;
        let writer = self.writer()?;
        for attempt in 0..NAMES {
            let named = numbered(name, attempt);
            let leaf = partial(&named);
            if !free(&dir, &named).await? || !free(&dir, &leaf).await? {
                continue;
            }
            let path = format!("{TMP}/{leaf}");
            writer.ask("begin", &path, &[]).await?;
            return Ok(Kept {
                writer,
                dir,
                leaf,
                path,
                named,
            });
        }
        Err(all_taken(name))
    }
}

/// Whether no file is called `leaf` in `dir`.
async fn free(dir: &FileSystemDirectoryHandle, leaf: &str) -> io::Result<bool> {
    match settle::<FileSystemFileHandle>(dir.get_file_handle(leaf)).await {
        Ok(_) => Ok(false),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(true),
        Err(e) => Err(e),
    }
}

/// `.drawbar/tmp/` of the private storage, made where it is missing.
async fn tmp() -> io::Result<FileSystemDirectoryHandle> {
    let options = FileSystemGetDirectoryOptions::new();
    options.set_create(true);
    let mut dir = private_root().await?;
    for name in TMP.split('/') {
        dir = settle(dir.get_directory_handle_with_options(name, &options)).await?;
    }
    Ok(dir)
}

/// One file made by [`Scratch::create`], held open by the writer until it is closed.
pub struct Kept {
    writer: Rc<Writer>,
    dir: FileSystemDirectoryHandle,
    /// Its partial name until [`Kept::finish`], then its own.
    leaf: String,
    /// From the private storage's root, as the writer names files.
    path: String,
    /// The name [`Kept::finish`] gives it.
    named: String,
}

impl Kept {
    /// Where the file is, in words a person can follow to it.
    pub fn place(&self) -> String {
        format!("{} in this browser's storage for drawbar", self.path)
    }

    /// Flush what was written and let the file go.
    pub async fn close(&mut self) -> io::Result<()> {
        self.writer.ask("end", &self.path, &[]).await.map(|_| ())
    }

    /// Give the file, closed, its own name. The browser offers no sync, so the move is
    /// ordered after the writes and no more.
    pub async fn finish(&mut self) -> io::Result<()> {
        let handle: FileSystemFileHandle = settle(self.dir.get_file_handle(&self.leaf)).await?;
        move_to(&handle, &self.dir, &self.named).await?;
        self.leaf = self.named.clone();
        self.path = format!("{TMP}/{}", self.leaf);
        Ok(())
    }

    /// The file, read from its start. It must be closed first.
    pub async fn source(&self) -> io::Result<impl FileSource> {
        let handle: FileSystemFileHandle = settle(self.dir.get_file_handle(&self.leaf)).await?;
        let file: web_sys::File = settle(handle.get_file()).await?;
        Ok(Slices {
            len: file.size() as u64,
            file,
        })
    }

    /// The file, as one to copy into the library from outside it. It must be closed
    /// first.
    pub async fn outside(&self) -> io::Result<crate::store::Outside> {
        let handle: FileSystemFileHandle = settle(self.dir.get_file_handle(&self.leaf)).await?;
        settle(handle.get_file()).await
    }

    /// Let the file go, if it is held, and delete it.
    pub async fn remove(self) -> io::Result<()> {
        self.writer
            .ask("abandon", &self.path, &[])
            .await
            .map(|_| ())
    }
}

impl FileSink for Kept {
    async fn write_at(&mut self, offset: u64, buf: &[u8]) -> io::Result<()> {
        let at = (offset as f64).into();
        let data = buffer(buf).into();
        self.writer
            .ask("write", &self.path, &[("at", at), ("data", data)])
            .await
            .map(|_| ())
    }
}

/// A file the browser gave, read by slices.
struct Slices {
    file: web_sys::File,
    len: u64,
}

impl FileSource for Slices {
    fn len(&self) -> u64 {
        self.len
    }

    async fn read_at(&mut self, offset: u64, buf: &mut [u8]) -> io::Result<()> {
        crate::ondisk::slice_into(&self.file, offset, buf).await
    }
}
