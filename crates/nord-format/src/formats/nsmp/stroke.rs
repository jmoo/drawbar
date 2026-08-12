//! The `stk` sections — one per zone, each holding one zone's encoded audio.

use crate::error::ParseError;

/// Within a stroke payload: the MIDI note the sample was recorded at.
const ROOT_KEY: usize = 5;

/// Encoded audio is emitted in fixed-size packets; the count varies with how
/// compressible the material is.
pub const PACKET_LEN: usize = 381;

/// Every stroke after the first has a fixed header, whatever the zone count.
const LATER_HEADER_LEN: usize = 372;

/// Bytes the metadata region reserves ahead of the first packet, over the `cat` and
/// `map` payloads and the first stroke's own header.
///
/// **Encoded audio begins at a fixed offset.** Everything before it — the `hdr`, the
/// category strings, the keyboard map, and the first stroke's header — is a preamble
/// of constant size, so the first stroke's header is not a field of its own so much as
/// whatever space the rest did not use. Adding a zone grows `map` by a record and takes
/// exactly that much off the header; when the metadata would fill the preamble
/// completely, the whole thing grows by one [`PACKET_LEN`] and the header starts over
/// with a full packet's worth of room.
///
/// Measured over every version-200 specimen: 39 of 42 put the first packet 1146 bytes
/// into the body and the other 3 put it at 1527. It also reconciles the vendor library
/// with our own output, which differ by 15 bytes of header at equal zone counts for the
/// mundane reason that their `cat` section is 9 bytes where ours is 24.
const PREAMBLE: usize = 990;

/// The first stroke's header, from the two sections that share the preamble with it.
///
/// Both lengths are payload sizes, excluding their 9-byte section headers.
pub fn first_header_len(cat_len: usize, map_len: usize) -> usize {
    let used = cat_len + map_len;
    let mut room = PREAMBLE;
    // Strictly greater: a header of zero does not occur, and an instrument whose
    // metadata lands exactly on the boundary takes the next step up. Twelve zones of
    // ours is that case.
    while room <= used {
        room += PACKET_LEN;
    }
    room - used
}

/// Bytes of stroke header. Only the first stroke's depends on anything.
pub fn header_len(index: usize, cat_len: usize, map_len: usize) -> usize {
    if index > 0 {
        LATER_HEADER_LEN
    } else {
        first_header_len(cat_len, map_len)
    }
}

/// One zone's audio.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Stroke {
    /// MIDI note the sample plays back untransposed at.
    pub root_key: u8,
    /// Encoded audio packets. Content-dependent: predictable material costs fewer.
    ///
    /// `None` only when the length does not decompose — a header this crate has
    /// mismodelled rather than a corrupt file, so it reports nothing instead of a
    /// number that would look measured. Everything else about the stroke still reads.
    pub packets: Option<usize>,
}

