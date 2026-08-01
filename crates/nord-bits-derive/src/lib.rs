//! `#[bitpanel]` — declare a bit-packed panel as an ordinary Rust struct.
//!
//! ```ignore
//! /// The attribute's argument is the panel's length in bytes.
//! #[bitpanel(7)]
//! #[derive(Default)]
//! pub struct CenterPanel {
//!     #[bits(0..=2)]
//!     pub left_part: Instrument,
//!     #[bits(35..=41)]
//!     pub gain: Level,
//!     /// Contiguous over the bytes, whatever it looks like in a hex dump.
//!     #[bits(61..=67)]
//!     pub equalizer_freq_gain: Level,
//! }
//! ```
//!
//! Bits are numbered MSB-first from byte 0 of the panel, as `nord-format`'s `bits` module
//! describes. Generates both directions of the conversion between the panel and its
//! `[u8; N]`, a `Debug` over the decoded fields, and a `Panel` impl that lists them,
//! describes them, and sets them by name. Bits no field claims are preserved, and are
//! reported in the panel's generated doc; two ranges may not overlap.
//!
//! `Panel::field_values` and `Panel::field_specs` are emitted in declaration order and
//! describe the same fields, so callers may zip them positionally.
//!
//! Encoding is total. Every field's type has to carry its own range, so the fit is proven
//! at compile time: a raw `u8` in a 7-bit slot fails to compile.
//!
//! Only usable inside `nord-format`: generated code names `crate::bits`, `crate::error`
//! and `crate::panel`.

use proc_macro::TokenStream;
use proc_macro2::TokenStream as TokenStream2;
use quote::quote;
use syn::spanned::Spanned;
use syn::{Expr, ExprRange, Ident, ItemStruct, Lit, LitInt, RangeLimits};

/// One field's `LO..=HI` placement, MSB-first from byte 0 of the panel.
struct Bits {
    lo: u32,
    hi: u32,
}

impl syn::parse::Parse for Bits {
    fn parse(input: syn::parse::ParseStream) -> syn::Result<Self> {
        let range: ExprRange = input.parse()?;
        if !input.is_empty() {
            return Err(input.error("a field has one `LO..=HI` placement"));
        }

        if !matches!(range.limits, RangeLimits::Closed(_)) {
            return Err(syn::Error::new_spanned(
                &range,
                "bit ranges are inclusive: write `0..=2`, not `0..2`",
            ));
        }

        let lo = literal(range.start.as_deref(), &range, "low bit")?;
        let hi = literal(range.end.as_deref(), &range, "high bit")?;

        if hi < lo {
            return Err(syn::Error::new_spanned(
                &range,
                format!("bit range ends before it starts: `{lo}..={hi}`"),
            ));
        }

        Ok(Bits { lo, hi })
    }
}

fn literal(expr: Option<&Expr>, at: &ExprRange, what: &str) -> syn::Result<u32> {
    match expr {
        Some(Expr::Lit(lit)) => match &lit.lit {
            Lit::Int(int) => int.base10_parse(),
            other => Err(syn::Error::new_spanned(
                other,
                format!("{what} must be an integer"),
            )),
        },
        _ => Err(syn::Error::new_spanned(
            at,
            format!("{what} is missing: a bit range needs both ends"),
        )),
    }
}

/// `Bits 24..=27 (byte 0x03, bits 7..4).` — the range as a hex dump reads it.
fn breakdown(lo: u32, hi: u32) -> String {
    let mut parts = Vec::new();
    let mut at = lo;
    while at <= hi {
        let byte = at / 8;
        let last = hi.min(byte * 8 + 7);
        let (first_bit, last_bit) = (7 - at % 8, 7 - last % 8);
        parts.push(if first_bit == last_bit {
            format!("byte {byte:#04x}, bit {first_bit}")
        } else {
            format!("byte {byte:#04x}, bits {first_bit}..{last_bit}")
        });
        at = last + 1;
    }
    format!("Bits {lo}..={hi} ({}).", parts.join("; "))
}

