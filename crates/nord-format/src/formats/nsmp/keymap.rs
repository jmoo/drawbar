//! The keyboard map ahead of the zone table in a v2 `map` section: the
//! instrument's own gain and detune, then one gain-and-detune record per MIDI
//! note.
//!
//! Nothing here touches the audio — a record is a playback-side level and
//! pitch offset for one key, and the instrument's record scales the whole map.
//! The sample editor writes the per-key records from its note list; its macro
//! spin controls have no storage of their own and write through to the same
//! table.
//!
//! Inferred from specimens; not confirmed on hardware.

use super::zone;
use crate::error::ParseError;

/// Schema version of a v2 `map` section carrying this table.
pub(super) const VERSION: u8 = 10;

/// `1.0` in every gain field of the map, a u24 linear ratio.
pub const GAIN_UNITY: u32 = 0x10_0000;

/// Largest value a gain field holds.
pub const GAIN_MAX: u32 = 0xFF_FFFF;

/// One semitone in every detune field of the map, an s24 count of 1/256
/// semitone. The same unit serves the zone record's own detune.
pub const DETUNE_PER_SEMITONE: i32 = 256;

const DETUNE_MIN: i32 = -(1 << 23);
const DETUNE_MAX: i32 = (1 << 23) - 1;

/// One record per MIDI note.
pub const KEYS: usize = 128;

/// Bytes per record: a u24 gain then an s24 detune, both big-endian.
pub const RECORD_LEN: usize = 6;

/// The instrument's own record, at the head of the payload.
const LEVEL_AT: usize = 0;

/// Where note 0's record sits; note `n` is `KEY_TABLE_AT + RECORD_LEN * n`.
pub const KEY_TABLE_AT: usize = 15;

const _: () = assert!(KEY_TABLE_AT + KEYS * RECORD_LEN + 2 == zone::COUNT_AT);

/// A gain and a pitch offset — the six-byte record the map is built from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Level {
    /// Linear gain, [`GAIN_UNITY`] for 1.0; never more than [`GAIN_MAX`].
    ///
    /// The editor keeps a key's gain within ±9 dB and the instrument's below
    /// +9 dB, clamping as it encodes without repairing its project. What the
    /// instrument does with a value outside that band is unmeasured.
    gain: u32,
    /// Pitch offset in 1/256 semitone, positive upward, within an s24.
    ///
    /// The editor truncates its cents toward zero: 1 cent stores 2, 8 cents
    /// store 20, and a whole octave stores 3072.
    detune: i32,
}

impl Level {
    /// Unity gain, no detune — what every record holds until it is set.
    pub const NEUTRAL: Level = Level {
        gain: GAIN_UNITY,
        detune: 0,
    };

    /// Checks both fields fit their 24 bits.
    pub fn new(gain: u32, detune: i32) -> Result<Level, ParseError> {
        if gain > GAIN_MAX {
            return Err(ParseError::AssertFail(format!(
                "gain {gain:#x} does not fit the 24-bit field (max {GAIN_MAX:#x})"
            )));
        }
        if !(DETUNE_MIN..=DETUNE_MAX).contains(&detune) {
            return Err(ParseError::AssertFail(format!(
                "detune {detune} does not fit the 24-bit field ({DETUNE_MIN}..={DETUNE_MAX})"
            )));
        }
        Ok(Level { gain, detune })
    }

    /// From a linear gain ratio and a detune in semitones. Gain is rounded to the
    /// nearest field unit; detune is truncated toward zero as the editor writes it.
    pub fn from_ratio(gain: f64, semitones: f64) -> Result<Level, ParseError> {
        let units = gain * f64::from(GAIN_UNITY);
        if !units.is_finite() || units < 0.0 || units > f64::from(GAIN_MAX) {
            return Err(ParseError::AssertFail(format!(
                "gain ratio {gain} is outside what the 24-bit field holds"
            )));
        }
        let detune = semitones * f64::from(DETUNE_PER_SEMITONE);
        if !detune.is_finite() || detune < f64::from(DETUNE_MIN) || detune > f64::from(DETUNE_MAX) {
            return Err(ParseError::AssertFail(format!(
                "detune of {semitones} semitones is outside what the 24-bit field holds"
            )));
        }
        Level::new(units.round() as u32, detune.trunc() as i32)
    }

