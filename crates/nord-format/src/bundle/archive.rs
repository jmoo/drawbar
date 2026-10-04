//! The store-method ZIP a Nord bundle is, read and written one member at a time.
//!
//! Inferred from specimens; not confirmed on hardware. NSM stores every member
//! uncompressed, end to end in directory order, with no data descriptor, and puts the
//! central directory right after the last member. This module accepts exactly that
//! shape and refuses any other, so every byte of an accepted archive belongs to a field
//! here and [`Writer`] reproduces it. Nothing needs the archive resident: the directory
//! comes from the tail, and each member is a byte range of the file.

use crate::crc::Crc32Stream;
use crate::error::{Error, ParseError};
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::ops::Range;

/// Declare a fixed-length little-endian record: its signature, then each field at its
/// offset. A record's names, extra fields and comment follow its fixed part.
macro_rules! record {
    (
        $(#[$meta:meta])*
        $name:ident, $len:literal, $signature:literal {
            $($field:ident: $ty:ty = $at:literal,)*
        }
    ) => {
        $(#[$meta])*
        #[derive(Clone, Copy, Debug, PartialEq, Eq)]
        struct $name {
            $($field: $ty,)*
        }

        impl $name {
            const LEN: usize = $len;
            const SIGNATURE: u32 = $signature;

            fn parse(bytes: &[u8]) -> Result<Self, ParseError> {
                let fixed = bytes.get(..$len).ok_or_else(|| {
                    refuse(format!("{} bytes", bytes.len()), concat!("a ", stringify!($name), " record"))
                })?;
                let signature = u32::from_le_bytes([fixed[0], fixed[1], fixed[2], fixed[3]]);
                if signature != Self::SIGNATURE {
                    return Err(refuse(
                        format!("signature {signature:#010x}"),
                        concat!("a ", stringify!($name), " record"),
                    ));
                }
                Ok(Self {
                    $($field: <$ty>::from_le_bytes(
                        fixed[$at..$at + size_of::<$ty>()].try_into().expect("a field inside its record"),
                    ),)*
                })
            }

            fn to_bytes(self) -> [u8; $len] {
                let mut out = [0; $len];
                out[..4].copy_from_slice(&Self::SIGNATURE.to_le_bytes());
                $(out[$at..$at + size_of::<$ty>()].copy_from_slice(&self.$field.to_le_bytes());)*
                out
            }
        }
    };
}

record! {
    /// The header before each member's bytes (APPNOTE 4.3.7).
    Local, 30, 0x0403_4b50 {
        needed: u16 = 4,
        flags: u16 = 6,
        method: u16 = 8,
        time: u16 = 10,
        date: u16 = 12,
        crc32: u32 = 14,
        compressed: u32 = 18,
        size: u32 = 22,
        name_len: u16 = 26,
        extra_len: u16 = 28,
    }
}

record! {
    /// One member's central directory header (APPNOTE 4.3.12).
    Central, 46, 0x0201_4b50 {
        made_by: u16 = 4,
        needed: u16 = 6,
        flags: u16 = 8,
        method: u16 = 10,
        time: u16 = 12,
        date: u16 = 14,
        crc32: u32 = 16,
        compressed: u32 = 20,
        size: u32 = 24,
        name_len: u16 = 28,
        extra_len: u16 = 30,
        comment_len: u16 = 32,
        disk: u16 = 34,
        internal: u16 = 36,
        external: u32 = 38,
        offset: u32 = 42,
    }
}

record! {
    /// The end of central directory record (APPNOTE 4.3.16).
    End, 22, 0x0605_4b50 {
        disk: u16 = 4,
        directory_disk: u16 = 6,
        disk_entries: u16 = 8,
        entries: u16 = 10,
        directory_len: u32 = 12,
        directory_at: u32 = 16,
        comment_len: u16 = 20,
    }
}

/// The only flag an accepted member may carry: bit 11, a UTF-8 name.
const UTF8_NAME: u16 = 0x0800;
const STORED: u16 = 0;

/// The longest tail that can hold the end record: the record and the longest comment.
pub const TAIL_MAX: u64 = End::LEN as u64 + u16::MAX as u64;

fn refuse(value: String, bound: &str) -> ParseError {
    ParseError::OutOfBounds {
        value,
        bound: bound.into(),
    }
}

/// An MS-DOS time and date word pair, the only timestamp a stored ZIP member needs.
/// Local time, two-second resolution, years 1980 to 2107.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct DosTime {
    pub time: u16,
    pub date: u16,
}

impl DosTime {
    /// The pair for a calendar time, or `None` outside what the words can hold. Odd
    /// seconds round down.
    pub fn new(year: u16, month: u8, day: u8, hour: u8, minute: u8, second: u8) -> Option<DosTime> {
        let valid = (1980..=2107).contains(&year)
            && (1..=12).contains(&month)
            && (1..=31).contains(&day)
            && hour < 24
            && minute < 60
            && second < 60;
        valid.then(|| DosTime {
            time: u16::from(hour) << 11 | u16::from(minute) << 5 | u16::from(second / 2),
            date: (year - 1980) << 9 | u16::from(month) << 5 | u16::from(day),
        })
    }

    /// The pair for a time in seconds since the Unix epoch, read as UTC, or `None`
    /// outside 1980 to 2107. NSM stamps local time, which this has no zone to give.
    pub fn from_unix(seconds: u64) -> Option<DosTime> {
        let days = i64::try_from(seconds / 86_400).ok()?;
        let of_day = seconds % 86_400;
        // Days to a proleptic Gregorian date, by eras of 400 years from 0000-03-01.
        let z = days + 719_468;
        let era = z.div_euclid(146_097);
        let day_of_era = z.rem_euclid(146_097);
        let year_of_era =
            (day_of_era - day_of_era / 1_460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
        let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
        let shifted_month = (5 * day_of_year + 2) / 153;
        let day = day_of_year - (153 * shifted_month + 2) / 5 + 1;
        let month = if shifted_month < 10 {
            shifted_month + 3
        } else {
            shifted_month - 9
        };
        let year = year_of_era + era * 400 + i64::from(month <= 2);
        DosTime::new(
            u16::try_from(year).ok()?,
            month as u8,
            day as u8,
            (of_day / 3_600) as u8,
            (of_day / 60 % 60) as u8,
            (of_day % 60) as u8,
        )
    }
}

/// The fields of a member's records that nothing here interprets, kept so a read
/// archive writes back unchanged.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Verbatim {
    pub made_by: u16,
    pub needed: u16,
    pub flags: u16,
    pub internal: u16,
    pub external: u32,
    pub local_extra: Vec<u8>,
    /// Unexplained: NSM writes one ASCII `0` or `1` here, which is not a well-formed
    /// extra field.
    pub central_extra: Vec<u8>,
    pub comment: Vec<u8>,
}

/// One member's directory entry: everything the archive records about it but its bytes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Entry {
    /// The member's `/`-separated path inside the archive.
    pub name: String,
    pub size: u32,
    /// CRC-32 (ISO-HDLC) of the member's bytes.
    pub crc32: u32,
    pub modified: DosTime,
    pub verbatim: Verbatim,
}

