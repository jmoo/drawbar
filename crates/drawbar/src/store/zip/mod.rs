//! ZIP archives of stored entries, as a copy of a library is kept: laid out from each
//! entry's length and CRC-32 alone, so the bytes go straight from the files, and read
//! back by position, so an archive is never held whole. Only what such a copy needs is
//! read: one disk, no compression, no encryption and no archive comment. ZIP64 records
//! are written only where a size, an offset or the count needs them.

#[cfg(any(target_arch = "wasm32", test))]
mod write;
#[cfg(any(target_arch = "wasm32", test))]
pub use write::{layout, Entry, Piece};

const LOCAL_SIG: u32 = 0x0403_4b50;
const CENTRAL_SIG: u32 = 0x0201_4b50;
const END_SIG: u32 = 0x0605_4b50;
const END64_SIG: u32 = 0x0606_4b50;
const LOCATOR_SIG: u32 = 0x0706_4b50;
const ZIP64_EXTRA: u16 = 0x0001;

/// The length of a local file header before its name.
pub const LOCAL: u64 = 30;
/// The length of the end of central directory record, with no comment.
pub const END: u64 = 22;
/// The length of the ZIP64 end of central directory locator, just before [`END`].
pub const LOCATOR: u64 = 20;
/// The length of the ZIP64 end of central directory record.
pub const END64: u64 = 56;

/// Bits 0, 6 and 13: some kind of encryption.
const ENCRYPTED: u16 = 1 | 1 << 6 | 1 << 13;
const STORED: u16 = 0;

const MAX32: u64 = u32::MAX as u64;
const MAX16: u64 = u16::MAX as u64;

