use crate::schema::header;
use binrw::BinRead;
use std::fmt;

#[derive(BinRead)]
#[br(assert(preamble.schema() == "nsmp"))]
pub struct Sample {
    preamble: header::Preamble,
}

impl Sample {
    pub fn schema(&self) -> String {
        self.preamble.schema().to_string()
    }
}

impl fmt::Debug for Sample {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(
            f,
            "\n  Sample: (\n    \
              Schema: {}\n  \
           )\n",
            self.schema(),
        )
    }
}
