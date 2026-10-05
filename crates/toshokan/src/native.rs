//! The file system of the machine, through `std::fs`.

use std::path::{Path, PathBuf};

use crate::error::Result;
use crate::fs::{Capabilities, DirEntry, Fs, Metadata, RelPath};

/// The library folder at `root` on the local file system.
pub struct NativeFs {
    root: PathBuf,
}

impl NativeFs {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    pub fn root(&self) -> &Path {
        &self.root
    }
}

impl Fs for NativeFs {
    fn capabilities(&self) -> Capabilities {
        todo!()
    }

    async fn metadata(&self, path: &RelPath) -> Result<Option<Metadata>> {
        let _ = path;
        todo!()
    }

    async fn list(&self, dir: &RelPath) -> Result<Vec<DirEntry>> {
        let _ = dir;
        todo!()
    }

    async fn read(&self, path: &RelPath) -> Result<Vec<u8>> {
        let _ = path;
        todo!()
    }

    async fn read_at(&self, path: &RelPath, offset: u64, len: usize) -> Result<Vec<u8>> {
        let _ = (path, offset, len);
        todo!()
    }

    async fn create_dir_all(&self, dir: &RelPath) -> Result<()> {
        let _ = dir;
        todo!()
    }

    async fn create(&self, path: &RelPath, bytes: &[u8]) -> Result<()> {
        let _ = (path, bytes);
        todo!()
    }

    async fn append(&self, path: &RelPath, bytes: &[u8]) -> Result<()> {
        let _ = (path, bytes);
        todo!()
    }

    async fn rename(&self, from: &RelPath, to: &RelPath) -> Result<()> {
        let _ = (from, to);
        todo!()
    }

    async fn hard_link(&self, from: &RelPath, to: &RelPath) -> Result<()> {
        let _ = (from, to);
        todo!()
    }

    async fn remove_file(&self, path: &RelPath) -> Result<()> {
        let _ = path;
        todo!()
    }

    async fn remove_dir(&self, path: &RelPath) -> Result<()> {
        let _ = path;
        todo!()
    }

    async fn sync(&self, path: &RelPath) -> Result<()> {
        let _ = path;
        todo!()
    }
}
