//! The `stk` sections — one per zone, each holding one zone's encoded audio.

use crate::error::ParseError;

/// Within a stroke payload: the MIDI note the sample was recorded at.
const ROOT_KEY: usize = 5;

/// Encoded audio is emitted in fixed-size packets; the count varies with how
/// compressible the material is.
pub const PACKET_LEN: usize = 381;

/// Stroke header for the first stroke of a single-zone instrument.
const FIRST_HEADER_LEN: usize = 165;

/// Each additional zone takes this much off the *first* stroke's header — the same
/// number of bytes a zone record adds to the `map` section.
const FIRST_HEADER_PER_EXTRA_ZONE: usize = 15;

/// Every stroke after the first has a fixed header, whatever the zone count.
const LATER_HEADER_LEN: usize = 372;

/// Highest zone count the first-stroke term is known to hold for.
///
/// It was derived from instruments of one, two and three zones, and files from the
/// vendor's own library need **15 more** at six zones and above — one whole zone
/// record. Past that the shrink cannot continue at all: sixteen zones would put the
/// header below zero. Where the crossover sits, and what floors it, needs specimens
/// between four and six zones that we do not have.
const FIRST_HEADER_MAX_ZONES: usize = 3;

/// Bytes of stroke header, which depends on the stroke's position and — for the first
/// stroke only — on how many zones the instrument has.
///
/// `None` when the zone count is outside the range the first-stroke term was derived
/// in, because a wrong header silently turns into a wrong packet count.
pub fn header_len(index: usize, zones: usize) -> Option<usize> {
    if index > 0 {
        return Some(LATER_HEADER_LEN);
    }
    (zones <= FIRST_HEADER_MAX_ZONES)
        .then(|| FIRST_HEADER_LEN - FIRST_HEADER_PER_EXTRA_ZONE * zones.saturating_sub(1))
}

/// One zone's audio.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Stroke {
    /// MIDI note the sample plays back untransposed at.
    pub root_key: u8,
    /// Encoded audio packets. Content-dependent: predictable material costs fewer.
    ///
    /// `None` when the stroke's header length is not known, which is the first stroke
    /// of anything past [`FIRST_HEADER_MAX_ZONES`] zones — the packets are there, but
    /// where they start is not established, and guessing would report a number that
    /// looks like a measurement. Everything else about the stroke still reads.
    pub packets: Option<usize>,
}

pub fn read(payload: &[u8], index: usize, zones: usize) -> Result<Stroke, ParseError> {
    let root_key = *payload.get(ROOT_KEY).ok_or_else(|| {
        ParseError::AssertFail(format!("stroke {index} is {} bytes", payload.len()))
    })?;
    let packets = match header_len(index, zones) {
        Some(header) => {
            let body = payload.len().checked_sub(header).ok_or_else(|| {
                ParseError::AssertFail(format!(
                    "stroke {index} is {} bytes, shorter than its {header}-byte header",
                    payload.len()
                ))
            })?;
            // A stroke that does not divide is a header we have mismodelled, not a
            // corrupt file: the vendor library is full of them. Report no count
            // rather than refusing the whole instrument over a field nothing
            // structural depends on.
            (body % PACKET_LEN == 0).then_some(body / PACKET_LEN)
        }
        None => None,
    };
    Ok(Stroke { root_key, packets })
}

