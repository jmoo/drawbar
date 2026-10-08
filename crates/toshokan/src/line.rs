//! The line codec of log segments.
//!
//! A line is `<json>\t<hash>\n`. The JSON is one object holding `prev`, the hash of
//! the entry before it in the writer's chain, and `hash` is
//! [`EntryHash::of`]`(prev, json)` in 32 hexadecimal digits. The hash is the entry's
//! id, the chain link and the line's checksum.

use serde::Deserialize;
use thiserror::Error as ThisError;

use crate::ids::EntryHash;

/// The longest line, newline included, a reader accepts.
pub const MAX_LINE: usize = 1 << 20;

const HASH_DIGITS: usize = 32;

/// How many bytes a line ends with that name it: a tab, its hash and LF.
pub const ENDING: u64 = HASH_DIGITS as u64 + 2;

/// The last [`ENDING`] bytes of the line whose hash is `hash`.
pub fn ending(hash: EntryHash) -> Vec<u8> {
    format!("\t{hash}\n").into_bytes()
}

/// What a process appends after the last line of a segment, `last`, when it
/// closes it, the seal marker: `sealed`, a tab, `last` and LF. It is not a line,
/// and its last [`ENDING`] bytes are those of the line it follows.
pub fn seal_marker(last: EntryHash) -> Vec<u8> {
    format!("sealed\t{last}\n").into_bytes()
}

/// How many bytes [`seal_marker`] writes.
pub const SEAL_MARKER: u64 = ENDING + 6;

/// One verified line.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Line {
    prev: EntryHash,
    hash: EntryHash,
    json: String,
}

/// Why a line cannot be read. Each ends the readable part of a file for now: a
/// later read of a longer or repaired file may get further.
#[derive(ThisError, Clone, PartialEq, Eq, Debug)]
pub enum LineError {
    #[error("the line has no final newline")]
    Unterminated,
    #[error("the line starts with a zero byte")]
    ZeroFilled,
    #[error("the line is longer than {MAX_LINE} bytes")]
    TooLong,
    #[error("the line is not UTF-8")]
    NotUtf8,
    #[error("the line has no tab before its hash")]
    NoHash,
    #[error("the hash is not 32 lowercase hexadecimal digits")]
    BadHash,
    #[error("the JSON is not an object with a valid `prev`")]
    NoPrev,
    #[error("the JSON contains a tab or newline")]
    Unescaped,
    #[error("the hash does not match the line")]
    Mismatch,
}

#[derive(Deserialize)]
struct Envelope {
    prev: EntryHash,
}

impl Line {
    /// The line for `json`, an object whose `prev` member names its predecessor.
    pub fn seal(json: String) -> Result<Self, LineError> {
        if json.contains(['\t', '\n']) {
            return Err(LineError::Unescaped);
        }
        if json.len() + HASH_DIGITS + 2 > MAX_LINE {
            return Err(LineError::TooLong);
        }
        let prev = prev_of(&json)?;
        let hash = EntryHash::of(prev, json.as_bytes());
        Ok(Self { prev, hash, json })
    }

    /// One line, its newline included.
    pub fn parse(line: &[u8]) -> Result<Self, LineError> {
        let (json, hash) = split(line)?;
        Self::checked(prev_of(json)?, json.to_owned(), hash)
    }

    /// The line of `json`, whose `prev` member is `prev`, once `hash` is its
    /// hash.
    pub(crate) fn checked(
        prev: EntryHash,
        json: String,
        hash: EntryHash,
    ) -> Result<Self, LineError> {
        if EntryHash::of(prev, json.as_bytes()) != hash {
            return Err(LineError::Mismatch);
        }
        Ok(Self { prev, hash, json })
    }

