use binrw::{BinRead, BinReaderExt};
use byteorder::ReadBytesExt;

use std::io;
use std::io::{Seek};
use crate::common::header;

pub enum FileType {
    Cbin,
    Xml,
    Zip,
}

pub struct Peek {
    pub format: String,
    pub file_type: FileType,
}

/**
 * Peek at the first byte of a file to determine its type.
 */
pub fn peek(reader: &mut impl BinReaderExt) -> Result<Peek, io::Error> {
    let head = match reader.read_u8() {
        Ok(head) => head,
        Err(e) => return Err(io::Error::new(io::ErrorKind::InvalidData, e.to_string())),
    };

    reader.seek(std::io::SeekFrom::Start(0))?;

    let result = match head {
        0x50 => Ok(Peek {
            format: String::from("unknown"),
            file_type: FileType::Zip,
        }),

        0x3c => Ok(Peek {
            format: String::from("unknown"),
            file_type: FileType::Xml,
        }),

        0x43 => {
            if let Ok(preamble) = header::Preamble::read_be(reader) {
                let format = preamble.format;
                Ok(Peek {
                    format,
                    file_type: FileType::Cbin,
                })
            } else {
                Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "Invalid file type",
                ))
            }
        }

        _ => Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "Invalid file type",
        )),
    };

    reader.seek(std::io::SeekFrom::Start(0))?;

    result
}