    /// The gain field's fixed-point value.
    pub const fn gain(self) -> u32 {
        self.gain
    }

    /// The detune field's signed 1/256-semitone value.
    pub const fn detune(self) -> i32 {
        self.detune
    }

    /// The gain as a linear ratio, 1.0 for unity.
    pub fn ratio(&self) -> f64 {
        f64::from(self.gain) / f64::from(GAIN_UNITY)
    }

    /// The detune in semitones.
    pub fn semitones(&self) -> f64 {
        f64::from(self.detune) / f64::from(DETUNE_PER_SEMITONE)
    }

    fn read(record: &[u8]) -> Level {
        let gain = u32::from_be_bytes([0, record[0], record[1], record[2]]);
        let raw = u32::from_be_bytes([0, record[3], record[4], record[5]]);
        // Sign-extend the s24.
        let detune = ((raw << 8) as i32) >> 8;
        Level { gain, detune }
    }

    fn write(&self, record: &mut [u8]) {
        record[..3].copy_from_slice(&self.gain.to_be_bytes()[1..]);
        record[3..RECORD_LEN].copy_from_slice(&(self.detune as u32).to_be_bytes()[1..]);
    }
}

/// The whole keyboard map: the instrument's record and one per MIDI note.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KeyTable {
    /// Scales and detunes the whole map.
    pub instrument: Level,
    /// Indexed by MIDI note.
    keys: [Level; KEYS],
}

impl KeyTable {
    /// Every record neutral — what the editor writes for an untouched map.
    pub const NEUTRAL: KeyTable = KeyTable {
        instrument: Level::NEUTRAL,
        keys: [Level::NEUTRAL; KEYS],
    };

    /// Reads the map ahead of the zone count.
    pub fn read(map: &[u8]) -> Result<KeyTable, ParseError> {
        fits(map)?;
        let mut keys = [Level::NEUTRAL; KEYS];
        for (note, key) in keys.iter_mut().enumerate() {
            *key = Level::read(&map[record_at(note)..][..RECORD_LEN]);
        }
        Ok(KeyTable {
            instrument: Level::read(&map[LEVEL_AT..][..RECORD_LEN]),
            keys,
        })
    }

    /// Writes the map ahead of the zone count, leaving the zone table alone.
    pub fn write(&self, map: &mut [u8]) -> Result<(), ParseError> {
        fits(map)?;
        self.instrument.write(&mut map[LEVEL_AT..][..RECORD_LEN]);
        for (note, key) in self.keys.iter().enumerate() {
            key.write(&mut map[record_at(note)..][..RECORD_LEN]);
        }
        Ok(())
    }

    /// The bytes ahead of the zone count, ready to head a new `map` payload.
    pub fn prefix(&self) -> [u8; zone::COUNT_AT] {
        let mut out = [0u8; zone::COUNT_AT];
        self.write(&mut out)
            .expect("a buffer sized to the zone count holds the whole keyboard map");
        out
    }

    /// The record for one MIDI note.
    pub fn key(&self, note: u8) -> Result<Level, ParseError> {
        self.keys
            .get(usize::from(note))
            .copied()
            .ok_or_else(|| ParseError::OutOfBounds {
                value: format!("MIDI note {note}"),
                bound: "a MIDI note from 0 through 127".into(),
            })
    }

    /// Sets the record for one MIDI note.
    pub fn set_key(&mut self, note: u8, level: Level) -> Result<(), ParseError> {
        let key = self
            .keys
            .get_mut(usize::from(note))
            .ok_or_else(|| ParseError::OutOfBounds {
                value: format!("MIDI note {note}"),
                bound: "a MIDI note from 0 through 127".into(),
            })?;
        *key = level;
        Ok(())
    }

    /// Notes whose record is not neutral.
    pub fn adjusted(&self) -> impl Iterator<Item = u8> + '_ {
        self.keys
            .iter()
            .enumerate()
            .filter(|(_, level)| **level != Level::NEUTRAL)
            .map(|(note, _)| note as u8)
    }
}

