//! Nord Drum 2 (`.nd2p`, `.nd2_bank`) — container-verified, bodies unmapped.
//!
//! The device ships banks as plain ZIP archives (stored, no compression) of one
//! `.nd2p` CBIN member per program. The member is what this module reads; the
//! bank walk lives with the other ZIP formats behind the `bundle` feature.

use super::raw::raw_format;

raw_format!(
    /// Programs (`.nd2p`) — usually met inside a `.nd2_bank` archive.
    ///
    /// The header's `aux` word holds a value on every specimen (`0x006d008f`),
    /// unlike the `0xFFFFFFFF` of the slot-addressed formats. Meaning unknown;
    /// preserved verbatim.
    program,
    "nd2p",
    175
);

#[cfg(feature = "bundle")]
pub mod bank {
    //! A `.nd2_bank` archive: its members, in archive order.

    use crate::cbin::{Cbin, RawBody};
    use crate::error::Error;
    use std::io::{Read, Seek};

    #[derive(Debug)]
    pub struct Bank {
        /// `(member name, program)`, in archive order.
        pub programs: Vec<(String, Cbin<RawBody>)>,
    }

    pub fn read_from(reader: &mut (impl Read + Seek)) -> Result<Bank, Error> {
        Ok(Bank {
            programs: super::super::zip_members(reader, super::program::FORMAT)?,
        })
    }
}
