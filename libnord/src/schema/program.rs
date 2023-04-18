use crate::schema::header::Header;
use binrw::BinRead;
use std::fmt;

#[derive(BinRead)]
pub struct Program {
    header: Header,
}

impl Program {
    pub fn bank(&self) -> u16 {
        self.header.bank()
    }

    pub fn location(&self) -> u16 {
        self.header.location()
    }

    pub fn schema(&self) -> String {
        self.header.schema()
    }
}

impl fmt::Debug for Program {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(
            f,
            "\n  Program: (\n    \
              Schema: {}\n    \
              Bank: {}\n    \
              Location: {}\n  \
           )\n",
            self.schema(),
            self.bank(),
            self.location(),
        )
    }
}
