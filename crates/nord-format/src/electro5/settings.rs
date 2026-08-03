//! The Electro 5 global settings format (`.ne5s`).
//!
//! Reads top-down: the format's constants, then [`Schema`] — the file as `binrw` sees
//! it — then [`Settings`]. The body is one `#[bitpanel]`, in [`panel`].

pub mod panel;

pub use panel::{
    B3TrigMode, CtrlPedalGain, CtrlPedalType, FineTune, GlobalTranspose, KeyClickLevel, LiveSlot,
    Menu, MidiChannel, MidiMessageMode, OutputRouting, PercDecay, PercVolume, ResonanceLevel,
    RotaryBalance, RotaryCtrlType, RotaryPedalMode, RotaryRate, RotarySpeakerType, Selection,
    Setting, SettingsPanel, SustainPedalMode, SustainPedalType, TonewheelMode, TransposeAt,
};

use crate::common;
use crate::crc::{CrcReader, CrcWriter};
use crate::electro5::program;
use crate::error::{Error, ParseError};
use crate::panel::{FieldError, Panel};
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

    // 0x2c..0x4d. Two panels share these bytes: the menu settings and the instrument's
    // selection state, interleaved rather than split (the set list song runs 30..=37 and
    // the first setting starts at 38), so the body is read once and decoded twice.
    #[brw(big, pad_before = 16)]
    #[br(temp)]
    #[bw(calc = panel::encode(panel, selection))]
    body: [u8; BODY_LEN],

    #[br(try_calc = SettingsPanel::try_from(body))]
    #[bw(ignore)]
    pub panel: SettingsPanel,

    #[br(try_calc = Selection::try_from(body))]
    #[bw(ignore)]
    pub selection: Selection,
}

impl Schema {
    /// Every settable field, menu panel then selection, in declaration order.
    pub fn fields(&self) -> Vec<program::Field> {
        let mut out = program::describe("panel", &self.panel);
        out.extend(program::describe("selection", &self.selection));
        out
    }

    /// Set one field, addressed as `panel.field` or `selection.field`.
    pub fn set_field(&mut self, path: &str, value: &str) -> Result<(), FieldError> {
        let (member, field) = path
            .split_once('.')
            .ok_or_else(|| FieldError::UnknownField {
                panel: "settings",
                name: path.to_string(),
            })?;
        match member {
            "panel" => self.panel.set_field(field, value),
            "selection" => self.selection.set_field(field, value),
            other => Err(FieldError::UnknownField {
                panel: "settings",
                name: other.to_string(),
            }),
        }
    }
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
                selection: Selection::default(),
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
        panel::encode(&self.schema.panel, &self.schema.selection)
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
        // Bit 18 is the only bit inside the decoded run that belongs to no field — the
        // third bit of `0x2e` — and `0x3e` onwards is past the last field, which ends at
        // bit 141. Both have to come back untouched.
        let mut body = settings.body();
        body[0x2e - 0x2c] = 0x20;
        body[0x3e - 0x2c] = 0x5a;
        settings.schema.panel = SettingsPanel::try_from(body).unwrap();
        settings.schema.selection = Selection::try_from(body).unwrap();

        let mut bytes = Vec::new();
        settings.write_to(&mut Cursor::new(&mut bytes)).unwrap();
        assert_eq!(bytes.len(), FILE_LEN);

        let back = Settings::read_from(&mut Cursor::new(&mut bytes)).unwrap();
        assert_eq!(back.body(), body);
    }

    /// Both panels share the body, so editing one must not revert the other.
    ///
    /// Each carries a stale copy of the other's fields in the bytes it was decoded from;
    /// this is what catches an encode that keeps only one panel's output instead of
    /// threading it through the other's.
    #[test]
    fn the_two_panels_do_not_overwrite_each_other() {
        let mut settings = Settings::new();
        settings.schema.panel.b3_tonewheel_mode = TonewheelMode::Vintage3;
        settings.schema.selection.live_mode = true;
        settings.schema.selection.program = (4, 2).try_into().unwrap();

        let mut bytes = Vec::new();
        settings.write_to(&mut Cursor::new(&mut bytes)).unwrap();
        let back = Settings::read_from(&mut Cursor::new(&mut bytes)).unwrap();

        assert_eq!(back.schema.panel.b3_tonewheel_mode, TonewheelMode::Vintage3);
        assert!(back.schema.selection.live_mode);
        assert_eq!(back.schema.selection.program.inner(), (4, 2));
    }

    /// Fields are addressed the way `--set` spells them, and both members answer.
    #[test]
    fn fields_are_set_through_schema_paths() {
        let mut settings = Settings::new();
        settings
            .schema
            .set_field("panel.global_transpose", "+3")
            .unwrap();
        settings
            .schema
            .set_field("selection.live_mode", "on")
            .unwrap();

        let mut bytes = Vec::new();
        settings.write_to(&mut Cursor::new(&mut bytes)).unwrap();
        let back = Settings::read_from(&mut Cursor::new(&mut bytes)).unwrap();
        assert_eq!(back.schema.panel.global_transpose.inner(), 3);
        assert!(back.schema.selection.live_mode);

        let paths: Vec<String> = settings
            .schema
            .fields()
            .into_iter()
            .map(|f| f.path)
            .collect();
        assert!(paths.contains(&"panel.global_transpose".to_string()));
        assert!(paths.contains(&"selection.program".to_string()));

        assert!(settings.schema.set_field("panel.no_such", "1").is_err());
        assert!(settings.schema.set_field("global_transpose", "1").is_err());
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