/// Why an archive cannot be read as one of stored entries.
#[derive(Debug, PartialEq, Eq)]
pub enum Unread {
    /// An entry is compressed.
    Compressed,
    /// Anything else, as a phrase.
    Malformed(&'static str),
}

/// Where the central directory is, and how many entries it holds.
#[derive(Debug, PartialEq, Eq)]
pub struct Directory {
    pub offset: u64,
    pub len: u64,
    pub count: u64,
}

/// What the end record says.
#[derive(Debug, PartialEq, Eq)]
pub enum End {
    Directory(Directory),
    /// The directory is named by the ZIP64 records, whose locator is just before.
    Zip64,
}

/// What the last [`END`] bytes of an archive say: an end record with no comment.
pub fn end(record: &[u8]) -> Result<End, Unread> {
    let mut r = Reader::new(record);
    if record.len() as u64 != END || r.u32()? != END_SIG {
        return Err(Unread::Malformed("it does not end as a ZIP archive does"));
    }
    let (disk, start) = (r.u16()?, r.u16()?);
    let (here, count) = (r.u16()?, r.u16()?);
    let (len, offset) = (r.u32()?, r.u32()?);
    if disk != 0 || start != 0 || here != count {
        return Err(Unread::Malformed("it spans several disks"));
    }
    if r.u16()? != 0 {
        return Err(Unread::Malformed("it has a comment"));
    }
    if u64::from(count) == MAX16 || u64::from(len) == MAX32 || u64::from(offset) == MAX32 {
        return Ok(End::Zip64);
    }
    Ok(End::Directory(Directory {
        offset: offset.into(),
        len: len.into(),
        count: count.into(),
    }))
}

/// Where the ZIP64 end record is, as the [`LOCATOR`] bytes say.
pub fn locator(bytes: &[u8]) -> Result<u64, Unread> {
    let mut r = Reader::new(bytes);
    if bytes.len() as u64 != LOCATOR || r.u32()? != LOCATOR_SIG {
        return Err(Unread::Malformed("its ZIP64 records are missing"));
    }
    let (disk, at, disks) = (r.u32()?, r.u64()?, r.u32()?);
    if disk != 0 || disks != 1 {
        return Err(Unread::Malformed("it spans several disks"));
    }
    Ok(at)
}

/// The directory the [`END64`] bytes of the ZIP64 end record name.
pub fn end64(bytes: &[u8]) -> Result<Directory, Unread> {
    let mut r = Reader::new(bytes);
    if bytes.len() as u64 != END64 || r.u32()? != END64_SIG {
        return Err(Unread::Malformed("its ZIP64 records are missing"));
    }
    let _size = r.u64()?;
    let (_made, _needed) = (r.u16()?, r.u16()?);
    let (disk, start) = (r.u32()?, r.u32()?);
    let (here, count) = (r.u64()?, r.u64()?);
    let (len, offset) = (r.u64()?, r.u64()?);
    if disk != 0 || start != 0 || here != count {
        return Err(Unread::Malformed("it spans several disks"));
    }
    Ok(Directory { offset, len, count })
}

/// One entry, as the central directory lists it.
#[derive(Debug, PartialEq, Eq)]
pub struct Stored {
    pub name: String,
    pub len: u64,
    pub crc: u32,
    /// Where its local header starts.
    pub at: u64,
}

/// The `count` entries of a central directory, which is all of `bytes`.
pub fn directory(bytes: &[u8], count: u64) -> Result<Vec<Stored>, Unread> {
    let mut r = Reader::new(bytes);
    let mut entries = Vec::new();
    for _ in 0..count {
        if r.u32()? != CENTRAL_SIG {
            return Err(Unread::Malformed("its directory is damaged"));
        }
        let (_made, _needed, flags, method) = (r.u16()?, r.u16()?, r.u16()?, r.u16()?);
        let (_time, _date, crc) = (r.u16()?, r.u16()?, r.u32()?);
        let (packed, len) = (r.u32()?, r.u32()?);
        let (name_len, extra_len, comment_len) = (r.u16()?, r.u16()?, r.u16()?);
        let (disk, _internal, _external, at) = (r.u16()?, r.u16()?, r.u32()?, r.u32()?);
        let name = r.take(name_len.into())?;
        let extra = r.take(extra_len.into())?;
        r.take(comment_len.into())?;
        if flags & ENCRYPTED != 0 {
            return Err(Unread::Malformed("it is encrypted"));
        }
        if method != STORED {
            return Err(Unread::Compressed);
        }
        let mut wide = zip64(extra)?;
        let mut field = |value: u32| match u64::from(value) {
            MAX32 => wide.u64(),
            value => Ok(value),
        };
        let (len, packed, at) = (field(len)?, field(packed)?, field(at)?);
        if disk != 0 && u64::from(disk) != MAX16 {
            return Err(Unread::Malformed("it spans several disks"));
        }
        if len != packed {
            return Err(Unread::Malformed("an entry's sizes disagree"));
        }
        let name = std::str::from_utf8(name)
            .map_err(|_| Unread::Malformed("a name is not UTF-8"))?
            .to_string();
        entries.push(Stored { name, len, crc, at });
    }
    if !r.rest().is_empty() {
        return Err(Unread::Malformed("its directory is damaged"));
    }
    Ok(entries)
}

/// The ZIP64 extra field's values, in the order their fields come; empty without one.
fn zip64(mut extra: &[u8]) -> Result<Reader<'_>, Unread> {
    while !extra.is_empty() {
        let mut r = Reader::new(extra);
        let (id, len) = (r.u16()?, r.u16()?);
        let data = r.take(len.into())?;
        if id == ZIP64_EXTRA {
            return Ok(Reader::new(data));
        }
        extra = r.rest();
    }
    Ok(Reader::new(&[]))
}

/// Where the data of `entry` starts, from the first [`LOCAL`] bytes of its local header
/// and the name after them: the header must agree with the directory.
pub fn data_at(header: &[u8], entry: &Stored) -> Result<u64, Unread> {
    let damaged = Unread::Malformed("an entry's header is damaged");
    let mut r = Reader::new(header);
    if r.u32()? != LOCAL_SIG {
        return Err(damaged);
    }
    let (_needed, flags, method) = (r.u16()?, r.u16()?, r.u16()?);
    let (_time, _date, _crc, _packed, _len) = (r.u16()?, r.u16()?, r.u32()?, r.u32()?, r.u32()?);
    let (name_len, extra_len) = (r.u16()?, r.u16()?);
    if flags & ENCRYPTED != 0 {
        return Err(Unread::Malformed("it is encrypted"));
    }
    if method != STORED {
        return Err(Unread::Compressed);
    }
    if r.take(name_len.into())? != entry.name.as_bytes() {
        return Err(damaged);
    }
    entry
        .at
        .checked_add(LOCAL + u64::from(name_len) + u64::from(extra_len))
        .ok_or(damaged)
}

