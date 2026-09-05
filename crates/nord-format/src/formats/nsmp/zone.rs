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

/// Within a record: the zone's gain, u24 big-endian with [`GAIN_BITS`] fractional bits.
const GAIN: usize = 3;

/// Fractional bits in a zone record's gain.
pub const GAIN_BITS: u32 = 20;

/// A zone gain of exactly 1.0 as the record encodes it.
pub const GAIN_UNITY: u32 = 1 << GAIN_BITS;

/// Within a record: the highest MIDI note this zone answers to.
const TOP_NOTE: usize = 9;

/// Within a record: the playing stroke's relative strength, u16 big-endian.
const REL_STRENGTH: usize = 10;

/// The relative strength the editor writes for a zone holding one sample.
pub const REL_STRENGTH_DEFAULT: u16 = 1;

/// A high-to-low keyboard zone storing only its upper bound.
/// Inferred from specimens; not confirmed on hardware.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Zone {
    /// Highest MIDI note this zone answers to.
    pub top_note: u8,
    /// The stroke that plays this zone, by global id — see `STROKE_ID`.
    pub stroke_id: u8,
    /// Linear playback gain, [`GAIN_BITS`] fractional bits — [`GAIN_UNITY`] is 1.0. The
    /// audio is stored unscaled; the same factor scales the stroke's statistic A.
    pub gain: u32,
    /// Where the playing stroke sits on the editor's 0..32767 strength axis.
    /// [`REL_STRENGTH_DEFAULT`] on a zone holding a single sample.
    ///
    /// ⚠️ A zone plays exactly one stroke, so this is a position and not a
    /// count: an editor project may hold several samples per zone, but only
    /// one of them is enabled and only that one is written. Reading it as a
    /// layer count is wrong on every file the format has ever carried.
    pub rel_strength: u16,
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
                gain: u32::from_be_bytes([0, r[GAIN], r[GAIN + 1], r[GAIN + 2]]),
                rel_strength: u16::from_be_bytes([r[REL_STRENGTH], r[REL_STRENGTH + 1]]),
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
    /// The velocities this zone answers to, where the layout stores a window
    /// (`map` v14/v21). `None` on v12, whose records are too short to hold one.
    pub velocity: Option<VelocityWindow>,
    /// Where the playing stroke sits on the editor's 0..32767 strength axis,
    /// where the layout stores it (`map` v14/v21). `None` on v12.
    ///
    /// The same field [`Zone::rel_strength`] holds one generation earlier, four
    /// bytes further into a wider record, and — like it — a position rather
    /// than a count: a zone plays exactly one stroke.
    pub rel_strength: Option<u16>,
}

/// The velocities a zone answers to, inclusive at both ends.
///
/// The format has carried this since `map` v14 and no shipped instrument uses
/// it: every zone of every vendor instrument reads [`VelocityWindow::FULL`].
/// It is nonetheless a live field — a project naming a narrower window renders
/// one into a v4 record — and a zone is silent outside its band, so a reader
/// must honour what is stored rather than assume the full range.
///
/// Inferred from specimens; not confirmed on hardware.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct VelocityWindow {
    pub low: u8,
    pub high: u8,
}

impl VelocityWindow {
    /// The window every shipped instrument carries.
    pub const FULL: VelocityWindow = VelocityWindow { low: 0, high: 127 };

    pub fn contains(&self, velocity: u8) -> bool {
        (self.low..=self.high).contains(&velocity)
    }
}

/// Within a wide zone record: the stroke's root key, duplicated from the stroke.
const WIDE_ROOT: usize = 0;

/// Within a wide zone record: the highest MIDI note this zone answers to.
const WIDE_TOP: usize = 1;

/// Within a wide zone record: the lowest, on the layouts that store one.
const WIDE_LOW: usize = 2;

/// Within a 16-byte wide zone record: the playing stroke's relative strength,
/// u16 big-endian, ahead of the velocity window.
const WIDE_REL_STRENGTH: usize = 12;

/// Within a 16-byte wide zone record: the velocity window's low then high
/// bound, the last two bytes of the record.
const WIDE_VELOCITY: usize = 14;

