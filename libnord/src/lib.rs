pub mod common;
pub mod crc;
pub mod electro5;
pub mod error;
pub mod util;

use error::Error;

use crate::common::sample::Sample;
use crate::common::{piano, sample};
use std::fs::File;
use std::io::{BufReader, Read, Seek};
use std::path::Path;
use util::{peek, FileType};
use crate::common::bank::Coordinates;

pub type NordResult<T> = Result<T, Error>;

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
    Bundle(Bundle),
}

pub fn from_stream(reader: &mut (impl Read + Seek + Sized)) -> Result<Entity, String> {
    let header = match peek(reader) {
        Ok(header) => header,
        Err(e) => return Err(e.to_string()),
    };

    match header.file_type {
        FileType::Zip => match electro5::Bundle::read_from(reader) {
            Ok(bundle) => Ok(Entity::Bundle(Bundle::Electro5(bundle))),
            Err(e) => Err(e.to_string()),
        },
        FileType::Cbin => match header.format.as_str() {
            sample::FORMAT => match sample::Sample::read_from(reader) {
                Ok(sample) => Ok(Entity::Sample(sample)),
                Err(e) => Err(e.to_string()),
            },
            piano::FORMAT => match piano::Piano::read_from(reader) {
                Ok(piano) => Ok(Entity::Piano(piano)),
                Err(e) => Err(e.to_string()),
            },
            electro5::song::FORMAT => match electro5::Song::read_from(reader) {
                Ok(song) => Ok(Entity::Song(Song::Electro5(song))),
                Err(e) => Err(e.to_string()),
            },
            electro5::program::FORMAT => match electro5::Program::read_from(reader) {
                Ok(program) => Ok(Entity::Program(Program::Electro5(program))),
                Err(e) => Err(e.to_string()),
            },
            // electro5::live::FORMAT => match electro5::Program::read_from(reader) {
            //     Ok(program) => Ok(Entity::Program(Program::Electro5(program))),
            //     Err(e) => Err(e.to_string()),
            // },
            electro5::settings::FORMAT => match electro5::Settings::read_from(reader) {
                Ok(settings) => Ok(Entity::Settings(Settings::Electro5(settings))),
                Err(e) => Err(e.to_string()),
            },
            _ => Err("Unknown schema".to_string()),
        },
        _ => Err("Unknown file type".to_string()),
    }
}

pub fn from_path<P: AsRef<Path>>(path: P) -> Result<Entity, String> {
    match File::open(path) {
        Ok(file) => from_stream(&mut BufReader::new(file)),
        Err(e) => Err(e.to_string()),
    }
}