pub fn set_root_key(payload: &mut [u8], note: u8) -> Result<(), ParseError> {
    let slot = payload
        .get_mut(ROOT_KEY)
        .ok_or_else(|| ParseError::AssertFail("stroke is too short to hold a root key".into()))?;
    *slot = note;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn stroke(index: usize, zones: usize, packets: usize, root: u8) -> Vec<u8> {
        let mut v = vec![0u8; header_len(index, zones).unwrap() + packets * PACKET_LEN];
        v[ROOT_KEY] = root;
        v
    }

    #[test]
    fn header_shrinks_only_for_the_first_stroke() {
        assert_eq!(header_len(0, 1), Some(165));
        assert_eq!(header_len(0, 2), Some(150));
        assert_eq!(header_len(0, 3), Some(135));
        // Position, not zone count, decides the rest.
        assert_eq!(header_len(1, 2), Some(372));
        assert_eq!(header_len(1, 3), Some(372));
        assert_eq!(header_len(2, 3), Some(372));
    }

    /// Past three zones the first stroke's header is not known, and the vendor library
    /// is the proof: those files need one zone record more than this term gives, and by
    /// sixteen zones it would be negative. Later strokes are unaffected — their header
    /// never depended on the zone count.
    #[test]
    fn the_first_stroke_header_is_unknown_past_the_derived_range() {
        assert_eq!(header_len(0, 4), None);
        assert_eq!(header_len(0, 16), None);
        assert_eq!(header_len(1, 16), Some(372));
    }

    #[test]
    fn reads_root_key_and_packet_count() {
        let s = read(&stroke(0, 1, 4, 60), 0, 1).unwrap();
        assert_eq!(s.root_key, 60);
        assert_eq!(s.packets, Some(4));
    }

    /// A vendor instrument: eleven zones, so the first stroke's header is unknown and
    /// its packet count with it — but the root key still reads, and the file is not
    /// refused. Reading `Xylophone__Korg 02 mono 2.0 [ne5].nsmp` used to fail here.
    #[test]
    fn a_many_zoned_first_stroke_reads_without_a_packet_count() {
        let mut v = vec![0u8; 7269];
        v[ROOT_KEY] = 72;
        let s = read(&v, 0, 11).expect("the stroke still reads");
        assert_eq!(s.root_key, 72);
        assert_eq!(s.packets, None);
        // The zones after it are ordinary.
        let mut later = vec![0u8; 372 + 3 * PACKET_LEN];
        later[ROOT_KEY] = 60;
        assert_eq!(read(&later, 1, 11).unwrap().packets, Some(3));
    }

    #[test]
    fn lengths_from_the_corpus_decompose_exactly() {
        // (index, zones, total length) taken off real instruments.
        for (index, zones, len, packets) in [
            (0, 1, 1689, 4), // single zone
            (0, 2, 1674, 4), // same audio, one more zone
            (1, 2, 1896, 4),
            (0, 3, 2040, 5),
            (1, 3, 1896, 4),
            (2, 3, 1896, 4),
            (0, 2, 2055, 5),
            (0, 1, 165, 0), // 16 frames: a header and no audio at all
        ] {
            let mut v = vec![0u8; len];
            v[ROOT_KEY] = 60;
            let s = read(&v, index, zones)
                .unwrap_or_else(|e| panic!("stroke {index} of {zones}, {len} bytes: {e}"));
            assert_eq!(
                s.packets,
                Some(packets),
                "stroke {index} of {zones}, {len} bytes"
            );
        }
    }

    /// A length that does not divide means the header is mismodelled, which is a thing
    /// we know happens — so it reports no count rather than refusing the instrument.
    #[test]
    fn a_length_that_is_not_header_plus_packets_has_no_count() {
        let mut v = stroke(0, 1, 2, 60);
        v.push(0);
        assert_eq!(read(&v, 0, 1).unwrap().packets, None);
    }

    #[test]
    fn a_stroke_shorter_than_its_header_is_rejected() {
        assert!(read(&[0u8; 8], 1, 2).is_err());
    }

    #[test]
    fn set_root_key_moves_one_byte() {
        let before = stroke(0, 1, 1, 60);
        let mut after = before.clone();
        set_root_key(&mut after, 48).unwrap();
        let differing: Vec<_> = (0..before.len())
            .filter(|&i| before[i] != after[i])
            .collect();
        assert_eq!(differing, vec![ROOT_KEY]);
    }
}
