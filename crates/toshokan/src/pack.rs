//! The binary encoding of what an install keeps for itself: entries in memory
//! and the cached view in its local root. Never written to the folder.
//!
//! An unsigned integer is a LEB128 varint; an id is its 16 bytes, big-endian; text
//! is its length then its UTF-8 bytes; a list is its length then its members; an
//! option is a byte, 0 for none or 1 then the value; a struct is its fields in
//! order; an enum is a byte naming the variant, then its fields.

use std::cell::RefCell;
use std::collections::{BTreeMap, HashMap};

use crate::ids::{EntityId, EntryHash, Hlc, Identity, Nonce, WriterId};
use crate::path::RelPath;
use crate::schema::Raw;

/// Why bytes do not unpack: truncated, or holding what no packing writes.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Bad(pub &'static str);

pub type Unpacked<T> = Result<T, Bad>;

/// Bytes being unpacked, from the front.
pub struct In<'a> {
    bytes: &'a [u8],
}

impl<'a> In<'a> {
    pub fn new(bytes: &'a [u8]) -> Self {
        Self { bytes }
    }

    pub fn take(&mut self, len: usize) -> Unpacked<&'a [u8]> {
        if len > self.bytes.len() {
            return Err(Bad("truncated"));
        }
        let (taken, rest) = self.bytes.split_at(len);
        self.bytes = rest;
        Ok(taken)
    }

    pub fn byte(&mut self) -> Unpacked<u8> {
        Ok(self.take(1)?[0])
    }

    /// A length, which cannot exceed what is left to read.
    pub fn len(&mut self) -> Unpacked<usize> {
        let len = u64::unpack(self)?;
        match usize::try_from(len) {
            Ok(len) if len <= self.bytes.len() => Ok(len),
            _ => Err(Bad("a length past the end")),
        }
    }

    pub fn str(&mut self) -> Unpacked<&'a str> {
        let len = self.len()?;
        std::str::from_utf8(self.take(len)?).map_err(|_| Bad("text that is not UTF-8"))
    }

    /// How many bytes are left to read.
    pub fn left(&self) -> usize {
        self.bytes.len()
    }

    /// Fails unless every byte was read.
    pub fn end(self) -> Unpacked<()> {
        match self.bytes.is_empty() {
            true => Ok(()),
            false => Err(Bad("bytes after the end")),
        }
    }
}

pub trait Pack {
    fn pack(&self, out: &mut Vec<u8>);
}

