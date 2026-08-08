//! Body layouts as data.
//!
//! `#[bitbody]` generates an implementation of [`BodyLayout`] alongside the codec,
//! so a body's bit map exists once in the source and is readable at runtime — for
//! generated documentation, for `nord inspect`, for anything that wants to answer
//! "which bits does this field own" without re-stating the layout. Nested bodies
//! chain to their own layouts, so the whole map is one recursive walk.

/// One field's placement: an inclusive bit range, MSB-first from byte 0 of the
/// body that declares it. For the file offset a hex dump shows, add the enclosing
/// placements and the container's body start — `0x2c` on a type-1 file, `0x18` on
/// a type-0.
#[derive(Clone)]
pub struct LayoutField {
    /// The field's registry path within its body — the field's own name. A walker
    /// prefixes nested children with this path and a dot.
    pub path: &'static str,
    /// The field's Rust type, as written.
    pub ty: &'static str,
    pub lo: u32,
    pub hi: u32,
    /// The nested body's own layout, for an `#[at]` field; `None` for a leaf.
    pub nested: Option<fn() -> &'static [LayoutField]>,
}

/// A structure whose bit map is declared once, by `#[bitbody]`.
pub trait BodyLayout {
    /// Every placed field, in declaration order. Bits no field claims are
    /// preserved by the codec but have no entry here — there is no name to
    /// report them under.
    fn layout() -> &'static [LayoutField];
}

impl std::fmt::Debug for LayoutField {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if self.nested.is_some() {
            write!(
                f,
                "{} bytes {:#04x}..{:#04x} ({})",
                self.path,
                self.lo / 8,
                (self.hi + 1) / 8,
                self.ty,
            )
        } else {
            write!(
                f,
                "{} bits {}..={} ({})",
                self.path, self.lo, self.hi, self.ty
            )
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cbin::{self, Cbin, Header};
    use nord_bits_derive::bitbody;
    use std::io::Cursor;

    /// A nested body: one flag, the rest of its two bytes unclaimed.
    #[bitbody(2)]
    #[derive(Default)]
    struct Inner {
        #[bits(0..=0)]
        pub flag: bool,
        #[bits(4..=11)]
        pub level: u8,
    }

    /// A body exercising both placements: a private leaf word, a nested body,
    /// and a public leaf, with unclaimed bits in between.
    #[bitbody(6)]
    struct Outer {
        #[bits(0..=15)]
        word: u16,

        #[at(0x02..0x04)]
        pub inner: Inner,

        #[bits(40..=47)]
        pub level: u8,
    }

    fn body() -> Outer {
        let mut b = Outer::try_from([0xab, 0xcd, 0x0f, 0xf0, 0xff, 0x00]).unwrap();
        b.word = 0x0102;
        b.inner.level = 0x55;
        b.level = 7;
        b
    }

    /// Both placement kinds serve both directions, and unclaimed bits ride along
    /// at every level.
    #[test]
    fn the_codec_is_the_declaration() {
        let raw = <[u8; 6]>::from(&body());
        // word rewritten; inner: flag clear (bit 0 of 0x0f), level 0x55 over bits
        // 4..=11, inner's unclaimed bits 1..=3 kept from 0x0f; byte 4 unclaimed
        // at the outer level, kept verbatim; the outer leaf rewritten.
        assert_eq!(raw, [0x01, 0x02, 0x05, 0x50, 0xff, 0x07]);
        let back = Outer::try_from(raw).unwrap();
        assert_eq!(back.word, 0x0102);
        assert_eq!(back.inner.level, 0x55);
        assert_eq!(back.level, 7);
    }

    /// The generated `Body` impl carries a bitbody through the container whole,
    /// both generations.
    #[test]
    fn a_bitbody_rides_the_container() {
        for generation in [cbin::Generation::V1, cbin::Generation::V0] {
            let mut header = Header::new("tstb", (2, 5), 7);
            header.generation = generation;
            let file = Cbin {
                header,
                body: body(),
            };
            let mut bytes = Cursor::new(Vec::new());
            file.write_to(&mut bytes).unwrap();
            let mut bytes = Cursor::new(bytes.into_inner());
            let back: Cbin<Outer> = cbin::read(&mut bytes, "tstb").unwrap();
            assert_eq!(back.header.slot(), (2, 5));
            assert_eq!(<[u8; 6]>::from(&back.body), <[u8; 6]>::from(&body()));
        }
    }

    /// Paths: a nested field prefixes its children with its own name, a leaf
    /// registers under its bare name, and private fields stay unregistered.
    #[test]
    fn paths_recurse_through_nested_bodies() {
        let b = body();
        let paths: Vec<String> = b.fields().into_iter().map(|f| f.path).collect();
        assert_eq!(paths, ["inner.flag", "inner.level", "level"]);

        let mut b = body();
        b.set_field("inner.level", "3").unwrap();
        assert_eq!(b.inner.level, 3);
        // ⚠️ `level` and `inner.level` are different fields: the bare name is the
        // outer leaf, and nothing about a nested body's child reaches it.
        b.set_field("level", "9").unwrap();
        assert_eq!(b.level, 9);
        assert_eq!(b.inner.level, 3);
        assert!(b.set_field("word", "1").is_err(), "private is not a path");
    }

    /// The layout publishes every placement — including the unregistered word —
    /// and a nested entry chains to the nested body's own layout.
    #[test]
    fn the_layout_is_readable_as_data() {
        let fields = Outer::layout();
        let rendered: Vec<String> = fields.iter().map(|f| format!("{f:?}")).collect();
        assert_eq!(
            rendered,
            [
                "word bits 0..=15 (u16)",
                "inner bytes 0x02..0x04 (Inner)",
                "level bits 40..=47 (u8)",
            ],
        );

        let nested = fields[1].nested.expect("inner is nested");
        let rendered: Vec<String> = nested().iter().map(|f| format!("{f:?}")).collect();
        assert_eq!(
            rendered,
            ["flag bits 0..=0 (bool)", "level bits 4..=11 (u8)"]
        );
    }
}
