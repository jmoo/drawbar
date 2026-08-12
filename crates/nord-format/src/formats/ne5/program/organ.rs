//! The organ panel, `0x4e..=0x92`.
//!
//! 69 bytes of per-model, per-preset state: the instrument keeps the full drawbar and
//! vib/perc registration for every model and both presets, so switching model or preset
//! is lossless.
//!
//! Fields are storage; the methods below are meaning. Which block a model reads, whether
//! a Farfisa nibble counts as on, where the b3-bass bars really live — none of that is
//! expressible as a placement, so it lives in an accessor.

use crate::bits::Packed;
use crate::components::{PercSpeed, VibChorus};
use crate::error::ParseError;
use crate::types::RangedU8;
use nord_bits_derive::bitbody;

use std::fmt::{self, Debug, Display, Formatter};

/// Length of the organ panel block, 0x4e..=0x92.
const ORGAN_LEN: usize = 0x92 - 0x4d;

/// A drawbar position, `0..=8`, in the nibble it is stored in.
type Bar = RangedU8<{ Drawbars::MAX }>;

/// The Electro 5's four organ models. (B3-bass shares the B3 storage slots, so
/// it isn't a separate model here.)
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OrganModel {
    B3,
    Vox,
    Farfisa,
    Pipe,
}

/// The organ panel: drawbar and vib/perc registration for every model and
/// both presets, stored in full so switching either is lossless.
#[bitbody(69)]
pub struct OrganPanel {
    // ── B3, 0x4e..=0x64 ────────────────────────────────────────────────────────
    /// Shared across presets.
    #[bits(24..=26)]
    pub b3_vib: B3Vib,
    /// Percussion third harmonic, shared across presets.
    #[bits(27..=27)]
    pub b3_perc_third: bool,
    /// Shared across presets.
    #[bits(28..=29)]
    pub b3_perc_speed: B3PercSpeed,
    #[bits(41..=41)]
    pub b3_preset2_selected: bool,
    #[bits(56..=91)]
    pub b3_preset1_drawbars: Drawbars,
    #[bits(92..=92)]
    pub b3_preset1_vib: bool,
    #[bits(93..=93)]
    pub b3_preset1_perc: bool,
    /// First bass drawbar of **b3+bass preset 1** — see [`OrganPanel::b3_bass_drawbars`].
    /// It is not in the nine-nibble block, and the two nibbles it shadows there hold
    /// stale leftovers.
    #[bits(94..=97)]
    pub b3_bass_bar1: Bar,
    /// Second bass drawbar of b3+bass preset 1. The four bits after it are unused.
    #[bits(98..=101)]
    pub b3_bass_bar2: Bar,
    #[bits(112..=147)]
    pub b3_preset2_drawbars: Drawbars,
    #[bits(148..=148)]
    pub b3_preset2_vib: bool,
    #[bits(149..=149)]
    pub b3_preset2_perc: bool,
    // Bits 150..=157 mirror the preset-1 bass pair above for preset 2, but the
    // firmware neither reads nor writes them: injected values play nothing in
    // b3+bass preset 2 (the bass manual follows the preset-1 field) and survive
    // a panel store untouched. Confirmed on hardware. Fresh programs hold 8,8
    // there. Left unclaimed so the field rides through re-encode verbatim.

    // ── Vox, 0x65..=0x74 ───────────────────────────────────────────────────────
    /// Shared across presets.
    #[bits(168..=170)]
    pub vox_vib: VoxVib,
    #[bits(185..=185)]
    pub vox_preset2_selected: bool,
    #[bits(200..=235)]
    pub vox_preset1_drawbars: Drawbars,
    #[bits(236..=236)]
    pub vox_preset1_vib: bool,
    #[bits(248..=283)]
    pub vox_preset2_drawbars: Drawbars,
    #[bits(284..=284)]
    pub vox_preset2_vib: bool,

