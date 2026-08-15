//! The rule schema: what a rule may say, and what it may say it about.

use crate::{Provenance, State};

/// A registry path, e.g. `center_panel.transpose`.
pub type Path = String;

/// A value spelled the way `Field::value` spells it: `B3`, `true`, `-5`.
pub type Value = String;

/// A test over a program's stored values.
///
/// ⚠️ A path the state does not carry reads as no value, so [`Cond::Is`] on it is false
/// and [`Cond::Not`] of that is true. A partial state therefore answers *fewer*
/// conditions, not wrong ones — but a caller building one by hand has to know it.
#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize))]
pub enum Cond {
    Is(Path, Value),
    In(Path, Vec<Value>),
    Any(Vec<Cond>),
    All(Vec<Cond>),
    Not(Box<Cond>),
}

impl Cond {
    /// A condition on nothing, which holds in every state.
    pub fn always() -> Cond {
        Cond::All(Vec::new())
    }

    pub fn holds(&self, state: &State) -> bool {
        match self {
            Cond::Is(path, want) => state.value(path) == Some(want.as_str()),
            Cond::In(path, want) => state
                .value(path)
                .is_some_and(|held| want.iter().any(|w| w == held)),
            Cond::Any(conds) => conds.iter().any(|c| c.holds(state)),
            Cond::All(conds) => conds.iter().all(|c| c.holds(state)),
            Cond::Not(cond) => !cond.holds(state),
        }
    }

    /// Every `(path, value)` this names, so the registry-validation test can walk them.
    pub fn pairs(&self) -> Vec<(&str, &str)> {
        match self {
            Cond::Is(path, value) => vec![(path.as_str(), value.as_str())],
            Cond::In(path, values) => values.iter().map(|v| (path.as_str(), v.as_str())).collect(),
            Cond::Any(conds) | Cond::All(conds) => conds.iter().flat_map(Cond::pairs).collect(),
            Cond::Not(cond) => cond.pairs(),
        }
    }
}

/// A registry path, or a trailing-`*` prefix over one: `organ_panel.b3_*`.
#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize))]
pub struct PathPattern(String);