impl Entry {
    /// A new member as NSM writes one: made by MS-DOS 3.2, version 2.0 needed to
    /// extract, no extra fields. A non-ASCII name sets the UTF-8 flag.
    ///
    /// Inferred from specimens; not confirmed on hardware. NSM's own members carry
    /// a one-byte central extra field this writes empty.
    pub fn new(name: String, size: u32, crc32: u32, modified: DosTime) -> Entry {
        let flags = if name.is_ascii() { 0 } else { UTF8_NAME };
        Entry {
            name,
            size,
            crc32,
            modified,
            verbatim: Verbatim {
                made_by: 0x0020,
                needed: 0x0014,
                flags,
                internal: 0,
                external: 0,
                local_extra: Vec::new(),
                central_extra: Vec::new(),
                comment: Vec::new(),
            },
        }
    }

    fn local(&self) -> Result<Local, ParseError> {
        Ok(Local {
            needed: self.verbatim.needed,
            flags: self.verbatim.flags,
            method: STORED,
            time: self.modified.time,
            date: self.modified.date,
            crc32: self.crc32,
            compressed: self.size,
            size: self.size,
            name_len: len16(self.name.len(), "a member name")?,
            extra_len: len16(self.verbatim.local_extra.len(), "a local extra field")?,
        })
    }

