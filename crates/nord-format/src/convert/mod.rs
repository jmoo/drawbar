//! Converting sample instruments between generations.
//!
//! Every conversion goes through one model of the instrument, read from the source and
//! written in the target. Each stroke's audio stays as its stream stores it, fields on
//! the 35,002 Hz lattice at their quantizer shift, so a generation change lays the same
//! fields out in the target's units and never resamples. Where the target's shift rule
//! is coarser than the source's, the result is the stream the editor renders in the
//! target generation.
//!
//! [`plan`] reads the source and works out the conversion before anything is written:
//! its [`Report`] names every value the target drops, holds differently, or fills in
//! by a rule, and [`Plan::open`] lists the losses the target can meet more than one
//! way. Each of those is a [`Choices`] field, and nothing answers one by default:
//! [`Plan::apply`] refuses while any is open.
//!
//! ```no_run
//! # use nord_format::convert::{self, Choices, Target};
//! # use nord_format::formats::nsmp::codec::Layout;
//! # let entity = nord_format::from_path("strings.nsmp4").unwrap();
//! let choices = Choices {
//!     gain: None,
//!     name: None,
//!     overlap: None,
//!     loop_mark: None,
//! };
//! let plan = convert::plan(&entity, Target::Nsmp(Layout::V2), &choices).unwrap();
//! for line in &plan.report().dropped {
//!     eprintln!("dropped {}: {} ({})", line.field, line.value, line.reason);
//! }
//! let instrument = plan.apply().unwrap();
//! ```

mod generation;
mod hub;
mod report;
#[cfg(test)]
mod tests;

pub use report::{Field, Line, Reason, Report, ZoneField};

use crate::error::Error;
use crate::formats::nsmp::codec::Layout;
use crate::{Entity, Sample};
use std::fmt;

/// What a conversion writes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Target {
    /// A sample instrument in the generation the layout names.
    Nsmp(Layout),
}

/// The answer to each loss the target can meet more than one way. `None` leaves it
/// open, and [`Plan::apply`] refuses while one that applies is open.
///
/// There is no default: a caller states every answer, so none is assumed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Choices {
    /// A zone gain past what the narrow zone record holds.
    pub gain: Option<GainChoice>,
    /// A name longer than the target's field, or a new name for any conversion.
    pub name: Option<NameChoice>,
    /// Zones whose key ranges overlap, going to the narrow chain, where zones tile.
    pub overlap: Option<OverlapChoice>,
    /// A loop mark nearer the resync point than the target's minimum gap.
    pub loop_mark: Option<LoopMarkChoice>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GainChoice {
    /// Store the largest gain the record holds.
    Clamp,
    /// Store the largest gain the record holds and scale the fields by the rest.
    Bake,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NameChoice {
    /// Keep the bytes the target's field holds.
    Truncate,
    Rename(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OverlapChoice {
    /// The lower zone keeps the keys both zones answer to.
    Lower,
    /// The upper zone keeps them.
    Upper,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LoopMarkChoice {
    /// Move the resync point earlier. The fields are unchanged.
    Resync,
    /// Move the mark later, repeating more of the loop.
    Push,
}

/// A loss that waits for a [`Choices`] answer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Choice {
    Gain,
    Name,
    Overlap,
    LoopMark,
}

/// One open choice: which, and the field and source value that need it.
#[derive(Debug, Clone, PartialEq)]
pub struct Open {
    pub choice: Choice,
    pub field: Field,
    pub value: String,
}

impl fmt::Display for Choice {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Choice::Gain => "a zone gain past what the target stores",
            Choice::Name => "a name longer than the target's field",
            Choice::Overlap => "overlapping zones where the target's zones tile",
            Choice::LoopMark => "a loop mark nearer the resync point than the target allows",
        })
    }
}

/// Why a conversion cannot be planned or applied.
#[derive(Debug, thiserror::Error)]
pub enum ConvertError {
    #[error("a {0:?} file, not a sample instrument")]
    NotASample(crate::EntityKind),
    #[error("{} choice(s) are open", .0.len())]
    Open(Vec<Open>),
    #[error("{0}")]
    Read(Error),
    #[error("{0}")]
    Write(Error),
}

/// A conversion worked out before anything is written.
#[derive(Debug)]
pub struct Plan {
    report: Report,
    open: Vec<Open>,
    output: Option<Sample>,
}

impl Plan {
    pub fn report(&self) -> &Report {
        &self.report
    }

    /// The choices still unanswered.
    pub fn open(&self) -> &[Open] {
        &self.open
    }

    /// The converted instrument, once no choice is open.
    pub fn apply(self) -> Result<Sample, ConvertError> {
        match self.output {
            Some(output) if self.open.is_empty() => Ok(output),
            _ => Err(ConvertError::Open(self.open)),
        }
    }
}

/// Work out converting `entity` to `target` under `choices`.
///
/// The report covers every field the model reads and every stored byte it does not.
/// Lines that depend on the converted streams, such as a shift the target's rule
/// coarsens, appear once no choice is open.
pub fn plan(entity: &Entity, target: Target, choices: &Choices) -> Result<Plan, ConvertError> {
    let Entity::Sample(sample) = entity else {
        return Err(ConvertError::NotASample(entity.kind()));
    };
    let Target::Nsmp(layout) = target;
    generation::plan(sample, layout, choices)
}
