//! The `sty` section — the instrument's default sound preset.
//!
//! Three schemas, one role. `sty` holds the preset the instrument loads with:
//! filter, envelope and velocity response, and at v4 a three-band EQ. It is not
//! a structural descriptor — a zone's layers, strokes and key range are all in
//! `map`, and `sty` is byte-identical across stack depth.
//!
//! No byte of one schema is a function of any byte of another over the vendor
//! pool's instruments that appear in more than one generation, so no value here
//! translates between generations. One *offset* does: the dynamics enable is at
//! `+3` in both v2 and v4, the single field the rewrite left where it was. v3
//! puts it at `+4` and scales it onto the block's 0..127 grid.
//!
//! What a project reaches differs by schema. v2 exposes the category's velocity
//! response, because the editor's loader installs a preset chosen by the
//! instrument's category and the encoder writes it through; the wide schemas
//! ignore the category entirely and hold a constant block plus the dynamics
//! group. The EQ is read-only in every schema: the editor bakes both the zone
//! and the instrument EQ into the audio and leaves [`StyV4::eq`] zero.
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

/// Within a v2 payload: how far velocity moves level, quantised to
/// [`VELOCITY_LEVELS`].
const V2_VELOCITY_TO_AMPLITUDE: usize = 4;

/// Within a v2 payload: how far velocity moves timbre, on the same scale.
const V2_VELOCITY_TO_TIMBRE: usize = 5;

/// Steps a v2 velocity depth takes. The project's own field has four; the byte
/// holds three, with the project's middle two both landing on 1.
pub const VELOCITY_LEVELS: u8 = 3;

/// The level [`StyV2::velocity_to_amplitude`] and [`StyV2::velocity_to_timbre`]
/// take for a `samplib_attrs` velocity depth.
///
/// The loader installs the depth from the instrument's category and never above
/// 3, so the top step is saturating rather than measured beyond that.
pub fn velocity_level(depth: u8) -> u8 {
    match depth {
        0 => 0,
        1 | 2 => 1,
        _ => VELOCITY_LEVELS - 1,
    }
}

/// The v2 preset: nine enum-quantised bytes.
///
/// The dynamics enable comes straight from the project. The two velocity depths
/// come from the preset the loader installs for the instrument's category,
/// which is the only route a project has to them — setting the fields directly
/// is undone on load. The remaining bytes are readable from the vendor pool and
/// not reachable at all: they hold the same value whatever the category and
/// whatever `samplib_attrs` says.
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
    /// curve itself is stored in no schema.
    pub fn dynamics_enabled(&self) -> bool {
        self.raw[V2_DYNAMICS_ENABLE] != 0
    }

    /// How far note velocity moves level, `0..VELOCITY_LEVELS`.
    pub fn velocity_to_amplitude(&self) -> u8 {
        self.raw[V2_VELOCITY_TO_AMPLITUDE]
    }

    /// How far note velocity moves timbre, `0..VELOCITY_LEVELS`.
    pub fn velocity_to_timbre(&self) -> u8 {
        self.raw[V2_VELOCITY_TO_TIMBRE]
    }
}

/// Within a v3 payload: the dynamics enable, on the block's 0..127 scale rather
/// than the flag v2 and v4 keep.
const V3_DYNAMICS_ENABLE: usize = 4;

/// Within a v3 payload: the dynamics response, one value where v4 holds one per
/// layer.
const V3_DYNAMICS_RESPONSE: usize = 12;

/// Within a v3 payload: the counterpart of [`V4_DYNAMICS_CURVE`]. No value of
/// it marks "no curve" the way [`DYNAMICS_CURVE_NONE`] does at v4.
const V3_DYNAMICS_CURVE: usize = 14;

/// The v3 preset: 24 bytes, of which the dynamics group is named.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StyV3 {
    pub raw: [u8; V3_LEN],
}

