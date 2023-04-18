use crate::schema::header::Header;
use binrw::BinRead;
use std::fmt;

#[derive(BinRead)]
#[br(assert(header.schema() == "npno"))]
pub struct Piano {
    header: Header,
}

impl Piano {
    pub fn schema(&self) -> String {
        self.header.schema()
    }
}

impl fmt::Debug for Piano {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(
            f,
            "\n  Piano: (\n    \
              Schema: {}\n  \
           )\n",
            self.schema(),
        )
    }
}
