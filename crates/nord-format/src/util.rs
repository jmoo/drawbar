use std::io::{Read, Seek, SeekFrom};

use crate::cbin;
use crate::error::{Error, ParseError};

pub enum FileType {
    Cbin,
    Xml,
    Zip,
}

impl FileType {
    pub fn as_str(&self) -> &str {
        match self {
            FileType::Cbin => "cbin",
            FileType::Xml => "xml",
            FileType::Zip => "zip",
        }
    }
}

pub struct Peek {
    /// The CBIN tag as text, NULs preserved (`nsp\0` matches the `"nsp\0"` const).
    pub format: String,
    pub file_type: FileType,
}

/// Identify a file by its magic, leaving the stream where it started.
pub fn peek(reader: &mut (impl Read + Seek)) -> Result<Peek, Error> {
    let mut head = [0u8; 1];
    reader.read_exact(&mut head)?;
    reader.seek(SeekFrom::Start(0))?;

    let result = match head[0] {
        0x50 => Ok(Peek {
            format: String::from("unknown"),
            file_type: FileType::Zip,
        }),

        0x3c => Ok(Peek {
            format: String::from("unknown"),
            file_type: FileType::Xml,
        }),

        0x43 => {
            let mut head = [0u8; 12];
            reader.read_exact(&mut head)?;
            if &head[0..4] != cbin::MAGIC {
                // Through `result`, never a bare return: the rewind below is what
                // makes this function leave the stream where it found it.
                Err(
                    ParseError::UnknownFormat(String::from_utf8_lossy(&head[0..4]).into_owned())
                        .into(),
                )
            } else {
                Ok(Peek {
                    format: String::from_utf8_lossy(&head[8..12]).into_owned(),
                    file_type: FileType::Cbin,
                })
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
}
