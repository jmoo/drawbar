//! An asset left in its file: a piano or sample instrument, which runs to hundreds of
//! megabytes, is read by range through its index rather than held.
//!
//! Only the desktop reads a file by range. The browser build compiles this module but
//! never opens one, and a read there answers [`io::ErrorKind::Unsupported`].

use std::fs::File;
use std::io::{self, BufReader, Read, Seek, SeekFrom};
use std::ops::Range;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::OnceLock;

use nord_format::cbin::{self, Header};
use nord_format::crc::Crc32Stream;
use nord_format::formats::{npno, nsmp};

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

/// Where each stroke or zone's audio sits in the file.
#[derive(Debug)]
pub enum Index {
    Piano(npno::Index),
    Sample(nsmp::Index),
}

impl Index {
    pub fn header(&self) -> &Header {
        match self {
            Index::Piano(index) => &index.library().header,
            Index::Sample(index) => index.header(),
        }
    }

    pub fn tag(&self) -> &'static str {
        match self {
            Index::Piano(_) => npno::FORMAT,
            Index::Sample(_) => nsmp::FORMAT,
        }
    }
}

/// A piano or sample instrument's file, held open, with its index.
///
/// ⚠️ The handle follows the file through a rename, and reads the contents it was indexed
/// over until the file is replaced. A file rewritten in place under it reads as whatever
/// is there now; a rescan notices the change and indexes the file again.
#[derive(Debug)]
pub struct OnDisk {
    file: File,
    /// The file's length when it was indexed.
    pub len: u64,
    /// CRC-32 over every byte of the file, once a pass over all of it has taken it.
    crc: OnceLock<u32>,
    /// Distinct for every file indexed in this run, so two indexings of one path are told
    /// apart.
    pub serial: u64,
    pub index: Index,
    /// Every range [`OnDisk::read`] was asked for.
    #[cfg(test)]
    reads: std::sync::Mutex<Vec<Range<u64>>>,
}

impl OnDisk {
    /// Index `file` when it is a piano or sample instrument. `None` for any other file,
    /// and for one whose index does not read, which is then read whole so that its decode
    /// says why.
    ///
    /// `crc` is the file's CRC-32 where it is already known; otherwise [`OnDisk::crc`]
    /// takes it when it is first asked for.
    pub fn open(file: File, crc: Option<u32>) -> io::Result<Option<OnDisk>> {
        static SERIAL: AtomicU64 = AtomicU64::new(0);
        let len = file.metadata()?.len();
        let mut head = [0u8; 12];
        if len < head.len() as u64 {
            return Ok(None);
        }
        At::new(&file, len).read_exact(&mut head)?;
        if head[..4] != cbin::MAGIC[..] {
            return Ok(None);
        }
        let index = match &head[8..12] {
            tag if tag == npno::FORMAT.as_bytes() => {
                npno::Index::read_from(&mut At::new(&file, len)).map(Index::Piano)
            }
            tag if tag == nsmp::FORMAT.as_bytes() => {
                nsmp::Index::read_from(&mut At::new(&file, len)).map(Index::Sample)
            }
            _ => return Ok(None),
        };
        let Ok(index) = index else {
            return Ok(None);
        };
        Ok(Some(OnDisk {
            file,
            len,
            crc: crc.map(OnceLock::from).unwrap_or_default(),
            serial: SERIAL.fetch_add(1, Ordering::Relaxed),
            index,
            #[cfg(test)]
            reads: Default::default(),
        }))
    }

    /// The bytes at `range`.
    pub fn read(&self, range: Range<u64>) -> io::Result<Vec<u8>> {
        if range.start > range.end || range.end > self.len {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                format!("{range:?} is not within the {}-byte file", self.len),
            ));
        }
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
        #[cfg(test)]
        self.reads.lock().expect("unpoisoned").push(range);
        Ok(bytes)
    }

    /// Every byte of the file.
    pub fn whole(&self) -> io::Result<Vec<u8>> {
        self.read(0..self.len)
    }

    /// A buffered reader over the whole file, which never moves the handle's own cursor.
    pub fn reader(&self) -> BufReader<At<'_>> {
        BufReader::with_capacity(CHUNK, At::new(&self.file, self.len))
    }

    /// CRC-32 over every byte of the file, taken in one streaming pass the first time it
    /// is asked for.
    ///
    /// ⚠️ That pass reads the whole file, hundreds of megabytes for a piano library, on
    /// the calling thread.
    pub fn crc(&self) -> io::Result<u32> {
        if let Some(crc) = self.crc.get() {
            return Ok(*crc);
        }
        let crc = crc_of(&mut self.reader())?;
        Ok(*self.crc.get_or_init(|| crc))
    }

    /// The CRC, where a pass has taken it already.
    pub fn known_crc(&self) -> Option<u32> {
        self.crc.get().copied()
    }

    /// Whether `bytes` are what the file held when it was indexed. Takes the file's CRC
    /// if nothing has yet.
    pub fn holds(&self, bytes: &[u8]) -> bool {
        self.len == bytes.len() as u64
            && self
                .crc()
                .is_ok_and(|crc| crc == nord_format::crc::crc32(bytes))
    }

    /// The ranges read so far, emptied.
    #[cfg(test)]
    pub fn take_reads(&self) -> Vec<Range<u64>> {
        std::mem::take(&mut self.reads.lock().expect("unpoisoned"))
    }
}

/// How much a streaming pass reads at a time.
const CHUNK: usize = 1 << 20;

/// CRC-32 over everything `r` yields.
pub fn crc_of(r: &mut impl Read) -> io::Result<u32> {
    let mut crc = Crc32Stream::new();
    let mut chunk = vec![0u8; CHUNK];
    loop {
        match r.read(&mut chunk) {
            Ok(0) => return Ok(crc.value()),
            Ok(n) => crc.update(&chunk[..n]),
            Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
            Err(e) => return Err(e),
        }
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
        let pos = match to {
            SeekFrom::Start(at) => Some(at),
            SeekFrom::End(delta) => self.len.checked_add_signed(delta),
            SeekFrom::Current(delta) => self.pos.checked_add_signed(delta),
        };
        self.pos = pos.ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::InvalidInput,
                "a seek before the file's start",
            )
        })?;
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
