//! The desktop's files, read by position through a handle held open.

use std::fs::File;
use std::io::{self, BufReader, Read, Seek, SeekFrom};
use std::ops::Range;
use std::sync::Arc;

use eframe::egui;

use super::{crc_of, seek, Index, OnDisk, CHUNK};
use crate::work::{self, Job, Progress};

/// Raise the soft limit on open files as far as the system lets this process, since each
/// piano or sample instrument left in its file holds one open, and macOS starts a process
/// at 256. Where the system refuses, the limit stays as it was.
#[cfg(unix)]
pub fn raise_open_files() {
    use rustix::process::{getrlimit, setrlimit, Resource, Rlimit};

    // macOS refuses a soft limit past `OPEN_MAX`, whatever the hard limit; Linux one past
    // `fs.nr_open`, whose default this is.
    const CEILING: u64 = match cfg!(target_os = "macos") {
        true => 10_240,
        false => 1 << 20,
    };
    let limit = getrlimit(Resource::Nofile);
    let wanted = limit.maximum.unwrap_or(u64::MAX).min(CEILING);
    if limit.current.is_none_or(|current| current >= wanted) {
        return;
    }
    let raised = Rlimit {
        current: Some(wanted),
        maximum: limit.maximum,
    };
    let _ = setrlimit(Resource::Nofile, raised);
}

/// The file, held open.
#[derive(Debug)]
pub(super) struct Source {
    file: File,
    len: u64,
}

impl OnDisk {
    /// Index `file` when it is a piano or sample instrument. `None` for any other file,
    /// and for one whose index does not read, which is then read whole so that its decode
    /// says why.
    ///
    /// `crc` is the file's CRC-32 where it is already known; otherwise [`OnDisk::crc`]
    /// takes it when it is first asked for.
    pub fn open(file: File, crc: Option<u32>) -> io::Result<Option<OnDisk>> {
        let len = file.metadata()?.len();
        let Some(index) = Index::read(&mut At::new(&file, len), len)? else {
            return Ok(None);
        };
        Ok(Some(OnDisk::indexed(Source { file, len }, len, crc, index)))
    }

    /// A buffered reader over the whole file, which never moves the handle's own cursor.
    pub fn reader(&self) -> BufReader<At<'_>> {
        self.source.reader()
    }

    /// [`OnDisk::reader`] unbuffered: each read is one read of the file at its position,
    /// for a caller that reads in ranges of its own.
    pub fn at(&self) -> At<'_> {
        At::new(&self.source.file, self.len)
    }
}

impl Source {
    pub(super) fn read(&self, _serial: u64, range: Range<u64>) -> io::Result<Vec<u8>> {
        let want = usize::try_from(range.end - range.start)
            .map_err(|_| io::Error::from(io::ErrorKind::OutOfMemory))?;
        let mut bytes = Vec::new();
        bytes
            .try_reserve_exact(want)
            .map_err(|_| io::Error::from(io::ErrorKind::OutOfMemory))?;
        bytes.resize(want, 0);
        let mut at = At::new(&self.file, self.len);
        at.seek(SeekFrom::Start(range.start))?;
        at.read_exact(&mut bytes)?;
        Ok(bytes)
    }

    pub(super) fn whole(&self, len: u64) -> io::Result<Vec<u8>> {
        self.read(0, 0..len)
    }

    pub(super) async fn crc_now(&self, _serial: u64, _len: u64) -> io::Result<u32> {
        crc_of(&mut self.reader())
    }

    fn reader(&self) -> BufReader<At<'_>> {
        BufReader::with_capacity(CHUNK, At::new(&self.file, self.len))
    }

    pub(super) fn pass<S: Send + 'static, T: Send + 'static>(
        file: &Arc<OnDisk>,
        ctx: &egui::Context,
        mut state: S,
        mut each: impl FnMut(&mut S, &[u8]) -> Result<(), String> + Send + 'static,
        then: impl FnOnce(&Progress, Result<S, String>) -> Result<T, String> + Send + 'static,
    ) -> Job<Result<T, String>> {
        let file = file.clone();
        work::run(ctx, move |progress| {
            progress.say("reading the file");
            let mut reader = file.source.reader();
            let mut chunk = vec![0u8; CHUNK];
            let read = loop {
                match reader.read(&mut chunk) {
                    Ok(0) => break Ok(()),
                    Ok(n) => {
                        if let Err(why) = each(&mut state, &chunk[..n]) {
                            break Err(why);
                        }
                    }
                    Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
                    Err(e) => break Err(e.to_string()),
                }
            };
            then(progress, read.map(|()| state))
        })
    }
}

/// A file read by position, so readers on several threads share one handle.
pub struct At<'a> {
    file: &'a File,
    pos: u64,
    len: u64,
}

impl<'a> At<'a> {
    fn new(file: &'a File, len: u64) -> At<'a> {
        At { file, pos: 0, len }
    }
}

impl Read for At<'_> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let left = self.len.saturating_sub(self.pos);
        let want = buf.len().min(usize::try_from(left).unwrap_or(usize::MAX));
        if want == 0 {
            return Ok(0);
        }
        let n = read_at(self.file, &mut buf[..want], self.pos)?;
        self.pos += n as u64;
        Ok(n)
    }
}

impl Seek for At<'_> {
    fn seek(&mut self, to: SeekFrom) -> io::Result<u64> {
        self.pos = seek(self.pos, self.len, to)?;
        Ok(self.pos)
    }
}

#[cfg(unix)]
fn read_at(file: &File, buf: &mut [u8], at: u64) -> io::Result<usize> {
    std::os::unix::fs::FileExt::read_at(file, buf, at)
}

#[cfg(windows)]
fn read_at(file: &File, buf: &mut [u8], at: u64) -> io::Result<usize> {
    std::os::windows::fs::FileExt::seek_read(file, buf, at)
}

#[cfg(not(any(unix, windows)))]
fn read_at(_: &File, _: &mut [u8], _: u64) -> io::Result<usize> {
    Err(io::ErrorKind::Unsupported.into())
}

#[cfg(all(test, unix))]
mod tests {
    #[test]
    fn the_open_file_limit_is_raised_as_far_as_the_system_allows() {
        use rustix::process::{getrlimit, Resource};

        super::raise_open_files();
        let limit = getrlimit(Resource::Nofile);
        let least = limit.maximum.unwrap_or(u64::MAX).min(10_240);
        assert!(
            limit.current.is_none_or(|current| current >= least),
            "{limit:?}"
        );
    }
}
