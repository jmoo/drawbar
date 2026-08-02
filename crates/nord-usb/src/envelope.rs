//! Converting between the wire's entity **body** and an on-disk `CBIN` file.
//!
//! The device transfers only the body — 121 bytes for an Electro 5 program — while a
//! `.ne5p` on disk is that body behind a 44-byte `CBIN` header. The header is fully
//! determined by the body plus the slot it came from, so a read can be turned into a
//! byte-exact file and a file can be stripped back down for a write.
//!
//! **Verified**: rebuilding the header from `(body, bank, slot, version)` reproduces all
//! 41 program specimens in the corpus byte-for-byte.
//!
//! ⚠️ The header is *not* fully determined by the body and slot alone: the schema version
//! at `0x14` differs per format tag (`ne5p` is 4, `ne5t` is 0 or 1), so it has to be
//! supplied by the caller from the device's own `0x1e` object-info response. Substituting
//! a constant reproduces programs correctly and silently corrupts every other class.
//!
//! This mirrors the layout `nord_format::common::header` parses — a second statement of
//! the same bytes, kept honest by the specimen tests here and by the CLI, which parses
//! every wrapped read back through `nord-format` before summarising it.

use crate::error::{Error, Result};
use crate::wire::Location;

/// Bytes of `CBIN` header ahead of the body.
pub const HEADER_LEN: usize = 44;

const MAGIC: &[u8; 4] = b"CBIN";
/// Offset of the CRC-32 within the header. The checksum covers the whole body.
const CRC_OFFSET: usize = 0x18;
/// Header type seen on every Electro 5 specimen (type-1, i.e. with CRC).
const HEADER_TYPE: u32 = 1;
/// Offset of the schema version within the header.
const VERSION_OFFSET: usize = 0x14;

/// CRC-32/ISO-HDLC, the same checksum `nord-format` verifies in the file header.
pub(crate) fn crc32(data: &[u8]) -> u32 {
    let mut crc = !0u32;
    for &b in data {
        crc ^= b as u32;
        for _ in 0..8 {
            crc = if crc & 1 != 0 {
                (crc >> 1) ^ 0xEDB8_8320
            } else {
                crc >> 1
            };
        }
    }
    !crc
}

/// Wrap a wire body in a `CBIN` header, producing the bytes of a `.ne5p`-style file.
///
/// `format` and `version` are the tag and schema version the device reported for the
/// slot — both come from `0x1e` object info. `version` is per format tag, so passing a
/// program's 4 for a set list writes a header `nord-format` will refuse to read.
pub fn wrap(format: &str, at: Location, version: u32, body: &[u8]) -> Result<Vec<u8>> {
    let tag = format.as_bytes();
    if tag.len() != 4 {
        return Err(Error::Transport(format!(
            "format tag {format:?} is not 4 characters"
        )));
    }

    let mut out = vec![0u8; HEADER_LEN + body.len()];
    out[0..4].copy_from_slice(MAGIC);
    out[4..8].copy_from_slice(&HEADER_TYPE.to_le_bytes());
    out[8..12].copy_from_slice(tag);
    // Location is two little-endian u16s, zero-indexed — the same numbering the wire
    // uses, and one below what the instrument displays.
    out[12..14].copy_from_slice(&(at.bank as u16).to_le_bytes());
    out[14..16].copy_from_slice(&(at.slot as u16).to_le_bytes());
    out[16..20].copy_from_slice(&u32::MAX.to_le_bytes());
    out[VERSION_OFFSET..VERSION_OFFSET + 4].copy_from_slice(&version.to_le_bytes());
    out[HEADER_LEN..].copy_from_slice(body);

    let checksum = crc32(&out[HEADER_LEN..]);
    out[CRC_OFFSET..CRC_OFFSET + 4].copy_from_slice(&checksum.to_le_bytes());
    Ok(out)
}

/// The inverse: take file bytes and hand back the body the wire wants, plus the
/// format tag and the slot the file claims to belong to.
pub fn unwrap(file: &[u8]) -> Result<(String, Location, &[u8])> {
    if file.len() <= HEADER_LEN {
        return Err(Error::Truncated {
            got: file.len(),
            need: HEADER_LEN + 1,
        });
    }
    if &file[0..4] != MAGIC {
        return Err(Error::Transport("not a CBIN file (bad magic)".into()));
    }

    let body = &file[HEADER_LEN..];
    let stored = u32::from_le_bytes(file[CRC_OFFSET..CRC_OFFSET + 4].try_into().unwrap());
    let actual = crc32(body);
    if stored != actual {
        return Err(Error::Transport(format!(
            "file checksum mismatch: header says {stored:08x}, body computes {actual:08x}"
        )));
    }

    let format = String::from_utf8_lossy(&file[8..12]).into_owned();
    let at = Location {
        bank: u16::from_le_bytes(file[12..14].try_into().unwrap()) as u32,
        slot: u16::from_le_bytes(file[14..16].try_into().unwrap()) as u32,
    };
    Ok((format, at, body))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A real `.ne5p` read off bank 8 slot 14, split at the header boundary.
    const HEADER: &str =
        "4342494e010000006e65357007000d00ffffffff04000000b65d46a500000000000000000000000000000000";
    const BODY: &str = "000401df06781fc60000000000000000000000000000000000000100000000000000000000400000000000000002200000000000022000400000008888000008008888000008000000000080000000000080000000000000000000800000000800800000000800020010060401020408140010000000000000";

    fn hex(s: &str) -> Vec<u8> {
        (0..s.len())
            .step_by(2)
            .map(|i| u8::from_str_radix(&s[i..i + 2], 16).unwrap())
            .collect()
    }

    #[test]
    fn wrap_rebuilds_the_original_file() {
        let body = hex(BODY);
        let file = [hex(HEADER), body.clone()].concat();
        // Bank 8 slot 14 on the instrument; 7 and 13 on the wire.
        let built = wrap("ne5p", Location::from_user(8, 14), 4, &body).unwrap();
        assert_eq!(built, file, "rebuilt header differs from the real file");
    }

    /// The version is the device's to report, not ours to assume: a set list is 0 or 1
    /// where a program is 4, and stamping a constant makes `nord-format` refuse the file.
    #[test]
    fn wrap_writes_the_version_it_is_given() {
        let body = hex(BODY);
        for version in [0u32, 1, 4, 540] {
            let file = wrap("ne5t", Location::from_user(1, 1), version, &body).unwrap();
            assert_eq!(
                u32::from_le_bytes(file[0x14..0x18].try_into().unwrap()),
                version
            );
        }
    }

    #[test]
    fn unwrap_is_the_inverse() {
        let body = hex(BODY);
        let file = wrap("ne5p", Location::from_user(8, 14), 4, &body).unwrap();
        let (format, at, got) = unwrap(&file).unwrap();
        assert_eq!(format, "ne5p");
        assert_eq!(at, Location::from_user(8, 14));
        assert_eq!(got, &body[..]);
    }

    #[test]
    fn unwrap_rejects_a_corrupted_body() {
        let body = hex(BODY);
        let mut file = wrap("ne5p", Location::from_user(8, 14), 4, &body).unwrap();
        *file.last_mut().unwrap() ^= 0xFF;
        assert!(
            unwrap(&file).is_err(),
            "a corrupted body should fail the checksum"
        );
    }
}
