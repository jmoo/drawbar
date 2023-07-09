use std::fmt::{Debug, Formatter};
use std::io::Read;

use modular_bitfield_msb::error::{InvalidBitPattern, OutOfBounds};
use modular_bitfield_msb::prelude::*;
use modular_bitfield_msb::Specifier;

trait PackedNumber: Specifier + Debug {}

pub struct PackedU7(ux::u7);

impl Specifier for PackedU7 {
    const BITS: usize = 0;
    const STRUCT: bool = false;
    type Bytes = ();
    type InOut = ();

    fn into_bytes(_input: Self::InOut) -> Result<Self::Bytes, OutOfBounds> {
        todo!()
    }

    fn from_bytes(_bytes: Self::Bytes) -> Result<Self::InOut, InvalidBitPattern<Self::Bytes>> {
        todo!()
    }
}

impl Debug for PackedU7 {
    fn fmt(&self, _f: &mut Formatter<'_>) -> std::fmt::Result {
        todo!()
    }
}

impl PackedNumber for PackedU7 {}

pub struct U7(ux::u7);

pub struct Foo {
    inner: u8,
}

trait CbinType: Specifier {}

impl Debug for U7 {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{:#06b}, {}", self.0, self.0)
    }
}

impl Specifier for U7 {
    const BITS: usize = 7;
    const STRUCT: bool = true;
    type Bytes = u8;
    type InOut = U7;

    fn into_bytes(input: Self::InOut) -> Result<Self::Bytes, OutOfBounds> {
        Ok(input.0.try_into().map_err(|_| OutOfBounds {})?)
    }

    fn from_bytes(bytes: Self::Bytes) -> Result<Self::InOut, InvalidBitPattern<Self::Bytes>> {
        println!("bytes: {:#06b}", bytes);
        Ok(U7(ux::u7::try_from(bytes >> 1).map_err(|_| {
            InvalidBitPattern {
                invalid_bytes: bytes,
            }
        })?))
    }
}

impl Debug for Foo {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{:#06b}, {}", self.inner, self.inner)
    }
}

#[bitfield]
#[derive(Debug, Default)]
pub struct ExtraBitfield {
    pub fx1_control: bool,

    //pub fx2_deep: bool,
    pub foo: U7,
}

fn main() {
    println!("{:?}", ExtraBitfield::from_bytes([0b01111111]));
    // let mut stdin = std::io::stdin();
    // let mut buffer = Vec::new();
    // stdin.read_to_end(&mut buffer).unwrap();
    //
    // let mut cursor = Cursor::new(&mut buffer);
    // let file = from_stream(&mut cursor);
    //
    // println!("{:?}", file);
}
