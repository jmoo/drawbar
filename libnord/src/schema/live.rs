use crate::schema::header::Header;
use binrw::BinRead;
use std::fmt;

#[derive(BinRead)]
pub struct Live {
    header: Header,
}

impl Live {
    pub fn schema(&self) -> String {
        self.header.schema()
    }
}

impl fmt::Debug for Live {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(
            f,
            "\n  Live: (\n    \
              Schema: {}\n  \
           )\n",
            self.schema(),
        )
    }
}