pub trait Unpack: Sized {
    fn unpack(input: &mut In<'_>) -> Unpacked<Self>;
}

/// `value` packed alone.
#[cfg(test)]
pub fn packed<T: Pack + ?Sized>(value: &T) -> Vec<u8> {
    let mut out = Vec::new();
    value.pack(&mut out);
    out
}

/// The value `bytes` hold, every byte of them.
#[cfg(test)]
pub fn unpacked<T: Unpack>(bytes: &[u8]) -> Unpacked<T> {
    let mut input = In::new(bytes);
    let value = T::unpack(&mut input)?;
    input.end()?;
    Ok(value)
}

/// A variant's byte that names no variant.
pub fn bad_variant<T>() -> Unpacked<T> {
    Err(Bad("a variant no packing writes"))
}

impl Pack for u8 {
    fn pack(&self, out: &mut Vec<u8>) {
        out.push(*self);
    }
}

impl Unpack for u8 {
    fn unpack(input: &mut In<'_>) -> Unpacked<Self> {
        input.byte()
    }
}

impl Pack for u64 {
    fn pack(&self, out: &mut Vec<u8>) {
        let mut value = *self;
        while value >= 0x80 {
            out.push(value as u8 | 0x80);
            value >>= 7;
        }
        out.push(value as u8);
    }
}

impl Unpack for u64 {
    fn unpack(input: &mut In<'_>) -> Unpacked<Self> {
        let mut value = 0u64;
        for shift in (0..64).step_by(7) {
            let byte = input.byte()?;
            let bits = u64::from(byte & 0x7f);
            if shift == 63 && bits > 1 {
                return Err(Bad("a varint past 64 bits"));
            }
            value |= bits << shift;
            if byte & 0x80 == 0 {
                return Ok(value);
            }
        }
        Err(Bad("a varint past 64 bits"))
    }
}

impl Pack for u32 {
    fn pack(&self, out: &mut Vec<u8>) {
        u64::from(*self).pack(out);
    }
}

impl Unpack for u32 {
    fn unpack(input: &mut In<'_>) -> Unpacked<Self> {
        u32::try_from(u64::unpack(input)?).map_err(|_| Bad("a number past 32 bits"))
    }
}

impl Pack for usize {
    fn pack(&self, out: &mut Vec<u8>) {
        (*self as u64).pack(out);
    }
}

impl Pack for bool {
    fn pack(&self, out: &mut Vec<u8>) {
        out.push(u8::from(*self));
    }
}

impl Unpack for bool {
    fn unpack(input: &mut In<'_>) -> Unpacked<Self> {
        match input.byte()? {
            0 => Ok(false),
            1 => Ok(true),
            _ => bad_variant(),
        }
    }
}

macro_rules! pack_id {
    ($($name:ident),*) => {$(
        impl Pack for $name {
            fn pack(&self, out: &mut Vec<u8>) {
                out.extend_from_slice(&self.to_bytes());
            }
        }

        impl Unpack for $name {
            fn unpack(input: &mut In<'_>) -> Unpacked<Self> {
                let bytes: [u8; 16] = input.take(16)?.try_into().expect("16 bytes");
                Ok(Self::from_u128(u128::from_be_bytes(bytes)))
            }
        }
    )*};
}

pack_id!(EntityId, EntryHash, Identity, Nonce, WriterId);

impl Pack for Hlc {
    fn pack(&self, out: &mut Vec<u8>) {
        self.wall_ms.pack(out);
        self.counter.pack(out);
    }
}

impl Unpack for Hlc {
    fn unpack(input: &mut In<'_>) -> Unpacked<Self> {
        Ok(Self {
            wall_ms: u64::unpack(input)?,
            counter: u32::unpack(input)?,
        })
    }
}

impl Pack for str {
    fn pack(&self, out: &mut Vec<u8>) {
        self.len().pack(out);
        out.extend_from_slice(self.as_bytes());
    }
}

impl Pack for String {
    fn pack(&self, out: &mut Vec<u8>) {
        self.as_str().pack(out);
    }
}

impl Unpack for String {
    fn unpack(input: &mut In<'_>) -> Unpacked<Self> {
        input.str().map(str::to_owned)
    }
}

impl Pack for RelPath {
    fn pack(&self, out: &mut Vec<u8>) {
        self.as_str().pack(out);
    }
}

impl Unpack for RelPath {
    fn unpack(input: &mut In<'_>) -> Unpacked<Self> {
        RelPath::new(input.str()?).map_err(|_| Bad("a path no packing writes"))
    }
}

impl Pack for Raw {
    fn pack(&self, out: &mut Vec<u8>) {
        self.as_str().pack(out);
    }
}

/// Values a thread unpacked lately, by text, so that a value held many times,
/// such as a tag, is checked once and its text shared.
const SHARED_VALUES: usize = 4096;

/// Only values this short are shared.
const SHARED_LEN: usize = 64;

impl Unpack for Raw {
    fn unpack(input: &mut In<'_>) -> Unpacked<Self> {
        thread_local! {
            static SHARED: RefCell<HashMap<Box<str>, Raw>> = RefCell::new(HashMap::new());
        }
        let text = input.str()?;
        let read = || Raw::new(text).map_err(|_| Bad("JSON no packing writes"));
        if text.len() > SHARED_LEN {
            return read();
        }
        SHARED.with(|shared| {
            let mut shared = shared.borrow_mut();
            if let Some(raw) = shared.get(text) {
                return Ok(raw.clone());
            }
            let raw = read()?;
            if shared.len() >= SHARED_VALUES {
                shared.clear();
            }
            shared.insert(text.into(), raw.clone());
            Ok(raw)
        })
    }
}

impl<T: Pack> Pack for [T] {
    fn pack(&self, out: &mut Vec<u8>) {
        self.len().pack(out);
        for member in self {
            member.pack(out);
        }
    }
}

impl<T: Pack> Pack for Vec<T> {
    fn pack(&self, out: &mut Vec<u8>) {
        self.as_slice().pack(out);
    }
}

/// How many members a list makes room for before it reads them, so that a
/// length naming more than a record could hold reserves no more than this.
pub(crate) const RESERVED: usize = 1 << 16;

/// A list whose members `member` unpacks.
pub fn list<'a, T>(
    input: &mut In<'a>,
    mut member: impl FnMut(&mut In<'a>) -> Unpacked<T>,
) -> Unpacked<Vec<T>> {
    let len = input.len()?;
    let mut list = Vec::with_capacity(len.min(RESERVED));
    for _ in 0..len {
        list.push(member(input)?);
    }
    list.shrink_to_fit();
    Ok(list)
}

impl<T: Unpack> Unpack for Vec<T> {
    fn unpack(input: &mut In<'_>) -> Unpacked<Self> {
        list(input, T::unpack)
    }
}

impl<T: Pack> Pack for Option<T> {
    fn pack(&self, out: &mut Vec<u8>) {
        match self {
            None => out.push(0),
            Some(value) => {
                out.push(1);
                value.pack(out);
            }
        }
    }
}

impl<T: Unpack> Unpack for Option<T> {
    fn unpack(input: &mut In<'_>) -> Unpacked<Self> {
        match input.byte()? {
            0 => Ok(None),
            1 => T::unpack(input).map(Some),
            _ => bad_variant(),
        }
    }
}

impl Pack for () {
    fn pack(&self, _: &mut Vec<u8>) {}
}

impl Unpack for () {
    fn unpack(_: &mut In<'_>) -> Unpacked<Self> {
        Ok(())
    }
}

impl<A: Pack, B: Pack> Pack for (A, B) {
    fn pack(&self, out: &mut Vec<u8>) {
        self.0.pack(out);
        self.1.pack(out);
    }
}

impl<A: Unpack, B: Unpack> Unpack for (A, B) {
    fn unpack(input: &mut In<'_>) -> Unpacked<Self> {
        Ok((A::unpack(input)?, B::unpack(input)?))
    }
}

impl<K: Pack, V: Pack> Pack for BTreeMap<K, V> {
    fn pack(&self, out: &mut Vec<u8>) {
        self.len().pack(out);
        for pair in self {
            pair.0.pack(out);
            pair.1.pack(out);
        }
    }
}

/// Refuses keys out of order, which no packing writes.
impl<K: Unpack + Ord, V: Unpack> Unpack for BTreeMap<K, V> {
    fn unpack(input: &mut In<'_>) -> Unpacked<Self> {
        let pairs = Vec::<(K, V)>::unpack(input)?;
        if !pairs.windows(2).all(|pair| pair[0].0 < pair[1].0) {
            return Err(Bad("keys out of order"));
        }
        Ok(pairs.into_iter().collect())
    }
}

impl<T: Pack + ?Sized> Pack for &T {
    fn pack(&self, out: &mut Vec<u8>) {
        (**self).pack(out);
    }
}

/// Packs and unpacks a struct as its fields, in the order named.
macro_rules! pack_struct {
    ($name:ident { $($field:ident),* $(,)? }) => {
        impl $crate::pack::Pack for $name {
            fn pack(&self, out: &mut Vec<u8>) {
                $($crate::pack::Pack::pack(&self.$field, out);)*
            }
        }

        impl $crate::pack::Unpack for $name {
            fn unpack(input: &mut $crate::pack::In<'_>) -> $crate::pack::Unpacked<Self> {
                Ok(Self {
                    $($field: $crate::pack::Unpack::unpack(input)?,)*
                })
            }
        }
    };
}

pub(crate) use pack_struct;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_varint_holds_every_width() {
        for value in [0, 1, 127, 128, 300, u64::from(u32::MAX), u64::MAX] {
            let bytes = packed(&value);
            assert_eq!(unpacked::<u64>(&bytes), Ok(value), "{value}");
        }
        assert_eq!(packed(&300u64), [0xac, 0x02]);
        let eleven = [0xff; 10]
            .iter()
            .chain(&[0x01])
            .copied()
            .collect::<Vec<u8>>();
        assert!(unpacked::<u64>(&eleven).is_err());
        assert!(unpacked::<u64>(
            &[0xff; 9]
                .iter()
                .chain(&[0x02])
                .copied()
                .collect::<Vec<u8>>()
        )
        .is_err());
    }

    #[test]
    fn a_length_past_the_end_is_refused_before_anything_is_held() {
        let mut bytes = packed(&u64::MAX);
        bytes.push(b'x');
        assert_eq!(
            unpacked::<String>(&bytes),
            Err(Bad("a length past the end"))
        );
        assert_eq!(
            unpacked::<Vec<u64>>(&bytes),
            Err(Bad("a length past the end"))
        );
        assert_eq!(unpacked::<String>(&packed("ok")), Ok("ok".to_owned()));
        assert!(unpacked::<String>(&[1, 0xff]).is_err(), "not UTF-8");
        assert_eq!(
            unpacked::<bool>(&[2]),
            Err(Bad("a variant no packing writes"))
        );
        assert_eq!(unpacked::<bool>(&[]), Err(Bad("truncated")));
    }
}