    fn central(&self, offset: u32) -> Result<Central, ParseError> {
        Ok(Central {
            made_by: self.verbatim.made_by,
            needed: self.verbatim.needed,
            flags: self.verbatim.flags,
            method: STORED,
            time: self.modified.time,
            date: self.modified.date,
            crc32: self.crc32,
            compressed: self.size,
            size: self.size,
            name_len: len16(self.name.len(), "a member name")?,
            extra_len: len16(self.verbatim.central_extra.len(), "a central extra field")?,
            comment_len: len16(self.verbatim.comment.len(), "a member comment")?,
            disk: 0,
            internal: self.verbatim.internal,
            external: self.verbatim.external,
            offset,
        })
    }

    /// A check of the member's bytes against this entry as they stream past.
    pub fn check(&self) -> Check {
        Check {
            name: self.name.clone(),
            crc32: self.crc32,
            size: self.size.into(),
            seen: 0,
            stream: Crc32Stream::new(),
        }
    }

    /// The bytes before the member's own: its local header, name and extra field.
    fn local_bytes(&self) -> Result<Vec<u8>, ParseError> {
        let mut out = self.local()?.to_bytes().to_vec();
        out.extend_from_slice(self.name.as_bytes());
        out.extend_from_slice(&self.verbatim.local_extra);
        Ok(out)
    }
}

fn len16(len: usize, what: &str) -> Result<u16, ParseError> {
    u16::try_from(len).map_err(|_| refuse(format!("{what} of {len} bytes"), "65535 bytes"))
}

/// A member found in the directory, and where its records and bytes lie in the archive.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Member {
    pub entry: Entry,
    /// The local header, name and extra field.
    pub header: Range<u64>,
    /// The member's bytes.
    pub body: Range<u64>,
}

impl Member {
    /// Checks the local header read from [`Member::header`] against the directory, and
    /// keeps its extra field.
    pub fn check_header(&mut self, bytes: &[u8]) -> Result<(), ParseError> {
        let local = Local::parse(bytes)?;
        let expected = Local {
            extra_len: local.extra_len,
            ..self.entry.local()?
        };
        let name = bytes.get(Local::LEN..Local::LEN + usize::from(local.name_len));
        if local != expected || name != Some(self.entry.name.as_bytes()) {
            return Err(refuse(
                format!("the local header of {}", self.entry.name),
                "the header its directory entry describes",
            ));
        }
        let extra = &bytes[Local::LEN + usize::from(local.name_len)..];
        if extra.len() != usize::from(local.extra_len) {
            return Err(refuse(
                format!("the local header of {}", self.entry.name),
                "the space before its bytes",
            ));
        }
        self.entry.verbatim.local_extra = extra.to_vec();
        Ok(())
    }

    /// A check of the member's bytes as they stream past.
    pub fn check(&self) -> Check {
        self.entry.check()
    }
}

/// Verifies a member's bytes against its entry as they are read or written.
pub struct Check {
    name: String,
    crc32: u32,
    size: u64,
    seen: u64,
    stream: Crc32Stream<'static>,
}

impl Check {
    pub fn update(&mut self, bytes: &[u8]) -> Result<(), ParseError> {
        self.seen = self.seen.saturating_add(bytes.len() as u64);
        if self.seen > self.size {
            return Err(self.wrong_length());
        }
        self.stream.update(bytes);
        Ok(())
    }

    /// Whether every byte arrived and they hash to the entry's CRC.
    pub fn finish(self) -> Result<(), ParseError> {
        if self.seen != self.size {
            return Err(self.wrong_length());
        }
        let crc32 = self.stream.value();
        if crc32 != self.crc32 {
            return Err(refuse(
                format!("CRC-32 {crc32:#010x} of {}", self.name),
                &format!("the {:#010x} its entry records", self.crc32),
            ));
        }
        Ok(())
    }

    fn wrong_length(&self) -> ParseError {
        ParseError::WrongBodyLength {
            format: self.name.clone(),
            got: self.seen,
            expected: self.size,
        }
    }
}

