//! The container's two checksums, as slices and as streams.
//!
//! One per header generation: a type-1 file carries a CRC-32 (ISO-HDLC) over the
//! body at `0x18`; a type-0 file ends with a CRC-16 (IBM-3740, a.k.a. CCITT-FALSE)
//! over every byte before it, stored little-endian.

use crcxx::crc16;
use crcxx::crc32;

const SLICES: usize = 16;

const CRC_32: crc32::Crc<crc32::LookupTable256xN<SLICES>> =
    crc32::Crc::<crc32::LookupTable256xN<SLICES>>::new(&crc32::catalog::CRC_32_ISO_HDLC);

const CRC_16: crc16::Crc<crc16::LookupTable256xN<SLICES>> =
    crc16::Crc::<crc16::LookupTable256xN<SLICES>>::new(&crc16::catalog::CRC_16_IBM_3740);

/// CRC-32 (ISO-HDLC) of a contiguous slice — the type-1 body checksum.
pub fn crc32(bytes: &[u8]) -> u32 {
    CRC_32.compute(bytes)
}

/// CRC-16 (IBM-3740) of a contiguous slice — the type-0 whole-file checksum.
///
/// Identified by matching the trailing two bytes of specimens from four families
/// (`nspg`, `ne5p`, `nsmp` v2, `nsmp3`); not confirmed on hardware.
pub fn crc16(bytes: &[u8]) -> u16 {
    CRC_16.compute(bytes)
}

/// Streaming CRC-32, for bytes that arrive in pieces.
pub struct Crc32Stream<'a>(crc32::ComputeMultipart<'a, crc32::LookupTable256xN<SLICES>>);

impl Crc32Stream<'_> {
    pub fn new() -> Crc32Stream<'static> {
        Crc32Stream(CRC_32.compute_multipart())
    }

    pub fn update(&mut self, bytes: &[u8]) {
        self.0.update(bytes);
    }

    pub fn value(&self) -> u32 {
        self.0.value()
    }
}

/// Streaming CRC-16, for bytes that arrive in pieces.
pub struct Crc16Stream<'a>(crc16::ComputeMultipart<'a, crc16::LookupTable256xN<SLICES>>);

impl Crc16Stream<'_> {
    pub fn new() -> Crc16Stream<'static> {
        Crc16Stream(CRC_16.compute_multipart())
    }

    pub fn update(&mut self, bytes: &[u8]) {
        self.0.update(bytes);
    }

    pub fn value(&self) -> u16 {
        self.0.value()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The catalog check values: what each algorithm returns for `"123456789"`.
    /// Pins the parameters (poly/init/reflect/xorout) against a swap to a
    /// neighboring variant, which the container tests could miss.
    #[test]
    fn the_algorithms_are_the_cataloged_ones() {
        assert_eq!(crc32(b"123456789"), 0xCBF4_3926, "not CRC-32/ISO-HDLC");
        assert_eq!(crc16(b"123456789"), 0x29B1, "not CRC-16/IBM-3740");
    }

    /// A stream fed in pieces equals the slice computed whole.
    #[test]
    fn streams_match_slices() {
        let data: Vec<u8> = (0u8..=255).cycle().take(1000).collect();

        let mut s32 = Crc32Stream::new();
        let mut s16 = Crc16Stream::new();
        for chunk in data.chunks(7) {
            s32.update(chunk);
            s16.update(chunk);
        }
        assert_eq!(s32.value(), crc32(&data));
        assert_eq!(s16.value(), crc16(&data));
    }
}
