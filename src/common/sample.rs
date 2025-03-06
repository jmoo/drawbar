use nom::bytes::complete::tag;
use nom::number::complete::le_u16;
use nom::{Compare, Input};
use nom_locate::LocatedSpan;
use std::convert::TryInto;
use std::io::{BufReader, Read, Write};
use std::{fmt, str};

use crate::common::header::{self, Preamble};

pub const FORMAT: &str = "nsmp";

#[derive(Debug)]
pub struct Sample {
    pub preamble: Preamble,
}

impl Sample {
    pub fn parse<I>(preamble: Preamble, input: I) -> nom::IResult<I, Self>
    where
        I: Input<Item = u8> + Compare<&'static [u8]>,
    {
        Ok((input, Sample { preamble }))
    }
}

impl Read for Sample {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        let mut written = 0;
        written += self.preamble.read(&mut buf[..])?;
        Ok(written)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    type Span<'a> = LocatedSpan<&'a [u8]>;

    #[test]
    fn test_read_write_sample() {
        let bytes = b"CBIN\x01\x00\x00\x00nsmp";
        let input = Span::new(bytes);
        let (input, preamble) = Preamble::parse(input).unwrap();
        let (_, entity) = Sample::parse(preamble, input).unwrap();

        let mut buffer = [0; 12];
        let mut reader = BufReader::new(entity);
        reader.read(buffer.as_mut()).unwrap();
        assert_eq!(*bytes, buffer)
    }
}