    /// The line whose JSON is `json` and whose hash is `hash`, kept by this
    /// install from a line [`Line::parse`] accepted, so its JSON is an object:
    /// `prev` is read from where this build writes it, its first member, when it
    /// is there. The hash is checked, so a damaged copy is refused.
    pub fn kept(json: String, hash: EntryHash) -> Result<Self, LineError> {
        if json.len() + HASH_DIGITS + 2 > MAX_LINE {
            return Err(LineError::TooLong);
        }
        if json.contains(['\t', '\n']) {
            return Err(LineError::Unescaped);
        }
        let prev = match leading_prev(&json) {
            Some(prev) => prev,
            None => prev_of(&json)?,
        };
        Self::checked(prev, json, hash)
    }

    /// The line of `json`, whose `prev` is `prev` and hash `hash`, as an entry
    /// verified when it was read keeps it.
    pub(crate) fn verified(prev: EntryHash, json: String, hash: EntryHash) -> Self {
        debug_assert_eq!(EntryHash::of(prev, json.as_bytes()), hash);
        Self { prev, hash, json }
    }

    pub fn prev(&self) -> EntryHash {
        self.prev
    }

    pub fn hash(&self) -> EntryHash {
        self.hash
    }

    pub fn json(&self) -> &str {
        &self.json
    }

    pub fn to_bytes(&self) -> Vec<u8> {
        format!("{}\t{}\n", self.json, self.hash).into_bytes()
    }
}

/// A line, its newline included, as its JSON and the hash it ends with, once it
/// has the shape of one: no zero first byte, at most [`MAX_LINE`] bytes, UTF-8, a
/// tab before the hash and none in the JSON. The JSON is not parsed.
pub(crate) fn split(line: &[u8]) -> Result<(&str, EntryHash), LineError> {
    if line.first() == Some(&0) {
        return Err(LineError::ZeroFilled);
    }
    if line.len() > MAX_LINE {
        return Err(LineError::TooLong);
    }
    let Some(body) = line.strip_suffix(b"\n") else {
        return Err(LineError::Unterminated);
    };
    let text = std::str::from_utf8(body).map_err(|_| LineError::NotUtf8)?;
    let tab = text
        .bytes()
        .rposition(|b| b == b'\t')
        .ok_or(LineError::NoHash)?;
    let (json, hash) = (&text[..tab], &text[tab + 1..]);
    let hash: EntryHash = hash.parse().map_err(|_| LineError::BadHash)?;
    if json.bytes().any(|b| matches!(b, b'\t' | b'\n')) {
        return Err(LineError::Unescaped);
    }
    Ok((json, hash))
}

/// `prev` as this build writes it: `{"prev":"<hash>",` or the whole object.
fn leading_prev(json: &str) -> Option<EntryHash> {
    let rest = json.strip_prefix(r#"{"prev":""#)?;
    let (hash, rest) = rest.split_at_checked(HASH_DIGITS)?;
    let follows = rest == "\"}" || rest.starts_with("\",");
    follows.then(|| hash.parse().ok()).flatten()
}

fn prev_of(json: &str) -> Result<EntryHash, LineError> {
    serde_json::from_str::<Envelope>(json)
        .map(|envelope| envelope.prev)
        .map_err(|_| LineError::NoPrev)
}

/// Where and why a file's readable lines end before its last byte.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Stop {
    /// The offset of the first byte not read as a line.
    pub offset: u64,
    pub error: LineError,
}

/// The lines of a segment's bytes from `offset` on, where `last` is the line
/// before them, if any.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Read<T = Line> {
    pub lines: Vec<T>,
    /// The offset just past the last line read.
    pub end: u64,
    /// Why reading stopped before the last byte; `None` when every byte was read,
    /// or when the bytes end with the seal marker after the last line.
    pub stop: Option<Stop>,
    /// Whether the bytes end with the seal marker after the last line.
    pub sealed: bool,
}

pub fn read(bytes: &[u8], offset: u64, last: Option<EntryHash>) -> Read {
    read_with(bytes, offset, last, Line::parse, Line::hash)
}

