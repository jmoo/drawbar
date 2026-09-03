//! The `sty` section — the instrument's default sound preset.
//!
//! Three schemas, one role. `sty` holds the preset the instrument loads with:
//! filter, envelope and velocity response, and at v4 a three-band EQ. It is not
//! a structural descriptor — a zone's layers, strokes and key range are all in
//! `map`, and `sty` is byte-identical across stack depth.
//!
//! No byte of one schema is a function of any byte of another over the vendor
//! pool's instruments that appear in more than one generation, so nothing here
//! translates between generations.
//!
//! Inferred from specimens; not confirmed on hardware.

use crate::error::ParseError;

/// Schema version of a v2 `sty` section.
pub const VERSION_V2: u8 = 5;

/// Schema version of a v3 `sty` section.
pub const VERSION_V3: u32 = 7;

/// Schema version of a v4 `sty` section — both [`V4_LEN`] and [`V4_LEN_LONG`]
/// carry it.
pub const VERSION_V4: u32 = 17;

pub const V2_LEN: usize = 9;
pub const V3_LEN: usize = 24;

/// The v4 payload as body versions 410 and 413 write it.
pub const V4_LEN: usize = 92;

/// The v4 payload with its trailing three scalar triples, on body versions 414
/// and 420. ⚠️ Both widths carry section version [`VERSION_V4`], and body
/// version 412 occurs at each — so a reader must size `sty` from the section
/// length and never from its version.
pub const V4_LEN_LONG: usize = 108;

/// Within a v2 payload: whether the category's dynamics curve is enabled.
const V2_DYNAMICS_ENABLE: usize = 3;

/// The v2 preset: nine enum-quantised bytes.
///
/// Only [`StyV2::dynamics_enabled`] is named. The editor writes a constant here
/// — the project's category and its whole `samplib_attrs` block leave every
/// other byte alone — so the rest is readable from the vendor pool and not
/// reachable from a project.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StyV2 {
    pub raw: [u8; V2_LEN],
}

impl StyV2 {
    pub fn parse(payload: &[u8]) -> Result<StyV2, ParseError> {
        let raw = payload.try_into().map_err(|_| {
            ParseError::AssertFail(format!(
                "sty section is {} bytes, not the {V2_LEN} a v2 preset is",
                payload.len()
            ))
        })?;
        Ok(StyV2 { raw })
    }

    /// Whether the instrument plays through its category's dynamics curve. The
    /// curve itself is not stored; only this enable reaches the file.
    pub fn dynamics_enabled(&self) -> bool {
        self.raw[V2_DYNAMICS_ENABLE] != 0
    }
}

/// One band of the v4 preset's EQ.
///
/// Gain and Q are held in whole tens across the pool; the divisor each uses is
/// not established, so both are the stored integers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EqBand {
    /// Centre frequency in Hz.
    pub frequency: u16,
    pub gain: i32,
    pub q: i32,
}

/// Bytes per EQ band. The third band's stride runs into the scalar triple that
/// follows it, so only [`EQ_BAND_BODY`] of the last one is the band's own.
const EQ_BAND_STRIDE: usize = 12;

/// The part of a band the fields occupy; the rest of the stride is zero.
const EQ_BAND_BODY: usize = 10;

/// Where the first band sits in a v4 payload.
const EQ_AT: usize = 45;

pub const EQ_BANDS: usize = 3;

const _: () = assert!(EQ_AT + (EQ_BANDS - 1) * EQ_BAND_STRIDE + EQ_BAND_BODY <= V4_LEN);

/// The v4 preset: a block of 0..127 scalars, each control stored three times —
/// once per dynamics layer — behind an enable byte valued 0 or 127, plus
/// [`EQ_BANDS`] EQ bands.
///
/// Only the EQ is named. The scalars are preserved verbatim.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StyV4 {
    pub raw: Vec<u8>,
}

