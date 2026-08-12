//! Typed values shared across models.
//!
//! A component owns its encoding, its validation and its `Display`, and knows nothing
//! about which panel or offset holds it — so the same impl serves every `#[bits(...)]`
//! placement of that value.
//!
//! Only what more than one model uses belongs here. A component with a single consumer
//! lives beside that consumer, in the panel module that names it.

use std::fmt::{self, Debug, Display, Formatter};

use crate::bits::{bits_for, Packed};
use crate::error::ParseError;
use crate::fields::{ControlKind, Unit};
use crate::types::RangedI8;

/// Octave shift. The range and the storage bias are the model's business, so each
/// names its own alias.
pub type OctaveShift<const OFFSET: u8, const MIN: i8, const MAX: i8> = RangedI8<OFFSET, MIN, MAX>;

/// Half-step transposition. As with [`OctaveShift`], the model fixes the parameters.
pub type Transpose<const OFFSET: u8, const MIN: i8, const MAX: i8> = RangedI8<OFFSET, MIN, MAX>;

/// A continuous control on the panel's own `0..10` — level, compression, gain, tone.
///
/// `FULL` is the stored value the panel reads as 10, and so also fixes the slot's width.
/// Use the [`Level`] and [`Level6`] aliases rather than naming it — nearly every one of
/// these is the seven-bit `0..=127`, and the Stage 4 puts a few of the same knobs in six
/// bits.
///
/// ⚠️ **A `0..=127` slot is not automatically one of these.** An envelope stage reads in
/// milliseconds, a filter cutoff in hertz, an equalizer band in decibels either side of a
/// centre — see [`Time`], [`Frequency`], [`Rate`] and [`Bipolar`]. Typing one of those as a
/// `Level` makes the panel reading wrong rather than merely absent.
#[derive(Copy, Clone, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct LevelOf<const FULL: u8> {
    inner: u8,
}

impl<const FULL: u8> LevelOf<FULL> {
    pub const MAX: u8 = FULL;

    pub fn new(value: u8) -> Result<Self, ParseError> {
        value.try_into()
    }

    /// The stored value, `0..=FULL`.
    pub fn as_u8(&self) -> u8 {
        self.inner
    }

    /// The panel's 0..10 reading.
    ///
    /// Confirmed on hardware: reverb wet reads `43` in the file and the panel shows
    /// 3.4, and `43 / 127 * 10 = 3.39`.
    pub fn as_panel(&self) -> f32 {
        f32::from(self.inner) / f32::from(FULL) * 10.0
    }
}

impl<const FULL: u8> TryFrom<u8> for LevelOf<FULL> {
    type Error = ParseError;

    fn try_from(value: u8) -> Result<Self, ParseError> {
        if value > FULL {
            return Err(ParseError::OutOfBounds {
                value: format!("{value}"),
                bound: format!("0..={FULL}"),
            });
        }
        Ok(LevelOf { inner: value })
    }
}

impl<const FULL: u8> Packed for LevelOf<FULL> {
    const MAX_BITS: u32 = bits_for(FULL as u64);
    const CONTROL: ControlKind = ControlKind::Knob(Unit::Panel10);
    type Error = ParseError;

    fn from_bits(bits: u64) -> Result<Self, ParseError> {
        (bits as u8).try_into()
    }

    fn to_bits(&self) -> u64 {
        self.inner as u64
    }
}

impl<const FULL: u8> Display for LevelOf<FULL> {
    /// Stored byte and panel reading: `96 (7.6)`.
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        write!(f, "{} ({:.1})", self.inner, self.as_panel())
    }
}

impl<const FULL: u8> Debug for LevelOf<FULL> {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.inner)
    }
}

impl<const FULL: u8> PartialEq<u8> for LevelOf<FULL> {
    fn eq(&self, other: &u8) -> bool {
        self.inner == *other
    }
}

/// The seven-bit panel knob, which is nearly every one of them.
pub type Level = LevelOf<127>;

/// The same knob in a six-bit slot, as a few Stage 4 parameters store it.
pub type Level6 = LevelOf<63>;