/// As [`read`], each line taken by `parse`, which names it by `hash`.
pub fn read_with<T>(
    bytes: &[u8],
    offset: u64,
    last: Option<EntryHash>,
    parse: impl FnMut(&[u8]) -> Result<T, LineError>,
    hash: impl Fn(&T) -> EntryHash,
) -> Read<T> {
    let mut read = Lines::new(bytes, parse);
    let found: Vec<T> = read.by_ref().collect();
    let last = found.last().map(hash).or(last);
    let Some(stop) = read.stop().cloned() else {
        return Read {
            lines: found,
            end: offset + bytes.len() as u64,
            stop: None,
            sealed: false,
        };
    };
    let rest = &bytes[stop.offset as usize..];
    let sealed = last.is_some_and(|last| rest == seal_marker(last));
    Read {
        lines: found,
        end: offset + stop.offset,
        stop: (!sealed).then(|| Stop {
            offset: offset + stop.offset,
            error: stop.error,
        }),
        sealed,
    }
}

/// The lines of a file, up to the first that cannot be read.
pub struct Lines<'a, F = fn(&[u8]) -> Result<Line, LineError>> {
    rest: &'a [u8],
    offset: u64,
    stop: Option<Stop>,
    parse: F,
}

pub fn lines(bytes: &[u8]) -> Lines<'_> {
    Lines::new(bytes, Line::parse)
}

impl<'a, T, F: FnMut(&[u8]) -> Result<T, LineError>> Lines<'a, F> {
    fn new(bytes: &'a [u8], parse: F) -> Self {
        Self {
            rest: bytes,
            offset: 0,
            stop: None,
            parse,
        }
    }

    /// Why reading stopped early; `None` while lines remain or when every byte was
    /// read.
    pub fn stop(&self) -> Option<&Stop> {
        self.stop.as_ref()
    }
}

