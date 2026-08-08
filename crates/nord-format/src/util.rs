use std::io::{Read, Seek, SeekFrom};

use crate::cbin;
use crate::error::{Error, ParseError};
use crate::formats::{cn3, midi};

pub enum FileType {
    Cbin,
    /// An Electro 2 `.cn3` library — `CNE3` magic, not CBIN.
    Cne3,
    Midi,
    Sysex,
    Xml,
    Zip,
}

impl FileType {
    pub fn as_str(&self) -> &str {
        match self {
            FileType::Cbin => "cbin",
            FileType::Cne3 => "cne3",
            FileType::Midi => "midi",
            FileType::Sysex => "sysex",
            FileType::Xml => "xml",
            FileType::Zip => "zip",
        }
    }
}

pub struct Peek {
    /// The CBIN tag as text, NULs preserved (`nsp\0` matches the `"nsp\0"` const).
    /// `"unknown"` for every non-CBIN type.
    pub format: String,
    pub file_type: FileType,
}

fn unknown(file_type: FileType) -> Peek {
    Peek {
        format: String::from("unknown"),
        file_type,
    }
}

/// Identify a file by its magic, leaving the stream where it started.
pub fn peek(reader: &mut (impl Read + Seek)) -> Result<Peek, Error> {
    let mut head = [0u8; 1];
    reader.read_exact(&mut head)?;
    reader.seek(SeekFrom::Start(0))?;

    let result = match head[0] {
        0x50 => Ok(unknown(FileType::Zip)),

        0x3c => Ok(unknown(FileType::Xml)),

        0xf0 => Ok(unknown(FileType::Sysex)),

        // 'M' — `MThd`, checked in full so a stray M is not called MIDI.
        0x4d => {
            let mut head = [0u8; 4];
            reader.read_exact(&mut head)?;
            if &head == midi::MAGIC {
                Ok(unknown(FileType::Midi))
            } else {
                // Through `result`, never a bare return: the rewind below is what
                // makes this function leave the stream where it found it.
                Err(ParseError::UnknownFormat(String::from_utf8_lossy(&head).into_owned()).into())
            }
        }

        // 'C' — CBIN, or the Electro 2 library's CNE3.
        0x43 => {
            let mut head = [0u8; 12];
            reader.read_exact(&mut head)?;
            if &head[0..4] == cbin::MAGIC {
                Ok(Peek {
                    format: String::from_utf8_lossy(&head[8..12]).into_owned(),
                    file_type: FileType::Cbin,
                })
            } else if &head[0..4] == cn3::MAGIC {
                Ok(unknown(FileType::Cne3))
            } else {
                Err(
                    ParseError::UnknownFormat(String::from_utf8_lossy(&head[0..4]).into_owned())
                        .into(),
                )
            }
        }

        b => Err(ParseError::UnknownFormat(format!("first_byte = {b:0x}")).into()),
    };

    reader.seek(SeekFrom::Start(0))?;

    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    /// A refusal still rewinds: a caller that falls back to another reader after
    /// `peek` says no must find the stream at byte 0, not mid-header.
    #[test]
    fn a_c_that_is_not_cbin_is_refused_with_the_stream_rewound() {
        let mut reader = Cursor::new(b"CRUD\0\0\0\0abcdefgh".to_vec());
        assert!(peek(&mut reader).is_err());
        assert_eq!(reader.stream_position().unwrap(), 0);
    }

    #[test]
    fn the_non_cbin_magics_classify() {
        for (bytes, want) in [
            (&b"\xf0\x33\x0f\x04\xf7\0\0\0\0\0\0\0\0"[..], "sysex"),
            (&b"MThd\0\0\0\x06\0\0\0\0\0"[..], "midi"),
            (&b"CNE3\x2c\x01\0\0\0\0\0\0\0"[..], "cne3"),
        ] {
            let mut reader = Cursor::new(bytes.to_vec());
            let peeked = peek(&mut reader).unwrap();
            assert_eq!(peeked.file_type.as_str(), want);
            assert_eq!(reader.stream_position().unwrap(), 0);
        }
    }
}
