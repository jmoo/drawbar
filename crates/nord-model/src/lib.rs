//! What a Nord instrument's panel allows, as data.
//!
//! `nord-format`'s registry answers what a field *is* — where its bits sit, which values
//! its type takes. This crate answers what the panel does with it: which controls are
//! live given the rest of the state, which values are offerable in context, what moves
//! together on an edit, and whether a stored state is one the panel could have produced.
//!
//! ⚠️ **Advisory only.** Nothing here gates decode or encode. Real programs hold states
//! the panel cannot reach, and `to_bytes(from_stream(x)) == x` is what keeps them
//! readable; [`DeviceModel::check`] reports such a state, it never refuses one.
//!
//! Rules key on registry paths (`center_panel.transpose`) and the value spellings
//! `Field::value` uses (`B3`, `true`, `-5`). Those strings are pinned by this crate's
//! tests against `field_specs()`, so a renamed field fails the build rather than the GUI.
//!
//! Not affiliated with, authorized, or endorsed by Clavia DMI AB.

mod engine;
mod rules;
mod state;

pub use engine::{Applied, Control, DeviceModel, Finding, Surface};
pub use rules::{unnamed, Cond, Path, PathPattern, Rule, Value, Vestige};
pub use state::State;

/// How a rule is known.
///
/// Three states, so a rule cannot be authored without picking one. An observation on the
/// instrument, an inference from stored files, and a fact with no panel explanation at
/// all are different things, and a reader downstream has to be able to tell them apart.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[cfg_attr(feature = "serde", derive(serde::Serialize))]
pub enum Provenance {
    /// Confirmed on hardware.
    ConfirmedOnHardware,
    /// Inferred from specimens; not confirmed on hardware.
    InferredFromSpecimens,
    /// Unexplained: real programs hold this, and the panel cannot produce it.
    Unexplained,
}

/// The hardware axis a file does not record.
///
/// A program says nothing about the instrument it was written on, so anything that
/// differs across keybeds — the split-point table above all — has to be told.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[cfg_attr(feature = "serde", derive(serde::Serialize))]
pub struct Variant {
    pub product: Product,
    pub keys: Keybed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[cfg_attr(feature = "serde", derive(serde::Serialize))]
pub enum Product {
    Electro5,
}

/// Which keyboard the instrument has.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[cfg_attr(feature = "serde", derive(serde::Serialize))]
pub enum Keybed {
    Keys61,
    Keys73,
    Hp,
}

impl Keybed {
    /// The split points this keybed offers, spelled as the registry spells them, or
    /// `None` where they are not known.
    ///
    /// ⚠️ `None` is *unanswered*, not unrestricted: only the 73-key table has been read
    /// off an instrument, and it is the one `nord_format::components::SplitPoint73`
    /// decodes. What a 61-key or HP panel offers at each of the eight stored positions
    /// is an open question — a rental would answer it in a minute.
    pub fn split_points(&self) -> Option<&'static [&'static str]> {
        match self {
            Keybed::Keys73 => Some(&["C3", "F3", "C4", "F4", "C5", "F5", "Upper", "Lower"]),
            Keybed::Keys61 | Keybed::Hp => None,
        }
    }
}
