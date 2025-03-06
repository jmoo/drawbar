use nom::bytes::complete::tag;
use nom::number::complete::le_u16;
use nom::{Compare, Input};
use nom_locate::LocatedSpan;
use std::convert::TryInto;
use std::io::{BufReader, Read, Write};
use std::{fmt, str};

use crate::common::header::{self, Header, Preamble};

pub const FORMAT: &str = "ne5p";

#[derive(Debug)]
pub struct Program {
    pub header: Header
}

impl Program {
    pub fn parse<I>(header: Header, input: I) -> nom::IResult<I, Self>
    where
        I: Input<Item = u8> + Compare<&'static [u8]>,
    {
        Ok((input, Program { header  }))
    }
}


impl Read for Program {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        let mut written = 0;
        written += self.header.read(&mut buf[..])?;
        Ok(written)
    } 
}

#[cfg(test)]
mod tests {
    use super::*;

    type Span<'a> = LocatedSpan<&'a [u8]>;

    #[test]
    fn test_read_write_program() {
        let bytes = b"CBIN\x01\x00\x00\x00ne5t\x07\x00\x02\x00\xff\xff\xff\xff";
        let input = Span::new(bytes);
        let (input, preamble) = Preamble::parse(input).unwrap();
        let (input, location) = header::Location::parse(input).unwrap();
        let (_, entity) = Program::parse(Header { preamble, location }, input).unwrap();

        let mut buffer = [0; 20];
        let mut reader = BufReader::new(entity);
        reader.read(buffer.as_mut()).unwrap();
        assert_eq!(*bytes, buffer)
    }
}