impl<T, F: FnMut(&[u8]) -> Result<T, LineError>> Iterator for Lines<'_, F> {
    type Item = T;

    fn next(&mut self) -> Option<T> {
        if self.rest.is_empty() || self.stop.is_some() {
            return None;
        }
        let end = self
            .rest
            .iter()
            .take(MAX_LINE)
            .position(|&b| b == b'\n')
            .map_or(self.rest.len().min(MAX_LINE + 1), |at| at + 1);
        let (line, rest) = self.rest.split_at(end);
        match (self.parse)(line) {
            Ok(parsed) => {
                self.rest = rest;
                self.offset += end as u64;
                Some(parsed)
            }
            Err(error) => {
                self.stop = Some(Stop {
                    offset: self.offset,
                    error,
                });
                None
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn json(prev: EntryHash, n: u32) -> String {
        format!(r#"{{"prev":"{prev}","n":{n}}}"#)
    }

    fn chain(count: u32) -> Vec<Line> {
        let mut prev = EntryHash::ZERO;
        (0..count)
            .map(|n| {
                let line = Line::seal(json(prev, n)).unwrap();
                prev = line.hash();
                line
            })
            .collect()
    }

    fn bytes_of(lines: &[Line]) -> Vec<u8> {
        lines.iter().flat_map(Line::to_bytes).collect()
    }

    // The hash is the first half of `b3sum` over 16 zero bytes and the JSON.
    #[test]
    fn a_line_is_json_tab_hash_newline() {
        let json = r#"{"prev":"00000000000000000000000000000000"}"#;
        let expected = format!("{json}\t21fbde4eb554d0b73a2edf6821643616\n");
        let line = Line::seal(json.into()).unwrap();
        assert_eq!(String::from_utf8(line.to_bytes()).unwrap(), expected);
        assert_eq!(Line::parse(expected.as_bytes()), Ok(line));
    }

    #[test]
    fn written_lines_read_back_in_order() {
        let written = chain(3);
        let bytes = bytes_of(&written);
        let mut read = lines(&bytes);
        assert_eq!(read.by_ref().collect::<Vec<_>>(), written);
        assert_eq!(read.stop(), None);
        assert_eq!(written[1].prev(), written[0].hash());
    }

    #[test]
    fn reading_stops_at_the_first_unreadable_line() {
        let written = chain(2);
        let whole = bytes_of(&written);
        let first = written[0].to_bytes().len();
        let mut flipped = whole.clone();
        flipped[first + 9] ^= 1;
        let mut zeroed = written[0].to_bytes();
        zeroed.extend(std::iter::repeat_n(0, 40));
        let cases = [
            (whole[..whole.len() - 1].to_vec(), LineError::Unterminated),
            (zeroed, LineError::ZeroFilled),
            (flipped, LineError::Mismatch),
        ];
        for (bytes, error) in cases {
            let mut read = lines(&bytes);
            assert_eq!(read.by_ref().count(), 1, "{error:?}");
            assert_eq!(
                read.stop(),
                Some(&Stop {
                    offset: first as u64,
                    error: error.clone()
                })
            );
        }
    }

    #[test]
    fn malformed_lines_are_refused_with_their_reason() {
        let prev = EntryHash::ZERO;
        let good = json(prev, 1);
        let hash = EntryHash::of(prev, good.as_bytes());
        let cases: [(String, LineError); 6] = [
            (format!("{good}{hash}\n"), LineError::NoHash),
            (
                format!("{good}\t{}\n", hash.to_string().to_uppercase()),
                LineError::BadHash,
            ),
            (format!("[1]\t{hash}\n"), LineError::NoPrev),
            (format!("{{\"prev\":1}}\t{hash}\n"), LineError::NoPrev),
            (
                format!("{}\t{hash}\n", good.replace("1}", "1\t}")),
                LineError::Unescaped,
            ),
            (format!("{}\t{hash}\n", json(prev, 2)), LineError::Mismatch),
        ];
        for (line, error) in cases {
            assert_eq!(Line::parse(line.as_bytes()), Err(error), "{line:?}");
        }
        assert_eq!(Line::parse(b"\xff\t\n"), Err(LineError::NotUtf8));
        assert_eq!(
            Line::seal(good.replace("1}", "1\n}")),
            Err(LineError::Unescaped)
        );
    }

    #[test]
    fn a_seal_marker_ends_a_segment_only_after_the_line_it_names() {
        let written = chain(2);
        let mut sealed = bytes_of(&written);
        let end = sealed.len() as u64;
        sealed.extend(seal_marker(written[1].hash()));
        assert_eq!(seal_marker(written[1].hash()).len() as u64, SEAL_MARKER);
        assert_eq!(
            sealed[sealed.len() - ENDING as usize..],
            ending(written[1].hash())
        );
        let whole = read(&sealed, 0, None);
        assert_eq!((whole.lines.len(), whole.end), (2, end));
        assert_eq!((whole.stop, whole.sealed), (None, true));

        let first = written[0].to_bytes().len();
        let rest = read(&sealed[first..], first as u64, Some(written[0].hash()));
        assert_eq!(rest.lines, written[1..]);
        assert_eq!((rest.end, rest.sealed), (end, true));

        let mut wrong = bytes_of(&written);
        wrong.extend(seal_marker(written[0].hash()));
        let mut followed = sealed.clone();
        followed.extend(written[0].to_bytes());
        for bytes in [wrong, followed] {
            let read = read(&bytes, 0, None);
            assert!(!read.sealed);
            assert_eq!(read.stop.map(|stop| stop.offset), Some(end));
        }
        let alone = read(&seal_marker(written[1].hash()), 0, None);
        assert!(!alone.sealed && alone.lines.is_empty());
    }

    #[test]
    fn an_overlong_line_is_refused_without_reading_past_the_limit() {
        let prev = EntryHash::ZERO;
        let long = format!(r#"{{"prev":"{prev}","pad":"{}"}}"#, "x".repeat(MAX_LINE));
        assert_eq!(Line::seal(long.clone()), Err(LineError::TooLong));
        let mut bytes = long.into_bytes();
        bytes.extend_from_slice(b"\t0\n");
        let mut read = lines(&bytes);
        assert_eq!(read.next(), None);
        assert_eq!(
            read.stop().map(|stop| &stop.error),
            Some(&LineError::TooLong)
        );
    }
}
