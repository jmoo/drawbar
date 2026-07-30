//! The Electro 5 program format (`.ne5p`).
//!
//! Reads top-down: the format's constants, then [`Schema`] — the file as `binrw` sees
//! it — then [`Program`], which is a `Schema` plus the slot it lives in. Each panel is a
//! `#[bitpanel]` in its own module.

mod center;
mod effects;
mod organ;
mod piano;
mod sample;

pub use center::{CenterPanel, OrganType};
pub use effects::{EffectsPanel, EqualizerPart, Fx1Type, Fx2Type, Fx3Type, Fx5Type, Routing};
pub use organ::{B3PercSpeed, B3Vib, Drawbars, FarfisaVib, OrganModel, OrganPanel, VoxVib};
pub use piano::{PianoCategory, PianoPanel};
pub use sample::SamplePanel;

use crate::common;
use crate::common::bank;
use crate::crc::{CrcReader, CrcWriter};
use crate::types::RangedU16Pair;
use binrw::{binrw, BinRead, BinReaderExt, BinWriterExt};

use std::io;

pub const FORMAT: &str = "ne5p";
/// Schema versions this build's field offsets have been validated against. Every corpus
/// program reports 4. See [`crate::error::ParseError::UnsupportedVersion`].
pub const KNOWN_VERSIONS: &[u32] = &[4];
/// Total file length: 44-byte CBIN header + 121-byte body.
pub const FILE_LEN: usize = 165;
pub const BANK_COUNT: u16 = 8;
pub const SLOT_COUNT: u16 = 50;

pub type Location = RangedU16Pair<BANK_COUNT, SLOT_COUNT>;
pub type Header = common::Header<Location>;
pub type Bank = bank::Bank<Program, Location>;
#[binrw]
#[derive(Debug)]
#[br(little, stream = r, map_stream = CrcReader::new(0x2c, 0xa4 - 0x2c), assert(r.checksum() == crc32, "bad checksum: {:#x?} != {:#x?}", r.checksum(), crc32))]
#[bw(little, stream = w, map_stream = CrcWriter::new(0x2c, 0xa4 - 0x2c))]
pub struct Schema {
    pub header: Header,

    pub version: u32,

    // 0x18..0x1a
    #[bw(try_calc = w.checksum())]
    crc32: u32,

    // 0x2c..0x2d
    #[brw(big, pad_before = 16)]
    program_version: u16,

    // 0x2e..0x34
    //
    // Decoding sits inside `try_map`, so a file with an impossible value fails to parse
    // rather than reaching a caller.
    #[br(try_map = |raw: [u8; 7]| CenterPanel::try_from(raw))]
    #[bw(map = |p: &CenterPanel| <[u8; 7]>::from(p))]
    pub center_panel: CenterPanel,

    // 0x35..0x3b
    pad1: [u8; (0x39 - 0x34) as usize],

    // 0x3a..0x41
    #[br(try_map = |raw: [u8; 8]| PianoPanel::try_from(raw))]
    #[bw(map = |p: &PianoPanel| <[u8; 8]>::from(p))]
    pub piano_panel: PianoPanel,

    // 0x42..0x45
    pad2: [u8; (0x45 - 0x41) as usize],

    // 0x46..0x4d
    #[br(try_map = |raw: [u8; 8]| SamplePanel::try_from(raw))]
    #[bw(map = |p: &SamplePanel| <[u8; 8]>::from(p))]
    pub sample_panel: SamplePanel,

    // 0x4e..0x92
    #[br(try_map = |raw: [u8; 69]| OrganPanel::try_from(raw))]
    #[bw(map = |p: &OrganPanel| <[u8; 69]>::from(p))]
    pub organ_panel: OrganPanel,

    // 0x93..0xa4
    #[br(try_map = |raw: [u8; 18]| EffectsPanel::try_from(raw))]
    #[bw(map = |p: &EffectsPanel| <[u8; 18]>::from(p))]
    pub effects_panel: EffectsPanel,
}

#[derive(Debug)]
pub struct Program {
    pub schema: Schema,
    location: Location,
    name: Option<String>,
}

impl Program {
    pub fn new(location: Location) -> Program {
        Program {
            location,
            name: None,
            schema: Schema {
                header: Header::new(1, FORMAT, location),
                version: 4,
                pad1: [0; (0x39 - 0x34) as usize],
                pad2: [0; (0x45 - 0x41) as usize],
                program_version: 4,
                center_panel: CenterPanel::default(),
                piano_panel: PianoPanel::default(),
                sample_panel: SamplePanel::default(),
                organ_panel: OrganPanel::default(),
                effects_panel: EffectsPanel::default(),
            },
        }
    }

    pub fn read_from(reader: &mut impl BinReaderExt) -> Result<Program, std::io::Error> {
        let schema = match Schema::read_be(reader) {
            Ok(schema) => schema,
            Err(e) => return Err(io::Error::new(io::ErrorKind::Other, e.to_string())),
        };
        if !KNOWN_VERSIONS.contains(&schema.version) {
            return Err(io::Error::other(
                crate::error::ParseError::UnsupportedVersion {
                    format: FORMAT,
                    version: schema.version,
                    supported: KNOWN_VERSIONS,
                }
                .to_string(),
            ));
        }

        Ok(Program {
            location: schema.header.location,
            name: None,
            schema,
        })
    }

    pub fn write_to(&mut self, writer: &mut impl BinWriterExt) -> Result<(), std::io::Error> {
        self.schema.header.location = self.location;

        match writer.write_be(&mut self.schema) {
            Ok(_) => Ok(()),
            Err(e) => Err(io::Error::new(io::ErrorKind::Other, e.to_string())),
        }
    }
}