/// An archive's central directory and the members it lists, in order.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Directory {
    pub members: Vec<Member>,
    pub comment: Vec<u8>,
}

/// Where the central directory lies, found from the archive's tail.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Tail {
    pub directory: Range<u64>,
    entries: u16,
    comment: Vec<u8>,
}

impl Tail {
    /// Finds the end record in the last bytes of an archive `len` bytes long. `tail`
    /// holds at least the last `min(len, TAIL_MAX)` bytes.
    ///
    /// The record must end the archive, and the directory must end where it starts.
    pub fn find(tail: &[u8], len: u64) -> Result<Tail, ParseError> {
        let tail_at = len
            .checked_sub(tail.len() as u64)
            .ok_or_else(|| refuse(format!("a tail of {} bytes", tail.len()), "the archive"))?;
        // The comment is the only variable part after the record, so the record is the
        // one whose comment length reaches exactly to the end.
        let at = (0..tail.len().saturating_sub(End::LEN - 1))
            .rev()
            .find(|&at| {
                End::parse(&tail[at..])
                    .is_ok_and(|end| at + End::LEN + usize::from(end.comment_len) == tail.len())
            })
            .ok_or_else(|| refuse("an archive".into(), "one ending in an end record"))?;
        let end = End::parse(&tail[at..])?;
        if (end.disk, end.directory_disk) != (0, 0)
            || end.disk_entries != end.entries
            || end.entries == u16::MAX
        {
            return Err(refuse(
                "a multi-disk or ZIP64 archive".into(),
                "one stored ZIP",
            ));
        }
        let record_at = tail_at + at as u64;
        let directory =
            u64::from(end.directory_at)..u64::from(end.directory_at) + u64::from(end.directory_len);
        if directory.end != record_at {
            return Err(refuse(
                format!("a directory ending at {}", directory.end),
                &format!("the end record at {record_at}"),
            ));
        }
        Ok(Tail {
            directory,
            entries: end.entries,
            comment: tail[at + End::LEN..].to_vec(),
        })
    }

    /// Reads the directory's bytes, which lie at [`Tail::directory`].
    ///
    /// Members must start at offset 0 and follow one another with nothing between, so
    /// each member's bytes end where the next member's header starts.
    pub fn directory(self, bytes: &[u8]) -> Result<Directory, ParseError> {
        let mut centrals = Vec::new();
        let mut at = 0;
        for _ in 0..self.entries {
            let central = Central::parse(&bytes[at..])?;
            let name_at = at + Central::LEN;
            let extra_at = name_at + usize::from(central.name_len);
            let comment_at = extra_at + usize::from(central.extra_len);
            let next = comment_at + usize::from(central.comment_len);
            let tail = bytes
                .get(name_at..next)
                .ok_or_else(|| refuse(format!("an entry ending at {next}"), "the directory"))?;
            let name = std::str::from_utf8(&tail[..usize::from(central.name_len)])
                .map_err(|_| refuse("a member name that is not UTF-8".into(), "a UTF-8 name"))?;
            centrals.push((
                central,
                name.to_string(),
                bytes[extra_at..comment_at].to_vec(),
                bytes[comment_at..next].to_vec(),
            ));
            at = next;
        }
        if at != bytes.len() {
            return Err(refuse(
                format!("a directory of {} bytes", bytes.len()),
                &format!("the {at} its {} entries fill", self.entries),
            ));
        }

        let mut members = Vec::with_capacity(centrals.len());
        let mut start = 0;
        for (i, (central, name, central_extra, comment)) in centrals.iter().enumerate() {
            if central.method != STORED
                || central.flags & !UTF8_NAME != 0
                || central.compressed != central.size
                || central.disk != 0
            {
                return Err(refuse(
                    format!(
                        "{name}: method {}, flags {:#06x}",
                        central.method, central.flags
                    ),
                    "a stored member with no data descriptor or encryption",
                ));
            }
            let offset = u64::from(central.offset);
            if offset != start {
                return Err(refuse(
                    format!("{name} at {offset}"),
                    &format!("the end of the previous member at {start}"),
                ));
            }
            let next = centrals
                .get(i + 1)
                .map_or(self.directory.start, |(next, ..)| u64::from(next.offset));
            let body = next.checked_sub(central.size.into()).map(|body| body..next);
            let header_min = offset + Local::LEN as u64 + u64::from(central.name_len);
            let Some(body) = body.filter(|body| body.start >= header_min) else {
                return Err(refuse(
                    format!("{name} ending at {next}"),
                    "room for its header",
                ));
            };
            members.push(Member {
                entry: Entry {
                    name: name.clone(),
                    size: central.size,
                    crc32: central.crc32,
                    modified: DosTime {
                        time: central.time,
                        date: central.date,
                    },
                    verbatim: Verbatim {
                        made_by: central.made_by,
                        needed: central.needed,
                        flags: central.flags,
                        internal: central.internal,
                        external: central.external,
                        local_extra: Vec::new(),
                        central_extra: central_extra.clone(),
                        comment: comment.clone(),
                    },
                },
                header: offset..body.start,
                body: body.clone(),
            });
            start = body.end;
        }
        if start != self.directory.start {
            return Err(refuse(
                format!("members ending at {start}"),
                &format!("the directory at {}", self.directory.start),
            ));
        }
        Ok(Directory {
            members,
            comment: self.comment,
        })
    }
}

