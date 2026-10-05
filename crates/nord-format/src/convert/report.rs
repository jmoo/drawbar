//! What a conversion loses, changes and fills in, as typed lines.

use crate::formats::nsmp::codec::Layout;
use std::fmt;
use std::ops::Range;

/// Every line a conversion reports, grouped as the user acts on them.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Report {
    /// Source values the target has no place for.
    pub dropped: Vec<Line>,
    /// Values the target holds differently from the source.
    pub changed: Vec<Line>,
    /// Target values the source does not state, filled in by a rule.
    pub from_rules: Vec<Line>,
}

impl Report {
    pub fn is_empty(&self) -> bool {
        self.dropped.is_empty() && self.changed.is_empty() && self.from_rules.is_empty()
    }
}

/// One field, the source's value for it, and why the target does not carry it as is.
#[derive(Debug, Clone, PartialEq)]
pub struct Line {
    pub field: Field,
    /// The source's value, as the field's own reader renders it.
    pub value: String,
    pub reason: Reason,
}

/// A field, named as `nord-format` names it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Field {
    /// The container header's content version.
    ContentVersion,
    /// The container header's `aux` word.
    Aux,
    /// The container header's library location.
    Location,
    Name,
    SubName,
    Category,
    SubCategory,
    Timbre,
    Envelope,
    Motion,
    Production,
    Origin,
    /// The keyboard map's per-key records.
    KeyTable,
    VelocityToAmplitude,
    VelocityToTimbre,
    /// A zone, by its index in the source's stored order.
    Zone {
        index: usize,
        field: ZoneField,
    },
    /// Stored bytes no field names, by section and range.
    Bytes {
        section: String,
        range: Range<usize>,
        /// What the bytes are, where that is known though not read.
        known_as: Option<&'static str>,
    },
}

/// A field of one zone or its stroke.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ZoneField {
    GlobalId,
    TopNote,
    LowNote,
    Gain,
    Velocity,
    LoopDecay,
    /// The stroke's quantizer shift.
    Shift,
    /// The stroke's statistic B.
    Peak,
    /// Where the stroke's loop mark sits.
    LoopMark,
    /// Where the stroke's stream resynchronizes.
    Resync,
    /// The stroke's record stream.
    Stream,
}

/// Why a line is reported.
#[derive(Debug, Clone, PartialEq)]
pub enum Reason {
    /// The target generation has no field for the value.
    NoField(Layout),
    /// No reader in this crate places these bytes.
    Unmodeled,
    /// The section's schema version is not the one the target writes.
    Schema {
        version: u32,
    },
    /// The early narrow chain stores no such field.
    EarlyChain,
    /// The writer stamps its own value.
    Written {
        value: String,
    },
    /// The target takes what the editor writes when nothing sets the field.
    EditorDefault,
    /// Shortened to the bytes the target's field holds.
    Truncated {
        capacity: usize,
    },
    Renamed,
    /// Set to the largest value the target stores.
    Clamped {
        to: String,
    },
    /// Set to the largest value the target stores, with the excess scaled into the
    /// fields; `clipped` fields then dequantize past 16 bits.
    Baked {
        factor: f64,
        clipped: usize,
    },
    /// Zones tile in the target, so a zone reaches down to the one below it.
    Tiled {
        reaches: u8,
    },
    /// The lower zone keeps the keys two zones share.
    LowerKeeps,
    /// The upper zone keeps the keys two zones share.
    UpperKeeps,
    /// Zone records name strokes by one byte, so every stroke is numbered again.
    Renumbered,
    /// The resync point moves earlier so the mark keeps the target's minimum gap.
    ResyncMoved {
        to: usize,
    },
    /// The mark moves later to the target's minimum gap, and the loop repeats that
    /// much more of itself.
    MarkPushed {
        to: usize,
    },
    /// The loop repeats less of itself, as the target's minimum gap allows. Playback is
    /// unchanged.
    MarkPulled {
        to: usize,
    },
    /// The loop is laid out over this many periods to fit the target's opening run
    /// and packets. Playback is unchanged.
    LoopRepeated {
        periods: usize,
    },
    /// The target's shift rule spends a bit the source kept.
    ShiftRule {
        layout: Layout,
        to: i32,
    },
    /// The wide header's decibels and statistic A come from the narrow record's
    /// linear gain, which is coarser than the decibels and wraps past a gain of 16.
    NarrowGain,
    /// The narrow record's linear gain comes from the wide header's decibels, which
    /// do not keep every bit of the gain they were computed from.
    DecibelGain,
    /// The narrow statistic stores no sign; it is taken from the stream's content.
    SignFromContent,
    /// The source's record coding is not the editor's, so the stream is laid out
    /// again. The fields are unchanged.
    Recoded,
    /// The writer cannot lay the source out again in its own generation, so whether
    /// its coding is the editor's is unknown, and the stream is laid out again.
    NotRebuilt {
        why: String,
    },
}