impl StyV3 {
    pub fn parse(payload: &[u8]) -> Result<StyV3, ParseError> {
        let raw = payload.try_into().map_err(|_| {
            ParseError::AssertFail(format!(
                "sty section is {} bytes, not the {V3_LEN} a v3 preset is",
                payload.len()
            ))
        })?;
        Ok(StyV3 { raw })
    }

    /// Whether the instrument plays through its category's dynamics curve.
    pub fn dynamics_enabled(&self) -> bool {
        self.raw[V3_DYNAMICS_ENABLE] != 0
    }

    /// Which dynamics curve the instrument loads with. Unlike v4 this schema
    /// offers no sentinel for "none", so the value is returned as it stands.
    pub fn dynamics_curve(&self) -> u8 {
        self.raw[V3_DYNAMICS_CURVE]
    }

    /// The dynamics response.
    ///
    /// ⚠️ `+16` holds the same control in a second representation and is a
    /// deterministic function of this byte across the pool, so a writer that
    /// changes one must restate the other.
    pub fn dynamics_response(&self) -> u8 {
        self.raw[V3_DYNAMICS_RESPONSE]
    }
}

/// Within a v4 payload: whether the category's dynamics curve is enabled — the
/// same offset the v2 schema puts it at, and the only byte the two share.
const V4_DYNAMICS_ENABLE: usize = 3;

/// Within a v4 payload: which dynamics curve, or [`DYNAMICS_CURVE_NONE`].
///
/// ⚠️ The project's own curve field selects nothing here: the editor writes 1
/// at both of that field's legal values whenever the dynamics enable is on.
const V4_DYNAMICS_CURVE: usize = 4;

/// The value [`V4_DYNAMICS_CURVE`] holds when no curve is selected. Over the
/// vendor pool's 557 v4 instruments this byte reads 6 exactly when the response
/// triple sits at its 127 ceiling, with no exception either way.
pub const DYNAMICS_CURVE_NONE: u8 = 6;

/// Within a v4 payload: the dynamics response, one value per layer.
const V4_DYNAMICS_RESPONSE: usize = 85;

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
const _: () = assert!(V4_DYNAMICS_RESPONSE + 3 <= V4_LEN);

/// The v4 preset: a block of 0..127 scalars, each control stored three times —
/// once per dynamics layer — behind an enable byte valued 0 or 127, plus
/// [`EQ_BANDS`] EQ bands.
///
/// The dynamics group and the EQ are named; the remaining scalars are
/// preserved verbatim. A project reaches the dynamics group and nothing else:
/// enabling the category's dynamics moves [`StyV4::dynamics_enabled`],
/// [`StyV4::dynamics_curve`] and [`StyV4::dynamics_response`] together, while
/// the whole `samplib_attrs` block, the instrument's category and both EQs
/// leave every byte alone. ⚠️ The EQs because the encoder bakes them into the
/// audio instead, which is why [`StyV4::eq`] reads zero on everything this
/// project can render and only vendor content fills it in.
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

    /// Whether the instrument plays through its category's dynamics curve.
    pub fn dynamics_enabled(&self) -> bool {
        self.raw[V4_DYNAMICS_ENABLE] != 0
    }

    /// Which dynamics curve the instrument loads with, `None` where none is
    /// selected. ⚠️ A project cannot choose between the curves the pool holds
    /// — the enable alone drives this byte off its sentinel.
    pub fn dynamics_curve(&self) -> Option<u8> {
        match self.raw[V4_DYNAMICS_CURVE] {
            DYNAMICS_CURVE_NONE => None,
            curve => Some(curve),
        }
    }

    /// The dynamics response, one 0..127 value per layer, in the order the pool
    /// holds them non-decreasing.
    ///
    /// Pinned to 127 in all three positions exactly while
    /// [`StyV4::dynamics_curve`] is `None`. ⚠️ The block's other triples sit
    /// behind enable bytes that only approximate the same relationship, so this
    /// is the one that is measured rather than assumed.
    pub fn dynamics_response(&self) -> [u8; 3] {
        std::array::from_fn(|i| self.raw[V4_DYNAMICS_RESPONSE + i])
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
    V3(StyV3),
    V4(StyV4),
}

