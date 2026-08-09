//! Nord Drum 3P (`.nd3k`, `.nd3_kitbank`) — container-verified, bodies unmapped.
//!
//! Same shape as the Drum 2: kit banks are stored ZIP archives of one `.nd3k`
//! CBIN member per kit.

use super::raw::raw_format;

raw_format!(
    /// Kits (`.nd3k`) — usually met inside a `.nd3_kitbank` archive.
    kit,
    "nd3k",
    247
);

#[cfg(feature = "bundle")]
pub mod kit_bank {
    //! A `.nd3_kitbank` archive: its members, in archive order.

    use crate::cbin::{Cbin, RawBody};
    use crate::error::Error;
    use std::io::{Read, Seek};

    /// A Drum 3P kit bank: its kits, in archive order.
    #[derive(Debug)]
    pub struct KitBank {
        /// `(member name, kit)`, in archive order.
        pub kits: Vec<(String, Cbin<RawBody>)>,
    }

    pub fn read_from(reader: &mut (impl Read + Seek)) -> Result<KitBank, Error> {
        Ok(KitBank {
            kits: super::super::zip_members(reader, super::kit::FORMAT)?,
        })
    }
}