    // ── Farfisa, 0x75..=0x84 ───────────────────────────────────────────────────
    /// Shared across presets.
    #[bits(296..=298)]
    pub farfisa_vib: FarfisaVib,
    #[bits(313..=313)]
    pub farfisa_preset2_selected: bool,
    /// Stored as positions, read by the instrument as tabs — see
    /// [`OrganPanel::farfisa_tabs`].
    #[bits(328..=363)]
    pub farfisa_preset1_drawbars: Drawbars,
    #[bits(364..=364)]
    pub farfisa_preset1_vib: bool,
    #[bits(376..=411)]
    pub farfisa_preset2_drawbars: Drawbars,
    #[bits(412..=412)]
    pub farfisa_preset2_vib: bool,

    // ── Pipe, 0x85..=0x92. No vibrato, no percussion. ──────────────────────────
    // Bit 492 — where the other models keep their preset-1 vib-on — is set in
    // nearly every real program, but the vib button does not respond while Pipe
    // is selected, so the panel cannot reach it. Confirmed on hardware. It rides
    // through unclaimed like the preset-2 bass pair above.
    #[bits(441..=441)]
    pub pipe_preset2_selected: bool,
    #[bits(456..=491)]
    pub pipe_preset1_drawbars: Drawbars,
    #[bits(504..=539)]
    pub pipe_preset2_drawbars: Drawbars,
}

/// `[u8; 69]` has no `Default` — the std impls stop at 32 — so this one goes through the
/// decode, which every organ field is total over.
impl Default for OrganPanel {
    fn default() -> Self {
        OrganPanel::try_from([0; ORGAN_LEN]).expect("every organ field decodes totally")
    }
}

impl OrganPanel {
    /// The selected preset (1 or 2) for `model`.
    pub fn preset(&self, model: OrganModel) -> u8 {
        if *self.preset_selected(model) {
            2
        } else {
            1
        }
    }

    /// The nine drawbar positions (physical, 0..=8) stored for `model`'s `preset`. This
    /// is the on-disk value; per-model display transforms (Farfisa on/off, Vox's ignored
    /// 8th bar, B3-bass bass-bar remap) are not applied.
    pub fn drawbars(&self, model: OrganModel, preset: u8) -> [u8; 9] {
        self.drawbar_block(model, preset).positions()
    }

    /// The two bass drawbars of **B3-with-bass, preset 1** — the bass manual.
    ///
    /// These are *not* in the nine-nibble block. In b3+bass mode preset 1 is the bass
    /// manual (only bars 1–2 are live) and preset 2 is the ordinary B3; the bass
    /// registration sits in its own four-bit pair after the block, which is why it does
    /// not move when the drawbars do.
    ///
    /// ⚠️ Do **not** read bars 1–2 from [`Self::drawbars`] in this mode — those two
    /// nibbles hold stale leftovers, not zero and not the bass values.
    ///
    /// Confirmed on hardware — captures `1100_400000000` and `1100_040000000`.
    pub fn b3_bass_drawbars(&self) -> [u8; 2] {
        [self.b3_bass_bar1.as_u8(), self.b3_bass_bar2.as_u8()]
    }

    /// Farfisa drawbars as the instrument actually treats them: **on/off tabs**, not
    /// continuous positions.
    ///
    /// A stored nibble of **≥5 reads as on**, anything lower as off. Use this rather
    /// than [`Self::drawbars`] for Farfisa — the raw 0..=8 value is stored faithfully
    /// but has no meaning beyond which side of the threshold it falls.
    pub fn farfisa_tabs(&self, preset: u8) -> [bool; 9] {
        self.drawbars(OrganModel::Farfisa, preset)
            .map(|bar| bar >= 5)
    }

    /// Whether vibrato/chorus is on for `model`'s `preset`. Pipe has none.
    pub fn vib_on(&self, model: OrganModel, preset: u8) -> bool {
        self.vib_flag(model, preset).is_some_and(|on| *on)
    }

    /// The vibrato/chorus mode selected for `model` (shared across presets), or `None`
    /// for Pipe and for an index the model does not use. Each model offers a different
    /// subset of the six modes at a different index, so the stored value is only
    /// meaningful alongside the model.
    pub fn vib_type(&self, model: OrganModel) -> Option<VibChorus> {
        match model {
            OrganModel::B3 => self.b3_vib.get(),
            OrganModel::Vox => self.vox_vib.get(),
            OrganModel::Farfisa => self.farfisa_vib.get(),
            OrganModel::Pipe => None,
        }
    }

