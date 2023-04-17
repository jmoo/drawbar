pub mod bank;
pub mod crc;
pub mod song;
pub mod program;
pub mod piano;
pub mod sample;
pub mod settings;

pub mod util;
pub use util::{peek, FileType};

pub mod header;
pub use header::Header;
pub use header::Preamble;

mod error;
pub use error::Error;
