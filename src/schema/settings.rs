use crate::schema::header::Header;
use binrw::BinRead;
use std::fmt;

#[derive(BinRead)]
pub struct Settings {
    header: Header,
}

impl Settings {
    pub fn schema(&self) -> String {
        self.header.schema()
    }
}

impl fmt::Debug for Settings {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(
            f,
            "\n  Settings: (\n    \
              Schema: {}\n  \
           )\n",
            self.schema(),
        )
    }
}
