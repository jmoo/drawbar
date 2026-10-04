//! The container's two checksums, as slices and as streams.
//!
//! One per header generation: a type-1 file stores a CRC-32 (ISO-HDLC) of the body
//! (which starts at `0x2c`) in the word at `0x18`; a type-0 file ends with a CRC-16
//! (IBM-3740, a.k.a. CCITT-FALSE) over every byte before it, stored little-endian.

use crate::error::ParseError;
use crcxx::crc16;
use crcxx::crc32;

const SLICES: usize = 16;

const CRC_32: crc32::Crc<crc32::LookupTable256xN<SLICES>> =
    crc32::Crc::<crc32::LookupTable256xN<SLICES>>::new(&crc32::catalog::CRC_32_ISO_HDLC);

const CRC_16: crc16::Crc<crc16::LookupTable256xN<SLICES>> =
    crc16::Crc::<crc16::LookupTable256xN<SLICES>>::new(&crc16::catalog::CRC_16_IBM_3740);

/// CRC-32 (ISO-HDLC) of a contiguous slice: the type-1 body checksum.
pub fn crc32(bytes: &[u8]) -> u32 {
    CRC_32.compute(bytes)
}

/// CRC-16 (IBM-3740) of a contiguous slice: the type-0 whole-file checksum.
///
/// Inferred from specimens; not confirmed on hardware. Identified by matching
/// the trailing two bytes of specimens from four families (`nspg`, `ne5p`,
/// `nsmp` v2, `nsmp3`).
pub fn crc16(bytes: &[u8]) -> u16 {
    CRC_16.compute(bytes)
}

/// A run of same-length bytes an edit replaces, `at` bytes into what a checksum covers.
#[derive(Debug, Clone, Copy)]
pub struct Change<'a> {
    pub at: u64,
    pub old: &'a [u8],
    pub new: &'a [u8],
}