impl PathPattern {
    pub fn new(pattern: impl Into<String>) -> PathPattern {
        PathPattern(pattern.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Whether the pattern names `path`.
    pub fn matches(&self, path: &str) -> bool {
        match self.0.strip_suffix('*') {
            Some(prefix) => path.starts_with(prefix),
            None => self.0 == path,
        }
    }

    /// Whether the pattern names everything under `prefix` rather than a part of it.
    ///
    /// What tells a blanket rule over a section from one of the section's own: an
    /// `organ_panel.*` gate covers the whole organ section, an `organ_panel.b3_*` gate
    /// speaks only for the B3's share of it.
    pub fn covers_all_under(&self, prefix: &str) -> bool {
        self.0
            .strip_suffix('*')
            .is_some_and(|pattern| prefix.starts_with(pattern))
    }
}

impl From<&str> for PathPattern {
    fn from(pattern: &str) -> PathPattern {
        PathPattern::new(pattern)
    }
}

/// One fact about the panel.
#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize))]
pub enum Rule {
    /// Controls live only while `when` holds.
    Gate {
        when: Cond,
        controls: Vec<PathPattern>,
        provenance: Provenance,
    },
    /// Under `when`, `path` offers only `to`. The current value is re-added by the
    /// engine, so narrowing never traps a program in a value it already holds.
    Narrow {
        path: Path,
        when: Cond,
        to: Vec<Value>,
        provenance: Provenance,
    },
    /// An edit to `edit` also sets `also` to `value`.
    Couple {
        edit: Path,
        also: Path,
        value: Value,
        provenance: Provenance,
    },
}

impl Rule {
    pub fn provenance(&self) -> Provenance {
        match self {
            Rule::Gate { provenance, .. }
            | Rule::Narrow { provenance, .. }
            | Rule::Couple { provenance, .. } => *provenance,
        }
    }
}

/// Bits no field claims, which real programs hold and the panel cannot produce.
///
/// No rule can say this: a rule keys on a registry path, and bits nothing declares have
/// none. They ride through a re-encode verbatim — that is the round-trip invariant doing
/// its job — and [`check`](crate::DeviceModel::check) is where they get named.
///
/// [`bits`](Self::bits) is an inclusive range over the *entity body*, MSB-first from
/// byte 0, the numbering `#[bits]` uses. Resolve it from the published layout rather
/// than counting by hand.
#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize))]
pub struct Vestige {
    /// What a finding calls it: the field it would be, if it were one.
    pub name: &'static str,
    pub bits: (u32, u32),
    /// The only value a panel store leaves here.
    pub panel_writes: u64,
    pub provenance: Provenance,
}

impl Vestige {
    /// The bits as `body` holds them, or `None` if the body is too short.
    pub fn read(&self, body: &[u8]) -> Option<u64> {
        let (lo, hi) = self.bits;
        if hi as usize / 8 >= body.len() {
            return None;
        }
        Some((lo..=hi).fold(0u64, |held, bit| {
            let set = body[bit as usize / 8] & (0x80 >> (bit % 8)) != 0;
            (held << 1) | set as u64
        }))
    }
}

/// The stored number behind a value the library could not name.
///
/// `nord-format` renders one as `unknown (5)`, and no panel position writes a value that
/// renders that way — the name is the library's admission that nothing is known about it.
pub fn unnamed(value: &str) -> Option<u32> {
    value
        .strip_prefix("unknown (")?
        .strip_suffix(')')?
        .parse()
        .ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_pattern_names_one_path_or_a_prefix_of_them() {
        let exact = PathPattern::new("center_panel.gain");
        assert!(exact.matches("center_panel.gain"));
        assert!(!exact.matches("center_panel.gain2"));

        let glob = PathPattern::new("organ_panel.b3_*");
        assert!(glob.matches("organ_panel.b3_vib"));
        assert!(!glob.matches("organ_panel.vox_vib"));
    }

    /// A blanket rule over a section and one of the section's own rules have to be
    /// distinguishable, or a section can never be asked about as a whole.
    #[test]
    fn only_a_blanket_pattern_covers_a_whole_section() {
        assert!(PathPattern::new("organ_panel.*").covers_all_under("organ_panel."));
        assert!(!PathPattern::new("organ_panel.b3_*").covers_all_under("organ_panel."));
        assert!(!PathPattern::new("organ_panel.b3_vib").covers_all_under("organ_panel."));
    }

    #[test]
    fn a_condition_on_a_path_the_state_does_not_carry_is_false() {
        let empty = State::new();
        let cond = Cond::Is("center_panel.lower_part".into(), "Organ".into());
        assert!(!cond.holds(&empty));
        assert!(Cond::Not(Box::new(cond)).holds(&empty));
        assert!(Cond::always().holds(&empty));
    }

    #[test]
    fn a_value_the_library_could_not_name_says_which_number_it_was() {
        assert_eq!(unnamed("unknown (6)"), Some(6));
        assert_eq!(unnamed("B3"), None);
    }

    /// The bits read MSB-first from byte 0, the way `#[bits]` counts them.
    #[test]
    fn a_vestige_reads_its_bits_out_of_the_body() {
        let vestige = |lo, hi| Vestige {
            name: "test",
            bits: (lo, hi),
            panel_writes: 0,
            provenance: Provenance::Unexplained,
        };
        let body = [0b1000_0001u8, 0b0110_0000];
        assert_eq!(vestige(0, 0).read(&body), Some(1));
        assert_eq!(vestige(1, 1).read(&body), Some(0));
        assert_eq!(vestige(7, 10).read(&body), Some(0b1011));
        assert_eq!(vestige(16, 16).read(&body), None);
    }
}
