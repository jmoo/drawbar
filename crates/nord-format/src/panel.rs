//! Field-level introspection over a bit-packed panel.
//!
//! [`Panel`] lets a caller walk a panel's fields without naming any of them, which is
//! what the decode snapshot needs: a new field appears in the output by being declared,
//! not by anyone remembering to add it.
//!
//! Both halves of [`FieldValue`] matter. `raw` is the field's bits with no type applied,
//! so it pins the placement — move a range by one bit and it changes on nearly every
//! specimen. `value` is the decoded rendering, so it pins the interpretation. A change to
//! either is visible, and they fail in different places.

use std::fmt::{self, Display, Formatter};

/// One decoded field of a panel.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FieldValue {
    /// The field's name as declared on the panel.
    pub name: &'static str,
    /// Where the bits sit, as `LO..=HI` over the panel's bytes.
    pub placement: &'static str,
    /// The field's bits, shifted down to bit 0. Carries no type, so it stays comparable
    /// across a retype.
    pub raw: u64,
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

/// A panel that can list its own fields. Implemented by `#[bitpanel]`.
pub trait Panel {
    /// The panel's type name.
    const NAME: &'static str;

    /// Every declared field, in declaration order. Bits no field claims are not
    /// reported — by construction there is no name to report them under.
    fn field_values(&self) -> Vec<FieldValue>;
}