/// The CRC-32 of a `len`-byte message after `changes`, from its CRC before them. No
/// unchanged byte is needed. The changes must not overlap.
pub fn crc32_patched(crc: u32, len: u64, changes: &[Change<'_>]) -> Result<u32, ParseError> {
    Ok(crc ^ REGISTER_32.delta(len, changes)?)
}

/// The CRC-16 of a `len`-byte message after `changes`, as [`crc32_patched`] restates a
/// CRC-32.
pub fn crc16_patched(crc: u16, len: u64, changes: &[Change<'_>]) -> Result<u16, ParseError> {
    let delta = REGISTER_16.delta(len, changes)?;
    Ok(crc ^ u16::try_from(delta).expect("a 16-bit register holds 16 bits"))
}

/// A CRC's shift register, stepped one bit at a time.
///
/// A CRC is affine in its message: for two messages of one length, the two CRCs differ
/// by the register run from zero over the bytes in which the messages differ. Initial
/// value and final XOR cancel, so they have no part here. Zero bytes leave a zero
/// register at zero, and the trailing ones shift a run's contribution by a power of the
/// one-zero-byte step, which squaring reaches in logarithmic time.
#[derive(Clone, Copy)]
struct Register {
    width: u32,
    /// Bit-reversed where the algorithm is reflected.
    poly: u32,
    reflected: bool,
}

/// CRC-32/ISO-HDLC, as [`CRC_32`] computes it.
const REGISTER_32: Register = Register {
    width: 32,
    poly: 0xEDB8_8320,
    reflected: true,
};

/// CRC-16/IBM-3740, as [`CRC_16`] computes it.
const REGISTER_16: Register = Register {
    width: 16,
    poly: 0x1021,
    reflected: false,
};

/// A linear map on a register, as the image of each bit.
type Matrix = [u32; 32];

impl Register {
    fn mask(self) -> u32 {
        u32::MAX >> (32 - self.width)
    }

    fn step(self, register: u32, byte: u8) -> u32 {
        let mut r = register;
        if self.reflected {
            r ^= u32::from(byte);
            for _ in 0..8 {
                r = (r >> 1) ^ (self.poly & (r & 1).wrapping_neg());
            }
        } else {
            let top = self.width - 1;
            r ^= u32::from(byte) << (self.width - 8);
            for _ in 0..8 {
                r = (r << 1) ^ (self.poly & ((r >> top) & 1).wrapping_neg());
            }
        }
        r & self.mask()
    }

    /// The register `register` becomes over `count` zero bytes.
    fn zeros(self, register: u32, count: u64) -> u32 {
        let mut step: Matrix = std::array::from_fn(|bit| match bit < self.width as usize {
            true => self.step(1 << bit, 0),
            false => 0,
        });
        let mut r = register;
        let mut count = count;
        while count > 0 {
            if count & 1 == 1 {
                r = apply(&step, r);
            }
            step = std::array::from_fn(|bit| apply(&step, step[bit]));
            count >>= 1;
        }
        r
    }

    /// How the CRC of a `len`-byte message moves under `changes`.
    fn delta(self, len: u64, changes: &[Change<'_>]) -> Result<u32, ParseError> {
        changes.iter().try_fold(0, |delta, change| {
            let end = u64::try_from(change.old.len())
                .ok()
                .and_then(|n| change.at.checked_add(n))
                .filter(|&end| end <= len && change.old.len() == change.new.len())
                .ok_or_else(|| ParseError::OutOfBounds {
                    value: format!(
                        "a change of {} bytes to {} at {}",
                        change.old.len(),
                        change.new.len(),
                        change.at
                    ),
                    bound: format!("a same-length change within the {len} bytes checksummed"),
                })?;
            let run = change
                .old
                .iter()
                .zip(change.new)
                .fold(0, |r, (old, new)| self.step(r, old ^ new));
            Ok(delta ^ self.zeros(run, len - end))
        })
    }
}

fn apply(matrix: &Matrix, register: u32) -> u32 {
    (0..32)
        .filter(|bit| (register >> bit) & 1 == 1)
        .fold(0, |out, bit| out ^ matrix[bit])
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

    /// `message` with each `(at, bytes)` written over it, and the changes that make it.
    fn edited<'a>(message: &'a [u8], edits: &'a [(usize, Vec<u8>)]) -> (Vec<u8>, Vec<Change<'a>>) {
        let mut out = message.to_vec();
        let changes = edits
            .iter()
            .map(|(at, bytes)| {
                let change = Change {
                    at: *at as u64,
                    old: &message[*at..*at + bytes.len()],
                    new: bytes,
                };
                out[*at..*at + bytes.len()].copy_from_slice(bytes);
                change
            })
            .collect();
        (out, changes)
    }

    #[test]
    fn a_patched_crc_is_the_crc_of_the_patched_message() {
        let message: Vec<u8> = (0..70_000u32)
            .map(|i| (i.wrapping_mul(2_654_435_761) >> 13) as u8)
            .collect();
        let cases: [&[(usize, Vec<u8>)]; 5] = [
            &[(0, vec![0xff])],
            &[(message.len() - 4, vec![1, 2, 3, 4])],
            &[(1000, vec![0; 300]), (65_000, b"renamed".to_vec())],
            &[(12, Vec::new())],
            &[(5, message[5..9].to_vec())],
        ];
        for edits in cases {
            let (patched, changes) = edited(&message, edits);
            let len = message.len() as u64;
            assert_eq!(
                crc32_patched(crc32(&message), len, &changes).unwrap(),
                crc32(&patched),
                "crc32 after {:?}",
                changes
            );
            assert_eq!(
                crc16_patched(crc16(&message), len, &changes).unwrap(),
                crc16(&patched),
                "crc16 after {:?}",
                changes
            );
        }
    }

    #[test]
    fn a_change_past_the_message_or_of_another_length_is_refused() {
        let message = [7u8; 16];
        let past = Change {
            at: 14,
            old: &message[..3],
            new: &[0; 3],
        };
        let longer = Change {
            at: 0,
            old: &message[..2],
            new: &[0; 3],
        };
        for change in [past, longer] {
            assert!(
                crc32_patched(crc32(&message), 16, &[change]).is_err(),
                "{change:?}"
            );
            assert!(
                crc16_patched(crc16(&message), 16, &[change]).is_err(),
                "{change:?}"
            );
        }
    }
}
