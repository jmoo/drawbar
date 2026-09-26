//! Field-level introspection over a `#[bitbody]`'s fields.
//!
//! Generated registries let callers inspect and edit declared fields without a
//! second, manually synchronized list of names.

use std::fmt::{self, Debug, Display, Formatter};

use crate::bits::Packed;

/// One decoded field of a panel.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FieldValue {
    /// The field's full registry path, e.g. `center_panel.transpose`.
    pub name: String,
    /// Where the bits sit, as `LO..=HI` over the declaring body's bytes.
    pub placement: &'static str,
    /// The field's bits as they were read, shifted down to bit 0. Carries no type, so it
    /// stays comparable across a retype.
    pub raw: u64,
    /// The bits the field's current value would write.
    ///
    /// Decode and encode are inverses, so this equals [`raw`](Self::raw) on a panel
    /// decoded from bytes and not edited since; the fields where the two differ are the
    /// pending changes. A `Default`-built panel has all-zero raw bytes, so every default
    /// that encodes nonzero reads as pending.
    pub bits: u64,
    /// The decoded value's `Debug` rendering.
    pub value: String,
}

impl Display for FieldValue {
    /// `lower_part  0..=2  raw 0  Organ`
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{:<22} {:<12} raw {:<11} {}",
            self.name, self.placement, self.raw, self.value
        )
    }
}

/// What a field is, without an instance of the panel to read it from.
#[derive(Clone)]
pub struct FieldSpec {
    /// The field's full registry path, e.g. `center_panel.transpose`.
    pub name: String,
    pub placement: &'static str,
    /// Width of the field in bits.
    pub width: u32,
    /// Every value the field's type accepts, rendered as `set_field` spells them. Empty
    /// for a field too wide to enumerate (see [`ENUMERABLE_BITS`]).
    pub legal: fn() -> Vec<String>,
    /// Which panel control this field is, from its type's
    /// [`CONTROL`](crate::bits::Packed::CONTROL).
    pub control: ControlKind,
}

impl FieldSpec {
    /// The full path of the parameter this field morphs, for a [`ControlKind::Morph`]
    /// that names one.
    ///
    /// The kind carries only the parent's sibling name, which is all the declaring body
    /// knows. The path is this field's path with its last segment replaced, so a nested
    /// body's prefix is kept.
    pub fn morph_parent(&self) -> Option<String> {
        let ControlKind::Morph { of: Some(parent) } = self.control else {
            return None;
        };
        Some(match self.name.rsplit_once('.') {
            Some((prefix, _)) => format!("{prefix}.{parent}"),
            None => parent.to_string(),
        })
    }
}

/// The kind of panel control a field is.
///
/// The registry says where a field sits and which values it takes; this says what kind of
/// control it is, so a caller can choose a widget without a table of field names. It
/// comes from the field's type: a `bool` is a button, and a [`Level`] is a knob.
///
/// [`Level`]: crate::components::Level
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ControlKind {
    /// A two-state button. Its two states may have names; see the field's `legal` values.
    Toggle,
    /// A selector over a fixed set of named values.
    Selector,
    /// A continuous knob or slider, reading in `unit`.
    Knob(Unit),
    /// A knob whose musical zero is its center, reading in `unit` on either side.
    Bipolar(Unit),
    /// One or more drawbars, each `0..=8`, drawn as bars.
    Drawbar {
        /// How many bars the field holds, in register order. The Stage models give each
        /// bar its own field, and the Electro 5 packs a whole register into one.
        bars: u8,
        /// Where the field's first bar sits in the register: 1 is the leftmost bar, 9 the
        /// rightmost of a nine-bar manual. A whole register starts at 1, and a single
        /// Stage bar carries its own position.
        ///
        /// ⚠️ A position, not a pitch. Which harmonic each position draws depends on the
        /// organ model: the B3's 16'/5⅓'/8' series is not the Vox's or the Farfisa's,
        /// and the same nine positions serve all of them. Only a caller that knows the
        /// field's organ model can label them.
        ///
        /// `None` where the declaration does not place the bar in a register: the
        /// Electro 5's bass manual, whose two bars have no established position.
        rank: Option<u8>,
        /// Bits one bar occupies.
        bits_per_bar: u8,
        /// Which end of the field the first bar sits at; only meaningful above one bar.
        /// The Electro 5 packs its nine nibbles high-first.
        order: PackedOrder,
    },
    /// The value a performance control morphs its parent parameter to. It belongs on the
    /// parent's control, not on a control of its own.
    Morph {
        /// The parent parameter's field name, as a sibling of this field.
        /// [`FieldSpec::morph_parent`] resolves the full path.
        ///
        /// `None` where the body declares no parameter under the name this slot's name
        /// implies; the slot then stands alone.
        of: Option<&'static str>,
    },
    /// A per-step pattern grid: `steps` steps of `bits_per_step` bits, the first step at
    /// the `order` end.
    Pattern {
        steps: u8,
        bits_per_step: u8,
        order: PackedOrder,
    },
    /// An opaque id into one of the instrument's libraries.
    Reference(Library),
    /// A signed shift, reading in `unit`.
    Shift(Unit),
    /// A plain integer: the default, for a field whose type says nothing more.
    Number,
}