/// Declare a 0..=127 knob whose panel reading is in `$unit` over a curve no manual
/// publishes.
///
/// [`Level`] is the same slot on the panel's own `0..10`, where the transform *is* known.
/// These are the ones where it is not: an envelope stage reads in milliseconds and a
/// filter cutoff in hertz, but no published table converts the stored byte, so the byte
/// is what they print. The unit is still worth carrying — it is what lets an interface
/// label the control and pick a taper without a table of field names beside it.
macro_rules! knob {
    ($(#[$meta:meta])* $name:ident, $unit:expr) => {
        knob!($(#[$meta])* $name, 127, 7, ControlKind::Knob($unit));
    };
    ($(#[$meta:meta])* $name:ident, $max:expr, $bits:expr, $control:expr) => {
        $(#[$meta])*
        #[derive(Copy, Clone, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
        pub struct $name {
            inner: u8,
        }

        impl $name {
            pub const MAX: u8 = $max;

            pub fn new(value: u8) -> Result<Self, ParseError> {
                value.try_into()
            }

            /// The stored value, 0..=127.
            pub fn as_u8(&self) -> u8 {
                self.inner
            }
        }

        impl TryFrom<u8> for $name {
            type Error = ParseError;

            fn try_from(value: u8) -> Result<Self, ParseError> {
                if value > Self::MAX {
                    return Err(ParseError::OutOfBounds {
                        value: format!("{value}"),
                        bound: format!("0..={}", Self::MAX),
                    });
                }
                Ok($name { inner: value })
            }
        }

        impl Packed for $name {
            const MAX_BITS: u32 = $bits;
            const CONTROL: ControlKind = $control;
            type Error = ParseError;

            fn from_bits(bits: u64) -> Result<Self, ParseError> {
                (bits as u8).try_into()
            }

            fn to_bits(&self) -> u64 {
                self.inner as u64
            }
        }

        /// The stored byte. There is no published transform to the unit, so printing one
        /// would invent precision the file does not carry.
        impl Debug for $name {
            fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
                write!(f, "{}", self.inner)
            }
        }

        impl Display for $name {
            fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
                write!(f, "{}", self.inner)
            }
        }

        impl PartialEq<u8> for $name {
            fn eq(&self, other: &u8) -> bool {
                self.inner == *other
            }
        }
    };
}

knob!(
    /// An envelope stage or a delay time. The panel reads it in milliseconds through
    /// seconds, over a curve no manual publishes.
    Time,
    Unit::Milliseconds
);

knob!(
    /// A filter cutoff or an equalizer sweep. The panel reads it in hertz — the Stage
    /// manuals give the endpoints of the mid sweep (200 Hz to 8 kHz) but not the taper.
    Frequency,
    Unit::Hertz
);

knob!(
    /// A modulation or LFO rate, read in hertz.
    ///
    /// ⚠️ Under a live master clock the same slot reads as a subdivision instead — see
    /// [`ClockDivision`]. The flag that switches it is a sibling field, so neither field
    /// answers alone.
    Rate,
    Unit::Hertz
);

knob!(
    /// An arpeggiator rate, read as quarter-note BPM.
    ///
    /// ⚠️ Under a live master clock this reads as a subdivision instead — see [`Rate`].
    Tempo,
    Unit::Bpm
);

knob!(
    /// A stereo position in a six-bit slot.
    ///
    /// ⚠️ The mapping is not established: over the Stage 4 factory programs the slot's
    /// mode is 0 rather than the mid-scale 32 a centre-encoded pan would show, so this
    /// makes no claim about where centre sits and prints the stored value. It carries
    /// only that the control is a pan.
    Pan,
    63,
    6,
    ControlKind::Knob(Unit::Pan)
);

knob!(
    /// A pitch offset in semitones.
    ///
    /// **Corpus:** the Stage 4's coarse oscillator pitch holds 0, 7, 12, 24 and 40 —
    /// unison, a fifth, an octave, two octaves — which is what makes the unit readable.
    /// The Stage 3 manual gives the same control as "semitone steps, ranging from 0 to
    /// 48".
    Interval,
    63,
    6,
    ControlKind::Shift(Unit::Semitones)
);

/// A 0..=127 slot whose musical zero is its centre, reading `±LIMIT` of `UNIT` either
/// side.
///
/// The Stage equalizer bands are the clearest case: the manuals give "the boost/cut range
/// is +/- 15 dB" for all three models, and rendering those on [`Level`]'s `0..10` reads a
/// cut as a small boost.
///
/// ⚠️ The centre is taken as 64 — the midpoint of the slot. Inferred; the corpus does not
/// distinguish 63 from 64, and no manual states it. A reading is therefore accurate at
/// the endpoints and approximate in between.
#[derive(Copy, Clone, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Bipolar<const LIMIT: i16> {
    inner: u8,
}

impl<const LIMIT: i16> Bipolar<LIMIT> {
    pub const MAX: u8 = 127;
    /// The stored value that reads as zero.
    pub const CENTER: u8 = 64;

    pub fn new(value: u8) -> Result<Self, ParseError> {
        value.try_into()
    }

    /// The stored value, 0..=127.
    pub fn as_u8(&self) -> u8 {
        self.inner
    }

    /// The panel's signed reading, `-LIMIT..=+LIMIT`.
    pub fn reading(&self) -> f32 {
        let from_center = f32::from(self.inner) - f32::from(Self::CENTER);
        let span = if from_center < 0.0 {
            f32::from(Self::CENTER)
        } else {
            f32::from(Self::MAX - Self::CENTER)
        };
        from_center / span * f32::from(LIMIT)
    }
}

impl<const LIMIT: i16> TryFrom<u8> for Bipolar<LIMIT> {
    type Error = ParseError;

    fn try_from(value: u8) -> Result<Self, ParseError> {
        if value > Self::MAX {
            return Err(ParseError::OutOfBounds {
                value: format!("{value}"),
                bound: format!("0..={}", Self::MAX),
            });
        }
        Ok(Bipolar { inner: value })
    }
}

impl<const LIMIT: i16> Packed for Bipolar<LIMIT> {
    const MAX_BITS: u32 = 7;
    const CONTROL: ControlKind = ControlKind::Bipolar(Unit::Decibels);
    type Error = ParseError;

    fn from_bits(bits: u64) -> Result<Self, ParseError> {
        (bits as u8).try_into()
    }

    fn to_bits(&self) -> u64 {
        self.inner as u64
    }
}

/// The stored byte, so a retype from a plain integer leaves the field dumps alone.
impl<const LIMIT: i16> Debug for Bipolar<LIMIT> {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.inner)
    }
}

impl<const LIMIT: i16> Display for Bipolar<LIMIT> {
    /// Stored byte and signed reading: `96 (+7.5)`.
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        write!(f, "{} ({:+.1})", self.inner, self.reading())
    }
}

impl<const LIMIT: i16> PartialEq<u8> for Bipolar<LIMIT> {
    fn eq(&self, other: &u8) -> bool {
        self.inner == *other
    }
}

/// An equalizer band, `±15 dB` — the range all three Stage manuals give.
pub type EqBand = Bipolar<15>;

/// The value a performance control morphs its parent parameter *to*.
///
/// Every morphable parameter has three of these beside it — `_wheel`, `_aftertouch` and
/// `_ctrl_pedal` — and together they are half of every Stage body's field count. They are
/// not controls of their own: an interface shows them **on the parent's knob**, as a
/// second handle, which is what [`ControlKind::Morph`] tells it to do.
///
/// `BITS` is the slot's width, which tracks the parent's: eight beside a `0..=127` knob,
/// five beside a drawbar, three beside a switch.
///
/// ⚠️ **The encoding is not established.** Over the Stage 4 factory programs the eight-bit
/// slots use the whole byte — 0..=255 — with 78.6% of them holding exactly 127. That
/// reads as a neutral centre, so the slot is plausibly a signed delta biased by 127, or a
/// destination at twice the parent's resolution. Inferred from specimens; not confirmed
/// on hardware. Until it is, [`Self::is_neutral`] is the only claim made, and `Debug`
/// prints the stored value rather than a reading nothing has confirmed.
///
/// The experiment that settles it: assign one morph at a known depth, store, and diff the
/// slot against its parent's value.
#[derive(Copy, Clone, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct MorphOf<const BITS: u32> {
    inner: u8,
}

