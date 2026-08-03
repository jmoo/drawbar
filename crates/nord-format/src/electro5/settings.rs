//! The Electro 5 global settings format (`.ne5s`).
//!
//! Reads top-down: the format's constants, then [`Schema`] — the file as `binrw` sees
//! it — then [`Settings`]. The body is one `#[bitpanel]`, in [`panel`].

pub mod panel;

pub use panel::{
    B3TrigMode, CtrlPedalGain, CtrlPedalType, FineTune, GlobalTranspose, KeyClickLevel, Menu,
    MidiChannel, MidiMessageMode, OutputRouting, PercDecay, PercVolume, ResonanceLevel,
    RotaryBalance, RotaryCtrlType, RotaryPedalMode, RotaryRate, RotarySpeakerType, Setting,
    SettingsPanel, SustainPedalMode, SustainPedalType, TonewheelMode, TransposeAt,
};

use crate::common;
use crate::crc::{CrcReader, CrcWriter};
use crate::error::{Error, ParseError};
use crate::types::RangedU16Pair;
use binrw::{binrw, BinRead, BinWriterExt};
use panel::BODY_LEN;
use std::fmt::Debug;
use std::io::{Read, Seek, Write};

pub const FORMAT: &str = "ne5s";
/// Schema versions validated against the corpus. Every corpus settings file reports 0.
pub const KNOWN_VERSIONS: &[u32] = &[0];
/// Total file length: 44-byte CBIN header + 34-byte body.
pub const FILE_LEN: usize = 78;

pub type Location = RangedU16Pair<0, 0>;
pub type Header = common::Header<Location>;

#[binrw]
#[derive(Debug)]
#[brw(assert(header.preamble.format == FORMAT))]
#[br(little, stream = r, map_stream = CrcReader::new(0x2c, 0x4d - 0x2c), assert(r.checksum() == crc32, "bad checksum: {:#x?} != {:#x?}", r.checksum(), crc32))]
#[bw(little, stream = w, map_stream = CrcWriter::new(0x2c, 0x4d - 0x2c))]
pub struct Schema {
    header: Header,

    pub version: u32,

    #[bw(try_calc = w.checksum())]
    crc32: u32,

    // 0x2c..0x4d
    #[brw(big, pad_before = 16)]
    #[br(try_map = |raw: [u8; BODY_LEN]| SettingsPanel::try_from(raw))]
    #[bw(map = |p: &SettingsPanel| <[u8; BODY_LEN]>::from(p))]
    pub panel: SettingsPanel,
}

/// Electro 5 global settings (`ne5s`): the System, MIDI and Sound menus.
///
/// The whole file is one panel plus its CBIN header; there is no slot to speak of, since
/// the instrument holds exactly one of these.
#[derive(Debug)]
pub struct Settings {
    pub schema: Schema,
}

impl Settings {
    pub fn new() -> Settings {
        Settings {
            schema: Schema {
                header: Header::new(1, FORMAT, (0, 0).try_into().unwrap()),
                panel: SettingsPanel::default(),
                version: 0,
            },
        }
    }

    pub fn read_from(reader: &mut (impl Read + Seek)) -> Result<Settings, Error> {
        let schema = Schema::read_be(reader)?;

        if !KNOWN_VERSIONS.contains(&schema.version) {
            return Err(ParseError::UnsupportedVersion {
                format: FORMAT,
                version: schema.version,
                supported: KNOWN_VERSIONS,
            }
            .into());
        }

        Ok(Settings { schema })
    }

    pub fn write_to(&self, writer: &mut (impl Write + Seek)) -> Result<(), Error> {
        writer.write_be(&self.schema)?;
        Ok(())
    }

    /// The settings body as stored, `0x2c..=0x4d`. Includes the bits no field claims.
    pub fn body(&self) -> [u8; BODY_LEN] {
        <[u8; BODY_LEN]>::from(&self.schema.panel)
    }
}

impl Default for Settings {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    /// A settings file writes out at its declared length and reads back.
    ///
    /// The CRC region ends at the last byte of the file, and `CrcWriter` only flushes
    /// its buffered body after the write position *passes* the region — so an
    /// off-by-one in the region bound silently truncates the output to the bytes ahead
    /// of the checksum. This is the only ne5 format whose body runs to EOF, which is
    /// why it needs its own writer test.
    #[test]
    fn a_settings_file_round_trips_at_full_length() {
        let settings = Settings::new();
        let mut bytes = Vec::new();
        settings.write_to(&mut Cursor::new(&mut bytes)).unwrap();
        assert_eq!(bytes.len(), FILE_LEN);

        let back = Settings::read_from(&mut Cursor::new(&mut bytes)).unwrap();
        assert_eq!(back.body(), settings.body());
    }

    /// Bits no field claims are kept, so a body the decoder accepts survives verbatim.
    #[test]
    fn the_unclaimed_bits_survive_a_round_trip() {
        let mut settings = Settings::new();
        // Bits 0..=37 are unexplained and belong to no field; 0x2e..0x30 is the middle of
        // that run. A pattern there has to come back untouched.
        let mut body = settings.body();
        for (i, at) in [0x2e, 0x2f, 0x30].into_iter().enumerate() {
            body[at - 0x2c] = [0x83, 0x74, 0x07][i];
        }
        settings.schema.panel = SettingsPanel::try_from(body).unwrap();

        let mut bytes = Vec::new();
        settings.write_to(&mut Cursor::new(&mut bytes)).unwrap();
        assert_eq!(bytes.len(), FILE_LEN);

        let back = Settings::read_from(&mut Cursor::new(&mut bytes)).unwrap();
        assert_eq!(back.body(), body);
    }

    /// An unknown schema version is refused at read rather than decoded on a guess: a
    /// firmware that bumps it could move every field below.
    #[test]
    fn an_unknown_schema_version_is_refused() {
        let settings = Settings::new();
        let mut bytes = Vec::new();
        settings.write_to(&mut Cursor::new(&mut bytes)).unwrap();
        assert!(Settings::read_from(&mut Cursor::new(&mut bytes.clone())).is_ok());

        // The schema version lives at 0x14, little-endian.
        bytes[0x14..0x18].copy_from_slice(&1u32.to_le_bytes());
        let err = Settings::read_from(&mut Cursor::new(&mut bytes))
            .expect_err("version 1 must not decode");
        assert!(
            matches!(
                err,
                Error::Parse(ParseError::UnsupportedVersion {
                    format: FORMAT,
                    version: 1,
                    ..
                })
            ),
            "unhelpful error: {err}",
        );
    }
}
