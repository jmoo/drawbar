use nom::bytes::complete::tag;
use nom::number::complete::{le_u16, le_u32, le_u8};
use nom::{Compare, Input};
use nom_locate::LocatedSpan;
use std::convert::TryInto;
use std::io::{BufReader, Read, Write};
use std::{fmt, str};

const CBIN_MAGIC: &[u8; 4] = b"CBIN";
const TRAILER_MAGIC: &[u8; 4] = b"\xff\xff\xff\xff";

#[derive(Clone, Copy, PartialEq, Eq)]
pub struct Format(u32);

impl Format {
    pub fn to_le_bytes(self) -> [u8; 4] {
        let value: u32 = self.into();
        value.to_le_bytes()
    }

    pub fn parse<I>(input: I) -> nom::IResult<I, Self>
    where
        I: Input<Item = u8> 
    {
        let (input, format) = le_u32(input)?;
        Ok((input, Format(format)))
    }
}

impl Into<[u8; 4]> for Format {
    fn into(self) -> [u8; 4] {
        self.to_le_bytes()
    }
}

impl Into<u32> for Format {
    fn into(self) -> u32 {
        self.0
    }
}

impl Read for Format {
    fn read(&mut self, mut buf: &mut [u8]) -> std::io::Result<usize> {
        let written = buf.write(&self.to_le_bytes())?;
        Ok(written)
    }
}

impl fmt::Debug for Format {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let binding = self.to_le_bytes();
        let s = str::from_utf8(&binding).unwrap();
        f.debug_tuple("Format").field(&s).finish()
    }
}

#[derive(Debug)]
pub struct Preamble {
    pub version: u32,
    pub format: Format,
}

impl Preamble {
    pub fn parse<I>(input: I) -> nom::IResult<I, Self>
    where
        I: Input<Item = u8> + Compare<&'static [u8]>,
    {
        let (input, _) = tag(&CBIN_MAGIC[..])(input)?;
        let (input, version) = le_u32(input)?;
        let (input, format) = le_u32(input)?;

        Ok((
            input,
            Preamble {
                version,
                format: Format(format),
            },
        ))
    }
}

impl Read for Preamble {
    fn read(&mut self, mut buf: &mut [u8]) -> std::io::Result<usize> {
        let mut written = 0;
        written += buf.write(CBIN_MAGIC)?;
        written += buf.write(&self.version.to_le_bytes())?;
        written += buf.write(&self.format.to_le_bytes())?;
        Ok(written)
    }
}

#[derive(Clone, Default, Copy, PartialEq, Eq, Hash)]
pub struct Location<const SLOT_MAX: u32, const BANK_MAX: u32>(u32);

impl<const SLOT_MAX: u32, const BANK_MAX: u32> Location<SLOT_MAX, BANK_MAX> {
    pub fn parse<I>(input: I) -> nom::IResult<I, Self>
    where
        I: Input<Item = u8> + Compare<&'static [u8]>,
    {
        let (input, location) = le_u32(input)?;
        Ok((input, Location(location)))
    }
}

impl<const SLOT_MAX: u32, const BANK_MAX: u32> Into<u32> for Location<SLOT_MAX, BANK_MAX> {
    fn into(self) -> u32 {
        self.0
    }
}

impl<const SLOT_MAX: u32, const BANK_MAX: u32> From<Location<SLOT_MAX, BANK_MAX>> for (u32, u32) {
    fn from(value: Location<SLOT_MAX, BANK_MAX>) -> (u32, u32) {
        let value: u32 = value.into();
        (value / BANK_MAX, value % BANK_MAX)
    }
}

impl<const SLOT_MAX: u32, const BANK_MAX: u32> TryInto<Location<SLOT_MAX, BANK_MAX>> for u32 {
    type Error = ();

    fn try_into(self) -> Result<Location<SLOT_MAX, BANK_MAX>, Self::Error> {
        if self < SLOT_MAX * BANK_MAX {
            Ok(Location(self))
        } else {
            Err(())
        }
    }
}

impl<const SLOT_MAX: u32, const BANK_MAX: u32> Read for Location<SLOT_MAX, BANK_MAX> {
    fn read(&mut self, mut buf: &mut [u8]) -> std::io::Result<usize> {
        let value: u32 = (*self).into();
        let written = buf.write(&value.to_le_bytes())?;
        Ok(written)
    }
}

impl<const SLOT_MAX: u32, const BANK_MAX: u32> std::fmt::Debug for Location<SLOT_MAX, BANK_MAX> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let value: u32 = (*self).into();
        let pair: (u32, u32) = (*self).try_into().unwrap();

        f.debug_struct("Location")
            .field("value", &value)
            .field("slot", &pair.0)
            .field("bank", &pair.1)
            .finish()
    }
}

pub struct Trailer();

impl Trailer {
    pub fn parse<I>(input: I) -> nom::IResult<I, Self>
    where
        I: Input<Item = u8> + Compare<&'static [u8]>,
    {
        let (input, _) = tag(&TRAILER_MAGIC[..])(input)?;
        Ok((input, Trailer()))
    }
}

impl Read for Trailer {
    fn read(&mut self, mut buf: &mut [u8]) -> std::io::Result<usize> {
        let written = buf.write(TRAILER_MAGIC)?;
        Ok(written)
    }
}

#[derive(Debug)]
pub struct Header<const SLOT_MAX: u32, const BANK_MAX: u32> {
    pub preamble: Preamble,
    pub location: Location<SLOT_MAX, BANK_MAX>,
}

impl<const SLOT_MAX: u32, const BANK_MAX: u32> Header<SLOT_MAX, BANK_MAX> {
    pub fn parse<I>(input: I) -> nom::IResult<I, Self>
    where
        I: Input<Item = u8> + Compare<&'static [u8]>,
    {
        let (input, preamble) = Preamble::parse(input)?;
        let (input, location) = Location::parse(input)?;
        let (input, _) = Trailer::parse(input)?;
        Ok((input, Header { preamble, location }))
    }
}

impl<const SLOT_MAX: u32, const BANK_MAX: u32> Read for Header<SLOT_MAX, BANK_MAX> {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        let mut written = 0;
        written += self.preamble.read(buf)?;
        written += self.location.read(&mut buf[written..])?;
        written += Trailer().read(&mut buf[written..])?;
        Ok(written)
    }
}

type Span<'a> = LocatedSpan<&'a [u8]>;

#[cfg(test)]
mod tests {
    // Note this useful idiom: importing names from outer (for mod tests) scope.
    use super::*;

    #[test]
    fn test_add() {
        assert_eq!(3, 3);
    }
}