impl<const BITS: u32> MorphOf<BITS> {
    /// The slot's midpoint — 127 in eight bits, which is where the corpus piles up.
    ///
    /// ⚠️ Only the eight-bit slots show that mode. For the narrower ones this is the
    /// midpoint by construction and not an observation.
    pub const NEUTRAL: u8 = ((1u16 << BITS) / 2 - 1) as u8;

    pub fn as_u8(&self) -> u8 {
        self.inner
    }

    /// Whether the slot holds [`NEUTRAL`](Self::NEUTRAL).
    pub fn is_neutral(&self) -> bool {
        self.inner == Self::NEUTRAL
    }
}

impl<const BITS: u32> Packed for MorphOf<BITS> {
    const MAX_BITS: u32 = BITS;
    const CONTROL: ControlKind = ControlKind::Morph;
    type Error = ::core::convert::Infallible;

    fn from_bits(bits: u64) -> Result<Self, Self::Error> {
        Ok(MorphOf { inner: bits as u8 })
    }

    fn to_bits(&self) -> u64 {
        self.inner as u64
    }
}

/// The stored value — the encoding is unconfirmed, so this prints what is there.
impl<const BITS: u32> Debug for MorphOf<BITS> {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.inner)
    }
}

impl<const BITS: u32> Display for MorphOf<BITS> {
    /// A neutral slot as `—`, anything else as the stored value.
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        if self.is_neutral() {
            f.write_str("—")
        } else {
            write!(f, "{}", self.inner)
        }
    }
}

impl<const BITS: u32> PartialEq<u8> for MorphOf<BITS> {
    fn eq(&self, other: &u8) -> bool {
        self.inner == *other
    }
}

/// The morph slot beside a `0..=127` knob.
pub type MorphTarget = MorphOf<8>;

/// The morph slot beside a drawbar.
pub type DrawbarMorph = MorphOf<5>;

/// The morph slot beside a three-position switch — the Stage 3's rotary speed.
pub type SwitchMorph = MorphOf<3>;

/// A [`Selector`] over a list too long for a byte — a waveform, a sample slot.
#[derive(Copy, Clone, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct WideSelector<const BITS: u32> {
    inner: u16,
}

impl<const BITS: u32> WideSelector<BITS> {
    /// The stored index.
    pub fn raw(&self) -> u16 {
        self.inner
    }
}

impl<const BITS: u32> Packed for WideSelector<BITS> {
    const MAX_BITS: u32 = BITS;
    const CONTROL: ControlKind = ControlKind::Selector;
    type Error = ::core::convert::Infallible;

    fn from_bits(bits: u64) -> Result<Self, Self::Error> {
        Ok(WideSelector { inner: bits as u16 })
    }

    fn to_bits(&self) -> u64 {
        self.inner as u64
    }
}

impl<const BITS: u32> Debug for WideSelector<BITS> {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.inner)
    }
}

impl<const BITS: u32> Display for WideSelector<BITS> {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.inner)
    }
}

impl<const BITS: u32> PartialEq<u16> for WideSelector<BITS> {
    fn eq(&self, other: &u16) -> bool {
        self.inner == *other
    }
}

/// One drawbar, in the four-bit slot the Stage models give it.
///
/// Positions are physical, `0..=8`. The slot holds four bits, so decoding is total: a
/// nibble above 8 is preserved and reported by [`Self::position`] as `None` rather than
/// refused, on the same rule as [`crate::types::RangedU8`] — the bound is the slot's, not
/// the instrument's.
///
/// ⚠️ On the Stage 2's Farfisa the register is a *tab*, and the file stores a bit rather
/// than a nibble, so those fields are `bool` and not this type.
#[derive(Copy, Clone, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Drawbar {
    inner: u8,
}

impl Drawbar {
    /// The highest position a drawbar can be pulled to.
    pub const MAX: u8 = 8;

    /// The stored nibble, whatever it holds.
    pub fn raw(&self) -> u8 {
        self.inner
    }

    /// The position, or `None` for a nibble past the drawbar's travel.
    pub fn position(&self) -> Option<u8> {
        (self.inner <= Self::MAX).then_some(self.inner)
    }
}

impl Packed for Drawbar {
    const MAX_BITS: u32 = 4;
    const CONTROL: ControlKind = ControlKind::Drawbar;
    type Error = ::core::convert::Infallible;

    fn from_bits(bits: u64) -> Result<Self, Self::Error> {
        Ok(Drawbar { inner: bits as u8 })
    }

    fn to_bits(&self) -> u64 {
        self.inner as u64
    }
}

impl Debug for Drawbar {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.inner)
    }
}

impl Display for Drawbar {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.inner)
    }
}

impl PartialEq<u8> for Drawbar {
    fn eq(&self, other: &u8) -> bool {
        self.inner == *other
    }
}

/// A four-bit octave shift stored in two's complement, as the Stage 4 stores it.
///
/// Inferred from specimens; not confirmed on hardware. Over the Stage 4 factory programs
/// the slot holds only 0, 1, 2, 14 and 15 — a distribution centred on zero with the
/// negative side wrapping, where the Stage 2 and 3 instead centre on a stored 7 and 6.
/// Those two are [`OctaveShift`] aliases; this is the third encoding.
#[derive(Copy, Clone, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct OctaveShiftNibble {
    /// The signed reading, -8..=7.
    inner: i8,
}

impl OctaveShiftNibble {
    /// The shift in octaves, `-8..=7`.
    pub fn octaves(&self) -> i8 {
        self.inner
    }
}

impl Packed for OctaveShiftNibble {
    const MAX_BITS: u32 = 4;
    const CONTROL: ControlKind = ControlKind::Shift(Unit::Octaves);
    type Error = ::core::convert::Infallible;

    fn from_bits(bits: u64) -> Result<Self, Self::Error> {
        let nibble = (bits & 0xf) as i8;
        Ok(OctaveShiftNibble {
            inner: if nibble >= 8 { nibble - 16 } else { nibble },
        })
    }

    fn to_bits(&self) -> u64 {
        (self.inner as u8 & 0xf) as u64
    }
}

impl Debug for OctaveShiftNibble {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.inner)
    }
}

impl Display for OctaveShiftNibble {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        write!(f, "{:+}", self.inner)
    }
}

