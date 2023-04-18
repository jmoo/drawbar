use binrw::{BinRead, BinWrite};

#[derive(BinRead, BinWrite, Debug)]
#[brw(magic = b"CBIN")]
pub struct Preamble {
    #[brw(little)]
    version: u32,

    #[br(count = 4, map = | x: Vec < u8 > | String::from_utf8_lossy(& x).to_string())]
    #[bw(big, map = |x| x.as_bytes().to_vec())]
    schema: String,
}

impl Preamble {
    pub fn schema(&self) -> &str {
        return self.schema.as_str();
    }
}

#[derive(BinRead, BinWrite)]
#[br(assert(trailer == 0xFFFFFFFF))]
pub struct Header {
    preamble: Preamble,

    #[brw(little)]
    bank: u16,

    #[brw(little)]
    location: u16,

    #[brw(little)]
    trailer: u32,
}

impl Header {
    pub fn new(schema: &str, bank: u16, location: u16) -> Header {
        Header {
            preamble: Preamble {
                version: 0,
                schema: schema.to_string(),
            },
            bank,
            location,
            trailer: 0xFFFFFFFF,
        }
    }

    pub fn bank(&self) -> u16 {
        return self.bank;
    }

    pub fn location(&self) -> u16 {
        return self.location;
    }

    pub fn schema(&self) -> String {
        return self.preamble.schema().to_string();
    }

    pub fn version(&self) -> u32 {
        return self.preamble.version;
    }
}
