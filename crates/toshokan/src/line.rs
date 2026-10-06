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
        let (json, hash) = text.rsplit_once('\t').ok_or(LineError::NoHash)?;
        let hash: EntryHash = hash.parse().map_err(|_| LineError::BadHash)?;
        if json.contains(['\t', '\n']) {
            return Err(LineError::Unescaped);
        }
        let prev = prev_of(json)?;
        if EntryHash::of(prev, json.as_bytes()) != hash {
            return Err(LineError::Mismatch);
        }
        Ok(Self {
            prev,
            hash,
            json: json.to_owned(),
        })
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

/// The lines of a file, up to the first that cannot be read.
pub struct Lines<'a> {
    rest: &'a [u8],
    offset: u64,
    stop: Option<Stop>,
}

pub fn lines(bytes: &[u8]) -> Lines<'_> {
    Lines {
        rest: bytes,
        offset: 0,
        stop: None,
    }
}

impl Lines<'_> {
    /// Why reading stopped early; `None` while lines remain or when every byte was
    /// read.
    pub fn stop(&self) -> Option<&Stop> {
        self.stop.as_ref()
    }
}

impl Iterator for Lines<'_> {
    type Item = Line;

    fn next(&mut self) -> Option<Line> {
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
        match Line::parse(line) {
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