impl PartialEq<i8> for OctaveShiftNibble {
    fn eq(&self, other: &i8) -> bool {
        self.inner == *other
    }
}

/// A selector whose positions are known to be a fixed set, but whose table is not.
///
/// The honest middle between an integer and a [`sparse_enum!`]. A caller building an
/// interface needs to know a field is a *selector* — a discrete list, drawn as a row of
/// positions rather than a knob — before it needs to know what each position is called,
/// and on the Stage 4 that is exactly the state of things: the placements came from an
/// external offset table and were confirmed against the corpus, but no specimen names a
/// value. This carries the shape without laundering a guess into a label.
///
/// Give a field a `sparse_enum!` the moment the table is known; that is strictly better.
#[derive(Copy, Clone, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Selector<const BITS: u32> {
    inner: u8,
}

impl<const BITS: u32> Selector<BITS> {
    /// The stored index.
    pub fn raw(&self) -> u8 {
        self.inner
    }
}

impl<const BITS: u32> Packed for Selector<BITS> {
    const MAX_BITS: u32 = BITS;
    const CONTROL: ControlKind = ControlKind::Selector;
    type Error = ::core::convert::Infallible;

    fn from_bits(bits: u64) -> Result<Self, Self::Error> {
        Ok(Selector { inner: bits as u8 })
    }

    fn to_bits(&self) -> u64 {
        self.inner as u64
    }
}

/// The stored index — this type exists precisely because there is no name to print.
impl<const BITS: u32> Debug for Selector<BITS> {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.inner)
    }
}

impl<const BITS: u32> Display for Selector<BITS> {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.inner)
    }
}

impl<const BITS: u32> PartialEq<u8> for Selector<BITS> {
    fn eq(&self, other: &u8) -> bool {
        self.inner == *other
    }
}

/// A subdivision of the master clock, as a rate slot reads when its clock flag is set.
///
/// The manuals give the vocabulary — "subdivisions of the Master Clock tempo, ranging
/// from 1/2 to 1/32 notes. Apart from straight subdivisions there are also swing (S),
/// triplet (T) and dotted (D) options" — which is sixteen readings for a four-bit slot.
///
/// ⚠️ Which index carries which subdivision is not established, so this names none of
/// them. The experiment that settles it: store one specimen per detent of a clocked rate
/// knob.
pub type ClockDivision = Selector<4>;

/// The balance between a split's lower and upper parts, as a 0..=127 crossfade.
///
/// ⚠️ Each side is clamped at 50, so the pair does not sum to 100 — a stored 16 reads
/// as `50.0/12.6`.
#[derive(Copy, Default, Clone, PartialEq, Eq)]
pub struct PartMix {
    inner: u8,
}

impl PartMix {
    pub fn inner(&self) -> u8 {
        self.inner
    }

    pub fn lower(&self) -> f32 {
        let lower = 100_f32 - ((self.inner() as f32) / 127.0) * 100_f32;

        if lower > 50_f32 {
            50_f32
        } else {
            lower
        }
    }

    pub fn upper(&self) -> f32 {
        let upper = ((self.inner() as f32) / 127.0) * 100_f32;

        if upper > 50_f32 {
            50_f32
        } else {
            upper
        }
    }

    pub fn as_string(&self) -> String {
        format!("{:.1}/{:.1}", self.lower(), self.upper())
    }

    pub fn as_tuple(&self) -> (f32, f32) {
        (self.lower(), self.upper())
    }
}

impl Debug for PartMix {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.as_string())
    }
}

impl Packed for PartMix {
    const MAX_BITS: u32 = 7;
    const CONTROL: ControlKind = ControlKind::Bipolar(Unit::None);
    type Error = ParseError;

    fn from_bits(bits: u64) -> Result<Self, ParseError> {
        (bits as u8).try_into()
    }

    fn to_bits(&self) -> u64 {
        self.inner() as u64
    }
}

impl TryFrom<u8> for PartMix {
    type Error = ParseError;

    fn try_from(value: u8) -> Result<Self, Self::Error> {
        if value > 127 {
            return Err(ParseError::OutOfBounds {
                value: format!("{value}"),
                bound: "0..=127".to_string(),
            });
        }

        Ok(PartMix { inner: value })
    }
}

/// Percussion decay speed. How it is stored is per-model; the Electro 5's B3 does not
/// store it in this order.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PercSpeed {
    Off,
    Soft,
    Fast,
    Both,
}

/// A keyboard split point as the 73-key models store it: one of six keys, or
/// the whole keyboard as Upper / Lower.
#[derive(Debug, Default, Copy, Clone, PartialEq, Eq)]
pub enum SplitPoint73 {
    #[default]
    C3,
    F3,
    C4,
    F4,
    C5,
    F5,
    Upper,
    Lower,
}

impl TryFrom<u8> for SplitPoint73 {
    type Error = &'static str;

    fn try_from(value: u8) -> Result<SplitPoint73, Self::Error> {
        match value {
            0 => Ok(SplitPoint73::C3),
            1 => Ok(SplitPoint73::F3),
            2 => Ok(SplitPoint73::C4),
            3 => Ok(SplitPoint73::F4),
            4 => Ok(SplitPoint73::C5),
            5 => Ok(SplitPoint73::F5),
            6 => Ok(SplitPoint73::Upper),
            7 => Ok(SplitPoint73::Lower),
            _ => Err("Value is out of range for split point"),
        }
    }
}

impl Packed for SplitPoint73 {
    const MAX_BITS: u32 = 3;
    const CONTROL: ControlKind = ControlKind::Selector;
    type Error = ParseError;

    fn from_bits(bits: u64) -> Result<Self, ParseError> {
        SplitPoint73::try_from(bits as u8).map_err(|_| ParseError::OutOfBounds {
            value: format!("{bits}"),
            bound: "0..=7 (SplitPoint73)".to_string(),
        })
    }

    fn to_bits(&self) -> u64 {
        *self as u64
    }
}

/// A vibrato (`V`) or chorus (`C`) organ modulation at one of three depths.
///
/// Which subset an organ offers is the model's business, and so is the index each sits
/// at — see the per-model tables beside the organ panel.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VibChorus {
    V1,
    C1,
    V2,
    C2,
    V3,
    C3,
}