/// Which byte of a zone record an edit names.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Field {
    /// The stroke's root key, duplicated from the stroke.
    Root,
    /// The highest MIDI note this zone answers to.
    Top,
    /// The lowest, on the layouts that store one.
    Low,
}

/// A wide `map`'s zone-record layout, selected by the section's own version
/// rather than by the file's content version.
///
/// Every record opens `[root][top]`; what the version decides is the record
/// width, where the stroke's global id sits inside it, and whether a low note
/// follows the top.
///
/// Inferred from specimens; not confirmed on hardware.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Wide {
    /// 11-byte records; zones tile, so no low note is stored.
    V12,
    /// 16-byte records with a low note. ⚠️ Stored low to high, the opposite of
    /// every other layout here.
    V14,
    /// [`Wide::V14`]'s records, behind a per-key table naming the zones
    /// around each note.
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

    /// Offset of the velocity window within a record, `None` on the layout
    /// whose records predate it.
    pub const fn velocity_at(self) -> Option<usize> {
        match self {
            Wide::V12 => None,
            Wide::V14 | Wide::V21 => Some(WIDE_VELOCITY),
        }
    }

    /// Offset of the relative strength within a record, `None` on the layout
    /// whose records predate it.
    pub const fn rel_strength_at(self) -> Option<usize> {
        match self {
            Wide::V12 => None,
            Wide::V14 | Wide::V21 => Some(WIDE_REL_STRENGTH),
        }
    }

    /// Offset of a field within a zone record, `None` where the layout stores
    /// no such field.
    const fn field_at(self, field: Field) -> Option<usize> {
        match field {
            Field::Root => Some(WIDE_ROOT),
            Field::Top => Some(WIDE_TOP),
            Field::Low if self.stores_low() => Some(WIDE_LOW),
            Field::Low => None,
        }
    }

    /// Whether the layout carries a per-key table ahead of its zone records.
    pub const fn has_key_map(self) -> bool {
        matches!(self, Wide::V21)
    }
}

/// The v21 `map`'s per-key table: one record per MIDI note, ahead of the zone
/// records, and a six-byte unit of the same shape at offset 0 holding the
/// instrument's own gain.
///
/// ⚠️ **The level comes first.** A record is
///
/// ```text
/// 6 + 10 × key:  [gain u24 BE][detune ×3][a][b][a][key]
/// ```
///
/// Framing it the other way round — quad first, level behind — puts key k+1's
/// level in key k's record, and lands the vendor level curve's slope changes
/// and its stop a key below the zone span they sit on.
///
/// The gain is linear with `0x100000` for unity; it is an authored per-key
/// curve that no zone layout predicts, and the three bytes behind it are where
/// a per-note detune lands. Both are carried across an edit untouched. Only the
/// quad follows from the zones, by [`partners`].
///
/// Inferred from specimens; not confirmed on hardware.
const KEY_TABLE_AT: usize = 6;
const KEY_STRIDE: usize = 10;
const KEY_QUAD_AT: usize = 6;
const KEYS: usize = 128;

/// The lowest key the per-key table ever describes, and the floor the editor's
/// project file counts its note list from. The editor writes it into the bottom
/// zone's `low`; the vendor library writes 0 there and means this.
const KEY_FLOOR: u8 = 17;

/// How far a partner root below a zone's own may be pitched up to cover it: a
/// minor third. Pitching down is unrestricted as far as any specimen shows.
const PARTNER_UP: u8 = 3;

/// What a `map`'s per-key table holds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeyMap {
    /// This layout carries no per-key table.
    Absent,
    /// Every record names its own key. The sample editor writes this whatever
    /// the zone layout, and it is also what [`partners`] gives an instrument no
    /// zone of which has an eligible partner.
    Neutral,
    /// Partner roots, filled in from the zone layout by the vendor's builder.
    Populated,
}

