//! Typed bit fields over a panel's bytes.
//!
//! A [`Field`] names an inclusive bit range and owns both directions of the conversion,
//! so a field's position is written once.
//!
//! Bits are numbered **MSB-first from byte 0 of the panel**: bit `i` is `byte i / 8`,
//! mask `1 << (7 - i % 8)`. Bits no field names are left untouched.

use std::convert::Infallible;
use std::marker::PhantomData;

use crate::fields::ControlKind;

/// A value that can live inside a packed bit field.
///
/// Implementors own their encoding and validation and know nothing about which panel or
/// offset holds them, so the same impl serves every field with that value.
pub trait Packed: Sized {
    /// Bits the widest value of this type occupies. Used to check statically that it
    /// fits its slot.
    const MAX_BITS: u32;

    /// Which panel control this value is, for a caller building an interface over the
    /// field registry.
    ///
    /// Defaults to [`ControlKind::Number`] — an integer nothing has been claimed about.
    /// A type that knows better says so, and a field gets the answer by choosing that
    /// type rather than by being annotated at its placement.
    const CONTROL: ControlKind = ControlKind::Number;

    /// Why a bit pattern is not a valid value. [`Infallible`] when every pattern is.
    type Error;

    /// Decode from the field's bits, already shifted down to bit 0 and masked.
    fn from_bits(bits: u64) -> Result<Self, Self::Error>;

    /// Encode to the field's bits, in the same shifted-down form.
    fn to_bits(&self) -> u64;
}

/// Number of bits needed to represent `max`.
pub const fn bits_for(max: u64) -> u32 {
    64 - max.leading_zeros()
}

impl Packed for bool {
    const MAX_BITS: u32 = 1;
    const CONTROL: ControlKind = ControlKind::Toggle;
    type Error = Infallible;

    fn from_bits(bits: u64) -> Result<Self, Infallible> {
        Ok(bits != 0)
    }

    fn to_bits(&self) -> u64 {
        *self as u64
    }
}

macro_rules! impl_packed_uint {
    ($($t:ty),* $(,)?) => { $(
        impl Packed for $t {
            const MAX_BITS: u32 = <$t>::BITS;
            type Error = Infallible;

            fn from_bits(bits: u64) -> Result<Self, Infallible> {
                Ok(bits as $t)
            }

            fn to_bits(&self) -> u64 {
                *self as u64
            }
        }
    )* };
}

impl_packed_uint!(u8, u16, u32, u64);

/// Read bits `lo..=hi` of `raw`, MSB-first, shifted down to bit 0.
const fn extract(raw: &[u8], lo: u32, hi: u32) -> u64 {
    let mut bits = 0;
    let mut i = lo;
    while i <= hi {
        bits = (bits << 1) | ((raw[(i / 8) as usize] >> (7 - i % 8)) & 1) as u64;
        i += 1;
    }
    bits
}

/// Replace bits `lo..=hi` of `raw` with the low `hi - lo + 1` bits of `bits`, leaving
/// every other bit alone.
fn splice(raw: &mut [u8], lo: u32, hi: u32, bits: u64) {
    for (n, i) in (lo..=hi).enumerate() {
        let mask = 1u8 << (7 - i % 8);
        let set = (bits >> (hi - lo - n as u32)) & 1 != 0;
        let byte = &mut raw[(i / 8) as usize];
        *byte = if set { *byte | mask } else { *byte & !mask };
    }
}

/// One value packed into bits `LO..=HI` of a panel, inclusive, MSB-first from byte 0.
///
/// Never instantiated — it names a position plus a conversion, used as
/// `MyField::get(&raw)` / `MyField::set(&mut raw, v)`.
pub struct Field<T, const LO: u32, const HI: u32>(PhantomData<fn() -> T>);

/// Compile-time check that a field lies inside the panel it is applied to.
struct SpanFits<const N: usize, const HI: u32>;

impl<const N: usize, const HI: u32> SpanFits<N, HI> {
    const OK: () = assert!((HI as usize) < 8 * N, "bit field extends past the panel");
}

impl<T: Packed, const LO: u32, const HI: u32> Field<T, LO, HI> {
    /// Width of the field in bits.
    pub const WIDTH: u32 = {
        assert!(HI >= LO, "a bit range must not end before it starts");
        HI - LO + 1
    };

    /// The field's mask, shifted down to bit 0.
    pub const MASK: u64 = if Self::WIDTH == 64 {
        u64::MAX
    } else {
        (1u64 << Self::WIDTH) - 1
    };

