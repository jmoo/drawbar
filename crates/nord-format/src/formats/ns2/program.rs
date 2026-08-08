//! The Stage 2 program body (`.ns2p`): 521 bytes, globals decoded, slots not.
//!
//! Every placement here is inferred from the Nord User Forum's ns3-program-viewer
//! documentation (github.com/Chris55/ns3-program-viewer); not confirmed on
//! hardware. The two slot blocks and the effects chains are documented there too
//! but not yet decoded; their bits ride along verbatim.

use crate::cbin::{self, Cbin, Header};
use crate::components::{MasterTempo, ProgramCategory, SplitNote, StageTranspose};
use crate::error::Error;
use std::io::{Read, Seek};

pub const FORMAT: &str = "ns2p";
/// Schema versions this build's field offsets have been validated against. The
/// corpus factory banks hold 6 and 7; the ns3-program-viewer accepts 2 through 7
/// with the same offsets.
pub const KNOWN_VERSIONS: &[u32] = &[2, 3, 4, 5, 6, 7];
pub const BODY_LEN: usize = 521;

/// The program-wide globals at the head of the body. Bits are MSB-first from body
/// byte 0 (`0x2c` in a type-1 file), so byte 0x02 bit 5 is bit 18.
#[nord_bits_derive::bitbody(521)]
pub struct Program {
    #[bits(18..=18)]
    pub dual_keyboard: bool,
    /// The Low split point in a three-zone split, or the only one in two zones.
    #[bits(20..=23)]
    pub split_low_note: SplitNote,
    /// ⚠️ Never `F2` (stored 0) on the panel: the High point's table starts at C3.
    #[bits(24..=27)]
    pub split_high_note: SplitNote,
    #[bits(28..=28)]
    pub split_three_zones: bool,
    #[bits(29..=29)]
    pub split_two_zones: bool,
    /// Touched at least once, not active: the untouched default stores 6 (= 0),
    /// and the EX factory live buffers hold the out-of-table 15.
    #[bits(34..=34)]
    pub transpose_enabled: bool,
    #[bits(35..=38)]
    pub transpose: StageTranspose,
    #[bits(43..=50)]
    pub master_clock: MasterTempo,
}

impl Program {
    /// Whether any split is active.
    pub fn split_enabled(&self) -> bool {
        self.split_two_zones || self.split_three_zones
    }
}

/// The category byte the header's `aux` word carries; the three bytes above it
/// are zero on every corpus specimen.
pub fn category(header: &Header) -> ProgramCategory {
    use crate::bits::Packed;
    ProgramCategory::from_bits((header.aux & 0xff) as u64).expect("decoding is total")
}

/// The `(bank, location)` pair from the header, uninterpreted: bank 0..=3,
/// location 0..=99 on current exports. Not validated — see the Stage 3's note on
/// out-of-range locations in old files.
pub fn location(file: &Cbin<Program>) -> (u16, u16) {
    file.header.slot()
}

pub fn read_from(reader: &mut (impl Read + Seek)) -> Result<Cbin<Program>, Error> {
    let file: Cbin<Program> = cbin::read(reader, FORMAT)?;
    crate::formats::known_version(FORMAT, file.header.version, KNOWN_VERSIONS)?;
    Ok(file)
}
