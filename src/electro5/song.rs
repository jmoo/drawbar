use nom::bytes::complete::tag;
use nom::number::complete::le_u16;
use nom::{Compare, Input};
use nom_locate::LocatedSpan;
use std::convert::TryInto;
use std::io::{BufReader, Read, Write};
use std::{fmt, str};

use crate::common::header::{self, Header, Preamble};

pub const FORMAT: &str = "ne5t";

#[derive(Clone, Default, Copy, PartialEq, Eq, Hash)]
pub struct Location<const SLOT_MAX: u16, const BANK_MAX: u16>(u16);

impl<const SLOT_MAX: u16, const BANK_MAX: u16> Location<SLOT_MAX, BANK_MAX> {
    pub fn parse<I>(input: I) -> nom::IResult<I, Self>
    where
        I: Input<Item = u8> + Compare<&'static [u8]>,
    {
        let (input, location) = le_u16(input)?;
        Ok((input, Location(location)))
    }
}

impl<const SLOT_MAX: u16, const BANK_MAX: u16> Into<u16> for Location<SLOT_MAX, BANK_MAX> {
    fn into(self) -> u16 {
        self.0
    }
}

impl<const SLOT_MAX: u16, const BANK_MAX: u16> From<Location<SLOT_MAX, BANK_MAX>> for (u16, u16) {
    fn from(value: Location<SLOT_MAX, BANK_MAX>) -> (u16, u16) {
        let value: u16 = value.into();
        (value / BANK_MAX, value % BANK_MAX)
    }
}

impl<const SLOT_MAX: u16, const BANK_MAX: u16> TryInto<Location<SLOT_MAX, BANK_MAX>> for u16 {
    type Error = ();

    fn try_into(self) -> Result<Location<SLOT_MAX, BANK_MAX>, Self::Error> {
        if self < SLOT_MAX * BANK_MAX {
            Ok(Location(self))
        } else {
            Err(())
        }
    }
}

impl<const SLOT_MAX: u16, const BANK_MAX: u16> Read for Location<SLOT_MAX, BANK_MAX> {
    fn read(&mut self, mut buf: &mut [u8]) -> std::io::Result<usize> {
        let value: u16 = (*self).into();
        let written = buf.write(&value.to_le_bytes())?;
        Ok(written)
    }
}

impl<const SLOT_MAX: u16, const BANK_MAX: u16> std::fmt::Debug for Location<SLOT_MAX, BANK_MAX> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let value: u16 = (*self).into();
        let pair: (u16, u16) = (*self).try_into().unwrap();

        f.debug_struct("Location")
            .field("value", &value)
            .field("slot", &pair.0)
            .field("bank", &pair.1)
            .finish()
    }
}

#[derive(Debug)]
pub struct Song {
    pub header: Header,
}

impl Song {
    pub fn parse<I>(header: Header, input: I) -> nom::IResult<I, Self>
    where
        I: Input<Item = u8> + Compare<&'static [u8]>,
    {
        Ok((input, Song { header }))
    }
}

impl Read for Song {
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
    fn test_read_write_song() {
        let bytes = b"CBIN\x01\x00\x00\x00ne5t\x07\x00\x02\x00\xff\xff\xff\xff";
        let input = Span::new(bytes);
        let (input, preamble) = Preamble::parse(input).unwrap();
        let (input, location) = header::Location::parse(input).unwrap();
        let (_, entity) = Song::parse(Header { preamble, location }, input).unwrap();

        let mut buffer = [0; 20];
        let mut reader = BufReader::new(entity);
        reader.read(buffer.as_mut()).unwrap();
        assert_eq!(*bytes, buffer)
    }
}