/// Zones as `(root, low, top)` ascending by root, which is the order the
/// partner law reads them in.
fn ladder(zones: &[ZoneV3]) -> Result<Vec<(u8, u8, u8)>, ParseError> {
    let mut out = zones
        .iter()
        .map(|z| {
            let low = z.low_note.ok_or_else(|| {
                ParseError::AssertFail(
                    "a per-key table needs each zone's low note, and this layout stores none"
                        .into(),
                )
            })?;
            Ok((z.root_key, low, z.top_note))
        })
        .collect::<Result<Vec<_>, ParseError>>()?;
    out.sort_by_key(|&(root, _, _)| root);
    Ok(out)
}

/// The keys a layout covers: from [`KEY_FLOOR`] — or the bottom zone's own low,
/// whichever is higher — up to the highest zone's top.
fn span(ladder: &[(u8, u8, u8)]) -> Option<(u8, u8)> {
    Some((ladder.first()?.1.max(KEY_FLOOR), ladder.last()?.2))
}

/// The partner roots one key names, or the identity where none is eligible.
///
/// Take the zone that claims the key and let `R` be its root. Eligible are the
/// roots below `R` within [`PARTNER_UP`] semitones and every root above it.
/// `a` is the nearest of those to `R`, ties going to the lower root. `b` is the
/// nearest of the rest when `a` is below `R`; when `a` is above, `b` reaches
/// back *across* `R` for the highest eligible root below it, and only when
/// nothing is below does it take the next root above `a`.
///
/// Outside the span the record is the identity, `a = b = key`. Inferred from
/// specimens; not confirmed on hardware.
fn partners(ladder: &[(u8, u8, u8)], key: u8) -> (u8, u8) {
    let identity = (key, key);
    let Some((lo, hi)) = span(ladder) else {
        return identity;
    };
    if !(lo..=hi).contains(&key) {
        return identity;
    }
    // The bottom zone reaches down to the floor whatever its own record says.
    let Some(claim) = ladder
        .iter()
        .enumerate()
        .filter(|&(j, z)| if j == 0 { lo } else { z.1 } <= key)
        .map(|(j, _)| j)
        .max()
    else {
        return identity;
    };
    let root = ladder[claim].0;

    let mut roots: Vec<u8> = ladder.iter().map(|&(r, _, _)| r).collect();
    roots.dedup();
    let below: Vec<u8> = roots
        .iter()
        .copied()
        .filter(|&r| r < root && root - r <= PARTNER_UP)
        .collect();
    let above: Vec<u8> = roots.iter().copied().filter(|&r| r > root).collect();
    let nearest = |set: &[u8]| set.iter().copied().min_by_key(|&r| (r.abs_diff(root), r));
    let eligible: Vec<u8> = below.iter().chain(&above).copied().collect();
    let Some(a) = nearest(&eligible) else {
        return identity;
    };
    let b = if a < root {
        let rest: Vec<u8> = eligible.iter().copied().filter(|&r| r != a).collect();
        nearest(&rest).unwrap_or(a)
    } else if let Some(&highest_below) = below.last() {
        highest_below
    } else {
        above
            .iter()
            .copied()
            .filter(|&r| r != a)
            .min_by_key(|&r| r - root)
            .unwrap_or(a)
    };
    (a, b)
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
                        velocity: self.wide.velocity_at().map(|at| VelocityWindow {
                            low: r[at],
                            high: r[at + 1],
                        }),
                        rel_strength: self
                            .wide
                            .rel_strength_at()
                            .map(|at| u16::from_be_bytes([r[at], r[at + 1]])),
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

    /// Write one field of one record, checked against the located table.
    ///
    /// ⚠️ A root key is stored twice — here and in the stroke — and the table
    /// stops reading if the two disagree, so a caller writing this one owes the
    /// other.
    pub fn set(
        &self,
        map: &mut [u8],
        index: usize,
        field: Field,
        note: u8,
    ) -> Result<(), ParseError> {
        self.fits(map)?;
        if index >= self.count {
            return Err(ParseError::AssertFail(format!(
                "zone {index} out of range, the instrument has {}",
                self.count
            )));
        }
        let at = self.wide.field_at(field).ok_or_else(|| {
            ParseError::AssertFail(
                "this map layout stores no low note: a zone reaches down to one above \
                 the next-lower zone's top"
                    .into(),
            )
        })?;
        map[self.at + index * self.wide.record_len() + at] = note;
        Ok(())
    }

    /// What the per-key table ahead of the records holds.
    pub fn key_map(&self, map: &[u8]) -> Result<KeyMap, ParseError> {
        if !self.wide.has_key_map() {
            return Ok(KeyMap::Absent);
        }
        let mut neutral = true;
        for key in 0..KEYS {
            let at = KEY_TABLE_AT + key * KEY_STRIDE + KEY_QUAD_AT;
            let quad = map.get(at..at + 4).ok_or_else(|| {
                ParseError::AssertFail(format!(
                    "map section is {} bytes, too short for a per-key table",
                    map.len()
                ))
            })?;
            neutral &= quad == [key as u8; 4];
        }
        Ok(if neutral {
            KeyMap::Neutral
        } else {
            KeyMap::Populated
        })
    }

    /// The per-key quads `zones` calls for, as `(offset, bytes)` writes.
    ///
    /// Empty unless the table is [`KeyMap::Populated`]: the sample editor leaves
    /// it neutral whatever the layout, so a neutral table stays neutral and only
    /// the vendor builder's is recomputed.
    ///
    /// Every key is planned from the layout alone, so the result does not depend
    /// on what the table held — except outside the zones' span, where the law is
    /// the identity and two vendor builders write `[0][0][0][key]` instead. That
    /// is a wider idea of the playable keyboard which nothing in the layout
    /// distinguishes, so a record already carrying it is left as it came.
    ///
    /// Nothing is written here: a layout the law cannot read refuses before the
    /// caller moves a byte.
    pub fn plan_key_map(
        &self,
        map: &[u8],
        zones: &[ZoneV3],
    ) -> Result<Vec<(usize, [u8; 4])>, ParseError> {
        if self.key_map(map)? != KeyMap::Populated {
            return Ok(Vec::new());
        }
        let ladder = ladder(zones)?;
        let Some((lo, hi)) = span(&ladder) else {
            return Ok(Vec::new());
        };
        let mut plan = Vec::new();
        for key in 0..KEYS {
            let k = key as u8;
            let at = KEY_TABLE_AT + key * KEY_STRIDE + KEY_QUAD_AT;
            let quad = map.get(at..at + 4).ok_or_else(|| {
                ParseError::AssertFail(format!(
                    "map section is {} bytes, too short for a per-key table",
                    map.len()
                ))
            })?;
            if !(lo..=hi).contains(&k) && quad == [0, 0, 0, k] {
                continue;
            }
            let (a, b) = partners(&ladder, k);
            plan.push((at, [a, b, a, k]));
        }
        Ok(plan)
    }

    /// Check that a populated per-key table follows the derived partner law.
    pub fn validate_key_map(&self, map: &[u8], zones: &[ZoneV3]) -> Result<(), ParseError> {
        for (at, expected) in self.plan_key_map(map, zones)? {
            let found = &map[at..at + expected.len()];
            if found != expected {
                return Err(ParseError::AssertFail(format!(
                    "per-key record at byte {at} does not match the zone layout"
                )));
            }
        }
        Ok(())
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
            Wide::V21 => KEY_TABLE_AT + KEYS * KEY_STRIDE + 6 + 26,
            Wide::V12 | Wide::V14 => 6 + 128 * 6,
        };
        let mut m = vec![0u8; preamble + 1 + zones.len() * wide.record_len() + tail];
        if wide.has_key_map() {
            for key in 0..KEYS {
                let r = KEY_TABLE_AT + key * KEY_STRIDE;
                m[r..r + 3].copy_from_slice(&[0x10, 0, 0]);
                m[r + KEY_QUAD_AT..][..4].fill(key as u8);
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

    #[test]
    fn wide_setters_move_exactly_one_byte() {
        for version in [12, 14, 21] {
            let zones = [(9u32, 60u8, 84u8, 48u8), (22, 72, 108, 85)];
            let before = wide_map(version, &zones, 1);
            let table = Table::locate(version, &before, &strokes(&zones)).unwrap();

            for field in [Field::Top, Field::Root] {
                let mut after = before.clone();
                table.set(&mut after, 1, field, 55).unwrap();
                let moved: Vec<_> = (0..before.len())
                    .filter(|&i| before[i] != after[i])
                    .collect();
                assert_eq!(moved.len(), 1, "map v{version} {field:?}: {moved:?}");
            }
        }
    }

    #[test]
    fn a_low_note_is_refused_where_zones_tile() {
        let zones = [(9u32, 60u8, 84u8, 0u8)];
        let mut map = wide_map(12, &zones, 0);
        let table = Table::locate(12, &map, &strokes(&zones)).unwrap();
        assert!(table.set(&mut map, 0, Field::Low, 48).is_err());

        let zones = [(9u32, 60u8, 84u8, 48u8)];
        let mut map = wide_map(14, &zones, 1);
        let table = Table::locate(14, &map, &strokes(&zones)).unwrap();
        table.set(&mut map, 0, Field::Low, 50).unwrap();
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
        assert!(table.set(&mut map, 1, Field::Top, 60).is_err());
    }

    #[test]
    fn a_record_disagreeing_with_its_stroke_is_refused() {
        let zones = [(9u32, 60u8, 84u8, 48u8)];
        let mut map = wide_map(14, &zones, 1);
        let table = Table::locate(14, &map, &strokes(&zones)).unwrap();
        table.set(&mut map, 0, Field::Root, 48).unwrap();
        assert!(read_v3(14, &map, &strokes(&zones)).is_err());
        assert!(read_v3(14, &map, &[(9, 48)]).is_ok());
    }

    #[test]
    fn a_neutral_key_map_is_told_from_a_populated_one() {
        let zones = [(9u32, 60u8, 84u8, 48u8)];
        let mut map = wide_map(21, &zones, 2);
        let table = Table::locate(21, &map, &strokes(&zones)).unwrap();
        assert_eq!(table.key_map(&map).unwrap(), KeyMap::Neutral);

        map[KEY_TABLE_AT + 40 * KEY_STRIDE + KEY_QUAD_AT] = 55;
        assert_eq!(table.key_map(&map).unwrap(), KeyMap::Populated);

        // The earlier layouts have no such table.
        for version in [12, 14] {
            let map = wide_map(version, &zones, if version == 12 { 0 } else { 1 });
            let table = Table::locate(version, &map, &strokes(&zones)).unwrap();
            assert_eq!(table.key_map(&map).unwrap(), KeyMap::Absent);
        }
    }

    #[test]
    fn a_neutral_key_map_survives_an_edit() {
        let zones = [(9u32, 60u8, 84u8, 48u8), (22, 72, 108, 85)];
        let map = wide_map(21, &zones, 2);
        let table = Table::locate(21, &map, &strokes(&zones)).unwrap();
        let read = table.read(&map, &strokes(&zones)).unwrap();
        assert!(table.plan_key_map(&map, &read).unwrap().is_empty());
    }

    #[test]
    fn an_unknown_populated_key_map_is_refused() {
        let zones = [(9u32, 60u8, 84u8, 48u8)];
        let mut map = wide_map(21, &zones, 2);
        let table = Table::locate(21, &map, &strokes(&zones)).unwrap();
        let read = table.read(&map, &strokes(&zones)).unwrap();
        map[KEY_TABLE_AT + 40 * KEY_STRIDE + KEY_QUAD_AT] = 55;

        assert!(table.validate_key_map(&map, &read).is_err());
    }

    /// The Kalimba's sixteen roots, whose four- and five-semitone spacing is what
    /// pins the minor-third ceiling: nothing is ever eligible below, so every one
    /// of its zones names the two roots above it.
    fn kalimba() -> Vec<(u8, u8, u8)> {
        vec![
            (47, 0, 49),
            (51, 50, 53),
            (55, 54, 57),
            (59, 58, 60),
            (62, 61, 64),
            (66, 65, 68),
            (71, 69, 73),
            (75, 74, 77),
            (80, 78, 82),
            (84, 83, 86),
            (88, 87, 90),
            (92, 91, 94),
            (96, 95, 97),
            (99, 98, 100),
            (102, 101, 103),
            (105, 104, 108),
        ]
    }

    // Inferred from specimens; not confirmed on hardware.
    // Expected partners are a hand-checked oracle from the populated Kalimba table.
    #[test]
    fn the_partner_law_matches_a_populated_table() {
        let zs = kalimba();
        for (key, want) in [
            // Below the bottom zone's own low but inside the span, which starts
            // at the floor: the bottom zone claims it.
            (17, (51, 55)),
            (49, (51, 55)),
            // Four semitones up puts nothing within a minor third below, so both
            // partners come from above.
            (50, (55, 59)),
            (58, (62, 66)),
            // Three semitones below 62 is eligible, so `a` drops below the root
            // and `b` takes the next nearest — which is above it.
            (61, (59, 66)),
            (64, (59, 66)),
            // At the top there is nothing above, so both partners come from below.
            (104, (102, 102)),
            // Outside the span the record is the identity.
            (16, (16, 16)),
            (109, (109, 109)),
            (0, (0, 0)),
            (127, (127, 127)),
        ] {
            assert_eq!(partners(&zs, key), want, "key {key}");
        }
    }

    #[test]
    fn the_second_partner_straddles_the_root() {
        let zs = vec![(61, 17, 62), (64, 63, 65), (66, 66, 68), (71, 69, 73)];
        // 66 is two semitones up and 61 is three down: `a` takes the nearer 66,
        // and `b` then takes 61 rather than 71.
        assert_eq!(partners(&zs, 64), (66, 61));
    }

    #[test]
    fn a_lone_zone_names_nobody() {
        let zs = vec![(60, 17, 84)];
        for key in [17, 60, 84] {
            assert_eq!(partners(&zs, key), (key, key), "key {key}");
        }
    }

    #[test]
    fn the_span_starts_at_the_floor() {
        let zs = vec![(47, 0, 49), (51, 50, 53)];
        assert_eq!(span(&zs), Some((KEY_FLOOR, 53)));
        assert_eq!(partners(&zs, 16), (16, 16));
        assert_eq!(partners(&zs, 17), (51, 51));
        assert_eq!(partners(&zs, 54), (54, 54));
    }

    #[test]
    fn a_record_yields_its_strength_as_a_big_endian_pair() {
        let mut map = vec![0u8; RECORDS_AT + RECORD_LEN];
        map[COUNT_AT] = 1;
        map[RECORDS_AT + STROKE_ID] = 4;
        map[RECORDS_AT + TOP_NOTE] = 60;
        map[RECORDS_AT + REL_STRENGTH] = 0x7f;
        map[RECORDS_AT + REL_STRENGTH + 1] = 0xff;
        let zones = read(&map).unwrap();
        assert_eq!(zones[0].rel_strength, 32767);
    }

    #[test]
    fn a_wide_record_yields_its_strength_and_window_and_v12_neither() {
        for (version, expected) in [(21u32, Some(300u16)), (12, None)] {
            let zs = [(9u32, 60u8, 84u8, 48u8)];
            let mut map = wide_map(version, &zs, 2);
            let wide = Wide::from_version(version).unwrap();
            let at = map.len() - 2 - wide.record_len();
            if let Some(off) = wide.rel_strength_at() {
                map[at + off..at + off + 2].copy_from_slice(&300u16.to_be_bytes());
            }
            if let Some(off) = wide.velocity_at() {
                map[at + off] = 64;
                map[at + off + 1] = 100;
            }
            let zone = Table::locate(version, &map, &strokes(&zs))
                .unwrap()
                .read(&map, &strokes(&zs))
                .unwrap()
                .remove(0);
            assert_eq!(zone.rel_strength, expected, "map v{version}");
            assert_eq!(
                zone.velocity,
                expected.map(|_| VelocityWindow { low: 64, high: 100 }),
                "map v{version}"
            );
        }
    }

    #[test]
    fn a_truncated_key_map_is_refused() {
        let zones = [(9u32, 60u8, 84u8, 48u8)];
        let map = wide_map(21, &zones, 2);
        let table = Table::locate(21, &map, &strokes(&zones)).unwrap();
        assert!(table.key_map(&[0u8; 32]).is_err());
    }
}
