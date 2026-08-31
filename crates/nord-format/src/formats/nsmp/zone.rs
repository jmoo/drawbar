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

/// Within a wide zone record: the stroke's root key, duplicated from the stroke.
const WIDE_ROOT: usize = 0;

/// Within a wide zone record: the highest MIDI note this zone answers to.
const WIDE_TOP: usize = 1;

/// Within a wide zone record: the lowest, on the layouts that store one.
const WIDE_LOW: usize = 2;

/// A wide `map`'s zone-record layout, selected by the section's own version
/// rather than by the file's content version.
///
/// Every record opens `[root][top]`; what the version decides is the record
/// width, where the stroke's global id sits inside it, and whether a low note
/// follows the top. Inferred from specimens; not confirmed on hardware.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Wide {
    /// 11-byte records; zones tile, so no low note is stored.
    V12,
    /// 16-byte records with a low note. ⚠️ Stored low to high, the opposite of
    /// every other layout here.
    V14,
    /// [`Wide::V14`]'s records, behind a per-key table that names zones.
    V21,
}

impl Wide {
    pub fn from_version(map_version: u32) -> Result<Wide, ParseError> {
        match map_version {
            12 => Ok(Wide::V12),
            14 => Ok(Wide::V14),
            21 => Ok(Wide::V21),
            v => Err(ParseError::AssertFail(format!(
                "map section version {v} has no zone layout derived from a specimen"
            ))),
        }
    }

    pub const fn record_len(self) -> usize {
        match self {
            Wide::V12 => 11,
            Wide::V14 | Wide::V21 => 16,
        }
    }

    /// Offset of the `u32` stroke id within a record.
    pub const fn gid_at(self) -> usize {
        match self {
            Wide::V12 => 5,
            Wide::V14 | Wide::V21 => 8,
        }
    }

    pub const fn stores_low(self) -> bool {
        matches!(self, Wide::V14 | Wide::V21)
    }

    /// Where the per-key records start, on the layout that carries them.
    const fn key_map_at(self) -> Option<usize> {
        match self {
            Wide::V21 => Some(KEY_MAP_AT),
            Wide::V12 | Wide::V14 => None,
        }
    }
}

/// Start of the v21 per-key records, one per MIDI note, ahead of the zone table.
const KEY_MAP_AT: usize = 12;

/// Bytes per per-key record. It opens `[a][b][a][key]`; the rest is unmapped and
/// holds the same six bytes the earlier layouts give a key outright.
const KEY_MAP_STRIDE: usize = 10;

/// One per-key record per MIDI note.
const KEY_MAP_KEYS: usize = 128;

/// Whether the `map`'s per-key records name zones rather than standing at rest.
///
/// A v21 `map` opens each per-key record with `[a][b][a][key]`. Where a single
/// zone claims the keyboard every record holds its own key number four times;
/// where several do, `a` and `b` hold other zones' root keys, and no rule
/// producing them has been derived from specimens. A retune or a remap would
/// leave those stale, so callers refuse the edit rather than write a file whose
/// two accounts of the keyboard disagree.
///
/// A `map` too short to hold the records counts as naming them: the layout is
/// not the one this was derived from, so nothing here can vouch for an edit.
pub fn key_map_names_zones(wide: Wide, map: &[u8]) -> bool {
    let Some(at) = wide.key_map_at() else {
        return false;
    };
    (0..KEY_MAP_KEYS).any(|key| {
        match map
            .get(at + key * KEY_MAP_STRIDE..)
            .and_then(|r| r.get(..4))
        {
            Some(head) => head != [key as u8; 4],
            None => true,
        }
    })
}

/// Maximum unmodelled suffix searched after a wide zone table.
const MAX_TAIL: usize = 8;

/// A located wide zone table: its layout, where its records start in the `map`
/// payload, and how many it holds.
///
/// Reading and editing go through the same location, so a setter cannot reach a
/// record the reader would not have decoded.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Table {
    pub wide: Wide,
    at: usize,
    count: usize,
}