impl bank::Item<Location> for Program {
    fn name(&self) -> Option<String> {
        self.name.clone()
    }

    fn set_name(&mut self, name: String) {
        self.name = Some(name);
    }

    fn location(&self) -> Location {
        self.location
    }

    fn set_location(&mut self, location: Location) {
        self.location = location;
    }
}

impl common::program::Program for Program {}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    /// An unknown schema version is refused at read, not decoded on a guess.
    ///
    /// Field offsets are only validated for the versions in the corpus. A future
    /// firmware bumping `ne5p` to 5 could move fields; decoding it with version-4
    /// offsets would yield plausible but wrong values, and writing it back would then
    /// persist them. Refusing is the only safe default.
    #[test]
    fn an_unknown_schema_version_is_refused() {
        use std::io::Cursor;

        let mut program = Program::new((0, 0).try_into().unwrap());
        let mut bytes = Vec::new();
        program.write_to(&mut Cursor::new(&mut bytes)).unwrap();
        assert_eq!(bytes.len(), FILE_LEN);

        // Sanity: as written, it reads back.
        assert!(Program::read_from(&mut Cursor::new(&mut bytes.clone())).is_ok());

        // The schema version lives at 0x14, little-endian.
        assert_eq!(u32::from_le_bytes(bytes[0x14..0x18].try_into().unwrap()), 4);
        bytes[0x14..0x18].copy_from_slice(&5u32.to_le_bytes());

        let err = Program::read_from(&mut Cursor::new(&mut bytes))
            .expect_err("version 5 must not decode");
        assert!(
            err.to_string().contains("not supported"),
            "unhelpful error: {err}",
        );
    }

    /// Every panel's encode is `From`, not `TryFrom`: no field can overrun its slot.
    ///
    /// The other half of that guarantee is not assertable from a test — giving a field a
    /// type wider than its slot is a const-eval panic out of `Field::FITS`, so retyping
    /// `PianoPanel::mono` from `bool` to `u8` fails to build rather than failing here.
    #[test]
    fn every_panels_encode_is_total() {
        fn total<P, W>(_: &P)
        where
            for<'a> W: From<&'a P>,
        {
        }

        let program = Program::new((0, 0).try_into().unwrap());
        total::<_, [u8; 7]>(&program.schema.center_panel);
        total::<_, [u8; 8]>(&program.schema.piano_panel);
        total::<_, [u8; 8]>(&program.schema.sample_panel);
        total::<_, [u8; 18]>(&program.schema.effects_panel);
    }

    /// Re-stamp the body CRC after corrupting a byte, so a decode test exercises the
    /// field check rather than the checksum.
    fn restamp_crc(bytes: &mut [u8]) {
        use crate::crc::MultipartCrc32;
        let mut crc = MultipartCrc32::new(0x2c, 0xa4 - 0x2c);
        crc.update(0, bytes);
        bytes[0x18..0x1c].copy_from_slice(&crc.checksum().to_le_bytes());
    }

    /// Validation is part of `BinRead`, not a step a caller has to remember.
    ///
    /// This shape gets it structurally: `#[br(try_map)]` runs the fallible decode inside
    /// the read, so `Schema::read_be` — public API that never touches
    /// [`Program::read_from`] — validates too, with nothing to forget. Note there is no
    /// way to build the corrupt input through the API at all: `lower_part` is an
    /// `Instrument`, so a panel in memory *cannot* hold the invalid value. It has to be
    /// forged in the bytes.
    #[test]
    fn no_decode_path_can_skip_validation() {
        use binrw::BinRead;

        let mut program = Program::new((0, 0).try_into().unwrap());
        let mut bytes = Vec::new();
        program.write_to(&mut Cursor::new(&mut bytes)).unwrap();

        // Self-check: re-stamping an untouched file must be a no-op.
        let pristine = bytes.clone();
        restamp_crc(&mut bytes);
        assert_eq!(bytes, pristine, "the CRC helper does not match the writer");

        // 0b111 is not an `Instrument`.
        bytes[0x2e] |= 0b1110_0000;
        restamp_crc(&mut bytes);

        let front = Program::read_from(&mut Cursor::new(&mut bytes))
            .expect_err("the front door accepted an undecodable panel");
        assert!(
            front.to_string().contains("exceeds bound"),
            "refused for the wrong reason: {front}",
        );
        assert!(
            Schema::read_be(&mut Cursor::new(&bytes)).is_err(),
            "`Schema::read_be` accepted an undecodable panel",
        );
        assert!(
            CenterPanel::try_from(<[u8; 7]>::try_from(&bytes[0x2e..0x35]).unwrap()).is_err(),
            "the conversion itself accepted an undecodable panel",
        );
    }

    /// Decode and encode are inverses on any bytes the decoder accepts.
    #[test]
    fn decode_and_encode_are_inverse() {
        for pattern in [0u64, u64::MAX, 0xa5a5_a5a5_a5a5_a5a5, 0x5a5a_5a5a_5a5a_5a5a] {
            let raw = pattern.to_be_bytes();
            let panel = PianoPanel::try_from(raw).unwrap();
            assert_eq!(<[u8; 8]>::from(&panel), raw);

            let panel = SamplePanel::try_from(raw).unwrap();
            assert_eq!(<[u8; 8]>::from(&panel), raw);

            let raw: [u8; 7] = raw[..7].try_into().unwrap();
            if let Ok(panel) = CenterPanel::try_from(raw) {
                assert_eq!(<[u8; 7]>::from(&panel), raw);
            }
        }
    }
}