/// A Stage program's transpose slot: stored `0..=12`, biased by 6, reading
/// `-6..=+6` semitones.
///
/// ⚠️ Not a [`RangedI8`]: the Stage 2 EX factory live
/// buffers hold 15 in this slot — an untouched buffer stores an out-of-table
/// pattern — so the unknown patterns are preserved rather than refused.
#[derive(Copy, Clone, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct StageTranspose {
    raw: u8,
}

impl StageTranspose {
    /// The stored 4-bit pattern.
    pub fn raw(&self) -> u8 {
        self.raw
    }

    /// The semitone reading, or `None` for a pattern past the panel's `+6`.
    pub fn semitones(&self) -> Option<i8> {
        (self.raw <= 12).then(|| self.raw as i8 - 6)
    }
}

impl Packed for StageTranspose {
    const MAX_BITS: u32 = 4;
    const CONTROL: ControlKind = ControlKind::Shift(Unit::Semitones);
    type Error = ParseError;

    fn from_bits(bits: u64) -> Result<Self, ParseError> {
        Ok(StageTranspose { raw: bits as u8 })
    }

    fn to_bits(&self) -> u64 {
        self.raw as u64
    }
}

impl Debug for StageTranspose {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        match self.semitones() {
            Some(s) => write!(f, "{s}"),
            None => write!(f, "unknown ({})", self.raw),
        }
    }
}

/// The master clock rate the Stage 2 and 3 store in a program: `stored + 30` BPM.
///
/// Inferred from the Nord User Forum's ns3-program-viewer documentation
/// (github.com/Chris55/ns3-program-viewer); not confirmed on hardware.
#[derive(Copy, Clone, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct MasterTempo {
    inner: u8,
}

impl MasterTempo {
    /// The stored byte.
    pub fn as_u8(&self) -> u8 {
        self.inner
    }

    /// The panel's BPM reading.
    pub fn bpm(&self) -> u16 {
        self.inner as u16 + 30
    }
}

impl Packed for MasterTempo {
    const MAX_BITS: u32 = 8;
    const CONTROL: ControlKind = ControlKind::Knob(Unit::Bpm);
    type Error = ParseError;

    fn from_bits(bits: u64) -> Result<Self, ParseError> {
        Ok(MasterTempo { inner: bits as u8 })
    }

    fn to_bits(&self) -> u64 {
        self.inner as u64
    }
}

impl Debug for MasterTempo {
    /// The BPM reading — the stored byte is recoverable as `bpm - 30`.
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.bpm())
    }
}