impl fmt::Display for Field {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Field::ContentVersion => write!(f, "header.version"),
            Field::Aux => write!(f, "header.aux"),
            Field::Location => write!(f, "header.location"),
            Field::Name => write!(f, "name"),
            Field::SubName => write!(f, "sub_name"),
            Field::Category => write!(f, "cat.category"),
            Field::SubCategory => write!(f, "cat.sub_category"),
            Field::Timbre => write!(f, "cat.timbre"),
            Field::Envelope => write!(f, "cat.envelope"),
            Field::Motion => write!(f, "cat.motion"),
            Field::Production => write!(f, "cat.production"),
            Field::Origin => write!(f, "cat.origin"),
            Field::KeyTable => write!(f, "key_table"),
            Field::VelocityToAmplitude => write!(f, "sty.velocity_to_amplitude"),
            Field::VelocityToTimbre => write!(f, "sty.velocity_to_timbre"),
            Field::Zone { index, field } => write!(f, "zones[{index}].{field}"),
            Field::Bytes {
                section,
                range,
                known_as,
            } => {
                write!(f, "{section}[{}..{}]", range.start, range.end)?;
                match known_as {
                    Some(name) => write!(f, " ({name})"),
                    None => Ok(()),
                }
            }
        }
    }
}

impl fmt::Display for ZoneField {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            ZoneField::GlobalId => "global_id",
            ZoneField::TopNote => "top_note",
            ZoneField::LowNote => "low_note",
            ZoneField::Gain => "gain",
            ZoneField::Velocity => "velocity",
            ZoneField::LoopDecay => "loop_decay",
            ZoneField::Shift => "shift",
            ZoneField::Peak => "peak",
            ZoneField::LoopMark => "loop_mark",
            ZoneField::Resync => "resync",
            ZoneField::Stream => "stream",
        })
    }
}

impl fmt::Display for Reason {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Reason::NoField(layout) => write!(f, "{} has no field for it", layout.generation()),
            Reason::Unmodeled => write!(f, "no reader places these bytes"),
            Reason::Schema { version } => {
                write!(f, "schema version {version} is not the one written")
            }
            Reason::EarlyChain => write!(f, "the early chain stores none"),
            Reason::Written { value } => write!(f, "the writer stamps {value}"),
            Reason::EditorDefault => write!(f, "the editor's default"),
            Reason::Truncated { capacity } => write!(f, "truncated to {capacity} bytes"),
            Reason::Renamed => write!(f, "renamed"),
            Reason::Clamped { to } => write!(f, "clamped to {to}"),
            Reason::Baked { factor, clipped } => write!(
                f,
                "clamped, with the rest scaled into the audio (x{factor:.4}); {clipped} \
                 field(s) clip"
            ),
            Reason::Tiled { reaches } => {
                write!(f, "zones tile, so the zone reaches down to note {reaches}")
            }
            Reason::LowerKeeps => write!(f, "the lower zone keeps the shared keys"),
            Reason::UpperKeeps => write!(f, "the upper zone keeps the shared keys"),
            Reason::Renumbered => write!(f, "zone records name strokes by one byte"),
            Reason::ResyncMoved { to } => write!(f, "the resync point moves to field {to}"),
            Reason::MarkPushed { to } => write!(f, "the mark moves to field {to}"),
            Reason::MarkPulled { to } => {
                write!(f, "the mark moves to field {to}; playback is unchanged")
            }
            Reason::LoopRepeated { periods } => {
                write!(
                    f,
                    "the loop is laid out over {periods} periods; playback is unchanged"
                )
            }
            Reason::ShiftRule { layout, to } => {
                write!(f, "the {} shift rule takes {to}", layout.generation())
            }
            Reason::NarrowGain => write!(f, "derived from the narrow record's linear gain"),
            Reason::DecibelGain => write!(f, "derived from the wide header's decibels"),
            Reason::SignFromContent => write!(f, "the sign is taken from the content"),
            Reason::Recoded => write!(f, "laid out again; the fields are unchanged"),
            Reason::NotRebuilt { why } => write!(
                f,
                "laid out again; its own generation cannot be written ({why})"
            ),
        }
    }
}
