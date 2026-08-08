//! The zone table at the tail of the `map` section.

use crate::error::ParseError;

/// Offset of the zone count within the `map` payload. Everything before it is identical
/// across every corpus specimen, whatever the zone layout.
pub const COUNT_AT: usize = 785;

/// First zone record.
pub const RECORDS_AT: usize = COUNT_AT + 1;

/// Bytes per zone record.
pub const RECORD_LEN: usize = 15;

/// Within a record: a 1-based index that counts *down* — the first record in a
/// three-zone instrument holds 3, the last holds 1.
const REVERSE_INDEX: usize = 2;

/// Within a record: the highest MIDI note this zone answers to.
const TOP_NOTE: usize = 9;

/// A keyboard zone.
///
/// Zones are stored **high to low**: the first record covers the top of the keyboard.
/// Only the top note is stored — a zone's bottom note is one above the next record's top
/// note, and the last record reaches down to the bottom of the keyboard.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Zone {
    /// Highest MIDI note this zone answers to.
    pub top_note: u8,
    /// The stored countdown, `zone_count - index`. Kept so a rewritten table can be
    /// compared byte-for-byte against the editor's own output.
    pub reverse_index: u8,
}

pub fn count(map: &[u8]) -> Result<usize, ParseError> {
    map.get(COUNT_AT).map(|n| *n as usize).ok_or_else(|| {
        ParseError::AssertFail(format!(
            "map section is {} bytes, too short for a zone table",
            map.len()
        ))
    })
}

pub fn read(map: &[u8]) -> Result<Vec<Zone>, ParseError> {
    let n = count(map)?;
    let need = RECORDS_AT + n * RECORD_LEN;
    if map.len() < need {
        return Err(ParseError::AssertFail(format!(
            "map declares {n} zones, needing {need} bytes, but the section is {}",
            map.len()
        )));
    }
    Ok((0..n)
        .map(|i| {
            let r = &map[RECORDS_AT + i * RECORD_LEN..][..RECORD_LEN];
            Zone {
                top_note: r[TOP_NOTE],
                reverse_index: r[REVERSE_INDEX],
            }
        })
        .collect())
}

/// Sets one zone's top note in place.
///
/// Nothing else moves: the top note is an isolated byte, and the encoded audio does not
/// depend on the key range a zone covers, so remapping never needs a re-encode.
pub fn set_top_note(map: &mut [u8], index: usize, note: u8) -> Result<(), ParseError> {
    let n = count(map)?;
    if index >= n {
        return Err(ParseError::AssertFail(format!(
            "zone {index} out of range, the instrument has {n}"
        )));
    }
    map[RECORDS_AT + index * RECORD_LEN + TOP_NOTE] = note;
    Ok(())
}

/// The key ranges the editor lays out for a set of root keys, high to low.
///
/// The top zone reaches `root + 24`; every zone below stops one short of the midpoint
/// between its root and the root above.
///
/// ⚠️ **A default, not a rule.** The top note is genuinely stored and can be moved off
/// this layout — the editor exposes it as the zone's upper key. Use this to fill in a
/// table when building an instrument, never to recompute one while reading.
pub fn derive_top_notes(roots_high_to_low: &[u8]) -> Vec<u8> {
    roots_high_to_low
        .iter()
        .enumerate()
        .map(|(i, &root)| {
            if i == 0 {
                root.saturating_add(24)
            } else {
                let above = roots_high_to_low[i - 1];
                (u16::from(root) + u16::from(above)).div_ceil(2) as u8 - 1
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn table(tops: &[u8]) -> Vec<u8> {
        let mut m = vec![0u8; RECORDS_AT + tops.len() * RECORD_LEN];
        m[COUNT_AT] = tops.len() as u8;
        for (i, &t) in tops.iter().enumerate() {
            let r = RECORDS_AT + i * RECORD_LEN;
            m[r + REVERSE_INDEX] = (tops.len() - i) as u8;
            m[r + TOP_NOTE] = t;
        }
        m
    }

    #[test]
    fn reads_the_table() {
        let zones = read(&table(&[96, 65, 53])).unwrap();
        assert_eq!(zones.len(), 3);
        assert_eq!(zones[0].top_note, 96);
        assert_eq!(zones[2].top_note, 53);
        // The countdown, not the position.
        assert_eq!(zones[0].reverse_index, 3);
        assert_eq!(zones[2].reverse_index, 1);
    }

    #[test]
    fn set_top_note_moves_exactly_one_byte() {
        let before = table(&[96, 65, 53]);
        let mut after = before.clone();
        set_top_note(&mut after, 1, 60).unwrap();
        let differing: Vec<_> = (0..before.len())
            .filter(|&i| before[i] != after[i])
            .collect();
        assert_eq!(differing, vec![RECORDS_AT + RECORD_LEN + TOP_NOTE]);
        assert_eq!(read(&after).unwrap()[1].top_note, 60);
    }

    #[test]
    fn out_of_range_zone_is_rejected() {
        let mut m = table(&[96, 65]);
        assert!(set_top_note(&mut m, 2, 60).is_err());
    }

    #[test]
    fn short_map_is_rejected() {
        assert!(read(&[0u8; 16]).is_err());
        let mut m = table(&[96, 65]);
        m[COUNT_AT] = 9; // more zones than there are records
        assert!(read(&m).is_err());
    }

    #[test]
    fn derived_ranges_match_the_editor() {
        // Root keys C5/C4/C3 give the ranges the editor writes.
        assert_eq!(derive_top_notes(&[72, 60, 48]), vec![96, 65, 53]);
        assert_eq!(derive_top_notes(&[60, 48]), vec![84, 53]);
        assert_eq!(derive_top_notes(&[60]), vec![84]);
    }

    #[test]
    fn derived_ranges_handle_an_odd_gap() {
        // Adjacent semitones leave no room between them.
        assert_eq!(derive_top_notes(&[61, 60]), vec![85, 60]);
    }
}
