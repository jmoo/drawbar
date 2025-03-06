use nom::bytes::complete::tag;
use nom::number::complete::{le_u16, le_u32};
use nom::{Compare, Input};
use nom_locate::LocatedSpan;
use std::io::{BufReader, Read, Write};
use std::{fmt, str};

pub const CBIN_MAGIC: &[u8; 4] = b"CBIN";
pub const TRAILER_MAGIC: &[u8; 4] = b"\xff\xff\xff\xff";

#[derive(Clone, Copy, PartialEq, Eq)]
pub struct Format(u32);

impl Format {
    pub fn to_le_bytes(self) -> [u8; 4] {
        let value: u32 = self.into();
        value.to_le_bytes()
    }

    pub fn parse<I>(input: I) -> nom::IResult<I, Self>
    where
        I: Input<Item = u8>,
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

#[derive(Debug)]
pub struct Location {
    pub bank: u16,
    pub slot: u16,
}

impl Location {
    pub fn parse<I>(input: I) -> nom::IResult<I, Self>
    where
        I: Input<Item = u8> + Compare<&'static [u8]>,
    {
        // let (input, preamble) = Preamble::parse(input)?;
        let (input, bank) = le_u16(input)?;
        let (input, slot) = le_u16(input)?;

        Ok((input, Location { bank, slot }))
    }
}

impl Read for Location {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        let mut written = 0;
        written += (&mut buf[written..]).write(&self.bank.to_le_bytes())?;
        written += (&mut buf[written..]).write(&self.slot.to_le_bytes())?;
        written += (&mut buf[written..]).write(&TRAILER_MAGIC[..])?;
        Ok(written)
    }
}

#[derive(Debug)]
pub struct Header {
    pub preamble: Preamble,
    pub location: Location,
}

impl Header {
    pub fn parse<I>(input: I) -> nom::IResult<I, Self>
    where
        I: Input<Item = u8> + Compare<&'static [u8]>,
    {
        let (input, preamble) = Preamble::parse(input)?;
        let (input, location) = Location::parse(input)?;
        let (input, _) = tag(&TRAILER_MAGIC[..])(input)?;
        Ok((input, Header { preamble, location }))
    }
}

impl Read for Header {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        let mut written = 0;
        written += self.preamble.read(&mut buf[..])?;
        written += self.location.read(&mut buf[written..])?;
        written += (&mut buf[written..]).write(&TRAILER_MAGIC[..])?;
        Ok(written)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    type Span<'a> = LocatedSpan<&'a [u8]>;

    #[test]
    fn test_parse_header() {
        let bytes = b"CBIN\x01\x00\x00\x00ne5p\x07\x00\x02\x00\xff\xff\xff\xff";
        let input = Span::new(bytes);
        let (_, header) = Header::parse(input).unwrap();
        assert_eq!(header.preamble.version, 1);
        assert_eq!(header.preamble.format.to_le_bytes(), *b"ne5p");
        assert_eq!(header.location.bank, 7);
        assert_eq!(header.location.slot, 2);

        let mut buffer = [0; 20];
        let mut reader = BufReader::new(header);
        reader.read(buffer.as_mut()).unwrap();
        assert_eq!(*bytes, buffer)
    }
}
