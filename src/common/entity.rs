use crate::electro5::{self, program};
use nom::{
    error::{Error, ErrorKind, ParseError},
    Compare, Err, Input,
};
use std::io::{BufReader, Read};
use std::{fmt, str};

use nom_locate::LocatedSpan;

use crate::common::header::{Header, Preamble};

use super::header;

#[derive(Debug)]
pub enum Program {
    Electro5(electro5::program::Program),
}

impl Read for Program {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        match self {
            Program::Electro5(a) => a.read(buf),
        }
    }
}

#[derive(Debug)]
pub enum Song {
    Electro5(electro5::song::Song),
}

impl Read for Song {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        match self {
            Song::Electro5(a) => a.read(buf),
        }
    }
}

#[derive(Debug)]
pub enum Entity {
    Song(Song),
    Program(Program),
    Piano(super::piano::Piano),
    Sample(super::sample::Sample),
}

impl Entity {
    pub fn parse<I>(input: I) -> nom::IResult<I, Self>
    where
        I: Input<Item = u8> + Compare<&'static [u8]>,
    {
        let (input, preamble) = Preamble::parse(input)?;
        let binding = preamble.format.to_le_bytes();
        let format = str::from_utf8(&binding).unwrap();

        match format {
            super::sample::FORMAT => {
                let (input, sample) = super::sample::Sample::parse(preamble, input)?;
                Ok((input, Entity::Sample(sample)))
            }
            super::piano::FORMAT => {
                let (input, piano) = super::piano::Piano::parse(preamble, input)?;
                Ok((input, Entity::Piano(piano)))
            }
            electro5::program::FORMAT => {
                let (input, location) = header::Location::parse(input)?;
                let (input, program) =
                    electro5::program::Program::parse(Header { preamble, location }, input)?;
                Ok((input, Entity::Program(Program::Electro5(program))))
            }
            electro5::song::FORMAT => {
                let (input, location) = header::Location::parse(input)?;
                let (input, song) =
                    electro5::song::Song::parse(Header { preamble, location }, input)?;
                Ok((input, Entity::Song(Song::Electro5(song))))
            }
            _ => Err(Err::Error(Error::from_error_kind(input, ErrorKind::NoneOf))),
        }
    }
}

impl Read for Entity {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        match self {
            Entity::Sample(f) => f.read(buf),
            Entity::Piano(f) => f.read(buf),
            Entity::Program(f) => f.read(buf),
            Entity::Song(f) => f.read(buf),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    type Span<'a> = LocatedSpan<&'a [u8]>;

    #[test]
    fn test_parse_entity() {
        let bytes = b"CBIN\x01\x00\x00\x00ne5t\x07\x00\x02\x00\xff\xff\xff\xff";
        let input = Span::new(bytes);
        let (_, entity) = Entity::parse(input).unwrap();

        let mut buffer = [0; 20];
        let mut reader = BufReader::new(entity);
        reader.read(buffer.as_mut()).unwrap();
        assert_eq!(*bytes, buffer)
    }
}
