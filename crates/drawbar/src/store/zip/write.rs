//! Laying out an archive of stored entries from each entry's length and CRC-32.

use super::*;

/// Bit 11: the name is UTF-8.
const UTF8: u16 = 1 << 11;
/// Version 2.0, and 4.5 for ZIP64, made on Unix, so a folder's mode is read.
const NEEDED: u16 = 20;
const NEEDED64: u16 = 45;
const MADE_BY: u16 = 3 << 8 | NEEDED64;
const FILE_MODE: u32 = 0o100_644;
const DIR_MODE: u32 = 0o040_755;
/// The MS-DOS attribute of a folder.
const DOS_DIR: u32 = 0x10;

/// One file or folder of an archive.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Entry {
    /// Its path, `/`-separated. A folder's ends in `/` and holds no bytes.
    pub name: String,
    pub len: u64,
    pub crc: u32,
    /// When it last changed, in milliseconds since the Unix epoch on a clock that reads
    /// local time, since an MS-DOS time has no zone.
    pub modified: Option<u64>,
}

/// One run of an archive's bytes, in order.
#[derive(Debug, PartialEq, Eq)]
pub enum Piece {
    Bytes(Vec<u8>),
    /// The bytes of the entry at this index.
    Data(usize),
}

/// The archive of `entries`, in order, as runs of header bytes and each entry's data, or
/// `None` where a name passes 65535 bytes or the archive the 2^64 ZIP64 can address.
pub fn layout(entries: &[Entry]) -> Option<Vec<Piece>> {
    let mut pieces = Vec::with_capacity(entries.len() * 2 + 1);
    let mut central = Vec::new();
    let mut at: u64 = 0;
    for (index, entry) in entries.iter().enumerate() {
        u16::try_from(entry.name.len()).ok()?;
        let local = local_header(entry);
        let header = at;
        at = at.checked_add(local.len() as u64)?.checked_add(entry.len)?;
        pieces.push(Piece::Bytes(local));
        if entry.len > 0 {
            pieces.push(Piece::Data(index));
        }
        central.extend(central_header(entry, header));
    }
    let count = entries.len() as u64;
    let (offset, len) = (at, central.len() as u64);
    let end64 = count >= MAX16 || len >= MAX32 || offset >= MAX32;
    if end64 {
        let record = offset.checked_add(len)?;
        put32(&mut central, END64_SIG);
        put64(&mut central, END64 - 12);
        put16(&mut central, MADE_BY);
        put16(&mut central, NEEDED64);
        put32(&mut central, 0);
        put32(&mut central, 0);
        put64(&mut central, count);
        put64(&mut central, count);
        put64(&mut central, len);
        put64(&mut central, offset);
        put32(&mut central, LOCATOR_SIG);
        put32(&mut central, 0);
        put64(&mut central, record);
        put32(&mut central, 1);
    }
    put32(&mut central, END_SIG);
    put16(&mut central, 0);
    put16(&mut central, 0);
    put16(&mut central, count.min(MAX16) as u16);
    put16(&mut central, count.min(MAX16) as u16);
    put32(&mut central, len.min(MAX32) as u32);
    put32(&mut central, offset.min(MAX32) as u32);
    put16(&mut central, 0);
    pieces.push(Piece::Bytes(central));
    Some(pieces)
}

fn local_header(entry: &Entry) -> Vec<u8> {
    let wide = entry.len >= MAX32;
    let mut out = Vec::with_capacity(LOCAL as usize + entry.name.len() + 20);
    put32(&mut out, LOCAL_SIG);
    put16(&mut out, if wide { NEEDED64 } else { NEEDED });
    head(&mut out, entry);
    let size = if wide { MAX32 } else { entry.len } as u32;
    put32(&mut out, size);
    put32(&mut out, size);
    put16(&mut out, entry.name.len() as u16);
    put16(&mut out, if wide { 20 } else { 0 });
    out.extend_from_slice(entry.name.as_bytes());
    if wide {
        put16(&mut out, ZIP64_EXTRA);
        put16(&mut out, 16);
        put64(&mut out, entry.len);
        put64(&mut out, entry.len);
    }
    out
}

fn central_header(entry: &Entry, offset: u64) -> Vec<u8> {
    let (wide, far) = (entry.len >= MAX32, offset >= MAX32);
    let mut extra = Vec::new();
    if wide {
        put64(&mut extra, entry.len);
        put64(&mut extra, entry.len);
    }
    if far {
        put64(&mut extra, offset);
    }
    let mut out = Vec::with_capacity(46 + entry.name.len() + extra.len() + 4);
    put32(&mut out, CENTRAL_SIG);
    put16(&mut out, MADE_BY);
    put16(&mut out, if wide || far { NEEDED64 } else { NEEDED });
    head(&mut out, entry);
    let size = entry.len.min(MAX32) as u32;
    put32(&mut out, size);
    put32(&mut out, size);
    put16(&mut out, entry.name.len() as u16);
    put16(
        &mut out,
        if extra.is_empty() {
            0
        } else {
            extra.len() as u16 + 4
        },
    );
    put16(&mut out, 0);
    put16(&mut out, 0);
    put16(&mut out, 0);
    let attributes = match entry.name.ends_with('/') {
        true => DIR_MODE << 16 | DOS_DIR,
        false => FILE_MODE << 16,
    };
    put32(&mut out, attributes);
    put32(&mut out, offset.min(MAX32) as u32);
    out.extend_from_slice(entry.name.as_bytes());
    if !extra.is_empty() {
        put16(&mut out, ZIP64_EXTRA);
        put16(&mut out, extra.len() as u16);
        out.extend(extra);
    }
    out
}

/// The fields a local and a central header share, from the flags to the CRC.
fn head(out: &mut Vec<u8>, entry: &Entry) {
    put16(out, UTF8);
    put16(out, STORED);
    let (time, date) = dos_time(entry.modified.unwrap_or(0));
    put16(out, time);
    put16(out, date);
    put32(out, entry.crc);
}

/// `ms` since the Unix epoch as MS-DOS writes a time and a date, which start in 1980
/// and end in 2107.
pub fn dos_time(ms: u64) -> (u16, u16) {
    const FIRST: (u16, u16) = (0, 1 << 5 | 1);
    let secs = ms / 1000;
    let (days, of_day) = (secs / 86_400, secs % 86_400);
    let (year, month, day) = civil(days);
    if !(1980..=2107).contains(&year) {
        return FIRST;
    }
    let time = (of_day / 3600) << 11 | (of_day / 60 % 60) << 5 | ((of_day % 60) / 2);
    let date = (year - 1980) << 9 | u64::from(month) << 5 | u64::from(day);
    (time as u16, date as u16)
}

/// The year, month and day `days` after 1970-01-01, by Howard Hinnant's
/// `civil_from_days`.
fn civil(days: u64) -> (u64, u32, u32) {
    let z = days + 719_468;
    let era = z / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let month = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    let year = yoe + era * 400 + u64::from(month <= 2);
    (year, month, day)
}

fn put16(out: &mut Vec<u8>, value: u16) {
    out.extend_from_slice(&value.to_le_bytes());
}

fn put32(out: &mut Vec<u8>, value: u32) {
    out.extend_from_slice(&value.to_le_bytes());
}

fn put64(out: &mut Vec<u8>, value: u64) {
    out.extend_from_slice(&value.to_le_bytes());
}