impl ControlKind {
    /// Name the parent a morph slot morphs, as a sibling field name, not a path.
    ///
    /// `#[bitbody]` applies this from the field's name, only where the body declares that
    /// sibling. Every other kind is returned unchanged, so a field named like a morph
    /// slot but typed as something else keeps its type's kind.
    pub const fn morphing(self, parent: &'static str) -> ControlKind {
        match self {
            ControlKind::Morph { .. } => ControlKind::Morph { of: Some(parent) },
            other => other,
        }
    }

    /// Place a drawbar in its register: `rank` 1 is the leftmost bar.
    ///
    /// Applied by `#[bitbody]` from a `…_N` field name, and ignored by every other kind:
    /// a field whose name ends in a digit is not a drawbar unless its type says so.
    pub const fn ranked(self, rank: u8) -> ControlKind {
        match self {
            ControlKind::Drawbar {
                bars,
                bits_per_bar,
                order,
                ..
            } => ControlKind::Drawbar {
                bars,
                rank: Some(rank),
                bits_per_bar,
                order,
            },
            other => other,
        }
    }
}

/// Which end of a field the first of its packed values sits at.
///
/// Only a field holding several values needs it (a drawbar register, a pattern row). Read
/// from the wrong end, a register comes out mirrored and still looks like a plausible
/// registration.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PackedOrder {
    /// The first value occupies the most significant bits.
    HighFirst,
    /// The first value occupies the least significant bits.
    LowFirst,
}

/// One of the instrument's stored libraries: what a [`ControlKind::Reference`] id points
/// into.
///
/// A file carries only the id, so this is the only record of which catalog resolves it.
/// Only libraries that a decoded body refers to are listed. The instruments hold others
/// (the live slots, the settings singleton) that no reference points at.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Library {
    /// Piano instruments (`.npno`).
    Piano,
    /// Sample instruments (`.nsmp`).
    Sample,
    /// The instrument's own programs.
    Program,
    /// Set lists, which name programs in turn.
    SetList,
}

impl Library {
    /// The library's numeric code.
    ///
    /// A const generic parameter cannot be an enum, so a type that carries its library,
    /// such as [`LibraryRefOf`](crate::components::LibraryRefOf), carries this code and
    /// turns it back with [`expect_code`](Self::expect_code).
    ///
    /// The numbers are the object-class codes the instruments use on the wire, and
    /// `nord-usb`'s `ObjectClass` takes its library codes from here, so a caller holding
    /// both has one table.
    pub const fn code(self) -> u8 {
        match self {
            Library::Piano => 1,
            Library::Sample => 3,
            Library::Program => 4,
            Library::SetList => 5,
        }
    }

    /// The library a [`code`](Self::code) names, or `None`; most bytes name none.
    pub const fn from_code(code: u8) -> Option<Library> {
        match code {
            1 => Some(Library::Piano),
            3 => Some(Library::Sample),
            4 => Some(Library::Program),
            5 => Some(Library::SetList),
            _ => None,
        }
    }

    /// The library a [`code`](Self::code) names, for the type-level parameter this
    /// vocabulary exists to carry.
    ///
    /// ⚠️ Panics on a code that names no library. That is a build failure only where the
    /// value is forced at compile time, as it is for the aliases in
    /// [`components`](crate::components). A `LibraryRefOf<7>` that nobody places
    /// compiles, and fails once a field declared with it asks for its control kind. Use
    /// [`from_code`](Self::from_code) wherever a code arrives at runtime.
    pub const fn expect_code(code: u8) -> Library {
        match Library::from_code(code) {
            Some(library) => library,
            None => panic!("no library has this code"),
        }
    }