const fn record_at(note: usize) -> usize {
    KEY_TABLE_AT + RECORD_LEN * note
}

fn fits(map: &[u8]) -> Result<(), ParseError> {
    if map.len() < zone::COUNT_AT {
        return Err(ParseError::AssertFail(format!(
            "map section is {} bytes, too short for a keyboard map",
            map.len()
        )));
    }
    Ok(())
}

pub(super) fn require_version(version: u8) -> Result<(), ParseError> {
    if version == VERSION {
        return Ok(());
    }
    Err(ParseError::AssertFail(format!(
        "map section version {version} has no keyboard table layout derived from specimens"
    )))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn neutral_prefix_is_unity_at_every_record_and_zero_between() {
        let prefix = KeyTable::NEUTRAL.prefix();
        assert_eq!(prefix.len(), zone::COUNT_AT);
        for (i, b) in prefix.iter().enumerate() {
            let expected = if i == LEVEL_AT
                || (i >= KEY_TABLE_AT
                    && (i - KEY_TABLE_AT).is_multiple_of(RECORD_LEN)
                    && i < KEY_TABLE_AT + KEYS * RECORD_LEN)
            {
                0x10
            } else {
                0
            };
            assert_eq!(*b, expected, "byte {i}");
        }
    }

    #[test]
    fn records_round_trip_through_their_bytes() {
        let mut table = KeyTable::NEUTRAL;
        table.instrument = Level::new(0x2d_1819, -256).unwrap();
        table
            .set_key(60, Level::new(0x08_0000, 20).unwrap())
            .unwrap();
        table
            .set_key(17, Level::new(0, -(1 << 23)).unwrap())
            .unwrap();
        table
            .set_key(127, Level::new(GAIN_MAX, (1 << 23) - 1).unwrap())
            .unwrap();
        let mut map = vec![0xAA; zone::COUNT_AT + 1 + zone::RECORD_LEN];
        table.write(&mut map).unwrap();
        assert_eq!(KeyTable::read(&map).unwrap(), table);
        assert!(map[LEVEL_AT + RECORD_LEN..KEY_TABLE_AT]
            .iter()
            .all(|&b| b == 0xAA));
        assert!(map[KEY_TABLE_AT + KEYS * RECORD_LEN..zone::COUNT_AT]
            .iter()
            .all(|&b| b == 0xAA));
        assert!(map[zone::COUNT_AT..].iter().all(|b| *b == 0xAA));
        assert_eq!(table.adjusted().collect::<Vec<_>>(), [17, 60, 127]);
    }

    #[test]
    fn detune_reads_signed() {
        let mut map = vec![0u8; zone::COUNT_AT];
        map[record_at(60) + 3..record_at(60) + 6].copy_from_slice(&[0xff, 0xff, 0xec]);
        assert_eq!(KeyTable::read(&map).unwrap().key(60).unwrap().detune(), -20);
    }

    #[test]
    fn fields_are_checked_against_their_width() {
        assert!(Level::new(GAIN_MAX + 1, 0).is_err());
        assert!(Level::new(0, 1 << 23).is_err());
        assert!(Level::new(0, -(1 << 23) - 1).is_err());
        assert_eq!(
            Level::from_ratio(0.5, -1.0).unwrap(),
            Level::new(0x08_0000, -256).unwrap()
        );
        assert_eq!(Level::from_ratio(1.0, 0.01).unwrap().detune(), 2);
        assert_eq!(Level::from_ratio(1.0, -0.01).unwrap().detune(), -2);
        assert!(Level::from_ratio(16.0, 0.0).is_err());
        assert!(Level::from_ratio(-0.5, 0.0).is_err());
        assert!(Level::from_ratio(1.0, f64::NAN).is_err());
    }

    #[test]
    fn short_maps_and_notes_outside_midi_are_refused() {
        let map = KeyTable::NEUTRAL.prefix().to_vec();
        assert!(KeyTable::read(&map[..700]).is_err());
        assert!(KeyTable::NEUTRAL.key(128).is_err());
        let mut table = KeyTable::NEUTRAL;
        assert!(table.set_key(128, Level::NEUTRAL).is_err());
    }
}
