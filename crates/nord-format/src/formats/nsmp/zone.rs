//! The zone table at the tail of the `map` section.

use crate::error::ParseError;

/// Offset of the zone count within the `map` payload. Everything before it is identical
/// across every corpus specimen, whatever the zone layout.
pub const COUNT_AT: usize = 785;

/// First zone record.
pub const RECORDS_AT: usize = COUNT_AT + 1;

/// Bytes per zone record.
pub const RECORD_LEN: usize = 15;

/// Stroke global ID, not a positional index.
const STROKE_ID: usize = 2;

/// Within a record: the highest MIDI note this zone answers to.
const TOP_NOTE: usize = 9;

/// A high-to-low keyboard zone storing only its upper bound.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Zone {
    /// Highest MIDI note this zone answers to.
    pub top_note: u8,
    /// The stroke that plays this zone, by global id — see `STROKE_ID`.
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

/// Set one zone's isolated top-note byte without re-encoding audio.
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

/// Wide-generation zone paired to a stroke GID and duplicated root key.
/// Inferred from specimens; not confirmed on hardware.
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

/// Maximum unmodelled suffix searched after a wide zone table.
const MAX_TAIL: usize = 8;

/// Find and validate a wide zone table against `(stroke GID, root key)` pairs.
pub fn read_v3(
    map_version: u32,
    map: &[u8],
    strokes: &[(u32, u8)],
) -> Result<Vec<ZoneV3>, ParseError> {
    let (record_len, gid_at, has_low) = match map_version {
        12 => (11usize, 5usize, false),
        14 | 21 => (16, 8, true),
        v => {
            return Err(ParseError::AssertFail(format!(
                "map section version {v} has no zone layout derived from a specimen"
            )))
        }
    };

    let n = strokes.len();
    let table = |tail: usize| -> Result<Vec<ZoneV3>, ParseError> {
        let table_len = n
            .checked_mul(record_len)
            .and_then(|bytes| bytes.checked_add(tail))
            .ok_or_else(|| ParseError::AssertFail("zone table size overflow".into()))?;
        let start = map
            .len()
            .checked_sub(table_len)
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
    };

    let mut first = None;
    for tail in 0..=MAX_TAIL {
        match table(tail) {
            Ok(zones) => return Ok(zones),
            // The complaint worth reporting is the one from the tightest fit; the
            // later placements only say the table is not there either.
            Err(e) => first.get_or_insert(e),
        };
    }
    Err(first
        .unwrap_or_else(|| ParseError::AssertFail(format!("map section is {} bytes", map.len()))))
}

/// Derive the editor's default high-to-low top notes from root keys.
/// This builds new maps; readers must preserve stored top notes.
pub fn derive_top_notes(roots_high_to_low: &[u8]) -> Vec<u8> {
    roots_high_to_low
        .iter()
        .enumerate()
        .map(|(i, &root)| {
            if i == 0 {
                root.saturating_add(24).min(127)
            } else {
                let above = roots_high_to_low[i - 1];
                (u16::from(root) + u16::from(above))
                    .div_ceil(2)
                    .saturating_sub(1)
                    .min(127) as u8
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

    #[test]
    fn derived_ranges_stay_in_the_midi_domain() {
        assert_eq!(derive_top_notes(&[127]), vec![127]);
        assert_eq!(derive_top_notes(&[0, 0]), vec![24, 0]);
        assert_eq!(derive_top_notes(&[255, 255]), vec![127, 127]);
    }
}