    /// The catalog's name, singular, as a caller would put it before "id".
    pub fn label(&self) -> &'static str {
        match self {
            Library::Piano => "piano",
            Library::Sample => "sample",
            Library::Program => "program",
            Library::SetList => "set list",
        }
    }
}

/// The unit a control reads in.
///
/// ⚠️ Naming a unit is not a promise that the stored value converts to it. Several Nord
/// knobs read in milliseconds or hertz over a curve no manual publishes; the unit says
/// what the panel shows, and the type's own `Display` prints a converted reading only
/// where the transform is known. See [`Unit::describes_a_known_transform`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Unit {
    /// The panel's own `0..10`, which most Nord knobs read in.
    Panel10,
    Decibels,
    Milliseconds,
    Hertz,
    /// Beats per minute, quarter-note.
    Bpm,
    /// A subdivision of the master clock, such as `1/8` or `1/4 T`.
    ClockDivision,
    Semitones,
    Octaves,
    /// A stereo position, left through center to right.
    Pan,
    /// No unit: a count, an index, or a raw byte.
    None,
}

impl Unit {
    /// The unit's numeric code.
    ///
    /// A const generic parameter cannot be an enum, so a type that carries its unit, such
    /// as [`BipolarOf`](crate::components::BipolarOf), carries this code and turns it
    /// back with [`expect_code`](Self::expect_code).
    pub const fn code(self) -> u8 {
        match self {
            Unit::Panel10 => 0,
            Unit::Decibels => 1,
            Unit::Milliseconds => 2,
            Unit::Hertz => 3,
            Unit::Bpm => 4,
            Unit::ClockDivision => 5,
            Unit::Semitones => 6,
            Unit::Octaves => 7,
            Unit::Pan => 8,
            Unit::None => 9,
        }
    }

    /// The unit a [`code`](Self::code) names, for the type-level parameter this
    /// vocabulary exists to carry.
    ///
    /// ⚠️ Panics on a code that names no unit. That is a build failure where the value
    /// is forced at compile time, as it is for the aliases in
    /// [`components`](crate::components).
    pub const fn expect_code(code: u8) -> Unit {
        match code {
            0 => Unit::Panel10,
            1 => Unit::Decibels,
            2 => Unit::Milliseconds,
            3 => Unit::Hertz,
            4 => Unit::Bpm,
            5 => Unit::ClockDivision,
            6 => Unit::Semitones,
            7 => Unit::Octaves,
            8 => Unit::Pan,
            9 => Unit::None,
            _ => panic!("no unit has this code"),
        }
    }

    /// Whether a value in this unit can be computed from the stored one.
    ///
    /// False for units whose panel curve is not published. A caller may still label an
    /// axis with the unit, but must print the stored value.
    pub fn describes_a_known_transform(&self) -> bool {
        matches!(
            self,
            Unit::Panel10 | Unit::Decibels | Unit::Semitones | Unit::Octaves | Unit::Pan
        )
    }
}

/// The widest field whose legal values are enumerated. Above it a field is spelled by its
/// stored bits, since walking every pattern would mean millions of strings.
pub const ENUMERABLE_BITS: u32 = 12;

/// Why a field could not be set.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FieldError {
    UnknownField {
        panel: &'static str,
        name: String,
    },
    BadValue {
        field: &'static str,
        given: String,
        legal: Vec<String>,
    },
}

impl Display for FieldError {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        match self {
            FieldError::UnknownField { panel, name } => {
                write!(f, "{panel} has no field {name:?}")
            }
            FieldError::BadValue {
                field,
                given,
                legal,
            } => {
                write!(f, "{given:?} is not a value of {field}")?;
                match legal.len() {
                    // Too wide to have named values; the stored bits are its only
                    // spelling.
                    0 => write!(f, " (accepts the stored bits, decimal or 0x…)"),
                    n if n > 12 => write!(f, " (accepts {} .. {})", legal[0], legal[n - 1]),
                    _ => write!(f, " (accepts {})", legal.join(", ")),
                }
            }
        }
    }
}

impl std::error::Error for FieldError {}

/// The generated field registry behind an entity, where its body declares one.
///
/// `#[bitbody]` generates these three methods on every body with public
/// fields; this trait puts them behind one name, so a caller can list and
/// set fields without naming the body type.
/// [`Entity::registry`](crate::Entity::registry) returns one. A body joins
/// by being declared there, and every consumer then sees it.
pub trait Registry {
    /// Every settable field, described under its full path.
    fn fields(&self) -> Vec<Field>;
    /// Every registered field's current value, in declaration order.
    fn field_values(&self) -> Vec<FieldValue>;
    /// Set one field by its full path.
    fn set_field(&mut self, path: &str, value: &str) -> Result<(), FieldError>;
}

