pub mod bits;
pub mod common;
pub mod crc;
pub mod electro5;
pub mod error;
pub mod panel;
pub mod types;
pub mod util;

use crate::common::sample::Sample;
use crate::common::{piano, sample};
use std::fs::File;
use std::io::{BufReader, Read, Seek};
use std::path::Path;
use util::{peek, FileType};

use crate::error::{Error, ParseError};

#[cfg(feature = "bundle")]
#[derive(Debug)]
pub enum Bundle {
    Electro5(electro5::Bundle),
}

#[derive(Debug)]
pub enum Program {
    Electro5(electro5::Program),
}

#[derive(Debug)]
pub enum Song {
    Electro5(electro5::Song),
}

#[derive(Debug)]
pub enum Settings {
    Electro5(electro5::Settings),
}

/// One decoded file.
///
/// `Program` is much the largest variant: a decoded panel holds its fields *and* the
/// bytes it came from, and the organ panel's bytes alone are 69. Left unboxed — one of
/// these exists per file being read, never in a collection.
#[allow(clippy::large_enum_variant)]
#[derive(Debug)]
pub enum Entity {
    Song(Song),
    Program(Program),
    Piano(piano::Piano),
    Settings(Settings),
    Sample(Sample),
    #[cfg(feature = "bundle")]
    Bundle(Bundle),
}

pub fn from_stream(reader: &mut (impl Read + Seek + Sized)) -> Result<Entity, Error> {
    let header = peek(reader)?;

    match header.file_type {
        #[cfg(feature = "bundle")]
        FileType::Zip => match electro5::Bundle::read_from(reader) {
            Ok(bundle) => Ok(Entity::Bundle(Bundle::Electro5(bundle))),
            Err(e) => Err(e.into()),
        },
        #[cfg(not(feature = "bundle"))]
        FileType::Zip => Err(Error::ParseError(ParseError::UnknownFileType(
            "zip (bundle feature disabled)".to_string(),
        ))),
        FileType::Cbin => match header.format.as_str() {
            sample::FORMAT => match sample::Sample::read_from(reader) {
                Ok(sample) => Ok(Entity::Sample(sample)),
                Err(e) => Err(e.into()),
            },
            piano::FORMAT => match piano::Piano::read_from(reader) {
                Ok(piano) => Ok(Entity::Piano(piano)),
                Err(e) => Err(e.into()),
            },
            electro5::song::FORMAT => match electro5::Song::read_from(reader) {
                Ok(song) => Ok(Entity::Song(Song::Electro5(song))),
                Err(e) => Err(e.into()),
            },
            electro5::program::FORMAT => match electro5::Program::read_from(reader) {
                Ok(program) => Ok(Entity::Program(Program::Electro5(program))),
                Err(e) => Err(e.into()),
            },
            electro5::settings::FORMAT => match electro5::Settings::read_from(reader) {
                Ok(settings) => Ok(Entity::Settings(Settings::Electro5(settings))),
                Err(e) => Err(e.into()),
            },
            e => Err(Error::ParseError(ParseError::UnknownFormat(e.to_string()))),
        },
        e => Err(Error::ParseError(ParseError::UnknownFileType(
            e.as_str().to_string(),
        ))),
    }
}

pub fn from_path<P: AsRef<Path>>(path: P) -> Result<Entity, Error> {
    match File::open(path) {
        Ok(file) => from_stream(&mut BufReader::new(file)),
        Err(e) => Err(e.into()),
    }
}

/// Serialise an [`Entity`] back to the bytes of its file — the counterpart to
/// [`from_stream`].
///
/// Every concrete type has a `write_to`, but the enum had no way out, so a caller
/// holding an `Entity` could not round-trip it without re-matching every variant and
/// depending on `binrw` directly. Keeping that inside the crate is the point: `binrw`
/// is an implementation detail.
///
/// For every format this crate decodes, `to_bytes(from_stream(x)) == x` byte-for-byte.
/// That is the crate's central invariant — decoded values are read-only views over a
/// verbatim body, so a re-emit cannot drift — and `nord verify` exists to check it
/// against real specimens.
///
/// Bundles are unsupported: [`electro5::Bundle`] is a ZIP walk over other entities,
/// not a `binrw` structure, so there is nothing to re-emit.
pub fn to_bytes(entity: &mut Entity) -> Result<Vec<u8>, Error> {
    use std::io::Cursor;

    let mut out = Cursor::new(Vec::new());

    // Fixed-size formats declare their file length, so a writer that emits the wrong
    // number of bytes can be caught here rather than producing a file that looks
    // plausible until something tries to load it. Pianos and samples are content of
    // arbitrary size and have no such length.
    let expected = match entity {
        Entity::Program(Program::Electro5(p)) => {
            p.write_to(&mut out)?;
            Some((electro5::program::FORMAT, electro5::program::FILE_LEN))
        }
        Entity::Song(Song::Electro5(s)) => {
            s.write_to(&mut out)?;
            Some((electro5::song::FORMAT, electro5::song::FILE_LEN))
        }
        Entity::Settings(Settings::Electro5(s)) => {
            s.write_to(&mut out)?;
            Some((electro5::settings::FORMAT, electro5::settings::FILE_LEN))
        }
        Entity::Piano(p) => {
            p.write_to(&mut out)?;
            None
        }
        Entity::Sample(s) => {
            s.write_to(&mut out)?;
            None
        }
        #[cfg(feature = "bundle")]
        Entity::Bundle(_) => {
            return Err(Error::ParseError(ParseError::UnknownFormat(
                "bundle (an archive, not a re-emittable entity)".to_string(),
            )))
        }
    };

    let bytes = out.into_inner();
    if let Some((format, expected)) = expected {
        if bytes.len() != expected {
            return Err(Error::ParseError(ParseError::BadEncodedLength {
                format,
                got: bytes.len(),
                expected,
            }));
        }
    }
    Ok(bytes)
}