/// Declare a sparse enumeration: known values, plus `Unknown` for the rest of the slot.
///
/// The slot is wider than the set of values we have names for, so anything unrecognized
/// decodes to `Unknown`, round-trips byte-exactly, and displays as `unknown (9)` — never
/// coerced to the nearest label. Match on it, or call `is_unknown()`, to find them.
macro_rules! sparse_enum {
    (
        $(#[$meta:meta])*
        $name:ident, $bits:expr, { $($value:expr => $variant:ident, $label:expr;)+ }
    ) => {
        $(#[$meta])*
        #[derive(Copy, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
        pub enum $name {
            $($variant,)+
            /// A stored value with no known meaning.
            Unknown(u8),
        }

        /// Named variants as their names; an unknown as `unknown (raw)`. ⚠️ The corpus
        /// tripwires match the lowercase spelling — a derived `Unknown(raw)` slips past
        /// them.
        impl ::core::fmt::Debug for $name {
            fn fmt(&self, f: &mut ::core::fmt::Formatter<'_>) -> ::core::fmt::Result {
                match self {
                    $($name::$variant => f.write_str(stringify!($variant)),)+
                    $name::Unknown(raw) => write!(f, "unknown ({raw})"),
                }
            }
        }

        impl $name {
            /// The label, or `None` for a value with no known meaning.
            pub fn label(&self) -> Option<&'static str> {
                match self {
                    $($name::$variant => Some($label),)+
                    $name::Unknown(_) => None,
                }
            }

            /// Whether the stored value has no known meaning.
            pub fn is_unknown(&self) -> bool {
                matches!(self, $name::Unknown(_))
            }

            /// The stored value, named or not.
            pub fn raw(&self) -> u8 {
                <Self as $crate::bits::Packed>::to_bits(self) as u8
            }
        }

        impl Default for $name {
            fn default() -> Self {
                <Self as $crate::bits::Packed>::from_bits(0).expect("decoding is total")
            }
        }

        impl $crate::bits::Packed for $name {
            const MAX_BITS: u32 = $bits;
            const CONTROL: $crate::fields::ControlKind = $crate::fields::ControlKind::Selector;
            type Error = ::core::convert::Infallible;

            fn from_bits(bits: u64) -> Result<Self, Self::Error> {
                Ok(match bits as u8 {
                    $($value => $name::$variant,)+
                    other => $name::Unknown(other),
                })
            }

            fn to_bits(&self) -> u64 {
                match self {
                    $($name::$variant => $value as u64,)+
                    $name::Unknown(raw) => *raw as u64,
                }
            }
        }

        impl ::core::fmt::Display for $name {
            fn fmt(&self, f: &mut ::core::fmt::Formatter<'_>) -> ::core::fmt::Result {
                match self.label() {
                    Some(label) => f.write_str(label),
                    None => write!(f, "unknown ({})", self.raw()),
                }
            }
        }
    };
}

pub(crate) use sparse_enum;

/// Declare a one-bit field whose two states have names.
///
/// A `bool` is the right shape for on/off, and the wrong one for a switch between two
/// *named* positions: `false` is not a reading anyone can act on when the panel says
/// Normal and Analog. This keeps the single bit and gives both states their word.
macro_rules! switch {
    (
        $(#[$meta:meta])*
        $name:ident, $clear:ident = $clear_label:expr, $set:ident = $set_label:expr
    ) => {
        $(#[$meta])*
        #[derive(Copy, Clone, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
        pub enum $name {
            /// The state stored as a clear bit.
            #[default]
            $clear,
            /// The state stored as a set bit.
            $set,
        }

        impl $name {
            /// The panel's word for this state.
            pub fn label(&self) -> &'static str {
                match self {
                    $name::$clear => $clear_label,
                    $name::$set => $set_label,
                }
            }

            /// Whether the bit is set.
            pub fn is_set(&self) -> bool {
                matches!(self, $name::$set)
            }
        }

        impl $crate::bits::Packed for $name {
            const MAX_BITS: u32 = 1;
            const CONTROL: $crate::fields::ControlKind = $crate::fields::ControlKind::Toggle;
            type Error = ::core::convert::Infallible;

            fn from_bits(bits: u64) -> Result<Self, Self::Error> {
                Ok(if bits != 0 { $name::$set } else { $name::$clear })
            }

            fn to_bits(&self) -> u64 {
                self.is_set() as u64
            }
        }

        /// The variant name, which is what `--set` takes. ⚠️ Not [`Display`], which is
        /// the panel's own word for the state and may differ.
        impl ::core::fmt::Debug for $name {
            fn fmt(&self, f: &mut ::core::fmt::Formatter<'_>) -> ::core::fmt::Result {
                match self {
                    $name::$clear => f.write_str(stringify!($clear)),
                    $name::$set => f.write_str(stringify!($set)),
                }
            }
        }

        impl ::core::fmt::Display for $name {
            fn fmt(&self, f: &mut ::core::fmt::Formatter<'_>) -> ::core::fmt::Result {
                f.write_str(self.label())
            }
        }
    };
}

/// Sixteen pattern steps, two bits each — the Stage 4 arpeggiator's accent, gate and pan
/// rows.
///
/// The panel edits these as a grid: the manual's Pattern Edit page moves a cursor with a
/// Position dial and sets the step under it, and the Pattern Pan page moves a step
/// "between Left, Center and Right". Three values per step is exactly two bits, and a
/// pattern runs to sixteen steps, which is exactly the 32-bit slot.
///
/// ⚠️ **Step order is inferred, not established.** Read low-bits-first the corpus values
/// fall out as music — `0x01010101` is an accent every fourth step, `0x55aa5500` is four
/// left then four right then four left — but correlating the highest non-zero step
/// against the sibling `arp_pattern_length` fails in both directions, so either the word
/// keeps all sixteen steps regardless of the active length or that field is not a step
/// count. Not confirmed on hardware.
///
/// The slot is wider than [`crate::fields::ENUMERABLE_BITS`], so `--set` spells it by its
/// stored bits — `0x55aa5500` is the readable form for a pattern anyway.
#[derive(Copy, Clone, Default, PartialEq, Eq, Hash)]
pub struct ArpPattern {
    inner: u32,
}

impl ArpPattern {
    /// Steps a pattern can hold.
    pub const STEPS: usize = 16;

    /// The stored word.
    pub fn raw(&self) -> u32 {
        self.inner
    }

    /// The sixteen steps, `0..=3` each, lowest bits first.
    pub fn steps(&self) -> [u8; Self::STEPS] {
        std::array::from_fn(|n| ((self.inner >> (2 * n)) & 0b11) as u8)
    }

    /// Whether every step is zero — an unset row.
    pub fn is_empty(&self) -> bool {
        self.inner == 0
    }
}

impl Packed for ArpPattern {
    const MAX_BITS: u32 = 32;
    const CONTROL: ControlKind = ControlKind::Pattern;
    type Error = ::core::convert::Infallible;

    fn from_bits(bits: u64) -> Result<Self, Self::Error> {
        Ok(ArpPattern { inner: bits as u32 })
    }

    fn to_bits(&self) -> u64 {
        self.inner as u64
    }
}

impl Debug for ArpPattern {
    /// The stored word in hex, which is what `--set` takes back.
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        write!(f, "{:#010x}", self.inner)
    }
}

impl Display for ArpPattern {
    /// The steps as a row: `1010 1010 ....` — a dot for a zero step.
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        for (n, step) in self.steps().into_iter().enumerate() {
            if n > 0 && n % 4 == 0 {
                f.write_str(" ")?;
            }
            match step {
                0 => f.write_str(".")?,
                s => write!(f, "{s}")?,
            }
        }
        Ok(())
    }
}

impl PartialEq<u32> for ArpPattern {
    fn eq(&self, other: &u32) -> bool {
        self.inner == *other
    }
}

/// An opaque id into one of the instrument's libraries — a piano model, a sample.
///
/// The id is only meaningful against the library that holds it; the file carries the
/// reference and nothing else, which is what [`ControlKind::Reference`] tells a caller.
#[derive(Copy, Clone, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct LibraryRef {
    inner: u32,
}

impl LibraryRef {
    /// The stored id. Zero is "nothing referenced" on every model in the corpus.
    pub fn id(&self) -> u32 {
        self.inner
    }

    pub fn is_none(&self) -> bool {
        self.inner == 0
    }
}

impl Packed for LibraryRef {
    const MAX_BITS: u32 = 32;
    const CONTROL: ControlKind = ControlKind::Reference;
    type Error = ::core::convert::Infallible;

    fn from_bits(bits: u64) -> Result<Self, Self::Error> {
        Ok(LibraryRef { inner: bits as u32 })
    }

    fn to_bits(&self) -> u64 {
        self.inner as u64
    }
}

impl Debug for LibraryRef {
    /// Hex, matching how `nord program deps` reports the same id.
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        write!(f, "{:#010x}", self.inner)
    }
}

impl Display for LibraryRef {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        if self.is_none() {
            f.write_str("none")
        } else {
            write!(f, "{:#010x}", self.inner)
        }
    }
}

impl PartialEq<u32> for LibraryRef {
    fn eq(&self, other: &u32) -> bool {
        self.inner == *other
    }
}

switch!(
    /// Which of the two delay lines is running.
    ///
    /// Both Stage manuals give the pair by name: "There are two different delay modes, the
    /// normal ('non-analog') mode, and the Analog Mode … In Analog Mode the pitch of any
    /// sounding repeats is altered if the tempo is changed."
    DelayCharacter, Normal = "normal", Analog = "analog"
);

switch!(
    /// How quickly the compressor recovers. Manual: "The FAST mode … makes the Compressor
    /// recover quicker after being triggered."
    CompressorResponse, Normal = "normal", Fast = "fast"
);

