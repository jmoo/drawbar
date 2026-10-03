//! The desktop's files, written and read by position through a handle.

use std::fs::File;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use nord_usb::envelope::Positional;
use nord_usb::{FileSink, FileSource};

use super::{all_taken, numbered, NAMES};

/// Where new files go: the folder the app last named, shared with the worker thread, and
/// drawbar's own data for where none is named or it cannot take one.
#[derive(Clone)]
pub struct Scratch {
    dir: Arc<Mutex<Option<PathBuf>>>,
    shelf: Option<PathBuf>,
}

impl Default for Scratch {
    fn default() -> Scratch {
        Scratch {
            dir: Arc::default(),
            shelf: eframe::storage_dir(crate::APP).map(|data| data.join(SHELF)),
        }
    }
}

/// The folder of drawbar's own data a file goes to where no library can take it. A file
/// there may be a slot's only copy, so nothing sweeps it.
const SHELF: &str = "rescued";

impl Scratch {
    /// [`Scratch::default`], putting the files no library takes in `shelf`.
    #[cfg(test)]
    pub fn shelved_in(shelf: PathBuf) -> Scratch {
        Scratch {
            shelf: Some(shelf),
            ..Scratch::default()
        }
    }

    /// Make new files in `dir` from now on, a writable library's `.drawbar/tmp/`, or in
    /// drawbar's own data with `None`.
    pub fn keep_in(&self, dir: Option<PathBuf>) {
        *self.dir.lock().expect("unpoisoned") = dir;
    }

    /// A new file named `name`, never one already there: in the folder named last, made
    /// where it is missing, or else in drawbar's own data. Never in the system's
    /// temporary folder, which may be emptied under a file that is a slot's only copy.
    pub async fn create(&self, name: &str) -> io::Result<Kept> {
        let dir = self.dir.lock().expect("unpoisoned").clone();
        let made = |dir: &Path| std::fs::create_dir_all(dir).and_then(|()| Kept::new(dir, name));
        if let Some(Ok(kept)) = dir.as_deref().map(made) {
            return Ok(kept);
        }
        let shelf = self.shelf.as_deref().ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::NotFound,
                "this system names no folder for drawbar's own data",
            )
        })?;
        made(shelf)
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::Temp;

    /// A library never written has no `.drawbar/tmp/` yet: it is made, and the file goes
    /// there.
    #[test]
    fn a_file_goes_to_a_fresh_librarys_tmp_folder() {
        let (library, shelf) = (Temp::new(), Temp::new());
        let scratch = Scratch::shelved_in(shelf.0.clone());
        scratch.keep_in(Some(library.at(".drawbar/tmp")));
        let kept = nord_usb::block_on(scratch.create("Grand.npno")).unwrap();
        assert_eq!(
            kept.place(),
            library.at(".drawbar/tmp/Grand.npno").display().to_string()
        );
    }

    /// With no library to take it, a file goes to drawbar's own data, made where missing.
    #[test]
    fn a_file_no_library_takes_goes_to_drawbars_own_data() {
        let data = Temp::new();
        let shelf = data.at(SHELF);
        let scratch = Scratch::shelved_in(shelf.clone());
        scratch.keep_in(None);
        let kept = nord_usb::block_on(scratch.create("Grand.npno")).unwrap();
        assert_eq!(kept.place(), shelf.join("Grand.npno").display().to_string());
    }
}