    /// Compile-time check that every value of `T` fits. Forced by [`Self::set`].
    const FITS: () = assert!(
        T::MAX_BITS <= HI - LO + 1,
        "this type can hold values wider than the field; give this field a type that \
         carries its range",
    );

    /// Decode the field out of `raw`.
    pub fn get<const N: usize>(raw: &[u8; N]) -> Result<T, T::Error> {
        let () = SpanFits::<N, HI>::OK;
        T::from_bits(extract(raw, LO, HI))
    }

    /// Write `value`, leaving every other bit of `raw` as it was.
    ///
    /// Only compiles when no value of `T` can overrun the field: a `u8` in a 7-bit slot
    /// is a compile error.
    pub fn set<const N: usize>(raw: &mut [u8; N], value: T) {
        let () = Self::FITS;
        let () = SpanFits::<N, HI>::OK;
        splice(raw, LO, HI, value.to_bits());
    }
}

impl<T: Packed<Error = Infallible>, const LO: u32, const HI: u32> Field<T, LO, HI> {
    /// Decode, when every bit pattern is a valid value.
    pub fn read<const N: usize>(raw: &[u8; N]) -> T {
        match Self::get(raw) {
            Ok(value) => value,
            Err(never) => match never {},
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A `BITS`-wide value. `set` only takes a type that cannot overrun its slot, so a
    /// test that writes has to name a width — a bare `u8` in a nibble does not compile.
    #[derive(Debug, PartialEq, Eq)]
    struct Small<const BITS: u32>(u8);

    impl<const BITS: u32> Packed for Small<BITS> {
        const MAX_BITS: u32 = BITS;
        type Error = Infallible;

        fn from_bits(bits: u64) -> Result<Self, Infallible> {
            Ok(Small(bits as u8))
        }

        fn to_bits(&self) -> u64 {
            self.0 as u64
        }
    }

    // Over a two-byte panel: `0xabcd` is bits 0..=15.
    type Nibble = Field<Small<4>, 4, 7>;
    type Byte = Field<u8, 8, 15>;
    type Flag = Field<bool, 11, 11>;

    #[test]
    fn a_field_reads_only_its_own_bits() {
        assert_eq!(Nibble::read(&[0xab, 0xcd]), Small(0xb));
        assert_eq!(Byte::read(&[0xab, 0xcd]), 0xcd);
        assert!(Flag::read(&[0x00, 0x10]));
        assert!(!Flag::read(&[0xff, 0xef]));
    }

    #[test]
    fn a_write_disturbs_no_other_bit() {
        let mut raw = [0xab, 0xcd];
        Nibble::set(&mut raw, Small(0x3));
        assert_eq!(raw, [0xa3, 0xcd]);

        let mut raw = [0b1010_1010];
        Field::<bool, 3, 3>::set(&mut raw, true);
        assert_eq!(raw, [0b1011_1010]);
        Field::<bool, 3, 3>::set(&mut raw, false);
        assert_eq!(raw, [0b1010_1010]);
    }

    /// A range crossing a byte boundary is an ordinary field: MSB-first indexing has no
    /// boundary in it to cross.
    #[test]
    fn a_field_may_span_bytes() {
        // `equalizer_freq_gain`'s shape: the low three bits of one byte and the high
        // four of the next.
        type Spanning = Field<Small<7>, 5, 11>;
        assert_eq!(Spanning::WIDTH, 7);
        assert_eq!(
            Spanning::read(&[0b0000_0101, 0b1101_0000]),
            Small(0b101_1101)
        );
        assert_eq!(Spanning::read(&[0, 0]), Small(0));

        let mut raw = [0b1111_1000, 0b0000_1111];
        Spanning::set(&mut raw, Small(0b101_1101));
        assert_eq!(raw, [0b1111_1101, 0b1101_1111]);
        assert_eq!(Spanning::read(&raw), Small(0b101_1101));
    }

    #[test]
    fn widths_and_masks_come_from_the_range_alone() {
        assert_eq!(Flag::WIDTH, 1);
        assert_eq!(Nibble::WIDTH, 4);
        assert_eq!(Nibble::MASK, 0xf);
        assert_eq!(Field::<u64, 0, 63>::MASK, u64::MAX);
    }

    #[test]
    fn bits_for_counts_what_a_value_needs() {
        assert_eq!(bits_for(0), 0);
        assert_eq!(bits_for(1), 1);
        assert_eq!(bits_for(12), 4);
        assert_eq!(bits_for(13), 4);
        assert_eq!(bits_for(127), 7);
        assert_eq!(bits_for(128), 8);
    }
}