    /// Whether B3 percussion is on for `preset` (B3 only).
    pub fn b3_perc_on(&self, preset: u8) -> bool {
        if preset == 2 {
            self.b3_preset2_perc
        } else {
            self.b3_preset1_perc
        }
    }

    /// Whether B3 percussion uses the third harmonic (shared across presets).
    pub fn b3_perc_third(&self) -> bool {
        self.b3_perc_third
    }

    /// B3 percussion decay speed (shared across presets). The on-disk encoding is not
    /// monotonic — soft, fast and both store 2, 1 and 3.
    pub fn b3_perc_speed(&self) -> PercSpeed {
        self.b3_perc_speed
            .get()
            .expect("all four two-bit indices are named")
    }

    // ── writes ──────────────────────────────────────────────────────────────────

    /// Select `preset` (1 or 2) for `model`.
    pub fn set_preset(&mut self, model: OrganModel, preset: u8) {
        *self.preset_selected_mut(model) = preset == 2;
    }

    /// Store nine drawbar positions, `0..=8`. A higher one is refused rather than
    /// truncated, since two bars share a byte.
    pub fn set_drawbars(
        &mut self,
        model: OrganModel,
        preset: u8,
        bars: [u8; 9],
    ) -> Result<(), ParseError> {
        *self.drawbar_block_mut(model, preset) = Drawbars::new(bars)?;
        Ok(())
    }

    /// Set the Farfisa tabs: on stores `8`, off stores `0`. Any other stored value is
    /// lost — the instrument only reads which side of the ≥5 threshold it falls on, but
    /// the byte does change, so this will not round-trip a program you only meant to
    /// read.
    pub fn set_farfisa_tabs(&mut self, preset: u8, tabs: [bool; 9]) {
        let bars = tabs.map(|on| if on { Drawbars::MAX } else { 0 });
        self.set_drawbars(OrganModel::Farfisa, preset, bars)
            .expect("0 and 8 are both in range");
    }

    /// Turn vibrato/chorus on or off for `model`'s `preset`. No-op for Pipe, which has
    /// none.
    pub fn set_vib_on(&mut self, model: OrganModel, preset: u8, on: bool) {
        if let Some(flag) = self.vib_flag_mut(model, preset) {
            *flag = on;
        }
    }

    /// Select the vibrato/chorus mode for `model`, shared across presets. Fails for a
    /// mode the model does not offer; Pipe has none.
    pub fn set_vib_type(&mut self, model: OrganModel, vib: VibChorus) -> Result<(), ParseError> {
        let refuse = |why: &str| ParseError::OutOfBounds {
            value: format!("{vib:?}"),
            bound: format!("{model:?} {why}"),
        };
        match model {
            OrganModel::B3 => {
                self.b3_vib = B3Vib::select(vib).ok_or_else(|| refuse("does not offer it"))?
            }
            OrganModel::Vox => {
                self.vox_vib = VoxVib::select(vib).ok_or_else(|| refuse("does not offer it"))?
            }
            OrganModel::Farfisa => {
                self.farfisa_vib =
                    FarfisaVib::select(vib).ok_or_else(|| refuse("does not offer it"))?
            }
            OrganModel::Pipe => return Err(refuse("has no vibrato/chorus")),
        }
        Ok(())
    }

    /// Turn B3 percussion on or off for `preset`.
    pub fn set_b3_perc_on(&mut self, preset: u8, on: bool) {
        if preset == 2 {
            self.b3_preset2_perc = on;
        } else {
            self.b3_preset1_perc = on;
        }
    }

    /// Percussion third harmonic (shared across presets).
    pub fn set_b3_perc_third(&mut self, on: bool) {
        self.b3_perc_third = on;
    }

    /// Percussion decay speed (shared across presets). Note the encoding is not
    /// monotonic — see [`Self::b3_perc_speed`].
    pub fn set_b3_perc_speed(&mut self, speed: PercSpeed) {
        self.b3_perc_speed =
            B3PercSpeed::select(speed).expect("all four speeds have a two-bit index");
    }