impl Table {
    /// Find the table and check every record against the strokes it names.
    ///
    /// The table sits at the end of the payload behind a count byte, with an
    /// unmodelled suffix of up to [`MAX_TAIL`] bytes; the fit is decided by that
    /// count and by every record naming a stroke that holds its root key.
    pub fn locate(
        map_version: u32,
        map: &[u8],
        strokes: &[(u32, u8)],
    ) -> Result<Table, ParseError> {
        let wide = Wide::from_version(map_version)?;
        let count = strokes.len();
        let mut first = None;
        for tail in 0..=MAX_TAIL {
            let table = count
                .checked_mul(wide.record_len())
                .and_then(|bytes| bytes.checked_add(tail))
                .and_then(|len| map.len().checked_sub(len))
                .filter(|&at| at >= 1)
                .map(|at| Table { wide, at, count });
            let attempt = match table {
                Some(table) => table.read(map, strokes).map(|_| table),
                None => Err(ParseError::AssertFail(format!(
                    "map section is {} bytes, too short for {count} zone records",
                    map.len()
                ))),
            };
            match attempt {
                Ok(table) => return Ok(table),
                // The complaint worth reporting is the one from the tightest fit;
                // the later placements only say the table is not there either.
                Err(e) => first.get_or_insert(e),
            };
        }
        Err(first.unwrap_or_else(|| {
            ParseError::AssertFail(format!("map section is {} bytes", map.len()))
        }))
    }

    pub fn count(&self) -> usize {
        self.count
    }

    /// Read every record, checking each against the stroke it names.
    pub fn read(&self, map: &[u8], strokes: &[(u32, u8)]) -> Result<Vec<ZoneV3>, ParseError> {
        self.fits(map)?;
        if map[self.at - 1] as usize != self.count {
            return Err(ParseError::AssertFail(format!(
                "zone count {} does not match the {} strokes",
                map[self.at - 1],
                self.count
            )));
        }
        (0..self.count)
            .map(|i| {
                let r = &map[self.at + i * self.wide.record_len()..][..self.wide.record_len()];
                let gid_at = self.wide.gid_at();
                let gid = u32::from_be_bytes(r[gid_at..gid_at + 4].try_into().unwrap());
                let root = strokes.iter().find(|(g, _)| *g == gid).map(|(_, r)| *r);
                match root {
                    Some(root) if root == r[WIDE_ROOT] => Ok(ZoneV3 {
                        stroke_gid: gid,
                        root_key: r[WIDE_ROOT],
                        top_note: r[WIDE_TOP],
                        low_note: self.wide.stores_low().then(|| r[WIDE_LOW]),
                    }),
                    Some(root) => Err(ParseError::AssertFail(format!(
                        "zone {i} carries root {} but its stroke {gid} holds {root}",
                        r[WIDE_ROOT]
                    ))),
                    None => Err(ParseError::AssertFail(format!(
                        "zone {i} references stroke {gid}, which the body does not hold"
                    ))),
                }
            })
            .collect()
    }

    /// Refuse a `map` the located table does not fit inside.
    fn fits(&self, map: &[u8]) -> Result<(), ParseError> {
        self.count
            .checked_mul(self.wide.record_len())
            .and_then(|bytes| bytes.checked_add(self.at))
            .filter(|&end| self.at >= 1 && end <= map.len())
            .map(|_| ())
            .ok_or_else(|| {
                ParseError::AssertFail(format!(
                    "map section is {} bytes, too short for {} zone records at {}",
                    map.len(),
                    self.count,
                    self.at
                ))
            })
    }

    /// Write one byte of one record, checked against the located table.
    fn set(&self, map: &mut [u8], index: usize, at: usize, note: u8) -> Result<(), ParseError> {
        self.fits(map)?;
        if index >= self.count {
            return Err(ParseError::AssertFail(format!(
                "zone {index} out of range, the instrument has {}",
                self.count
            )));
        }
        map[self.at + index * self.wide.record_len() + at] = note;
        Ok(())
    }