/// Little-endian fields read in order from a slice; running out is malformed.
struct Reader<'a>(&'a [u8]);

impl<'a> Reader<'a> {
    fn new(bytes: &'a [u8]) -> Reader<'a> {
        Reader(bytes)
    }

    fn take(&mut self, len: usize) -> Result<&'a [u8], Unread> {
        if self.0.len() < len {
            return Err(Unread::Malformed("it ends early"));
        }
        let (taken, rest) = self.0.split_at(len);
        self.0 = rest;
        Ok(taken)
    }

    fn rest(&self) -> &'a [u8] {
        self.0
    }

    fn u16(&mut self) -> Result<u16, Unread> {
        Ok(u16::from_le_bytes(
            self.take(2)?.try_into().expect("two bytes"),
        ))
    }

    fn u32(&mut self) -> Result<u32, Unread> {
        Ok(u32::from_le_bytes(
            self.take(4)?.try_into().expect("four bytes"),
        ))
    }

    fn u64(&mut self) -> Result<u64, Unread> {
        Ok(u64::from_le_bytes(
            self.take(8)?.try_into().expect("eight bytes"),
        ))
    }
}

#[cfg(test)]
mod tests {
    use std::io::{Cursor, Read as _, Seek, SeekFrom, Write as _};

    use super::*;

    fn file(name: &str, bytes: &[u8]) -> (Entry, Vec<u8>) {
        let entry = Entry {
            name: name.to_string(),
            len: bytes.len() as u64,
            crc: nord_format::crc::crc32(bytes),
            modified: None,
        };
        (entry, bytes.to_vec())
    }

    /// The archive `layout` lays out for `files`, whole.
    fn archive(files: &[(Entry, Vec<u8>)]) -> Vec<u8> {
        let entries: Vec<Entry> = files.iter().map(|(entry, _)| entry.clone()).collect();
        let mut out = Vec::new();
        for piece in layout(&entries).expect("an archive that fits") {
            match piece {
                Piece::Bytes(bytes) => out.extend(bytes),
                Piece::Data(at) => out.extend(&files[at].1),
            }
        }
        out
    }

    /// The entries this module's reader finds in `archive`, each with its bytes.
    fn read_back(archive: &[u8]) -> Result<Vec<(Stored, Vec<u8>)>, Unread> {
        let len = archive.len() as u64;
        let tail = &archive[(len - END) as usize..];
        let directory = match end(tail)? {
            End::Directory(directory) => directory,
            End::Zip64 => {
                let at = (len - END - LOCATOR) as usize;
                let record = locator(&archive[at..at + LOCATOR as usize])? as usize;
                end64(&archive[record..record + END64 as usize])?
            }
        };
        let (from, to) = (
            directory.offset as usize,
            (directory.offset + directory.len) as usize,
        );
        let entries = super::directory(&archive[from..to], directory.count)?;
        entries
            .into_iter()
            .map(|entry| {
                let header = &archive[entry.at as usize..];
                let data = data_at(header, &entry)? as usize;
                let bytes = archive[data..data + entry.len as usize].to_vec();
                Ok((entry, bytes))
            })
            .collect()
    }

    #[test]
    fn an_archive_laid_out_reads_back_whole_through_an_independent_reader() {
        let files = [
            file("Grand.ne5p", b"grand piano"),
            file("Set/", b""),
            file("Set/Strings.ne5p", &[7; 70_000]),
            file("Tom\u{e9}.ne5p", b""),
        ];
        let mut read = ::zip::ZipArchive::new(Cursor::new(archive(&files))).unwrap();
        assert_eq!(read.len(), files.len());
        for (at, (entry, bytes)) in files.iter().enumerate() {
            let mut found = read.by_index(at).unwrap();
            assert_eq!(found.name(), entry.name);
            assert_eq!(found.is_dir(), entry.name.ends_with('/'), "{}", entry.name);
            assert_eq!(found.compression(), ::zip::CompressionMethod::Stored);
            let mut back = Vec::new();
            found.read_to_end(&mut back).unwrap();
            assert_eq!(&back, bytes, "{}", entry.name);
        }
    }

