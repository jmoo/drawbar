//! Electro 4 and Electro 4D (`.ne4p`, `.ne4l`, `.ne4s`): container-verified,
//! bodies unmapped.
//!
//! As with the Electro 3 pair, the 4 and 4D factory exports are byte-identical.
//!
//! The program body has the same layout as the Electro 3's `nepg`; see [`super::ne3`]
//! for the measurement. Decoding either decodes both. Inferred from specimens; not
//! confirmed on hardware.

use super::raw::raw_format;

raw_format!(
    /// Programs (`.ne4p`).
    program,
    "ne4p",
    110
);
raw_format!(
    /// Live slots (`.ne4l`), the same length as a program.
    live,
    "ne4l",
    110
);
raw_format!(
    /// Settings (`.ne4s`).
    settings,
    "ne4s",
    33
);