    /// Write the root key duplicated into a record. The stroke holds the other
    /// copy, and the two must move together or the table stops reading.
    pub fn set_root_key(&self, map: &mut [u8], index: usize, note: u8) -> Result<(), ParseError> {
        self.set(map, index, WIDE_ROOT, note)
    }

    pub fn set_top_note(&self, map: &mut [u8], index: usize, note: u8) -> Result<(), ParseError> {
        self.set(map, index, WIDE_TOP, note)
    }

    /// Write a zone's lowest note, on the layouts that store one.
    pub fn set_low_note(&self, map: &mut [u8], index: usize, note: u8) -> Result<(), ParseError> {
        if !self.wide.stores_low() {
            return Err(ParseError::AssertFail(
                "this map layout stores no low note: a zone reaches down to one above \
                 the next-lower zone's top"
                    .into(),
            ));
        }
        self.set(map, index, WIDE_LOW, note)
    }
}

/// Find and validate a wide zone table against `(stroke GID, root key)` pairs.
pub fn read_v3(
    map_version: u32,
    map: &[u8],
    strokes: &[(u32, u8)],
) -> Result<Vec<ZoneV3>, ParseError> {
    Table::locate(map_version, map, strokes)?.read(map, strokes)
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

    /// A wide `map`: a preamble of per-key records, the count byte, the zone
    /// records, and an unmodelled tail — the shape every wide specimen has.
    fn wide_map(version: u32, zones: &[(u32, u8, u8, u8)], tail: usize) -> Vec<u8> {
        let wide = Wide::from_version(version).unwrap();
        let preamble = match wide {
            Wide::V21 => KEY_MAP_AT + KEY_MAP_KEYS * KEY_MAP_STRIDE,
            Wide::V12 | Wide::V14 => 6 + 128 * 6,
        };
        let mut m = vec![0u8; preamble + 1 + zones.len() * wide.record_len() + tail];
        if let Some(at) = wide.key_map_at() {
            for key in 0..KEY_MAP_KEYS {
                m[at + key * KEY_MAP_STRIDE..][..4].fill(key as u8);
            }
        }
        m[preamble] = zones.len() as u8;
        for (i, &(gid, root, top, low)) in zones.iter().enumerate() {
            let r = preamble + 1 + i * wide.record_len();
            m[r + WIDE_ROOT] = root;
            m[r + WIDE_TOP] = top;
            if wide.stores_low() {
                m[r + WIDE_LOW] = low;
            }
            m[r + wide.gid_at()..][..4].copy_from_slice(&gid.to_be_bytes());
        }
        m
    }

    fn strokes(zones: &[(u32, u8, u8, u8)]) -> Vec<(u32, u8)> {
        zones.iter().map(|&(gid, root, _, _)| (gid, root)).collect()
    }

    #[test]
    fn a_wide_table_reads_behind_an_unmodelled_tail() {
        // The tails every generation was seen with: v12 none, v14 one byte,
        // v21 two or six.
        for (version, tail) in [(12, 0), (14, 1), (21, 2), (21, 6)] {
            let zones = [(9u32, 60u8, 84u8, 48u8), (22, 72, 108, 85)];
            let map = wide_map(version, &zones, tail);
            let read = read_v3(version, &map, &strokes(&zones)).unwrap_or_else(|e| {
                panic!("map v{version} with a {tail}-byte tail: {e}");
            });
            assert_eq!(read.len(), 2);
            assert_eq!(read[0].root_key, 60);
            assert_eq!(read[0].top_note, 84);
            assert_eq!(read[1].stroke_gid, 22);
            assert_eq!(
                read[0].low_note,
                (version != 12).then_some(48),
                "map v{version} low note"
            );
        }
    }

    #[test]
    fn an_undescribed_map_version_is_refused() {
        assert!(Wide::from_version(13).is_err());
        assert!(Wide::from_version(0).is_err());
    }

    /// Each wide setter moves the one byte it names and nothing else, whatever
    /// the record width.
    #[test]
    fn wide_setters_move_exactly_one_byte() {
        for version in [12, 14, 21] {
            let zones = [(9u32, 60u8, 84u8, 48u8), (22, 72, 108, 85)];
            let before = wide_map(version, &zones, 1);
            let table = Table::locate(version, &before, &strokes(&zones)).unwrap();

            for (what, apply) in [
                (
                    "top_note",
                    Table::set_top_note as fn(&Table, &mut [u8], usize, u8) -> _,
                ),
                ("root_key", Table::set_root_key),
            ] {
                let mut after = before.clone();
                apply(&table, &mut after, 1, 55).unwrap();
                let moved: Vec<_> = (0..before.len())
                    .filter(|&i| before[i] != after[i])
                    .collect();
                assert_eq!(moved.len(), 1, "map v{version} {what}: {moved:?}");
            }
        }
    }

    /// A layout that stores no low note refuses one rather than writing a byte
    /// that means something else.
    #[test]
    fn a_low_note_is_refused_where_zones_tile() {
        let zones = [(9u32, 60u8, 84u8, 0u8)];
        let mut map = wide_map(12, &zones, 0);
        let table = Table::locate(12, &map, &strokes(&zones)).unwrap();
        assert!(table.set_low_note(&mut map, 0, 48).is_err());

        let zones = [(9u32, 60u8, 84u8, 48u8)];
        let mut map = wide_map(14, &zones, 1);
        let table = Table::locate(14, &map, &strokes(&zones)).unwrap();
        table.set_low_note(&mut map, 0, 50).unwrap();
        assert_eq!(
            read_v3(14, &map, &strokes(&zones)).unwrap()[0].low_note,
            Some(50)
        );
    }

    #[test]
    fn a_zone_past_the_table_is_refused() {
        let zones = [(9u32, 60u8, 84u8, 48u8)];
        let mut map = wide_map(21, &zones, 2);
        let table = Table::locate(21, &map, &strokes(&zones)).unwrap();
        assert!(table.set_top_note(&mut map, 1, 60).is_err());
    }

    /// A retune has to move both copies of the root key: the table stops reading
    /// when the record and the stroke disagree.
    #[test]
    fn a_record_disagreeing_with_its_stroke_is_refused() {
        let zones = [(9u32, 60u8, 84u8, 48u8)];
        let mut map = wide_map(14, &zones, 1);
        let table = Table::locate(14, &map, &strokes(&zones)).unwrap();
        table.set_root_key(&mut map, 0, 48).unwrap();
        assert!(read_v3(14, &map, &strokes(&zones)).is_err());
        assert!(read_v3(14, &map, &[(9, 48)]).is_ok());
    }

    /// Only v21 carries per-key records, and only ones naming something other
    /// than their own key stand in the way of an edit.
    #[test]
    fn a_key_map_naming_zones_is_recognised() {
        let zones = [(9u32, 60u8, 84u8, 48u8)];
        let mut map = wide_map(21, &zones, 2);
        assert!(!key_map_names_zones(Wide::V21, &map));

        map[KEY_MAP_AT + 40 * KEY_MAP_STRIDE] = 55;
        assert!(key_map_names_zones(Wide::V21, &map));

        // The earlier layouts have no such table to disagree with.
        assert!(!key_map_names_zones(Wide::V12, &wide_map(12, &zones, 0)));
        assert!(!key_map_names_zones(Wide::V14, &wide_map(14, &zones, 1)));
    }

    /// A `map` too short to hold the records cannot be vouched for, so it counts
    /// as naming them.
    #[test]
    fn a_truncated_key_map_blocks_an_edit() {
        assert!(key_map_names_zones(Wide::V21, &[0u8; 32]));
    }
}