/// The ranges of `0..bits` no field claims.
fn unclaimed(claimed: &[(u32, u32)], bits: u32) -> Vec<(u32, u32)> {
    let mut sorted = claimed.to_vec();
    sorted.sort_unstable();

    let mut gaps = Vec::new();
    let mut next = 0;
    for (lo, hi) in sorted {
        if lo > next {
            gaps.push((next, lo - 1));
        }
        next = hi + 1;
    }
    if next < bits {
        gaps.push((next, bits - 1));
    }
    gaps
}

#[proc_macro_attribute]
pub fn bitpanel(attr: TokenStream, item: TokenStream) -> TokenStream {
    match expand(attr.into(), item.into()) {
        Ok(tokens) => tokens.into(),
        Err(e) => e.to_compile_error().into(),
    }
}

fn expand(attr: TokenStream2, item: TokenStream2) -> syn::Result<TokenStream2> {
    let len: LitInt = syn::parse2(attr.clone()).map_err(|_| {
        syn::Error::new(
            attr.span(),
            "expected the panel's length in bytes — e.g. `#[bitpanel(7)]`",
        )
    })?;
    let bytes: usize = len.base10_parse()?;
    if bytes == 0 {
        return Err(syn::Error::new_spanned(&len, "a panel needs a byte"));
    }
    let span_bits = (bytes * 8) as u32;

    let panel: ItemStruct = syn::parse2(item)?;
    let name = &panel.ident;
    let vis = &panel.vis;
    let attrs = &panel.attrs;

    let syn::Fields::Named(named) = &panel.fields else {
        return Err(syn::Error::new_spanned(
            &panel,
            "a panel must be a struct with named fields",
        ));
    };

    let mut claimed: Vec<(u32, u32, &Ident)> = Vec::new();

    let mut decode = Vec::new();
    let mut encode = Vec::new();
    let mut debug = Vec::new();
    let mut fields = Vec::new();
    let mut values = Vec::new();
    let mut specs = Vec::new();
    let mut setters = Vec::new();

    for field in &named.named {
        let ident = field.ident.as_ref().expect("named fields");
        let ty = &field.ty;

        let attr = field
            .attrs
            .iter()
            .find(|a| a.path().is_ident("bits"))
            .ok_or_else(|| {
                syn::Error::new_spanned(
                    field,
                    "every field of a panel needs a `#[bits(LO..=HI)]` placement",
                )
            })?;
        let Bits { lo, hi } = attr.parse_args()?;

        if hi >= span_bits {
            return Err(syn::Error::new_spanned(
                attr,
                format!(
                    "bit {hi} is past the end of a {bytes}-byte panel, whose last bit is {}",
                    span_bits - 1,
                ),
            ));
        }
        if let Some(&(olo, ohi, other)) = claimed.iter().find(|&&(l, h, _)| lo <= h && l <= hi) {
            return Err(syn::Error::new_spanned(
                attr,
                format!("bits {lo}..={hi} overlap `{other}`, at {olo}..={ohi}"),
            ));
        }
        claimed.push((lo, hi, ident));

        // Keep everything but the placement, which has served its purpose.
        let kept: Vec<_> = field
            .attrs
            .iter()
            .filter(|a| !a.path().is_ident("bits"))
            .collect();
        let placement_doc = breakdown(lo, hi);
        let field_vis = &field.vis;
        fields.push(quote! {
            #(#kept)*
            #[doc = ""]
            #[doc = #placement_doc]
            #field_vis #ident: #ty
        });

        let f = quote! { crate::bits::Field::<#ty, #lo, #hi> };
        decode.push(quote! { #ident: #f::get(&raw)? });
        encode.push(quote! { #f::set(&mut raw, p.#ident); });

        // The field's bits with no type applied — see `crate::panel::FieldValue`.
        let placement = format!("{lo}..={hi}");
        values.push(quote! {
            crate::panel::FieldValue {
                name: stringify!(#ident),
                placement: #placement,
                raw: crate::bits::Field::<u64, #lo, #hi>::read(&self.raw),
                bits: <#ty as crate::bits::Packed>::to_bits(&self.#ident),
                value: ::std::format!("{:?}", &self.#ident),
            }
        });

        let width = hi - lo + 1;
        specs.push(quote! {
            crate::panel::FieldSpec {
                name: stringify!(#ident),
                placement: #placement,
                width: #width,
                legal: || crate::panel::legal_values::<#ty>(#width),
            }
        });

        // The parse is the type's, so a value it cannot hold fails here rather than
        // being clamped into the slot.
        setters.push(quote! {
            stringify!(#ident) => {
                self.#ident = crate::panel::parse_field::<#ty>(#width, value)
                    .map_err(|e| e.at(stringify!(#ident)))?;
                Ok(())
            }
        });

        debug.push(quote! { .field(stringify!(#ident), &self.#ident) });
    }

    let gaps = unclaimed(
        &claimed.iter().map(|&(l, h, _)| (l, h)).collect::<Vec<_>>(),
        span_bits,
    );
    let gap_doc = if gaps.is_empty() {
        format!("Every one of the panel's {span_bits} bits is named.")
    } else {
        format!(
            "Unclaimed bits, kept verbatim through a re-encode: {}.",
            gaps.iter()
                .map(|&(l, h)| if l == h {
                    format!("{l}")
                } else {
                    format!("{l}..={h}")
                })
                .collect::<Vec<_>>()
                .join(", "),
        )
    };

    Ok(quote! {
        #(#attrs)*
        #[doc = ""]
        #[doc = #gap_doc]
        #vis struct #name {
            /// The bytes this panel was decoded from, so bits no field claims survive a
            /// re-encode. Named fields take precedence on write.
            raw: [u8; #bytes],
            #(#fields,)*
        }

        impl ::core::convert::TryFrom<[u8; #bytes]> for #name {
            type Error = crate::error::ParseError;

            fn try_from(raw: [u8; #bytes]) -> ::core::result::Result<Self, Self::Error> {
                Ok(#name { raw, #(#decode,)* })
            }
        }

        impl ::core::convert::From<&#name> for [u8; #bytes] {
            fn from(p: &#name) -> Self {
                let mut raw = p.raw;
                #(#encode)*
                raw
            }
        }

        impl crate::panel::Panel for #name {
            const NAME: &'static str = stringify!(#name);

            fn field_values(&self) -> ::std::vec::Vec<crate::panel::FieldValue> {
                ::std::vec![#(#values,)*]
            }

            fn field_specs() -> ::std::vec::Vec<crate::panel::FieldSpec> {
                ::std::vec![#(#specs,)*]
            }

            fn set_field(
                &mut self,
                name: &str,
                value: &str,
            ) -> ::core::result::Result<(), crate::panel::FieldError> {
                match name {
                    #(#setters)*
                    other => Err(crate::panel::FieldError::UnknownField {
                        panel: <Self as crate::panel::Panel>::NAME,
                        name: other.to_string(),
                    }),
                }
            }
        }

        /// The decoded values; the backing bytes are not printed.
        impl ::core::fmt::Debug for #name {
            fn fmt(&self, f: &mut ::core::fmt::Formatter<'_>) -> ::core::fmt::Result {
                f.debug_struct(stringify!(#name))
                    #(#debug)*
                    .finish()
            }
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_breakdown_reads_like_a_hex_dump() {
        assert_eq!(breakdown(24, 27), "Bits 24..=27 (byte 0x03, bits 7..4).");
        assert_eq!(breakdown(23, 23), "Bits 23..=23 (byte 0x02, bit 0).");
        assert_eq!(
            breakdown(61, 67),
            "Bits 61..=67 (byte 0x07, bits 2..0; byte 0x08, bits 7..4).",
        );
    }

    #[test]
    fn unclaimed_finds_the_holes_and_both_ends() {
        assert_eq!(unclaimed(&[(0, 2), (5, 9)], 16), vec![(3, 4), (10, 15)]);
        assert_eq!(unclaimed(&[(0, 7)], 8), vec![]);
        assert_eq!(unclaimed(&[(4, 7)], 8), vec![(0, 3)]);
    }
}
