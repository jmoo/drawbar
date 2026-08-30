//! Container sniffing: classify a stream by its leading bytes, before any
//! format-specific reader runs.

use std::io::{Read, Seek, SeekFrom};

use crate::cbin;
use crate::error::{Error, ParseError};
use crate::formats::{cn3, midi, nsmpproj};

/// The container classes [`peek`] distinguishes.
pub enum FileType {
    Cbin,
    /// An Electro 2 `.cn3` library — `CNE3` magic, not CBIN.
    Cne3,
    Midi,
    /// A Nord Sample Editor project — `SMACEditorProject {` text, not CBIN.
    SampleProject,
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
            FileType::SampleProject => "nsmpproj",
            FileType::Sysex => "sysex",
            FileType::Xml => "xml",
            FileType::Zip => "zip",
        }
    }
}

/// What [`peek`] learned from the leading bytes.
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
    let start = reader.stream_position()?;
    let result = (|| {
        let mut head = [0u8; 1];
        reader.read_exact(&mut head)?;
        reader.seek(SeekFrom::Start(start))?;

        match head[0] {
            // 'P' — a ZIP local-file header, checked in full so a stray P (or a bare
            // central directory) is not called an archive.
            0x50 => {
                let mut head = [0u8; 4];
                reader.read_exact(&mut head)?;
                if &head == b"PK\x03\x04" {
                    Ok(unknown(FileType::Zip))
                } else {
                    Err(
                        ParseError::UnknownFormat(String::from_utf8_lossy(&head).into_owned())
                            .into(),
                    )
                }
            }

            0x3c => Ok(unknown(FileType::Xml)),

            // 'S' — a Sample Editor project, checked in full so a stray S is not one.
            0x53 => {
                let mut head = vec![0u8; nsmpproj::MAGIC.len()];
                reader.read_exact(&mut head)?;
                if head == nsmpproj::MAGIC {
                    Ok(unknown(FileType::SampleProject))
                } else {
                    Err(
                        ParseError::UnknownFormat(String::from_utf8_lossy(&head).into_owned())
                            .into(),
                    )
                }
            }

            0xf0 => Ok(unknown(FileType::Sysex)),

            // 'M' — `MThd`, checked in full so a stray M is not called MIDI.
            0x4d => {
                let mut head = [0u8; 4];
                reader.read_exact(&mut head)?;
                if &head == midi::MAGIC {
                    Ok(unknown(FileType::Midi))
                } else {
                    Err(
                        ParseError::UnknownFormat(String::from_utf8_lossy(&head).into_owned())
                            .into(),
                    )
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
                    Err(ParseError::UnknownFormat(
                        String::from_utf8_lossy(&head[0..4]).into_owned(),
                    )
                    .into())
                }
            }

            b => Err(ParseError::UnknownFormat(format!("first_byte = {b:0x}")).into()),
        }
    })();

    reader.seek(SeekFrom::Start(start))?;

    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    #[test]
    fn a_refusal_restores_the_starting_position() {
        let mut reader = Cursor::new(b"skipCRUD\0\0\0\0abcdefgh".to_vec());
        reader.set_position(4);
        assert!(peek(&mut reader).is_err());
        assert_eq!(reader.stream_position().unwrap(), 4);
    }

    #[test]
    fn a_short_header_restores_the_starting_position() {
        let mut reader = Cursor::new(b"skipMTh".to_vec());
        reader.set_position(4);
        assert!(peek(&mut reader).is_err());
        assert_eq!(reader.stream_position().unwrap(), 4);
    }

    #[test]
    fn a_match_restores_the_starting_position() {
        let mut reader = Cursor::new(b"skipMThd\0\0\0\x06\0\0\0\0\0".to_vec());
        reader.set_position(4);
        assert_eq!(peek(&mut reader).unwrap().file_type.as_str(), "midi");
        assert_eq!(reader.stream_position().unwrap(), 4);
    }

    #[test]
    fn the_non_cbin_magics_classify() {
        for (bytes, want) in [
            (&b"\xf0\x33\x0f\x04\xf7\0\0\0\0\0\0\0\0"[..], "sysex"),
            (&b"MThd\0\0\0\x06\0\0\0\0\0"[..], "midi"),
            (&b"CNE3\x2c\x01\0\0\0\0\0\0\0"[..], "cne3"),
            (&b"SMACEditorProject {\n}\n"[..], "nsmpproj"),
        ] {
            let mut reader = Cursor::new(bytes.to_vec());
            let peeked = peek(&mut reader).unwrap();
            assert_eq!(peeked.file_type.as_str(), want);
            assert_eq!(reader.stream_position().unwrap(), 0);
        }
    }
}