    /// Set the two bass drawbars of b3+bass preset 1, `0..=8`.
    pub fn set_b3_bass_drawbars(&mut self, bars: [u8; 2]) -> Result<(), ParseError> {
        self.b3_bass_bar1 = bars[0].try_into()?;
        self.b3_bass_bar2 = bars[1].try_into()?;
        Ok(())
    }

    // ── which field a model and preset name ─────────────────────────────────────

    fn drawbar_block(&self, model: OrganModel, preset: u8) -> &Drawbars {
        match (model, preset == 2) {
            (OrganModel::B3, false) => &self.b3_preset1_drawbars,
            (OrganModel::B3, true) => &self.b3_preset2_drawbars,
            (OrganModel::Vox, false) => &self.vox_preset1_drawbars,
            (OrganModel::Vox, true) => &self.vox_preset2_drawbars,
            (OrganModel::Farfisa, false) => &self.farfisa_preset1_drawbars,
            (OrganModel::Farfisa, true) => &self.farfisa_preset2_drawbars,
            (OrganModel::Pipe, false) => &self.pipe_preset1_drawbars,
            (OrganModel::Pipe, true) => &self.pipe_preset2_drawbars,
        }
    }

    fn drawbar_block_mut(&mut self, model: OrganModel, preset: u8) -> &mut Drawbars {
        match (model, preset == 2) {
            (OrganModel::B3, false) => &mut self.b3_preset1_drawbars,
            (OrganModel::B3, true) => &mut self.b3_preset2_drawbars,
            (OrganModel::Vox, false) => &mut self.vox_preset1_drawbars,
            (OrganModel::Vox, true) => &mut self.vox_preset2_drawbars,
            (OrganModel::Farfisa, false) => &mut self.farfisa_preset1_drawbars,
            (OrganModel::Farfisa, true) => &mut self.farfisa_preset2_drawbars,
            (OrganModel::Pipe, false) => &mut self.pipe_preset1_drawbars,
            (OrganModel::Pipe, true) => &mut self.pipe_preset2_drawbars,
        }
    }

    fn preset_selected(&self, model: OrganModel) -> &bool {
        match model {
            OrganModel::B3 => &self.b3_preset2_selected,
            OrganModel::Vox => &self.vox_preset2_selected,
            OrganModel::Farfisa => &self.farfisa_preset2_selected,
            OrganModel::Pipe => &self.pipe_preset2_selected,
        }
    }

    fn preset_selected_mut(&mut self, model: OrganModel) -> &mut bool {
        match model {
            OrganModel::B3 => &mut self.b3_preset2_selected,
            OrganModel::Vox => &mut self.vox_preset2_selected,
            OrganModel::Farfisa => &mut self.farfisa_preset2_selected,
            OrganModel::Pipe => &mut self.pipe_preset2_selected,
        }
    }

    fn vib_flag(&self, model: OrganModel, preset: u8) -> Option<&bool> {
        Some(match (model, preset == 2) {
            (OrganModel::B3, false) => &self.b3_preset1_vib,
            (OrganModel::B3, true) => &self.b3_preset2_vib,
            (OrganModel::Vox, false) => &self.vox_preset1_vib,
            (OrganModel::Vox, true) => &self.vox_preset2_vib,
            (OrganModel::Farfisa, false) => &self.farfisa_preset1_vib,
            (OrganModel::Farfisa, true) => &self.farfisa_preset2_vib,
            (OrganModel::Pipe, _) => return None,
        })
    }

