use crate::common;
use binrw::binrw;
use common::bank::Location;
use std::fmt::Debug;

#[binrw]
#[derive(Debug)]
#[brw(magic = b"CBIN")]
pub struct Preamble {
    #[brw(little)]
    pub version: u32,

    #[br(count = 4, map = | x: Vec < u8 > | String::from_utf8_lossy(& x).to_string())]
    #[bw(big, map = | x | x.as_bytes().to_vec())]
    pub format: String,
}

#[binrw]
#[derive(Debug)]
#[br(assert(trailer == 0xFFFFFFFF))]
pub struct Header<L>
where
    L: Location,
{
    pub preamble: Preamble,

    #[brw(little)]
    #[bw(calc = location.x())]
    bank: u16,

    #[brw(little)]
    #[bw(calc = location.y())]
    slot: u16,

    #[br(try_calc = match (bank, slot).try_into() { Ok(x) => Ok(x), Err(_) => Err(format!("invalid location: {} {}", bank, slot)) })]
    #[bw(ignore)]
    pub location: L,

    #[brw(little)]
    pub trailer: u32,
}

impl<L: Location> Header<L> {
    pub fn new(version: u32, schema: &str, location: L) -> Header<L> {
        Header {
            preamble: Preamble {
                version,
                format: schema.to_string(),
            },
            location,
            trailer: 0xFFFFFFFF,
        }
    }
}
