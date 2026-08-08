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

/// Bytes of stroke header, which depends on the stroke's position and — for the first
/// stroke only — on how many zones the instrument has.
pub fn header_len(index: usize, zones: usize) -> usize {
    if index == 0 {
        FIRST_HEADER_LEN.saturating_sub(FIRST_HEADER_PER_EXTRA_ZONE * zones.saturating_sub(1))
    } else {
        LATER_HEADER_LEN
    }
}

/// One zone's audio.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Stroke {
    /// MIDI note the sample plays back untransposed at.
    pub root_key: u8,
    /// Encoded audio packets. Content-dependent: predictable material costs fewer.
    pub packets: usize,
}

pub fn read(payload: &[u8], index: usize, zones: usize) -> Result<Stroke, ParseError> {
    let header = header_len(index, zones);
    let root_key = *payload.get(ROOT_KEY).ok_or_else(|| {
        ParseError::AssertFail(format!("stroke {index} is {} bytes", payload.len()))
    })?;
    let body = payload.len().checked_sub(header).ok_or_else(|| {
        ParseError::AssertFail(format!(
            "stroke {index} is {} bytes, shorter than its {header}-byte header",
            payload.len()
        ))
    })?;
    if body % PACKET_LEN != 0 {
        return Err(ParseError::AssertFail(format!(
            "stroke {index}: {} bytes is not a {header}-byte header plus whole \
             {PACKET_LEN}-byte packets ({} left over)",
            payload.len(),
            body % PACKET_LEN
        )));
    }
    Ok(Stroke {
        root_key,
        packets: body / PACKET_LEN,
    })
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
        let mut v = vec![0u8; header_len(index, zones) + packets * PACKET_LEN];
        v[ROOT_KEY] = root;
        v
    }

    #[test]
    fn header_shrinks_only_for_the_first_stroke() {
        assert_eq!(header_len(0, 1), 165);
        assert_eq!(header_len(0, 2), 150);
        assert_eq!(header_len(0, 3), 135);
        // Position, not zone count, decides the rest.
        assert_eq!(header_len(1, 2), 372);
        assert_eq!(header_len(1, 3), 372);
        assert_eq!(header_len(2, 3), 372);
    }

    #[test]
    fn reads_root_key_and_packet_count() {
        let s = read(&stroke(0, 1, 4, 60), 0, 1).unwrap();
        assert_eq!(s.root_key, 60);
        assert_eq!(s.packets, 4);
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
            assert_eq!(s.packets, packets, "stroke {index} of {zones}, {len} bytes");
        }
    }

    #[test]
    fn a_length_that_is_not_header_plus_packets_is_rejected() {
        let mut v = stroke(0, 1, 2, 60);
        v.push(0);
        assert!(read(&v, 0, 1).is_err());
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
