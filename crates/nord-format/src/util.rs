use binrw::{BinRead, BinReaderExt};

use crate::common::header;

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
    pub format: String,
    pub file_type: FileType,
}

/**
 * Peek at the first byte of a file to determine its type.
 */
pub fn peek(reader: &mut impl BinReaderExt) -> Result<Peek, Error> {
    let mut head = [0u8; 1];
    if let Err(e) = reader.read_exact(&mut head) {
        return Err(e.into());
    }
    let head = head[0];

    reader.seek(std::io::SeekFrom::Start(0))?;

    let result: Result<Peek, Error> = match head {
        0x50 => Ok(Peek {
            format: String::from("unknown"),
            file_type: FileType::Zip,
        }),

        0x3c => Ok(Peek {
            format: String::from("unknown"),
            file_type: FileType::Xml,
        }),

        0x43 => match header::Preamble::read_be(reader) {
            Ok(preamble) => {
                let format = preamble.format;
                Ok(Peek {
                    format,
                    file_type: FileType::Cbin,
                })
            }
            Err(e) => Err(e.into()),
        },

        _ => Err(ParseError::UnknownFormat(format!("first_byte = {:0x}", head)).into()),
    };

    reader.seek(std::io::SeekFrom::Start(0))?;

    match result {
        Ok(peek) => Ok(peek),
        Err(e) => Err(e),
    }
}