impl StyV4 {
    pub fn parse(payload: &[u8]) -> Result<StyV4, ParseError> {
        if payload.len() != V4_LEN && payload.len() != V4_LEN_LONG {
            return Err(ParseError::AssertFail(format!(
                "sty section is {} bytes, not the {V4_LEN} or {V4_LEN_LONG} a v4 preset is",
                payload.len()
            )));
        }
        Ok(StyV4 {
            raw: payload.to_vec(),
        })
    }

    pub fn eq(&self) -> [EqBand; EQ_BANDS] {
        std::array::from_fn(|i| {
            let b = &self.raw[EQ_AT + i * EQ_BAND_STRIDE..][..EQ_BAND_BODY];
            EqBand {
                frequency: u16::from_be_bytes([b[0], b[1]]),
                gain: i32::from_be_bytes([b[2], b[3], b[4], b[5]]),
                q: i32::from_be_bytes([b[6], b[7], b[8], b[9]]),
            }
        })
    }
}

/// A `sty` section read under the schema its own length and version select.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Sty {
    V2(StyV2),
    /// 24 bytes. No field is named: the schema shares no byte with the one
    /// before it or the one after.
    V3(Box<[u8; V3_LEN]>),
    V4(StyV4),
}

impl Sty {
    /// Reads a wide-chain `sty` payload, refusing a version no specimen carries.
    pub fn parse_wide(version: u32, payload: &[u8]) -> Result<Sty, ParseError> {
        match version {
            VERSION_V3 => payload
                .try_into()
                .map(|raw| Sty::V3(Box::new(raw)))
                .map_err(|_| {
                    ParseError::AssertFail(format!(
                        "sty section is {} bytes, not the {V3_LEN} a v3 preset is",
                        payload.len()
                    ))
                }),
            VERSION_V4 => StyV4::parse(payload).map(Sty::V4),
            v => Err(ParseError::AssertFail(format!(
                "sty section version {v} has no preset layout derived from a specimen"
            ))),
        }
    }

    pub fn raw(&self) -> &[u8] {
        match self {
            Sty::V2(s) => &s.raw,
            Sty::V3(s) => s.as_slice(),
            Sty::V4(s) => &s.raw,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn v2_names_only_the_dynamics_enable() {
        let off = StyV2::parse(&[0, 1, 0, 0, 1, 1, 0, 0, 0]).unwrap();
        let on = StyV2::parse(&[0, 1, 0, 1, 1, 1, 0, 0, 0]).unwrap();
        assert!(!off.dynamics_enabled());
        assert!(on.dynamics_enabled());
    }

    #[test]
    fn a_short_v2_preset_is_refused() {
        assert!(StyV2::parse(&[0; V2_LEN - 1]).is_err());
    }

    #[test]
    fn both_v4_widths_are_accepted_under_one_version() {
        assert!(Sty::parse_wide(VERSION_V4, &[0; V4_LEN]).is_ok());
        assert!(Sty::parse_wide(VERSION_V4, &[0; V4_LEN_LONG]).is_ok());
        assert!(Sty::parse_wide(VERSION_V4, &[0; V4_LEN + 1]).is_err());
    }

    #[test]
    fn an_unknown_wide_version_is_refused_rather_than_guessed() {
        assert!(Sty::parse_wide(VERSION_V3 + 1, &[0; V3_LEN]).is_err());
    }

    #[test]
    fn v4_eq_bands_read_frequency_gain_and_q() {
        let mut raw = [0u8; V4_LEN];
        for (i, (f, g, q)) in [(3000u16, -50i32, 10i32), (4000, -50, 10), (5000, 0, 0)]
            .into_iter()
            .enumerate()
        {
            let at = EQ_AT + i * EQ_BAND_STRIDE;
            raw[at..at + 2].copy_from_slice(&f.to_be_bytes());
            raw[at + 2..at + 6].copy_from_slice(&g.to_be_bytes());
            raw[at + 6..at + 10].copy_from_slice(&q.to_be_bytes());
        }
        let bands = StyV4::parse(&raw).unwrap().eq();
        assert_eq!(bands[0].frequency, 3000);
        assert_eq!(bands[1].gain, -50);
        assert_eq!(
            bands[2],
            EqBand {
                frequency: 5000,
                gain: 0,
                q: 0
            }
        );
    }
}
