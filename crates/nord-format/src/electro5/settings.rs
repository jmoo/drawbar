use crate::common;
use crate::crc::{CrcReader, CrcWriter};
use crate::types::RangedU16Pair;
use binrw::{binrw, BinRead, BinReaderExt, BinWriterExt};
use std::fmt::Debug;
use std::io;

pub const FORMAT: &str = "ne5s";
/// Schema versions validated against the corpus. Both specimens report 0.
pub const KNOWN_VERSIONS: &[u32] = &[0];
/// Total file length: 44-byte CBIN header + 34-byte body.
pub const FILE_LEN: usize = 78;

/// Length of the settings body block (`0x2c..=0x4d`), the region covered by the
/// CRC. Currently held verbatim; see [`Settings`] for the decode target.
const BODY_LEN: usize = 0x4e - 0x2c;

pub type Location = RangedU16Pair<0, 0>;
pub type Header = common::Header<Location>;

#[binrw]
#[derive(Debug)]
#[brw(assert(header.preamble.format == FORMAT))]
#[br(little, stream = r, map_stream = CrcReader::new(0x2c, 0x4d - 0x2c), assert(r.checksum() == crc32, "bad checksum: {:#x?} != {:#x?}", r.checksum(), crc32))]
#[bw(little, stream = w, map_stream = CrcWriter::new(0x2c, 0x4d - 0x2c))]
struct Schema {
    header: Header,

    pub version: u32,

    #[bw(try_calc = w.checksum())]
    crc32: u32,

    /// The raw settings body (`0x2c..=0x4d`), stored verbatim so the file
    /// round-trips byte-exact. Field decode is pending a specimen corpus — see
    /// the catalog on [`Settings`].
    #[brw(big, pad_before = 16)]
    raw: [u8; BODY_LEN],
}

/// Electro 5 global settings (`ne5s`): system, MIDI, and sound preferences.
///
/// **System:** memory protection; rotary ctrl type (closed/open/half-moon);
/// rotary pedal mode (hold/toggle); sustain pedal mode; B3 trig mode
/// (normal/fast); output routing (stereo / L+U split); sustain pedal type;
/// ctrl pedal type (ev7/fc7/exp2/xvp10/fv500l/fatar-SL); global transpose
/// (-6..6 semitones); fine tune (-50..50 cent); ctrl pedal gain (1..10).
///
/// **MIDI:** local control; global channel (off/1..16); lower/upper receive
/// channels; upper split channel; control-change mode; program-change mode;
/// transpose-at (in/out).
///
/// **Sound:** piano string-resonance level (-6..6 dB); B3 tonewheel mode
/// (clean/vintage1-3); B3 keyclick level; B3 keybounce; B3 perc DB9 mute; B3
/// perc decay fast/slow; B3 perc volume normal/soft; rotary speaker type;
/// rotary bass/horn balance; rotary horn speed/acc; rotary rotor speed/acc.
#[derive(Debug)]
pub struct Settings {
    schema: Schema,
}

impl Settings {
    pub fn new() -> Settings {
        Settings {
            schema: Schema {
                header: Header::new(1, FORMAT, (0, 0).try_into().unwrap()),
                raw: [0; BODY_LEN],
                version: 0,
            },
        }
    }

    pub fn read_from(reader: &mut impl BinReaderExt) -> Result<Settings, std::io::Error> {
        let schema = match Schema::read_be(reader) {
            Ok(schema) => schema,
            Err(e) => return Err(io::Error::other(e.to_string())),
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

        Ok(Settings { schema })
    }

    pub fn write_to(&mut self, writer: &mut impl BinWriterExt) -> Result<(), std::io::Error> {
        match writer.write_be(&self.schema) {
            Ok(_) => Ok(()),
            Err(e) => Err(io::Error::other(e.to_string())),
        }
    }

    /// The raw 34-byte settings body (`0x2c..=0x4d`). Field decode is not yet
    /// implemented (see the [`Settings`] catalog); this exposes the bytes for
    /// inspection and future reverse-engineering.
    pub fn raw(&self) -> &[u8] {
        &self.schema.raw
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
        let mut settings = Settings::new();
        let mut bytes = Vec::new();
        settings.write_to(&mut Cursor::new(&mut bytes)).unwrap();
        assert_eq!(bytes.len(), FILE_LEN);

        let back = Settings::read_from(&mut Cursor::new(&mut bytes)).unwrap();
        assert_eq!(back.raw(), settings.raw());
    }

    /// The body is held verbatim, so any byte pattern in it survives a round trip.
    #[test]
    fn the_raw_body_survives_verbatim() {
        let mut settings = Settings::new();
        for (i, b) in settings.schema.raw.iter_mut().enumerate() {
            *b = (i as u8) ^ 0xa5;
        }
        let mut bytes = Vec::new();
        settings.write_to(&mut Cursor::new(&mut bytes)).unwrap();
        assert_eq!(bytes.len(), FILE_LEN);

        let back = Settings::read_from(&mut Cursor::new(&mut bytes)).unwrap();
        assert_eq!(back.raw(), settings.raw());
    }
}
