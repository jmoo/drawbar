//! Nord Stage 4 (`.ns4p`, `.ns4l`, `.ns4y`, `.ns4n`, `.ns4o`, `.ns4t`) —
//! container-verified, bodies unmapped.
//!
//! The Stage 4 splits presets by section: synth (`.ns4y`), piano (`.ns4n`) and
//! organ (`.ns4o`) each bank separately beside the programs.

use super::raw::raw_format;

raw_format!(
    /// Programs (`.ns4p`).
    program,
    "ns4p",
    824
);
raw_format!(
    /// Live slots (`.ns4l`) — same length as a program.
    live,
    "ns4l",
    824
);
raw_format!(
    /// Synth presets (`.ns4y`).
    synth,
    "ns4y",
    497
);
raw_format!(
    /// Piano presets (`.ns4n`).
    piano_preset,
    "ns4n",
    151
);
raw_format!(
    /// Organ presets (`.ns4o`).
    organ_preset,
    "ns4o",
    139
);
raw_format!(
    /// Settings (`.ns4t`).
    settings,
    "ns4t",
    80
);
