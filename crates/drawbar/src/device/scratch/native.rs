//! The desktop's files, written and read by position through a handle.

use std::fs::File;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use nord_usb::envelope::Positional;
use nord_usb::{FileSink, FileSource};

use super::{all_taken, numbered, NAMES};

/// Where new files go: the folder the app last named, shared with the worker thread.
#[derive(Clone, Default)]
pub struct Scratch {
    dir: Arc<Mutex<Option<PathBuf>>>,
}

impl Scratch {
    /// Make new files in `dir` from now on, or in the system's temporary folder with
    /// `None`.
    pub fn keep_in(&self, dir: Option<PathBuf>) {
        *self.dir.lock().expect("unpoisoned") = dir;
    }

    /// A new file named `name`, never one already there. A folder that cannot take it,
    /// such as a library's `tmp/` drawbar has not made yet, sends it to the system's
    /// temporary folder.
    pub async fn create(&self, name: &str) -> io::Result<Kept> {
        let dir = self.dir.lock().expect("unpoisoned").clone();
        match dir.map(|dir| Kept::new(&dir, name)) {
            Some(Ok(kept)) => Ok(kept),
            Some(Err(_)) | None => Kept::new(&std::env::temp_dir(), name),
        }
    }
}

/// One file made by [`Scratch::create`], open for writing.
pub struct Kept {
    path: PathBuf,
    file: File,
}

impl Kept {
    fn new(dir: &Path, name: &str) -> io::Result<Kept> {
        for attempt in 0..NAMES {
            let path = dir.join(numbered(name, attempt));
            match File::options()
                .read(true)
                .write(true)
                .create_new(true)
                .open(&path)
            {
                Ok(file) => return Ok(Kept { path, file }),
                Err(e) if e.kind() == io::ErrorKind::AlreadyExists => continue,
                Err(e) => return Err(e),
            }
        }
        Err(all_taken(name))
    }

    /// Where the file is, in words a person can follow to it.
    pub fn place(&self) -> String {
        self.path.display().to_string()
    }

    /// Put what was written on the disk.
    pub async fn close(&mut self) -> io::Result<()> {
        self.file.sync_all()
    }

    /// The file, read from its start.
    pub async fn source(&self) -> io::Result<impl FileSource> {
        Positional::new(File::open(&self.path)?)
    }

    pub async fn remove(self) -> io::Result<()> {
        drop(self.file);
        std::fs::remove_file(&self.path)
    }
}

impl FileSink for Kept {
    async fn write_at(&mut self, offset: u64, buf: &[u8]) -> io::Result<()> {
        FileSink::write_at(&mut self.file, offset, buf).await
    }
}
