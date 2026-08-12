//! Field-level introspection over a `#[bitbody]`'s fields.
//!
//! The generated registry lets a caller walk a body's fields without naming any of
//! them, which is what the decode snapshot needs: a new field appears in the output
//! by being declared, not by anyone remembering to add it. `set_field` is the same
//! idea in the write direction — a field becomes settable by being declared, so a
//! CLI cannot fall behind the library.
//!
//! Both halves of the snapshot's view of a [`FieldValue`] matter. `raw` is the field's
//! bits with no type applied, so it pins the placement — move a range by one bit and it
//! changes on nearly every specimen. `value` is the decoded rendering, so it pins the
//! interpretation. A change to either is visible, and they fail in different places.

use std::fmt::{self, Debug, Display, Formatter};

use crate::bits::Packed;

/// One decoded field of a panel.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FieldValue {
    /// The field's full registry path, e.g. `center_panel.transpose`.
    pub name: String,
    /// Where the bits sit, as `LO..=HI` over the declaring body's bytes.
    pub placement: &'static str,
    /// The field's bits as they were *read*, shifted down to bit 0. Carries no type, so
    /// it stays comparable across a retype.
    pub raw: u64,
    /// The bits the field's current value would *write*.
    ///
    /// Equal to [`raw`](Self::raw) on any panel that has not been edited — decode and
    /// encode are inverses — so the two diverging is exactly the set of pending changes.
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
    /// Every value the field's type accepts, rendered as `set_field` spells them. Empty for a field too wide to enumerate — see [`ENUMERABLE_BITS`].
    pub legal: fn() -> Vec<String>,
    /// Which panel control this field is, from its type's
    /// [`CONTROL`](crate::bits::Packed::CONTROL).
    pub control: ControlKind,
}

/// What the panel puts under a reader's finger.
///
/// The registry already says where a field sits and which values it takes; this says what
/// *kind* of thing it is, so a caller can choose a widget without a table of field names
/// beside it. It comes from the field's type, so a field gets it right by being declared
/// with the type that matches the control — a `bool` is a button, a [`Level`] is a knob.
///
/// [`Level`]: crate::components::Level
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ControlKind {
    /// A two-state button. Its two states may have names — see the field's `legal` values.
    Toggle,
    /// A selector over a fixed set of named values.
    Selector,
    /// A continuous knob or slider, reading in `unit`.
    Knob(Unit),
    /// A knob whose musical zero is its centre, reading in `unit` either side.
    Bipolar(Unit),
    /// A drawbar, `0..=8`, drawn as a bar rather than a number.
    Drawbar,
    /// The value a performance control morphs its parent parameter *to*. Belongs on that
    /// parent's control, not on one of its own.
    Morph,
    /// A per-step pattern grid.
    Pattern,
    /// An opaque id into one of the instrument's libraries.
    Reference,
    /// A signed shift, reading in `unit`.
    Shift(Unit),
    /// An integer nothing has been claimed about — the default, and a standing invitation
    /// to give the field a type that says more.
    Number,
}

/// What a control's reading is *in*.
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
    /// A subdivision of the master clock — `1/8`, `1/4 T`.
    ClockDivision,
    Semitones,
    Octaves,
    /// A stereo position, left through centre to right.
    Pan,
    /// No unit: a count, an index, or a raw byte.
    None,
}

impl Unit {
    /// Whether a value in this unit can be *computed* from the stored one.
    ///
    /// False for the units where the panel's curve is not published — a caller that wants
    /// to label an axis may still use the unit, but must print the stored value.
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
                    n if n > 12 => write!(
                        f,
                        " (accepts {} .. {})",
                        legal.first().unwrap(),
                        legal.last().unwrap()
                    ),
                    _ => write!(f, " (accepts {})", legal.join(", ")),
                }
            }
        }
    }
}

impl std::error::Error for FieldError {}

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

/// Every value of `T` that fits a `LO..=HI` field, in stored order, asked of the type
/// itself rather than kept in a second list beside it.
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
/// **The rendering is the vocabulary**: this walks the field's own bit patterns and takes
/// the one whose `Debug` matches, so a type gets string parsing from its `Debug` alone,
/// and a value outside its range has no pattern to match and fails here rather than being
/// clamped.
///
/// ⚠️ An unexplained value can therefore only be written by *naming* it as unexplained: a
/// sparse enum renders an unrecognized `9` as `Unknown(9)`, so a bare `9` matches nothing
/// and `Unknown(9)` is the only spelling.
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
        // A field too wide to walk is spelled by its stored bits. For the nine-nibble
        // drawbar blocks that is the readable form anyway: `0x087654321` is the nine
        // positions in order.
        if let Ok(v) = T::from_bits(bits) {
            return Ok(v);
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
/// Narrow fields are named — `Organ`, `-5`, `true` — and that name is what `--set` takes.
/// A field too wide to enumerate has no name, so its stored bits are the spelling, and
/// `raw` is exactly those bits.
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
    use crate::formats::ne5::{Level, Transpose};

    #[test]
    fn a_value_is_parsed_out_of_the_way_it_prints() {
        let v: Transpose = parse_field(4, "-5").unwrap();
        assert_eq!(v.inner(), -5);
        // The bias is the type's business, not the caller's: -5 stores as 1.
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

    /// A wide numeric field enumerates, so `--fields` can still say what it takes.
    #[test]
    fn legal_values_come_from_the_type() {
        assert_eq!(legal_values::<bool>(1), vec!["false", "true"]);
        let levels = legal_values::<Level>(7);
        assert_eq!(levels.len(), 128);
        assert_eq!(levels.last().unwrap(), "127");
    }

    /// The message has to name a way forward, or it is just a rejection.
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