/// One settable field of a body, addressed the way `--set` addresses it.
pub struct Field {
    /// The field's full registry path, e.g. `center_panel.transpose`.
    pub path: String,
    pub spec: FieldSpec,
    /// What the field currently holds, spelled the way `set_field` takes it.
    /// Feeding this straight back is always a no-op.
    pub value: String,
    /// The same value as `nord inspect` renders it. Differs from `value` only for a
    /// field too wide to have named values, where the rendering is a list and the
    /// spelling is the stored bits.
    pub display: String,
}

/// Every value of `T` that fits a field `width` bits wide, in stored order, taken from
/// the type itself.
pub fn legal_values<T: Packed + Debug>(width: u32) -> Vec<String> {
    if width > ENUMERABLE_BITS {
        return Vec::new();
    }
    let mut seen = Vec::new();
    for bits in 0..(1u64 << width) {
        if let Ok(v) = T::from_bits(bits) {
            let rendered = format!("{v:?}");
            if !seen.contains(&rendered) {
                seen.push(rendered);
            }
        }
    }
    seen
}

/// Parse a field's value out of the way the field prints it.
///
/// This walks the field's own bit patterns and takes the one whose `Debug` matches, so a
/// type gets string parsing from its `Debug` alone. A value outside its range has no
/// pattern to match and fails here instead of being clamped.
///
/// ⚠️ An unexplained value can only be written by its unexplained spelling: a sparse enum
/// renders an unrecognized `9` as `Unknown(9)`, so a bare `9` matches nothing and
/// `Unknown(9)` is the only spelling.
pub fn parse_field<T: Packed + Debug>(width: u32, given: &str) -> Result<T, FieldError> {
    let wanted = normalize(given);
    // A truth word for a `bool` field: its `Debug` is `true`/`false`, which no numeric
    // field renders, so trying the canonical spelling second cannot collide.
    let alias = match wanted.as_str() {
        "on" | "yes" | "1" => Some("true"),
        "off" | "no" | "0" => Some("false"),
        _ => None,
    };

    if width <= ENUMERABLE_BITS {
        for bits in 0..(1u64 << width) {
            let Ok(v) = T::from_bits(bits) else { continue };
            let rendered = normalize(&format!("{v:?}"));
            if rendered == wanted || Some(rendered.as_str()) == alias {
                return Ok(v);
            }
        }
    } else if let Some(bits) = stored_value(&wanted) {
        // Wide fields use stored bits; for a drawbar block the hex digits are its bars.
        // Check before decoding: storage-backed implementations may cast and truncate.
        if width >= 64 || bits < (1u64 << width) {
            if let Ok(v) = T::from_bits(bits) {
                return Ok(v);
            }
        }
    }
    Err(FieldError::BadValue {
        // Filled in by the caller, which knows the field's name.
        field: "",
        given: given.to_string(),
        legal: legal_values::<T>(width),
    })
}

/// Case-folded, `+`-stripped, whitespace-trimmed: `+5`, `5` and ` 5 ` are one value, and
/// so are `Organ` and `organ`.
fn normalize(s: &str) -> String {
    s.trim()
        .trim_start_matches('+')
        .to_ascii_lowercase()
        .to_string()
}

/// A field's stored bits, written decimal or `0x`-prefixed. Already normalized.
fn stored_value(s: &str) -> Option<u64> {
    match s.strip_prefix("0x") {
        Some(hex) => u64::from_str_radix(hex, 16).ok(),
        None => s.parse().ok(),
    }
}

/// How a field of this width spells its current value back to a caller.
///
/// Narrow fields are named (`Organ`, `-5`, `true`), and that name is what `--set` takes.
/// A field too wide to enumerate is spelled by its stored bits, which `raw` holds.
pub fn settable_form(width: u32, debug: &str, raw: u64) -> String {
    if width <= ENUMERABLE_BITS {
        debug.to_string()
    } else {
        format!("{raw:#x}")
    }
}

