//! Base64, because a store keeps strings and an asset is bytes.
//!
//! Standard alphabet with `=` padding. Small enough to own rather than depend on, and
//! the only property that matters is that it round-trips.

const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

pub fn encode(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let b = |i: usize| *chunk.get(i).unwrap_or(&0) as u32;
        let word = (b(0) << 16) | (b(1) << 8) | b(2);
        for i in 0..4 {
            // A chunk of one byte spells two characters and two pads; a chunk of two
            // spells three and one.
            out.push(match i > chunk.len() {
                true => '=',
                false => ALPHABET[(word >> (18 - 6 * i)) as usize & 0x3f] as char,
            });
        }
    }
    out
}

/// The bytes a string spells, or `None` if it is not base64 at all.
pub fn decode(text: &str) -> Option<Vec<u8>> {
    let mut out = Vec::with_capacity(text.len() / 4 * 3);
    let mut word = 0u32;
    let mut have = 0u32;
    for c in text.bytes() {
        // Whitespace is layout, not data — a store that wrapped its lines still reads.
        if c.is_ascii_whitespace() || c == b'=' {
            continue;
        }
        let value = ALPHABET.iter().position(|a| *a == c)? as u32;
        word = (word << 6) | value;
        have += 6;
        if have >= 8 {
            have -= 8;
            out.push((word >> have) as u8);
        }
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The one property worth having.
    #[test]
    fn bytes_survive_a_round_trip_at_every_length() {
        for len in 0..64 {
            let bytes: Vec<u8> = (0..len).map(|n: u32| n.wrapping_mul(37) as u8).collect();
            assert_eq!(decode(&encode(&bytes)).as_deref(), Some(bytes.as_slice()));
        }
    }

    /// The standard spelling, so a store written here is readable by anything else.
    #[test]
    fn the_spelling_is_the_standard_one() {
        assert_eq!(encode(b""), "");
        assert_eq!(encode(b"f"), "Zg==");
        assert_eq!(encode(b"fo"), "Zm8=");
        assert_eq!(encode(b"foo"), "Zm9v");
        assert_eq!(encode(b"foob"), "Zm9vYg==");
        assert_eq!(encode(b"fooba"), "Zm9vYmE=");
        assert_eq!(encode(b"foobar"), "Zm9vYmFy");
        assert_eq!(decode("Zm9vYmFy").as_deref(), Some(&b"foobar"[..]));
    }

    /// Anything that is not base64 comes back as nothing, rather than as bytes that
    /// were never written.
    #[test]
    fn text_that_is_not_base64_decodes_to_nothing() {
        assert_eq!(decode("not base64!"), None);
        assert_eq!(decode("Zm9v*"), None);
    }
}
