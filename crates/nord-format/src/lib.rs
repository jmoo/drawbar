pub mod common;
pub mod crc;
pub mod electro5;
pub mod error;
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
            e => Err(Error::ParseError(ParseError::UnknownFormat(
                e.parse().unwrap(),
            ))),
        },
        e => Err(Error::ParseError(ParseError::UnknownFileType(
            e.as_str().parse().unwrap(),
        ))),
    }
}

pub fn from_path<P: AsRef<Path>>(path: P) -> Result<Entity, Error> {
    match File::open(path) {
        Ok(file) => from_stream(&mut BufReader::new(file)),
        Err(e) => Err(e.into()),
    }
}
