use binrw::{BinRead, BinWrite};

#[derive(BinRead, BinWrite, Debug)]
#[brw(magic = b"CBIN")]
pub struct Preamble {
    #[brw(little)]
    pub version: u32,

    #[br(count = 4, map = | x: Vec < u8 > | String::from_utf8_lossy(& x).to_string())]
    #[bw(big, map = |x| x.as_bytes().to_vec())]
    pub format: String,
}

#[derive(BinRead, BinWrite)]
#[br(assert(trailer == 0xFFFFFFFF))]
pub struct Header {
    pub preamble: Preamble,

    #[brw(little)]
    pub bank: u16,

    #[brw(little)]
    pub slot: u16,

    #[brw(little)]
    pub trailer: u32,
}

impl Header {
    pub fn new(schema: &str, bank: u16, slot: u16) -> Header {
        Header {
            preamble: Preamble {
                version: 0,
                format: schema.to_string(),
            },
            bank,
            slot,
            trailer: 0xFFFFFFFF,
        }
    }
}