impl Sty {
    /// Reads a wide-chain `sty` payload, refusing a version no specimen carries.
    pub fn parse_wide(version: u32, payload: &[u8]) -> Result<Sty, ParseError> {
        match version {
            VERSION_V3 => StyV3::parse(payload).map(Sty::V3),
            VERSION_V4 => StyV4::parse(payload).map(Sty::V4),
            v => Err(ParseError::AssertFail(format!(
                "sty section version {v} has no preset layout derived from a specimen"
            ))),
        }
    }

    pub fn raw(&self) -> &[u8] {
        match self {
            Sty::V2(s) => &s.raw,
            Sty::V3(s) => &s.raw,
            Sty::V4(s) => &s.raw,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn v2_reads_the_dynamics_enable_and_both_velocity_depths() {
        let off = StyV2::parse(&[0, 1, 0, 0, 1, 1, 0, 0, 0]).unwrap();
        let on = StyV2::parse(&[0, 1, 0, 1, 0, 2, 0, 0, 0]).unwrap();
        assert!(!off.dynamics_enabled());
        assert_eq!(off.velocity_to_amplitude(), 1);
        assert_eq!(off.velocity_to_timbre(), 1);
        assert!(on.dynamics_enabled());
        assert_eq!(on.velocity_to_amplitude(), 0);
        assert_eq!(on.velocity_to_timbre(), 2);
    }

    #[test]
    fn v3_reads_its_dynamics_group() {
        let mut raw = [0u8; V3_LEN];
        raw[V3_DYNAMICS_RESPONSE] = 127;
        raw[V3_DYNAMICS_CURVE] = 2;
        let off = Sty::parse_wide(VERSION_V3, &raw).unwrap();
        let Sty::V3(off) = off else {
            panic!("not a v3 preset");
        };
        assert!(!off.dynamics_enabled());
        assert_eq!(off.dynamics_curve(), 2);
        assert_eq!(off.dynamics_response(), 127);

        raw[V3_DYNAMICS_ENABLE] = 43;
        raw[V3_DYNAMICS_RESPONSE] = 74;
        raw[V3_DYNAMICS_CURVE] = 1;
        let on = StyV3::parse(&raw).unwrap();
        assert!(on.dynamics_enabled());
        assert_eq!(on.dynamics_curve(), 1);
        assert_eq!(on.dynamics_response(), 74);
    }

    #[test]
    fn a_velocity_depth_quantises_onto_three_levels() {
        assert_eq!(
            [0, 1, 2, 3].map(velocity_level),
            [0, 1, 1, VELOCITY_LEVELS - 1]
        );
        assert_eq!(velocity_level(u8::MAX), VELOCITY_LEVELS - 1);
    }

    #[test]
    fn a_short_v3_preset_is_refused() {
        assert!(StyV3::parse(&[0; V3_LEN - 1]).is_err());
        assert!(Sty::parse_wide(VERSION_V3, &[0; V3_LEN + 1]).is_err());
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
    fn v4_dynamics_reads_its_enable_curve_and_response() {
        let mut raw = [0u8; V4_LEN];
        raw[V4_DYNAMICS_CURVE] = DYNAMICS_CURVE_NONE;
        raw[V4_DYNAMICS_RESPONSE..][..3].fill(127);
        let off = StyV4::parse(&raw).unwrap();
        assert!(!off.dynamics_enabled());
        assert_eq!(off.dynamics_curve(), None);
        assert_eq!(off.dynamics_response(), [127; 3]);

        raw[V4_DYNAMICS_ENABLE] = 1;
        raw[V4_DYNAMICS_CURVE] = 1;
        raw[V4_DYNAMICS_RESPONSE..][..3].copy_from_slice(&[74, 82, 90]);
        let on = StyV4::parse(&raw).unwrap();
        assert!(on.dynamics_enabled());
        assert_eq!(on.dynamics_curve(), Some(1));
        assert_eq!(on.dynamics_response(), [74, 82, 90]);
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
