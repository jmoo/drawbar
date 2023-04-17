pub mod common;
pub mod electro5;
pub mod schema;

pub use common::util;

use crate::common::Error;
use std::fs::File;
use std::io::{BufReader, Read, Seek};
use std::path::Path;
use util::{peek, FileType};

pub type NordResult<T> = Result<T, Error>;

pub enum Program {
    Electro5(electro5::Program),
}

pub enum Song {
    Electro5(electro5::Song),
}

pub enum Entity {
    Song(Song),
    Program(Program),
}

pub fn from_stream(reader: &mut (impl Read + Seek + Sized)) -> Result<Entity, String> {
    let header = match peek(reader) {
        Ok(header) => header,
        Err(e) => return Err(e.to_string()),
    };

    match header.file_type {
        FileType::Cbin => match header.format.as_str() {
            electro5::song::FORMAT => match electro5::Song::read_from(reader) {
                Ok(song) => Ok(Entity::Song(Song::Electro5(song))),
                Err(e) => Err(e.to_string()),
            },
            electro5::program::FORMAT => match electro5::Program::read_from(reader) {
                Ok(program) => Ok(Entity::Program(Program::Electro5(program))),
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
