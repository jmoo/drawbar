//! The `meta` section — the wide chain's own length, written down.
//!
//! Twelve bytes at section version 1, closing every v3 and v4 body:
//!
//! ```text
//! +0..1   0x0002, constant
//! +2..5   u32 BE: bytes of chain ahead of this section's header
//! +6..11  zero
//! ```
//!
//! The length counts from the `NSMP` tag to the `meta` header, so it is the
//! whole body but for `meta` itself — the only place a wide file states its own
//! size. ⚠️ A writer that resizes any section has to restate it.
//!
//! Inferred from specimens; not confirmed on hardware.

use crate::error::ParseError;

/// Schema version of a `meta` section.
pub const VERSION: u32 = 1;

pub const LEN: usize = 12;

/// Within the payload: the length of the chain ahead of `meta`.
const CHAIN_LEN_AT: usize = 2;

/// A `meta` section: the chain length it states.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Meta {
    /// Bytes of chain from the `NSMP` tag up to the `meta` section header.
    pub chain_len: u32,
}

impl Meta {
    pub fn parse(version: u32, payload: &[u8]) -> Result<Meta, ParseError> {
        if version != VERSION {
            return Err(ParseError::AssertFail(format!(
                "meta section version {version} has no layout derived from a specimen"
            )));
        }
        let chain_len = payload
            .get(CHAIN_LEN_AT..CHAIN_LEN_AT + 4)
            .ok_or_else(|| {
                ParseError::AssertFail(format!(
                    "meta section is {} bytes, not the {LEN} a chain length needs",
                    payload.len()
                ))
            })?
            .try_into()
            .map(u32::from_be_bytes)
            .expect("a four-byte slice");
        Ok(Meta { chain_len })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn meta_reads_the_chain_length_it_states() {
        let mut raw = [0u8; LEN];
        raw[0..2].copy_from_slice(&2u16.to_be_bytes());
        raw[CHAIN_LEN_AT..CHAIN_LEN_AT + 4].copy_from_slice(&6140u32.to_be_bytes());
        assert_eq!(Meta::parse(VERSION, &raw).unwrap().chain_len, 6140);
    }

    #[test]
    fn a_short_or_unknown_meta_is_refused_rather_than_guessed() {
        assert!(Meta::parse(VERSION, &[0; LEN - 8]).is_err());
        assert!(Meta::parse(VERSION + 1, &[0; LEN]).is_err());
    }
}