    #[test]
    fn an_archive_laid_out_reads_back_through_this_reader() {
        let files = [file("a/", b""), file("a/b.ne5p", b"b"), file("c", &[1; 9])];
        let back = read_back(&archive(&files)).unwrap();
        let names: Vec<&str> = back.iter().map(|(entry, _)| entry.name.as_str()).collect();
        assert_eq!(names, ["a/", "a/b.ne5p", "c"]);
        for ((entry, bytes), (stored, read)) in files.iter().zip(&back) {
            assert_eq!(
                (stored.len, stored.crc),
                (entry.len, entry.crc),
                "{}",
                entry.name
            );
            assert_eq!(read, bytes, "{}", entry.name);
        }
    }

    #[test]
    fn an_archive_another_tool_stored_reads_back_through_this_reader() {
        let mut written = ::zip::ZipWriter::new(Cursor::new(Vec::new()));
        let stored = ::zip::write::SimpleFileOptions::default()
            .compression_method(::zip::CompressionMethod::Stored);
        written.add_directory("Set/", stored).unwrap();
        written.start_file("Set/a.ne5p", stored).unwrap();
        written.write_all(b"another tool's bytes").unwrap();
        let archive = written.finish().unwrap().into_inner();
        let back = read_back(&archive).unwrap();
        let (entry, bytes) = &back[1];
        assert_eq!(entry.name, "Set/a.ne5p");
        assert_eq!(bytes, b"another tool's bytes");
        assert_eq!(entry.crc, nord_format::crc::crc32(bytes));
    }

    #[test]
    fn a_compressed_entry_is_refused_as_compressed() {
        let mut written = ::zip::ZipWriter::new(Cursor::new(Vec::new()));
        let deflated = ::zip::write::SimpleFileOptions::default()
            .compression_method(::zip::CompressionMethod::Deflated);
        written.start_file("a.ne5p", deflated).unwrap();
        written.write_all(&[0; 1000]).unwrap();
        let archive = written.finish().unwrap().into_inner();
        assert_eq!(read_back(&archive).unwrap_err(), Unread::Compressed);
    }

    #[test]
    fn an_archive_with_a_comment_is_refused() {
        let mut written = ::zip::ZipWriter::new(Cursor::new(Vec::new()));
        written.set_comment("notes");
        let archive = written.finish().unwrap().into_inner();
        assert!(read_back(&archive).is_err());
    }

    #[test]
    fn a_directory_cut_short_is_refused() {
        let archive = archive(&[file("a", b"a"), file("b", b"b")]);
        let tail = &archive[archive.len() - END as usize..];
        let Ok(End::Directory(directory)) = end(tail) else {
            panic!("{:?}", end(tail))
        };
        let (from, to) = (
            directory.offset as usize,
            (directory.offset + directory.len) as usize,
        );
        assert_eq!(
            super::directory(&archive[from..to - 1], directory.count),
            Err(Unread::Malformed("it ends early"))
        );
    }

    #[test]
    fn a_header_that_disagrees_with_the_directory_is_refused() {
        let files = [file("a", b"a")];
        let mut archive = archive(&files);
        archive[30] = b'z';
        assert!(read_back(&archive).is_err());
    }

    /// More entries than a 16-bit count holds take the ZIP64 end records.
    #[test]
    fn more_entries_than_sixteen_bits_count_read_back_through_an_independent_reader() {
        let files: Vec<(Entry, Vec<u8>)> =
            (0..70_000).map(|n| file(&format!("{n}"), b"")).collect();
        let archive = archive(&files);
        assert_eq!(read_back(&archive).unwrap().len(), files.len());
        let read = ::zip::ZipArchive::new(Cursor::new(archive)).unwrap();
        assert_eq!(read.len(), files.len());
    }

    /// An archive whose entries' bytes are zeros it never holds: read as a file, each
    /// entry's data reads as zeros, and only `real` holds bytes.
    struct Sparse {
        runs: Vec<(u64, Vec<u8>)>,
        len: u64,
        at: u64,
    }

    impl Sparse {
        fn new(entries: &[Entry], real: (usize, &[u8])) -> Sparse {
            let mut runs = Vec::new();
            let mut at = 0;
            for piece in layout(entries).unwrap() {
                let (len, bytes) = match piece {
                    Piece::Bytes(bytes) => (bytes.len() as u64, Some(bytes)),
                    Piece::Data(index) if index == real.0 => {
                        (real.1.len() as u64, Some(real.1.to_vec()))
                    }
                    Piece::Data(index) => (entries[index].len, None),
                };
                runs.extend(bytes.map(|bytes| (at, bytes)));
                at += len;
            }
            Sparse {
                runs,
                len: at,
                at: 0,
            }
        }
    }