switch!(
    /// The rotary speaker's rotor speed. Manual: "Switch between fast and slow rotor
    /// speeds."
    ///
    /// ⚠️ Stopped is not one of these — it is a separate flag, so neither field answers
    /// on its own.
    RotorSpeed, Slow = "slow", Fast = "fast"
);

sparse_enum!(
    /// Which of the four keyboard zones a section occupies, as the Stage 3 and 4 store it.
    ///
    /// The Stage 3 byte-map docs give the table as an occupancy picture — `o---` is the
    /// leftmost zone alone, `oooo` the whole keyboard. **Corpus:** the Stage 3's piano and
    /// synth zone slots hold only values inside this table, with `oooo` dominating, so all
    /// three sections share it.
    ///
    /// ⚠️ The Stage 4 reaches a stored **10**, which this table does not name — see
    /// `organ_a.kb_zones` and `synth_c_performance.kb_zones` in the Stage 4 decode
    /// snapshot. Either the Stage 4 offers an eleventh combination or the value means
    /// something else there; it decodes to `Unknown(10)` and rides through verbatim
    /// rather than being folded into a neighbor.
    KbZone4, 4, {
        0 => V0, "o---";
        1 => V1, "-o--";
        2 => V2, "--o-";
        3 => V3, "---o";
        4 => V4, "oo--";
        5 => V5, "-oo-";
        6 => V6, "--oo";
        7 => V7, "ooo-";
        8 => V8, "-ooo";
        9 => V9, "oooo";
    }
);

sparse_enum!(
    /// Which of the three keyboard zones a section occupies, as the Stage 2 stores it.
    ///
    /// The Stage 2 splits into two or three zones rather than four, and its panel spells
    /// them in words. From the `ns2-*-kb-zone` tables in the Stage byte-map docs.
    KbZone3, 3, {
        0 => Lo, "LO";
        1 => LoUp, "LO UP";
        2 => Up, "UP";
        3 => UpHi, "UP HI";
        4 => Hi, "HI";
        5 => LoUpHi, "LO UP HI";
    }
);

sparse_enum!(
    /// A Stage split boundary, one of the ten notes the panel offers.
    ///
    /// The Stage 2 and 3 store the same ten-note table. Inferred from the Nord User
    /// Forum's ns3-program-viewer documentation (github.com/Chris55/ns3-program-viewer);
    /// not confirmed on hardware.
    SplitNote, 4, {
        0 => F2, "F2";
        1 => C3, "C3";
        2 => F3, "F3";
        3 => C4, "C4";
        4 => F4, "F4";
        5 => C5, "C5";
        6 => F5, "F5";
        7 => C6, "C6";
        8 => F6, "F6";
        9 => C7, "C7";
    }
);

sparse_enum!(
    /// A Stage 3 split crossfade width, in semitones.
    ///
    /// Inferred from the ns3-program-viewer documentation; not confirmed on hardware.
    SplitWidth, 2, {
        0 => One, "1";
        1 => Six, "6";
        2 => Twelve, "12";
    }
);

sparse_enum!(
    /// The program category byte the Stage 2 and 3 keep in the header's `aux` word.
    ///
    /// Inferred from the ns3-program-viewer documentation; not confirmed on hardware.
    /// The gaps are real: no name is known for the values between these.
    ProgramCategory, 8, {
        0x00 => Acoustic, "Acoustic";
        0x01 => Bass, "Bass";
        0x02 => Wind, "Wind";
        0x04 => Fantasy, "Fantasy";
        0x05 => Fx, "FX";
        0x06 => Lead, "Lead";
        0x07 => Organ, "Organ";
        0x08 => Pad, "Pad";
        0x0a => Pluck, "Pluck";
        0x0b => String, "String";
        0x0c => Synth, "Synth";
        0x0d => Vocal, "Vocal";
        0x0e => User, "User";
        0x11 => None_, "None";
        0x15 => Grand, "Grand";
        0x16 => Upright, "Upright";
        0x17 => EPiano1, "EPiano1";
        0x18 => EPiano2, "EPiano2";
        0x1b => Clavinet, "Clavinet";
        0x1c => Harpsi, "Harpsi";
        0x1e => Arpeggio, "Arpeggio";
        0xff => Undefined, "Undefined";
    }
);

sparse_enum!(
    /// From the `ns2-effect-1-type` table in the Stage byte-map docs.
    Effect1Type, 3, {
        0 => APan, "A-Pan";
        1 => Trem, "Trem";
        2 => Rm, "RM";
        3 => WaWa, "WA-WA";
        4 => AWa1, "A-WA1";
        5 => AWa2, "A-WA2";
    }
);

sparse_enum!(
    /// From the `ns2-effect-2-type` table in the Stage byte-map docs.
    Effect2Type, 3, {
        0 => Phas1, "PHAS1";
        1 => Phas2, "PHAS2";
        2 => Flang, "FLANG";
        3 => Vibe, "VIBE";
        4 => Chor1, "CHOR1";
        5 => Chor2, "CHOR2";
    }
);