impl Directory {
    /// Reads the directory of a seekable archive and checks every member's local header.
    /// No member's bytes are read.
    pub fn read_from(r: &mut (impl Read + Seek)) -> Result<Directory, Error> {
        let len = r.seek(SeekFrom::End(0))?;
        let tail_len = len.min(TAIL_MAX);
        let tail = Tail::find(&read_range(r, len - tail_len..len)?, len)?;
        let bytes = read_range(r, tail.directory.clone())?;
        let mut directory = tail.directory(&bytes)?;
        for member in &mut directory.members {
            member.check_header(&read_range(r, member.header.clone())?)?;
        }
        Ok(directory)
    }

    /// The member named `name`.
    pub fn get(&self, name: &str) -> Option<&Member> {
        self.members.iter().find(|member| member.entry.name == name)
    }
}

/// The bytes of a range no longer than a directory or a header. The caller has bounded
/// it: a tail is at most [`TAIL_MAX`], and a directory or header lies inside the archive.
fn read_range(r: &mut (impl Read + Seek), range: Range<u64>) -> io::Result<Vec<u8>> {
    r.seek(SeekFrom::Start(range.start))?;
    let mut out = Vec::new();
    r.take(range.end - range.start).read_to_end(&mut out)?;
    if out.len() as u64 != range.end - range.start {
        return Err(io::ErrorKind::UnexpectedEof.into());
    }
    Ok(out)
}

/// Copies one member's bytes out of a seekable archive into `out`, checking them.
pub fn copy_member(
    r: &mut (impl Read + Seek),
    member: &Member,
    out: &mut impl Write,
) -> Result<(), Error> {
    r.seek(SeekFrom::Start(member.body.start))?;
    let mut check = member.check();
    let mut rest = r.take(member.body.end - member.body.start);
    let mut buf = vec![0; 64 * 1024];
    loop {
        let n = rest.read(&mut buf)?;
        if n == 0 {
            break;
        }
        check.update(&buf[..n])?;
        out.write_all(&buf[..n])?;
    }
    check.finish()?;
    Ok(())
}

/// Writes a stored archive to any sink, one member at a time.
///
/// Each member's size and CRC-32 go in its header ahead of its bytes, so the caller
/// states them in the [`Entry`] and the writer checks the bytes against them. A member
/// whose bytes disagree fails the write, and the output is then unusable.
pub struct Writer<W: Write> {
    sink: W,
    at: u64,
    written: Vec<(Entry, u32)>,
    open: Option<Check>,
}

impl<W: Write> Writer<W> {
    pub fn new(sink: W) -> Writer<W> {
        Writer {
            sink,
            at: 0,
            written: Vec::new(),
            open: None,
        }
    }

    /// Starts a member: writes its header. Its bytes follow through [`Writer::write`].
    pub fn begin(&mut self, entry: Entry) -> Result<(), Error> {
        self.end()?;
        let offset = u32::try_from(self.at)
            .map_err(|_| refuse(format!("a member at {}", self.at), "a 4 GiB archive"))?;
        let header = entry.local_bytes()?;
        self.put(&header)?;
        self.open = Some(entry.check());
        self.written.push((entry, offset));
        Ok(())
    }