    fn vib_flag_mut(&mut self, model: OrganModel, preset: u8) -> Option<&mut bool> {
        Some(match (model, preset == 2) {
            (OrganModel::B3, false) => &mut self.b3_preset1_vib,
            (OrganModel::B3, true) => &mut self.b3_preset2_vib,
            (OrganModel::Vox, false) => &mut self.vox_preset1_vib,
            (OrganModel::Vox, true) => &mut self.vox_preset2_vib,
            (OrganModel::Farfisa, false) => &mut self.farfisa_preset1_vib,
            (OrganModel::Farfisa, true) => &mut self.farfisa_preset2_vib,
            (OrganModel::Pipe, _) => return None,
        })
    }
}
/// Declare a model's index into a shared enumeration.
///
/// The slot holds an index, not the value: which modes an organ offers, and in what
/// order, differs per model. An index the model does not use decodes to `None` rather
/// than being coerced to a neighbor, and round-trips whatever it held.
macro_rules! model_index {
    ($(#[$meta:meta])* $name:ident, $bits:expr, $of:ty, [$($variant:ident),+ $(,)?]) => {
        $(#[$meta])*
        #[derive(Copy, Clone, Default, PartialEq, Eq)]
        pub struct $name(u8);

        impl $name {
            /// What this model offers at the stored index, or `None` if it offers
            /// nothing there.
            pub fn get(&self) -> Option<$of> {
                Self::TABLE.get(self.0 as usize).copied()
            }

            /// The index this model stores `value` at, or `None` if it does not offer it.
            pub fn select(value: $of) -> Option<Self> {
                Self::TABLE.iter().position(|&v| v == value).map(|i| $name(i as u8))
            }

            /// The stored index, named or not.
            pub fn raw(&self) -> u8 {
                self.0
            }

            const TABLE: &'static [$of] = &[$(<$of>::$variant),+];
        }

        impl Packed for $name {
            const MAX_BITS: u32 = $bits;
            const CONTROL: $crate::fields::ControlKind = $crate::fields::ControlKind::Selector;
            type Error = ::core::convert::Infallible;

            fn from_bits(bits: u64) -> Result<Self, Self::Error> {
                Ok($name(bits as u8))
            }

            fn to_bits(&self) -> u64 {
                self.0 as u64
            }
        }

        impl Debug for $name {
            fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
                match self.get() {
                    Some(value) => write!(f, "{value:?}"),
                    None => write!(f, "unknown ({})", self.0),
                }
            }
        }

        impl Display for $name {
            fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
                write!(f, "{self:?}")
            }
        }
    };
}

model_index!(
    /// The B3's vibrato/chorus selection. It offers all six modes.
    B3Vib, 3, VibChorus, [V1, C1, V2, C2, V3, C3]
);

model_index!(
    /// The Vox's vibrato selection: three depths, no chorus.
    VoxVib, 3, VibChorus, [V1, V2, V3]
);

model_index!(
    /// The Farfisa's vibrato/chorus selection.
    FarfisaVib, 3, VibChorus, [V1, V2, C2, C3]
);

model_index!(
    /// The B3's percussion decay speed. The stored order is not the panel's: soft, fast
    /// and both are 2, 1 and 3.
    B3PercSpeed, 2, PercSpeed, [Off, Fast, Soft, Both]
);

/// Nine drawbar positions, nibble-packed high-nibble first — the on-disk form every
/// organ model shares.
///
/// Positions are physical, `0..=8`, stored identity. Decoding is total: a nibble outside
/// that range is preserved rather than refused. [`Drawbars::new`] refuses one on the way
/// in.
///
/// Per-model display transforms — Farfisa's on/off threshold, Vox's ignored 8th bar,
/// the b3-bass remap — are not applied here.
#[derive(Copy, Clone, Default, PartialEq, Eq)]
pub struct Drawbars([u8; 9]);

impl Drawbars {
    /// The number of bars, and so the nibbles this occupies.
    pub const BARS: usize = 9;
    /// The highest position a drawbar can be pulled to.
    pub const MAX: u8 = 8;

    /// Nine positions, each `0..=8`.
    pub fn new(bars: [u8; Self::BARS]) -> Result<Self, ParseError> {
        match bars.iter().find(|&&b| b > Self::MAX) {
            Some(&bad) => Err(ParseError::OutOfBounds {
                value: format!("{bad}"),
                bound: format!("0..={}", Self::MAX),
            }),
            None => Ok(Drawbars(bars)),
        }
    }

    /// The nine positions as stored.
    pub fn positions(&self) -> [u8; Self::BARS] {
        self.0
    }
}

impl Packed for Drawbars {
    const MAX_BITS: u32 = 4 * Drawbars::BARS as u32;
    const CONTROL: crate::fields::ControlKind = crate::fields::ControlKind::Drawbar;
    type Error = ::core::convert::Infallible;

    fn from_bits(bits: u64) -> Result<Self, Self::Error> {
        Ok(Drawbars(std::array::from_fn(|n| {
            let shift = Self::MAX_BITS - 4 * (n as u32 + 1);
            ((bits >> shift) & 0xf) as u8
        })))
    }

