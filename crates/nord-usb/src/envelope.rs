//! Converting between the wire's entity **body** and an on-disk `CBIN` file.
//!
//! The device transfers an entity body without its on-disk `CBIN` header. The
//! format, schema version, slot, and body reported by the device determine that
//! header; the container codec and checksum remain in `nord_format::cbin`.

use crate::error::{Error, Result};
use crate::wire::Location;
use nord_format::cbin::{self, Cbin, Header, RawBody};
use std::io::Cursor;

/// CRC-32/ISO-HDLC over a wire body — the same checksum the type-1 container
/// carries, for comparing against the device's own `0x1e` report.
pub fn crc32(data: &[u8]) -> u32 {
    nord_format::crc::crc32(data)
}

/// A wire slot as the header's `(bank, slot)` pair. Zero-indexed on both sides — one
/// below the display.
fn slot(at: Location) -> Result<(u16, u16)> {
    let bank = u16::try_from(at.bank)
        .map_err(|_| Error::InvalidArgument(format!("bank {} does not fit in CBIN", at.bank)))?;
    let slot = u16::try_from(at.slot)
        .map_err(|_| Error::InvalidArgument(format!("slot {} does not fit in CBIN", at.slot)))?;
    Ok((bank, slot))
}

/// The slot a header addresses, as the wire spells it.
pub fn location(header: &Header) -> Location {
    let (bank, slot) = header.slot();
    Location {
        bank: bank as u32,
        slot: slot as u32,
    }
}

/// The header's format tag as text.
pub fn tag(header: &Header) -> String {
    String::from_utf8_lossy(&header.tag).into_owned()
}

/// Wrap a wire body in a `CBIN` header, producing the bytes of a `.ne5p`-style file.
///
/// `format` and `version` are the tag and schema version the device reported for the
/// slot — both come from `0x1e` object info. `version` is per format tag, so passing a
/// program's 4 for a set list writes a header `nord-format` will refuse to read.
pub fn wrap(format: &str, at: Location, version: u32, body: &[u8]) -> Result<Vec<u8>> {
    if format.len() != 4 {
        return Err(Error::Envelope(format!(
            "format tag {format:?} is not 4 characters"
        )));
    }

    let file = Cbin {
        header: Header::new(format, slot(at)?, version),
        body: RawBody(body.to_vec()),
    };
    let mut out = Cursor::new(Vec::new());
    file.write_to(&mut out)
        .map_err(|e| Error::Envelope(e.to_string()))?;
    Ok(out.into_inner())
}

/// The inverse: take file bytes and hand back the container — the header the device
/// implies, and the body the wire wants. The checksum is verified on the way.
pub fn unwrap(file: &[u8]) -> Result<Cbin<RawBody>> {
    let read =
        cbin::read_raw(&mut Cursor::new(file)).map_err(|e| Error::Envelope(e.to_string()))?;
    // The container is content with a header and nothing after it; the wire is not —
    // the body is the whole payload of a write.
    if read.body.0.is_empty() {
        return Err(Error::Envelope(
            "the file is a bare CBIN header with no body to send".into(),
        ));
    }
    Ok(read)
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
        let got = unwrap(&file).unwrap();
        assert_eq!(tag(&got.header), "ne5p");
        assert_eq!(location(&got.header), Location::from_user(8, 14));
        assert_eq!(got.header.version, 4);
        assert_eq!(got.body.0, body);
    }

    /// A well-formed header with nothing behind it passes every container check, and
    /// still has nothing to transfer.
    #[test]
    fn unwrap_rejects_a_headers_worth_of_file() {
        let file = Cbin {
            header: Header::new("ne5p", (0, 0), 4),
            body: RawBody(Vec::new()),
        };
        let mut bytes = Cursor::new(Vec::new());
        file.write_to(&mut bytes).unwrap();
        let bytes = bytes.into_inner();
        assert!(cbin::read_raw(&mut Cursor::new(&bytes)).is_ok());
        assert!(unwrap(&bytes).is_err(), "an empty body has nothing to send");
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

    #[test]
    fn wrap_rejects_an_address_the_container_cannot_represent() {
        let at = Location {
            bank: u16::MAX as u32 + 1,
            slot: 0,
        };
        assert!(matches!(
            wrap("ne5p", at, 4, &[1]),
            Err(Error::InvalidArgument(_))
        ));
    }
}
