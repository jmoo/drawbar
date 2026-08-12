//! The zone table at the tail of the `map` section.

use crate::error::ParseError;

/// Offset of the zone count within the `map` payload. Everything before it is identical
/// across every corpus specimen, whatever the zone layout.
pub const COUNT_AT: usize = 785;

/// First zone record.
pub const RECORDS_AT: usize = COUNT_AT + 1;

/// Bytes per zone record.
pub const RECORD_LEN: usize = 15;

/// Within a record: which stroke plays this zone, by the global id the stroke carries
/// in its own first four bytes.
///
/// ⚠️ **Not a positional index.** Instruments the editor builds in one pass number
/// their strokes `n…1` in file order, so this byte reads as a countdown and pairing
/// zones with strokes by position appears to work. It is a coincidence of how those
/// files were made: the vendor library ships ids like `13 12 6 9 5 25`, in an order
/// that matches nothing, and pairing by position there hands every zone the wrong
/// sample. Same idea as the v3/v4 table, one generation earlier — see [`ZoneV3`].
const STROKE_ID: usize = 2;

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
    /// The stroke that plays this zone, by global id — see [`STROKE_ID`].
    pub stroke_id: u8,
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
                stroke_id: r[STROKE_ID],
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

/// A v3/v4 keyboard zone, from the record table at the tail of the wide
/// chain's `map` section.
///
/// Layouts by `map` section version — derived from the corpus, where every
/// record names a real stroke and carries that stroke's root key at byte 0:
///
/// | map | record | gid at | low note | stored order |
/// |---|---|---|---|---|
/// | 12 | 11 B, no trailer | +5 | tiled, not stored | high → low |
/// | 14 | 16 B, 1 B trailer | +8 | at +2 | low → high |
/// | 21 | 16 B, 2 B trailer | +8 | at +2 | high → low |
///
/// A one-byte zone count sits immediately before the records, and equals the
/// stroke count on every specimen. Inferred from specimens; not confirmed on
/// hardware. (A `map` version 13 is reported to share the 16-byte layout, but
/// no specimen shows it, so it is refused rather than assumed.)
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ZoneV3 {
    /// The referenced stroke's global id — the u32 its `stk` payload leads with.
    pub stroke_gid: u32,
    /// The stroke's root key, duplicated into the record.
    pub root_key: u8,
    /// Highest MIDI note this zone answers to.
    pub top_note: u8,
    /// Lowest note, where the layout stores one (`map` v14/v21). On v12 zones
    /// tile: a zone's bottom is one above the next-lower zone's top.
    pub low_note: Option<u8>,
}

/// Reads the v3/v4 zone table. `strokes` is the body's `(gid, root key)` list,
/// used to size the table and to verify every record against the stroke it
/// names — a misaligned read cannot pass it.
pub fn read_v3(
    map_version: u32,
    map: &[u8],
    strokes: &[(u32, u8)],
) -> Result<Vec<ZoneV3>, ParseError> {
    let (record_len, gid_at, trailer, has_low) = match map_version {
        12 => (11usize, 5usize, 0usize, false),
        14 => (16, 8, 1, true),
        21 => (16, 8, 2, true),
        v => {
            return Err(ParseError::AssertFail(format!(
                "map section version {v} has no zone layout derived from a specimen"
            )))
        }
    };

    let n = strokes.len();
    let start = map
        .len()
        .checked_sub(trailer + n * record_len)
        .filter(|&s| s >= 1)
        .ok_or_else(|| {
            ParseError::AssertFail(format!(
                "map section is {} bytes, too short for {n} zone records",
                map.len()
            ))
        })?;
    if map[start - 1] as usize != n {
        return Err(ParseError::AssertFail(format!(
            "zone count {} does not match the {n} strokes",
            map[start - 1]
        )));
    }

    (0..n)
        .map(|i| {
            let r = &map[start + i * record_len..][..record_len];
            let gid = u32::from_be_bytes(r[gid_at..gid_at + 4].try_into().unwrap());
            let root = strokes.iter().find(|(g, _)| *g == gid).map(|(_, r)| *r);
            match root {
                Some(root) if root == r[0] => Ok(ZoneV3 {
                    stroke_gid: gid,
                    root_key: r[0],
                    top_note: r[1],
                    low_note: has_low.then(|| r[2]),
                }),
                Some(root) => Err(ParseError::AssertFail(format!(
                    "zone {i} carries root {} but its stroke {gid} holds {root}",
                    r[0]
                ))),
                None => Err(ParseError::AssertFail(format!(
                    "zone {i} references stroke {gid}, which the body does not hold"
                ))),
            }
        })
        .collect()
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

    /// A table whose stroke ids run `n…1`, which is what the editor emits when it
    /// builds an instrument in one pass.
    fn table(tops: &[u8]) -> Vec<u8> {
        table_with_ids(tops, &(1..=tops.len() as u8).rev().collect::<Vec<_>>())
    }

    fn table_with_ids(tops: &[u8], ids: &[u8]) -> Vec<u8> {
        let mut m = vec![0u8; RECORDS_AT + tops.len() * RECORD_LEN];
        m[COUNT_AT] = tops.len() as u8;
        for (i, (&t, &id)) in tops.iter().zip(ids).enumerate() {
            let r = RECORDS_AT + i * RECORD_LEN;
            m[r + STROKE_ID] = id;
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
        assert_eq!(zones[0].stroke_id, 3);
        assert_eq!(zones[2].stroke_id, 1);
    }

    /// The vendor library's own ids: unordered, not a countdown, and nowhere near the
    /// zone count. A reader treating the byte as a position would accept these and
    /// silently pair every zone with the wrong stroke.
    #[test]
    fn stroke_ids_need_not_be_a_countdown() {
        let zones = read(&table_with_ids(
            &[108, 90, 77, 66, 60, 53],
            &[13, 12, 6, 9, 5, 25],
        ))
        .unwrap();
        assert_eq!(
            zones.iter().map(|z| z.stroke_id).collect::<Vec<_>>(),
            [13, 12, 6, 9, 5, 25]
        );
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