/// `cat_len` and `map_len` are payload sizes; only the first stroke uses them.
pub fn read(
    payload: &[u8],
    index: usize,
    cat_len: usize,
    map_len: usize,
) -> Result<Stroke, ParseError> {
    let root_key = *payload.get(ROOT_KEY).ok_or_else(|| {
        ParseError::AssertFail(format!("stroke {index} is {} bytes", payload.len()))
    })?;
    let header = header_len(index, cat_len, map_len);
    let packets = payload
        .len()
        .checked_sub(header)
        .filter(|body| body % PACKET_LEN == 0)
        .map(|body| body / PACKET_LEN);
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

    /// `map` payload for `z` zones, per the section's own rule.
    fn map_len(z: usize) -> usize {
        801 + 15 * (z - 1)
    }

    /// Our editor's `cat` section; the vendor's is 9.
    const OUR_CAT: usize = 24;

    fn stroke(index: usize, zones: usize, packets: usize, root: u8) -> Vec<u8> {
        let mut v = vec![0u8; header_len(index, OUR_CAT, map_len(zones)) + packets * PACKET_LEN];
        v[ROOT_KEY] = root;
        v
    }

    #[test]
    fn header_shrinks_only_for_the_first_stroke() {
        assert_eq!(header_len(0, OUR_CAT, map_len(1)), 165);
        assert_eq!(header_len(0, OUR_CAT, map_len(2)), 150);
        assert_eq!(header_len(0, OUR_CAT, map_len(3)), 135);
        // Position, not the zone table, decides the rest.
        assert_eq!(header_len(1, OUR_CAT, map_len(2)), 372);
        assert_eq!(header_len(2, OUR_CAT, map_len(3)), 372);
    }

    /// The zone-count ladder, generated for exactly this question: identical audio in
    /// the first zone at 4, 6, 8, 12 and 16 zones. The header shrinks a record per zone
    /// and then, when the metadata would fill the preamble, the whole thing gains a
    /// packet and the header starts over — which is why a plain linear term went
    /// negative past twelve zones.
    #[test]
    fn the_preamble_grows_by_a_packet_rather_than_going_negative() {
        for (zones, header) in [(4, 120), (6, 90), (8, 60), (12, 381), (16, 321)] {
            assert_eq!(
                header_len(0, OUR_CAT, map_len(zones)),
                header,
                "{zones} zones"
            );
        }
        // Eleven zones is the tightest fit before the step: 24 + 951 + 15 = 990.
        assert_eq!(header_len(0, OUR_CAT, map_len(11)), 15);
    }

    /// The vendor library's `cat` section is 9 bytes where ours is 24, and that alone
    /// is why their first stroke carries 15 bytes more header at the same zone count.
    /// The preamble is the same size in both.
    #[test]
    fn a_smaller_cat_section_lends_its_bytes_to_the_header() {
        const VENDOR_CAT: usize = 9;
        assert_eq!(header_len(0, VENDOR_CAT, map_len(6)), 105);
        assert_eq!(header_len(0, VENDOR_CAT, map_len(11)), 30);
        // Their smaller cat also defers the step by a zone: ours steps at twelve.
        assert_eq!(header_len(0, VENDOR_CAT, map_len(12)), 15);
        assert_eq!(header_len(0, VENDOR_CAT, map_len(16)), 336);
    }

    #[test]
    fn reads_root_key_and_packet_count() {
        let s = read(&stroke(0, 1, 4, 60), 0, OUR_CAT, map_len(1)).unwrap();
        assert_eq!(s.root_key, 60);
        assert_eq!(s.packets, Some(4));
    }

    #[test]
    fn lengths_from_the_corpus_decompose_exactly() {
        // (index, zones, total length, packets) taken off real instruments — ours at
        // the top, then the vendor files that used to be unreadable.
        for (index, zones, cat, len, packets) in [
            (0, 1, OUR_CAT, 1689, 4), // single zone
            (0, 2, OUR_CAT, 1674, 4), // same audio, one more zone
            (1, 2, OUR_CAT, 1896, 4),
            (0, 3, OUR_CAT, 2040, 5),
            (2, 3, OUR_CAT, 1896, 4),
            (0, 1, OUR_CAT, 165, 0), // 16 frames: a header and no audio at all
            (0, 4, OUR_CAT, 2787, 7), // the zone ladder
            (0, 12, OUR_CAT, 3048, 7), // same audio, past the step
            (0, 16, OUR_CAT, 2988, 7),
            (0, 6, 9, 10773, 28), // vendor Kalimba
            (0, 11, 9, 7269, 19), // vendor Xylophone
            (0, 16, 9, 1860, 4),  // vendor Clarinet, past the step
        ] {
            let mut v = vec![0u8; len];
            v[ROOT_KEY] = 60;
            let s = read(&v, index, cat, map_len(zones))
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
        assert_eq!(read(&v, 0, OUR_CAT, map_len(1)).unwrap().packets, None);
    }

    /// Shorter than its own header: no count, and still no refusal — the root key is
    /// the only thing a caller structurally needs, and it is present.
    #[test]
    fn a_stroke_shorter_than_its_header_has_no_count() {
        assert_eq!(
            read(&[0u8; 8], 1, OUR_CAT, map_len(2)).unwrap().packets,
            None
        );
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
