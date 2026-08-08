pub mod bits;
pub mod cbin;
pub mod common;
pub mod crc;
pub mod electro5;
pub mod error;
pub mod layout;
pub mod panel;
pub mod types;
pub mod util;

use crate::cbin::Cbin;
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
    Electro5(Cbin<electro5::Program>),
}

/// The live buffer — the panel as it stands, not a saved program. Same body as
/// [`Program`], under its own format tag.
#[derive(Debug)]
pub enum Live {
    Electro5(Cbin<electro5::Program>),
}

#[derive(Debug)]
pub enum Song {
    Electro5(Cbin<electro5::Song>),
}

#[derive(Debug)]
pub enum Settings {
    Electro5(Cbin<electro5::Settings>),
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
    Live(Live),
    Piano(piano::Piano),
    Settings(Settings),
    Sample(Cbin<Sample>),
    #[cfg(feature = "bundle")]
    Bundle(Bundle),
}

pub fn from_stream(reader: &mut (impl Read + Seek + Sized)) -> Result<Entity, Error> {
    let header = peek(reader)?;

    match header.file_type {
        #[cfg(feature = "bundle")]
        FileType::Zip => Ok(Entity::Bundle(Bundle::Electro5(
            electro5::Bundle::read_from(reader)?,
        ))),
        #[cfg(not(feature = "bundle"))]
        FileType::Zip => {
            Err(ParseError::UnknownFileType("zip (bundle feature disabled)".to_string()).into())
        }
        FileType::Cbin => match header.format.as_str() {
            sample::FORMAT => Ok(Entity::Sample(sample::read_from(reader)?)),
            piano::FORMAT => Ok(Entity::Piano(piano::Piano::read_from(reader)?)),
            electro5::song::FORMAT => Ok(Entity::Song(Song::Electro5(electro5::song::read_from(
                reader,
            )?))),
            electro5::program::FORMAT => Ok(Entity::Program(Program::Electro5(
                electro5::program::read_from(reader)?,
            ))),
            electro5::live::FORMAT => Ok(Entity::Live(Live::Electro5(electro5::live::read_from(
                reader,
            )?))),
            electro5::settings::FORMAT => Ok(Entity::Settings(Settings::Electro5(
                electro5::settings::read_from(reader)?,
            ))),
            e => Err(ParseError::UnknownFormat(e.to_string()).into()),
        },
        e => Err(ParseError::UnknownFileType(e.as_str().to_string()).into()),
    }
}

pub fn from_path<P: AsRef<Path>>(path: P) -> Result<Entity, Error> {
    from_stream(&mut BufReader::new(File::open(path)?))
}

/// Serialise an [`Entity`] back to the bytes of its file — the counterpart to
/// [`from_stream`].
///
/// For every format this crate decodes, `to_bytes(from_stream(x)) == x` byte-for-byte,
/// whichever header generation `x` carries. That is the crate's central invariant —
/// decoded values are read-only views over a verbatim body, so a re-emit cannot
/// drift — and `nord verify` exists to check it against real specimens. Fixed-length
/// formats declare their body length on their [`cbin::Body`] impl, and the container
/// refuses to emit a wrong-sized file.
///
/// Bundles are unsupported: [`electro5::Bundle`] is a ZIP walk over other entities,
/// not a re-emittable structure.
pub fn to_bytes(entity: &Entity) -> Result<Vec<u8>, Error> {
    use std::io::Cursor;

    let mut out = Cursor::new(Vec::new());
    match entity {
        Entity::Program(Program::Electro5(p)) => p.write_to(&mut out)?,
        Entity::Live(Live::Electro5(l)) => l.write_to(&mut out)?,
        Entity::Song(Song::Electro5(s)) => s.write_to(&mut out)?,
        Entity::Settings(Settings::Electro5(s)) => s.write_to(&mut out)?,
        Entity::Piano(p) => p.write_to(&mut out)?,
        Entity::Sample(s) => s.write_to(&mut out)?,
        #[cfg(feature = "bundle")]
        Entity::Bundle(_) => {
            return Err(ParseError::UnknownFormat(
                "bundle (an archive, not a re-emittable entity)".to_string(),
            )
            .into())
        }
    }
    Ok(out.into_inner())
}
