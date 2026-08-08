//! The Electro 5 global settings format (`.ne5s`).
//!
//! Reads top-down: the format's constants, then the read that pairs a header with
//! [`Settings`] — the 34 bytes after it, one flat `#[bitbody]` in [`panel`]. A file
//! is a `Cbin<Settings>`, which derefs to the body.

pub mod panel;

pub use panel::{
    B3TrigMode, CtrlPedalGain, CtrlPedalType, FineTune, GlobalTranspose, KeyClickLevel, LiveSlot,
    Menu, MidiChannel, MidiMessageMode, OutputRouting, PercDecay, PercVolume, ResonanceLevel,
    RotaryBalance, RotaryCtrlType, RotaryPedalMode, RotaryRate, RotarySpeakerType, Setting,
    Settings, SustainPedalMode, SustainPedalType, TonewheelMode, TransposeAt,
};

use crate::cbin::{self, Cbin, Header};
use crate::electro5::program;
use crate::error::{Error, ParseError};
use panel::BODY_LEN;
use std::io::{Read, Seek};

pub const FORMAT: &str = "ne5s";
/// Schema versions validated against the corpus. Every corpus settings file reports 0.
pub const KNOWN_VERSIONS: &[u32] = &[0];
/// Type-1 file length: 44-byte CBIN header + 34-byte body.
pub const FILE_LEN: usize = 0x2c + BODY_LEN;

/// A default settings file.
///
/// There is no slot to speak of: the instrument holds exactly one of these, and every
/// specimen addresses it to bank 0 slot 0.
pub fn new() -> Cbin<Settings> {
    Cbin {
        header: Header::new(FORMAT, (0, 0), 0),
        body: Settings::default(),
    }
}

pub fn read_from(reader: &mut (impl Read + Seek)) -> Result<Cbin<Settings>, Error> {
    let file: Cbin<Settings> = cbin::read(reader, FORMAT)?;
    program::known_version(FORMAT, file.header.version, KNOWN_VERSIONS)?;
    program::unset_aux(FORMAT, &file.header)?;
    // The instrument holds exactly one settings file, so the location field has
    // nothing to address; every specimen holds bank 0 slot 0.
    let (bank, slot) = file.header.slot();
    if (bank, slot) != (0, 0) {
        return Err(ParseError::AssertFail(format!(
            "{FORMAT}: location is {bank} {slot}, and settings live at 0 0"
        ))
        .into());
    }
    Ok(file)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    #[test]
    fn settings_round_trip_at_the_declared_length() {
        let settings = new();
        let mut bytes = Vec::new();
        settings.write_to(&mut Cursor::new(&mut bytes)).unwrap();
        assert_eq!(bytes.len(), FILE_LEN);
        assert_eq!(&bytes[0x08..0x0c], FORMAT.as_bytes());

        let back = read_from(&mut Cursor::new(&mut bytes)).unwrap();
        let mut again = Vec::new();
        back.write_to(&mut Cursor::new(&mut again)).unwrap();
        assert_eq!(bytes, again);
    }

    /// There is one settings file per instrument, so a file claiming a slot is not
    /// one of them.
    #[test]
    fn a_settings_file_addressed_to_a_slot_is_refused() {
        let mut settings = new();
        settings.header.set_slot((1, 0));
        let mut bytes = Vec::new();
        settings.write_to(&mut Cursor::new(&mut bytes)).unwrap();

        let err = read_from(&mut Cursor::new(&bytes))
            .expect_err("a located settings file must not decode");
        assert!(
            matches!(err, Error::Parse(ParseError::AssertFail(_))),
            "refused for the wrong reason: {err}",
        );
    }

    #[test]
    fn a_settings_field_set_by_name_survives_a_round_trip() {
        let mut settings = new();
        settings.set_field("global_transpose", "-3").unwrap();

        let mut bytes = Vec::new();
        settings.write_to(&mut Cursor::new(&mut bytes)).unwrap();
        let back = read_from(&mut Cursor::new(&mut bytes)).unwrap();
        let listed = back
            .fields()
            .into_iter()
            .find(|f| f.path == "global_transpose")
            .expect("declared");
        assert_eq!(listed.display, "-3");
    }
}