impl FieldError {
    /// Attach the field's name to an error raised before it was known.
    pub fn at(self, field: &'static str) -> Self {
        match self {
            FieldError::BadValue { given, legal, .. } => FieldError::BadValue {
                field,
                given,
                legal,
            },
            other => other,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::components::MorphTarget;
    use crate::formats::ne5::{Level, Transpose};

    /// A refinement adds what the declaration site knows and the type cannot. A field
    /// whose name looks like a morph slot or a drawbar, but whose type says otherwise,
    /// keeps its type's kind.
    #[test]
    fn a_refinement_only_reaches_the_kind_it_is_for() {
        assert_eq!(
            ControlKind::Morph { of: None }.morphing("organ_a_volume"),
            ControlKind::Morph {
                of: Some("organ_a_volume")
            }
        );
        let bar = |rank| ControlKind::Drawbar {
            bars: 1,
            rank,
            bits_per_bar: 4,
            order: PackedOrder::HighFirst,
        };
        assert_eq!(bar(None).ranked(7), bar(Some(7)));

        let knob = ControlKind::Knob(Unit::Panel10);
        assert_eq!(knob.morphing("delay_tempo"), knob);
        assert_eq!(knob.ranked(2), knob);
    }

    #[test]
    fn a_morph_slot_resolves_its_parents_full_path() {
        let spec = |name: &str| FieldSpec {
            name: name.to_string(),
            placement: "0..=7",
            width: 8,
            legal: || Vec::new(),
            control: <MorphTarget as Packed>::CONTROL.morphing("drawbar_1"),
        };
        assert_eq!(
            spec("organ_a.drawbar_1_wheel").morph_parent().as_deref(),
            Some("organ_a.drawbar_1"),
        );
        assert_eq!(
            spec("drawbar_1_wheel").morph_parent().as_deref(),
            Some("drawbar_1"),
        );
        // A slot with no parameter beside it stands alone.
        let mut orphan = spec("drawbar_1_wheel");
        orphan.control = <MorphTarget as Packed>::CONTROL;
        assert_eq!(orphan.morph_parent(), None);
    }

    #[test]
    fn a_value_is_parsed_out_of_the_way_it_prints() {
        let v: Transpose = parse_field(4, "-5").unwrap();
        assert_eq!(v.inner(), -5);
        // The type applies the bias: -5 stores as 1.
        assert_eq!(<Transpose as Packed>::to_bits(&v), 1);
    }

    #[test]
    fn a_leading_plus_and_stray_space_are_the_same_value() {
        for spelling in ["+3", "3", " 3 "] {
            assert_eq!(parse_field::<Transpose>(4, spelling).unwrap().inner(), 3);
        }
    }

    /// Out of range has no bit pattern to match, so it cannot reach an encode.
    #[test]
    fn a_value_outside_the_types_range_is_refused() {
        let err = parse_field::<Transpose>(4, "9")
            .unwrap_err()
            .at("transpose");
        assert!(
            err.to_string().contains("not a value of transpose"),
            "{err}"
        );
    }

    #[test]
    fn a_bool_takes_the_words_people_actually_type() {
        for yes in ["true", "on", "yes", "1"] {
            assert!(parse_field::<bool>(1, yes).unwrap(), "{yes}");
        }
        for no in ["false", "off", "no", "0"] {
            assert!(!parse_field::<bool>(1, no).unwrap(), "{no}");
        }
    }

    #[test]
    fn a_wide_numeric_value_must_fit_its_declared_width() {
        assert!(parse_field::<u16>(16, "70000").is_err());
        assert!(parse_field::<u32>(32, "4294967296").is_err());
        assert_eq!(
            parse_field::<u64>(64, "18446744073709551615").unwrap(),
            u64::MAX
        );
    }

    /// A wide numeric field enumerates, so `--fields` can still say what it takes.
    #[test]
    fn legal_values_come_from_the_type() {
        assert_eq!(legal_values::<bool>(1), vec!["false", "true"]);
        let levels = legal_values::<Level>(7);
        assert_eq!(levels.len(), 128);
        assert_eq!(levels.last().unwrap(), "127");
    }

    /// The message names the values the field accepts.
    #[test]
    fn the_error_lists_a_short_value_set_and_ranges_a_long_one() {
        let short = FieldError::BadValue {
            field: "split",
            given: "maybe".into(),
            legal: vec!["false".into(), "true".into()],
        };
        assert!(short.to_string().contains("accepts false, true"));

        let long = FieldError::BadValue {
            field: "gain",
            given: "200".into(),
            legal: (0..128).map(|n| n.to_string()).collect(),
        };
        assert!(long.to_string().contains("accepts 0 .. 127"), "{long}");
    }
}