    /// A whole member from a reader.
    pub fn member(&mut self, entry: Entry, body: &mut impl Read) -> Result<(), Error> {
        self.begin(entry)?;
        io::copy(body, self)?;
        self.end()
    }

    /// Writes the central directory and end record, and hands back the sink.
    pub fn finish(mut self, comment: &[u8]) -> Result<W, Error> {
        self.end()?;
        let trailer = trailer(&self.written, self.at, comment)?;
        self.put(&trailer)?;
        self.sink.flush()?;
        Ok(self.sink)
    }

    /// The sink, for draining between members on a target that writes asynchronously.
    pub fn sink_mut(&mut self) -> &mut W {
        &mut self.sink
    }

    fn end(&mut self) -> Result<(), Error> {
        match self.open.take() {
            Some(check) => Ok(check.finish()?),
            None => Ok(()),
        }
    }

    fn put(&mut self, bytes: &[u8]) -> io::Result<()> {
        self.sink.write_all(bytes)?;
        self.at += bytes.len() as u64;
        Ok(())
    }
}

/// The central directory and end record for `written` members, each with its header's
/// offset, when the directory starts at `at`.
fn trailer(written: &[(Entry, u32)], at: u64, comment: &[u8]) -> Result<Vec<u8>, ParseError> {
    let too_big = |what: String| refuse(what, "a 4 GiB archive of 65534 members");
    let directory_at = u32::try_from(at).map_err(|_| too_big(format!("a directory at {at}")))?;
    let entries = u16::try_from(written.len())
        .ok()
        .filter(|&n| n != u16::MAX)
        .ok_or_else(|| too_big(format!("{} members", written.len())))?;
    let mut out = Vec::new();
    for (entry, offset) in written {
        out.extend_from_slice(&entry.central(*offset)?.to_bytes());
        out.extend_from_slice(entry.name.as_bytes());
        out.extend_from_slice(&entry.verbatim.central_extra);
        out.extend_from_slice(&entry.verbatim.comment);
    }
    let directory_len = u32::try_from(out.len())
        .ok()
        .filter(|&len| directory_at.checked_add(len).is_some())
        .ok_or_else(|| too_big(format!("a directory of {} bytes", out.len())))?;
    let end = End {
        disk: 0,
        directory_disk: 0,
        disk_entries: entries,
        entries,
        directory_len,
        directory_at,
        comment_len: len16(comment.len(), "an archive comment")?,
    };
    out.extend_from_slice(&end.to_bytes());
    out.extend_from_slice(comment);
    Ok(out)
}

/// The bytes around members another writer places, such as a browser assembling a file
/// from parts: `headers[i]` goes before member `i`'s bytes, and `trailer` after the
/// last member's.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Frame {
    pub headers: Vec<Vec<u8>>,
    pub trailer: Vec<u8>,
}

impl Frame {
    /// The frame for members `entries` in order, whose sizes and CRCs it trusts.
    pub fn new(entries: &[Entry], comment: &[u8]) -> Result<Frame, ParseError> {
        let mut headers = Vec::with_capacity(entries.len());
        let mut written = Vec::with_capacity(entries.len());
        let mut at: u64 = 0;
        for entry in entries {
            let offset = u32::try_from(at)
                .map_err(|_| refuse(format!("a member at {at}"), "a 4 GiB archive"))?;
            let header = entry.local_bytes()?;
            at += header.len() as u64 + u64::from(entry.size);
            headers.push(header);
            written.push((entry.clone(), offset));
        }
        Ok(Frame {
            trailer: trailer(&written, at, comment)?,
            headers,
        })
    }
}

/// A member's bytes, between [`Writer::begin`] and the next member or
/// [`Writer::finish`].
impl<W: Write> Write for Writer<W> {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        let check = self
            .open
            .as_mut()
            .ok_or_else(|| io::Error::other("member bytes with no member begun"))?;
        check
            .update(bytes)
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
        self.put(bytes)?;
        Ok(bytes.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        self.sink.flush()
    }
}