    impl std::io::Read for Sparse {
        fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
            let n = buf.len().min(self.len.saturating_sub(self.at) as usize);
            buf[..n].fill(0);
            for (start, bytes) in &self.runs {
                let (from, to) = (
                    self.at.max(*start),
                    (self.at + n as u64).min(start + bytes.len() as u64),
                );
                if from < to {
                    let (into, of) = ((from - self.at) as usize, (from - start) as usize);
                    let len = (to - from) as usize;
                    buf[into..into + len].copy_from_slice(&bytes[of..of + len]);
                }
            }
            self.at += n as u64;
            Ok(n)
        }
    }

    impl Seek for Sparse {
        fn seek(&mut self, to: SeekFrom) -> std::io::Result<u64> {
            self.at = match to {
                SeekFrom::Start(at) => at,
                SeekFrom::End(back) => self.len.checked_add_signed(back).unwrap(),
                SeekFrom::Current(by) => self.at.checked_add_signed(by).unwrap(),
            };
            Ok(self.at)
        }
    }

    /// An entry past 4 GiB long, and one that starts past 4 GiB, take ZIP64 fields, as
    /// an independent reader finds without the archive's bytes being held.
    #[test]
    fn sizes_and_offsets_past_four_gib_read_back_through_an_independent_reader() {
        let big = 5 << 30;
        let after = b"after the large one";
        let entries = [
            Entry {
                name: "Grand.npno".to_string(),
                len: big,
                crc: 0,
                modified: None,
            },
            file("after.ne5p", after).0,
        ];
        let mut read = ::zip::ZipArchive::new(Sparse::new(&entries, (1, after))).unwrap();
        assert_eq!(read.by_index(0).unwrap().size(), big);
        let mut found = read.by_index(1).unwrap();
        assert!(found.header_start() > u64::from(u32::MAX));
        let mut back = Vec::new();
        found.read_to_end(&mut back).unwrap();
        assert_eq!(back, after);
    }

    #[test]
    fn sizes_and_offsets_past_four_gib_read_back_through_this_reader() {
        let big = 5u64 << 30;
        let entries = [
            Entry {
                name: "Grand.npno".to_string(),
                len: big,
                crc: 7,
                modified: None,
            },
            file("after.ne5p", b"after").0,
        ];
        let mut sparse = Sparse::new(&entries, (1, b"after"));
        let len = sparse.len;
        let mut read = |at: u64, len: u64| {
            let mut bytes = vec![0; len as usize];
            sparse.seek(SeekFrom::Start(at)).unwrap();
            sparse.read_exact(&mut bytes).unwrap();
            bytes
        };
        assert_eq!(end(&read(len - END, END)), Ok(End::Zip64));
        let record = locator(&read(len - END - LOCATOR, LOCATOR)).unwrap();
        let directory = end64(&read(record, END64)).unwrap();
        let listed = read(directory.offset, directory.len);
        let stored = super::directory(&listed, directory.count).unwrap();
        assert_eq!((stored[0].len, stored[0].crc), (big, 7));
        assert!(stored[1].at > big);
        let header = read(stored[1].at, LOCAL + stored[1].name.len() as u64);
        let data = data_at(&header, &stored[1]).unwrap();
        assert_eq!(read(data, 5), b"after");
    }

    /// The time an entry last changed reads back as the same second, to two.
    #[test]
    fn a_modified_time_reads_back_through_an_independent_reader() {
        let (mut entry, bytes) = file("a", b"a");
        entry.modified = Some(1_793_363_696_000);
        let read = ::zip::ZipArchive::new(Cursor::new(archive(&[(entry, bytes)])));
        let mut read = read.unwrap();
        let at = read.by_index(0).unwrap().last_modified().unwrap();
        let said = (
            at.year(),
            at.month(),
            at.day(),
            at.hour(),
            at.minute(),
            at.second(),
        );
        assert_eq!(said, (2026, 10, 30, 12, 34, 56));
    }

    #[test]
    fn a_time_before_1980_is_written_as_its_first_day() {
        assert_eq!(super::write::dos_time(0), (0, 1 << 5 | 1));
    }
}