sparse_enum!(
    /// From the `ns2-reverb-type` table in the Stage byte-map docs.
    ReverbType, 3, {
        0 => Room1, "Room 1";
        1 => Room2, "Room 2";
        2 => Stage1, "Stage 1";
        3 => Stage2, "Stage 2";
        4 => Hall1, "Hall 1";
        5 => Hall2, "Hall 2";
    }
);

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fields::{ControlKind, Unit};

    /// The whole point of the vocabulary: a field gets its control kind by choosing a
    /// type, so an interface never needs a table of field names of its own.
    #[test]
    fn a_type_says_what_kind_of_control_it_is() {
        assert_eq!(<Level as Packed>::CONTROL, ControlKind::Knob(Unit::Panel10));
        assert_eq!(
            <Time as Packed>::CONTROL,
            ControlKind::Knob(Unit::Milliseconds)
        );
        assert_eq!(
            <EqBand as Packed>::CONTROL,
            ControlKind::Bipolar(Unit::Decibels)
        );
        assert_eq!(<MorphTarget as Packed>::CONTROL, ControlKind::Morph);
        assert_eq!(<Drawbar as Packed>::CONTROL, ControlKind::Drawbar);
        assert_eq!(<ArpPattern as Packed>::CONTROL, ControlKind::Pattern);
        assert_eq!(<LibraryRef as Packed>::CONTROL, ControlKind::Reference);
        assert_eq!(<KbZone4 as Packed>::CONTROL, ControlKind::Selector);
        assert_eq!(<bool as Packed>::CONTROL, ControlKind::Toggle);
        assert_eq!(
            <OctaveShiftNibble as Packed>::CONTROL,
            ControlKind::Shift(Unit::Octaves)
        );
        // The default, and the standing invitation to give a field a better type.
        assert_eq!(<u8 as Packed>::CONTROL, ControlKind::Number);
    }

    /// A unit is a label, not a promise. Printing a millisecond reading off a curve no
    /// manual publishes would be inventing precision the file does not carry.
    #[test]
    fn a_unit_says_whether_it_can_be_computed() {
        assert!(Unit::Panel10.describes_a_known_transform());
        assert!(Unit::Decibels.describes_a_known_transform());
        assert!(!Unit::Milliseconds.describes_a_known_transform());
        assert!(!Unit::Hertz.describes_a_known_transform());
        // So the type prints the stored byte rather than a converted one.
        assert_eq!(Time::new(96).unwrap().to_string(), "96");
        assert_eq!(Level::new(96).unwrap().to_string(), "96 (7.6)");
    }

    /// The Stage 4 stores octave shift in two's complement, where the Stage 2 and 3 store
    /// it biased. Corpus: the Stage 4 factory banks hold only 0, 1, 2, 14 and 15.
    #[test]
    fn the_stage4_octave_shift_wraps_where_the_others_bias() {
        let read = |bits| OctaveShiftNibble::from_bits(bits).unwrap().octaves();
        assert_eq!(read(0), 0);
        assert_eq!(read(1), 1);
        assert_eq!(read(2), 2);
        assert_eq!(read(15), -1);
        assert_eq!(read(14), -2);
        // Every pattern round-trips, so an unreached value rides through a re-encode.
        for bits in 0..16u64 {
            assert_eq!(OctaveShiftNibble::from_bits(bits).unwrap().to_bits(), bits);
        }
    }

    /// Every eight-bit pattern is a morph value, and 127 is the one the corpus piles up
    /// on. The reading is unconfirmed, so `Debug` stays numeric and only `Display` says
    /// anything.
    #[test]
    fn a_morph_slot_names_its_neutral_and_keeps_the_rest() {
        assert_eq!(MorphTarget::NEUTRAL, 127);
        let neutral = MorphTarget::from_bits(127).unwrap();
        assert!(neutral.is_neutral());
        assert_eq!(neutral.to_string(), "—");
        assert_eq!(format!("{neutral:?}"), "127");

        let moved = MorphTarget::from_bits(254).unwrap();
        assert!(!moved.is_neutral());
        assert_eq!(moved.to_string(), "254");
        // The whole byte is in use, so nothing may be refused or clamped.
        for bits in 0..256u64 {
            assert_eq!(MorphTarget::from_bits(bits).unwrap().to_bits(), bits);
        }
    }

    /// Sixteen steps of two bits, lowest bits first — the reading that makes the corpus
    /// values fall out as music.
    #[test]
    fn an_arp_pattern_reads_as_steps() {
        // Accent on every fourth step.
        let accent = ArpPattern::from_bits(0x0101_0101).unwrap();
        assert_eq!(
            accent.steps(),
            [1, 0, 0, 0, 1, 0, 0, 0, 1, 0, 0, 0, 1, 0, 0, 0]
        );
        assert_eq!(accent.to_string(), "1... 1... 1... 1...");

        // Four left, four right, four left — a pan row.
        let pan = ArpPattern::from_bits(0x55aa_5500).unwrap();
        assert_eq!(&pan.steps()[4..12], &[1, 1, 1, 1, 2, 2, 2, 2]);

        assert!(ArpPattern::default().is_empty());
        // The slot is wider than the enumerable ceiling, so `--set` spells it in hex.
        assert_eq!(format!("{pan:?}"), "0x55aa5500");
    }

    /// An equalizer band reads +/- 15 dB either side of the slot's centre.
    #[test]
    fn a_bipolar_band_reads_signed() {
        assert_eq!(EqBand::new(64).unwrap().reading(), 0.0);
        assert_eq!(EqBand::new(0).unwrap().to_string(), "0 (-15.0)");
        assert_eq!(EqBand::new(127).unwrap().to_string(), "127 (+15.0)");
        // `Debug` is the stored byte, so retyping a plain integer leaves field dumps alone.
        assert_eq!(format!("{:?}", EqBand::new(96).unwrap()), "96");
    }

    /// A switch keeps its single bit and gives both states a word. `Debug` is the
    /// variant, which is what `--set` takes; `Display` is the panel's own wording.
    #[test]
    fn a_switch_names_both_of_its_states() {
        let normal = DelayCharacter::from_bits(0).unwrap();
        let analog = DelayCharacter::from_bits(1).unwrap();
        assert_eq!(format!("{normal:?}"), "Normal");
        assert_eq!(analog.to_string(), "analog");
        assert_eq!(analog.to_bits(), 1);
        assert!(analog.is_set());
        assert_eq!(<DelayCharacter as Packed>::MAX_BITS, 1);
    }

    /// A drawbar is total over its nibble: a position past the bar's travel is preserved
    /// and reported as unnamed rather than refused.
    #[test]
    fn a_drawbar_keeps_a_nibble_past_its_travel() {
        assert_eq!(Drawbar::from_bits(8).unwrap().position(), Some(8));
        assert_eq!(Drawbar::from_bits(9).unwrap().position(), None);
        assert_eq!(Drawbar::from_bits(9).unwrap().raw(), 9);
        for bits in 0..16u64 {
            assert_eq!(Drawbar::from_bits(bits).unwrap().to_bits(), bits);
        }
    }

    #[test]
    fn a_level_carries_the_panel_transform() {
        assert_eq!(Level::new(0).unwrap().to_string(), "0 (0.0)");
        assert_eq!(Level::new(127).unwrap().to_string(), "127 (10.0)");
        assert_eq!(Level::new(96).unwrap().to_string(), "96 (7.6)");
        assert!(Level::new(128).is_err(), "128 does not fit seven bits");
    }
}