    fn to_bits(&self) -> u64 {
        self.0
            .iter()
            .fold(0, |bits, &bar| (bits << 4) | (bar as u64 & 0xf))
    }
}

impl Debug for Drawbars {
    /// The positions alone, so a panel's `Debug` reads as the array it is.
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        write!(f, "{:?}", self.0)
    }
}

impl Display for Drawbars {
    /// `888000000` — the form the corpus filenames use.
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        for bar in self.0 {
            write!(f, "{bar}")?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Panel-relative index of the byte at absolute Electro 5 file offset `abs`.
    const fn org(abs: usize) -> usize {
        abs - 0x4e
    }

    /// Build an organ panel from `(absolute offset, byte)` pairs; everything else 0.
    fn panel(bytes: &[(usize, u8)]) -> OrganPanel {
        let mut raw = [0u8; ORGAN_LEN];
        for &(at, b) in bytes {
            raw[org(at)] = b;
        }
        OrganPanel::try_from(raw).expect("every organ field decodes totally")
    }

    /// The bass drawbars of b3+bass preset 1 live outside the nine-nibble block, in a
    /// 12-bit field across `0x59`'s low nibble and `0x5a`. Values are from real
    /// specimens: `1100_400000000`, `1100_040000000`, `1100_87gfedcba`.
    #[test]
    fn b3_bass_drawbars_decode_from_the_packed_field() {
        // bar1 = 4, bar2 = 0  ->  field 0x100
        assert_eq!(
            panel(&[(0x59, 0x01), (0x5a, 0x00)]).b3_bass_drawbars(),
            [4, 0]
        );
        // bar1 = 0, bar2 = 4  ->  field 0x010
        assert_eq!(
            panel(&[(0x59, 0x00), (0x5a, 0x10)]).b3_bass_drawbars(),
            [0, 4]
        );
        // bar1 = 8, bar2 = 7  ->  field 0x21c
        assert_eq!(
            panel(&[(0x59, 0x02), (0x5a, 0x1c)]).b3_bass_drawbars(),
            [8, 7]
        );
        // bar1 = 8, bar2 = 8  ->  field 0x220
        assert_eq!(
            panel(&[(0x59, 0x02), (0x5a, 0x20)]).b3_bass_drawbars(),
            [8, 8]
        );
        // all down
        assert_eq!(panel(&[]).b3_bass_drawbars(), [0, 0]);
    }

    /// `0x59` is shared. Its high nibble is bar 9 of the main block and bits 3/2 are
    /// vibrato/percussion — none of which may leak into the bass drawbars. Regression
    /// guard for the placement.
    #[test]
    fn b3_bass_drawbars_ignore_the_flags_sharing_that_byte() {
        // Same field as `1100_88iiiiiii`: bar 9 = 8 in the high nibble, bars still 8,8.
        assert_eq!(
            panel(&[(0x59, 0x82), (0x5a, 0x20)]).b3_bass_drawbars(),
            [8, 8]
        );
        // Vibrato (0x08) and percussion (0x04) on must not disturb the reading.
        assert_eq!(
            panel(&[(0x59, 0x0e), (0x5a, 0x20)]).b3_bass_drawbars(),
            [8, 8]
        );
        assert_eq!(
            panel(&[(0x59, 0xfe), (0x5a, 0x20)]).b3_bass_drawbars(),
            [8, 8]
        );
    }

    /// Farfisa's drawbars are on/off tabs: >= 5 is on. The raw nibble is still stored
    /// faithfully, it just carries no meaning beyond the threshold.
    #[test]
    fn farfisa_drawbars_are_on_off_tabs() {
        // 0x77 is Farfisa preset 1: nine nibbles, high-nibble first.
        // bars 0..8 = 8,7,6,5,4,3,2,1,0 -> on for >= 5.
        let p = panel(&[
            (0x77, 0x87),
            (0x78, 0x65),
            (0x79, 0x43),
            (0x7a, 0x21),
            (0x7b, 0x00),
        ]);
        assert_eq!(
            p.drawbars(OrganModel::Farfisa, 1),
            [8, 7, 6, 5, 4, 3, 2, 1, 0]
        );
        assert_eq!(
            p.farfisa_tabs(1),
            [true, true, true, true, false, false, false, false, false]
        );
        // The threshold sits between 4 and 5.
        let edge = panel(&[
            (0x77, 0x54),
            (0x78, 0x00),
            (0x79, 0x00),
            (0x7a, 0x00),
            (0x7b, 0x00),
        ]);
        assert!(edge.farfisa_tabs(1)[0], "5 should read as on");
        assert!(!edge.farfisa_tabs(1)[1], "4 should read as off");
    }

    /// Preset 2 reads from its own block, so the two presets never alias.
    #[test]
    fn farfisa_presets_are_independent() {
        let p = panel(&[(0x77, 0x80), (0x7d, 0x08)]);
        assert!(p.farfisa_tabs(1)[0]);
        assert!(!p.farfisa_tabs(1)[1]);
        assert!(!p.farfisa_tabs(2)[0]);
        assert!(p.farfisa_tabs(2)[1]);
    }

    /// Every model and preset reads its own nine nibbles and writes them back where it
    /// found them — no placement lands on a neighbor's block.
    #[test]
    fn every_model_and_preset_has_its_own_block() {
        for (n, (model, preset)) in [
            OrganModel::B3,
            OrganModel::Vox,
            OrganModel::Farfisa,
            OrganModel::Pipe,
        ]
        .into_iter()
        .flat_map(|m| [(m, 1u8), (m, 2)])
        .enumerate()
        {
            let bars = [(n as u8) % 9; 9];
            let mut p = OrganPanel::default();
            p.set_drawbars(model, preset, bars).unwrap();

            let raw = <[u8; ORGAN_LEN]>::from(&p);
            let back = OrganPanel::try_from(raw).unwrap();
            assert_eq!(
                back.drawbars(model, preset),
                bars,
                "{model:?} preset {preset}"
            );

            let others: Vec<_> = [
                OrganModel::B3,
                OrganModel::Vox,
                OrganModel::Farfisa,
                OrganModel::Pipe,
            ]
            .into_iter()
            .flat_map(|m| [(m, 1u8), (m, 2)])
            .filter(|&(m, p)| !(m == model && p == preset))
            .filter(|&(m, p)| back.drawbars(m, p) != [0; 9])
            .collect();
            assert!(
                others.is_empty(),
                "{model:?} preset {preset} also wrote {others:?}"
            );
        }
    }

    /// A mode the model does not offer is refused rather than stored at some free index.
    #[test]
    fn a_model_only_accepts_the_modes_it_has() {
        let mut p = OrganPanel::default();
        p.set_vib_type(OrganModel::Vox, VibChorus::V3).unwrap();
        assert_eq!(p.vib_type(OrganModel::Vox), Some(VibChorus::V3));
        assert!(p.set_vib_type(OrganModel::Vox, VibChorus::C1).is_err());
        assert!(p.set_vib_type(OrganModel::Pipe, VibChorus::V1).is_err());

        p.set_vib_type(OrganModel::B3, VibChorus::C3).unwrap();
        assert_eq!(p.vib_type(OrganModel::B3), Some(VibChorus::C3));
        // The Vox selection is at the same index in its own table and must not move.
        assert_eq!(p.vib_type(OrganModel::Vox), Some(VibChorus::V3));
    }

    /// The two speed bits are not in panel order.
    #[test]
    fn perc_speed_stores_soft_fast_and_both_as_2_1_and_3() {
        let bits = |speed| {
            let mut p = OrganPanel::default();
            p.set_b3_perc_speed(speed);
            (<[u8; ORGAN_LEN]>::from(&p)[org(0x51)] >> 2) & 0b11
        };
        assert_eq!(bits(PercSpeed::Off), 0);
        assert_eq!(bits(PercSpeed::Fast), 1);
        assert_eq!(bits(PercSpeed::Soft), 2);
        assert_eq!(bits(PercSpeed::Both), 3);

        let mut p = OrganPanel::default();
        p.set_b3_perc_speed(PercSpeed::Soft);
        assert_eq!(p.b3_perc_speed(), PercSpeed::Soft);
    }
}
