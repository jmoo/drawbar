//! The engine: what the rules answer, given a state.

use crate::rules::{unnamed, Cond, PathPattern, Rule, Value, Vestige};
use crate::{Provenance, State, Variant};

/// One instrument's behavior.
///
/// Rules are plain data and the engine only reads them, so a model is authored as rows
/// and reviewed as rows. ⚠️ Every answer is advisory: nothing here decides whether bytes
/// decode.
#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize))]
pub struct DeviceModel {
    pub variant: Variant,
    rules: Vec<Rule>,
    vestiges: Vec<Vestige>,
}

/// One control, as the panel would present it in this state.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Control {
    pub path: String,
    /// Whether the panel is listening to it at all.
    pub live: bool,
    /// The values a picker would offer. Always contains the current value.
    pub offerable: Vec<Value>,
}

/// What the panel shows for one state.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Surface {
    pub controls: Vec<Control>,
}

impl Surface {
    pub fn get(&self, path: &str) -> Option<&Control> {
        self.controls.iter().find(|control| control.path == path)
    }

    /// Whether the panel is listening to `path`. False for a path the surface has never
    /// heard of, which is the conservative reading.
    pub fn live(&self, path: &str) -> bool {
        self.get(path).is_some_and(|control| control.live)
    }
}

/// An edit, with everything it drags along.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Applied {
    /// The edits to write, the asked-for ones first.
    pub sets: Vec<(String, Value)>,
    /// The state those edits leave behind.
    pub state: State,
}

/// A stored value the panel could not have produced.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Finding {
    /// The registry path, or the vestige's name where no field claims the bits.
    pub path: String,
    /// What is stored there.
    pub value: String,
    /// What the panel would hold instead.
    pub why: String,
    pub provenance: Provenance,
}

impl DeviceModel {
    pub fn new(variant: Variant) -> DeviceModel {
        DeviceModel {
            variant,
            rules: Vec::new(),
            vestiges: Vec::new(),
        }
    }

    pub fn rule(mut self, rule: Rule) -> DeviceModel {
        self.rules.push(rule);
        self
    }

    pub fn vestige(mut self, vestige: Vestige) -> DeviceModel {
        self.vestiges.push(vestige);
        self
    }

    pub fn rules(&self) -> &[Rule] {
        &self.rules
    }

    pub fn vestiges(&self) -> &[Vestige] {
        &self.vestiges
    }

    // ── the questions ───────────────────────────────────────────────────────────

    /// Liveness and offerable values for every path the state carries.
    pub fn surface(&self, state: &State) -> Surface {
        Surface {
            controls: state
                .paths()
                .map(str::to_string)
                .collect::<Vec<_>>()
                .into_iter()
                .map(|path| Control {
                    live: self.live(state, &path),
                    offerable: self.choices(
                        state,
                        &path,
                        state.legal(&path),
                        state.value(&path).unwrap_or_default(),
                    ),
                    path,
                })
                .collect(),
        }
    }

    /// An edit and everything coupled to it.
    ///
    /// A coupled write never overrides one the caller asked for: an edit that names both
    /// halves of a pair itself means what it says.
    pub fn apply(&self, state: &State, edits: &[(String, Value)]) -> Applied {
        let mut sets: Vec<(String, Value)> = edits.to_vec();
        for (path, _) in edits {
            for (also, value) in self.coupled(path) {
                if !sets.iter().any(|(named, _)| named == also) {
                    sets.push((also.to_string(), value.to_string()));
                }
            }
        }
        let mut after = state.clone();
        for (path, value) in &sets {
            after.set(path.clone(), value.clone());
        }
        Applied { sets, state: after }
    }

    /// Stored values outside what the panel can produce.
    ///
    /// Lint output, never a refusal — every finding is a state the codec reads and
    /// writes back verbatim.
    pub fn check(&self, state: &State) -> Vec<Finding> {
        let mut findings = Vec::new();
        for path in state.paths().map(str::to_string).collect::<Vec<_>>() {
            let Some(held) = state.value(&path) else {
                continue;
            };
            if unnamed(held).is_some() {
                findings.push(Finding {
                    path: path.clone(),
                    value: held.to_string(),
                    why: "no panel position writes a value the library cannot name".into(),
                    provenance: Provenance::Unexplained,
                });
                continue;
            }
            for rule in &self.rules {
                let Rule::Narrow {
                    path: narrowed,
                    when,
                    to,
                    provenance,
                } = rule
                else {
                    continue;
                };
                if narrowed == &path && when.holds(state) && !to.iter().any(|v| v == held) {
                    findings.push(Finding {
                        path: path.clone(),
                        value: held.to_string(),
                        why: format!("the panel offers only {}", to.join(", ")),
                        provenance: *provenance,
                    });
                }
            }
        }
        let Some(body) = state.body() else {
            return findings;
        };
        for vestige in &self.vestiges {
            let Some(held) = vestige.read(body) else {
                continue;
            };
            if held != vestige.panel_writes {
                findings.push(Finding {
                    path: vestige.name.to_string(),
                    value: format!("{held:#x}"),
                    why: format!("a panel store leaves {:#x} here", vestige.panel_writes),
                    provenance: vestige.provenance,
                });
            }
        }
        findings
    }

    // ── the queries a projection asks one at a time ─────────────────────────────

    /// Whether the panel is listening to `path`: every gate naming it holds.
    pub fn live(&self, state: &State, path: &str) -> bool {
        self.gates()
            .filter(|(_, controls)| controls.iter().any(|p| p.matches(path)))
            .all(|(when, _)| when.holds(state))
    }

    /// Whether anything under `section` can be live: every gate speaking for the whole
    /// section holds.
    ///
    /// The section's own gates are not consulted — a section with no model selected is
    /// still a section the panel shows.
    pub fn section_live(&self, state: &State, section: &str) -> bool {
        self.gates()
            .filter(|(_, controls)| controls.iter().any(|p| p.covers_all_under(section)))
            .all(|(when, _)| when.holds(state))
    }

    /// Whether `path` is live within `section`, taking the section's own liveness as
    /// given.
    ///
    /// What a section projecting its own contents wants: whether *this* control is the
    /// one the current selection speaks through, regardless of whether the section is on
    /// screen at all.
    pub fn live_within(&self, state: &State, section: &str, path: &str) -> bool {
        self.gates_within(section, path)
            .all(|(when, _)| when.holds(state))
    }

    /// Whether the section has a gate of its own for `path`, rather than only the
    /// blanket one over the whole section.
    ///
    /// A newly declared field no rule mentions answers false, so it surfaces as the
    /// plain field it is instead of vanishing into a section that does not speak for it.
    pub fn gated_within(&self, section: &str, path: &str) -> bool {
        self.gates_within(section, path).next().is_some()
    }

    /// Whether `value` is one the panel would offer for `path` in this state.
    ///
    /// ⚠️ Not a claim that the value may be stored. A file holding an unofferable value
    /// is legal, and [`choices`](Self::choices) keeps it listed so an edit away from it
    /// stays possible.
    pub fn offers(&self, state: &State, path: &str, value: &str) -> bool {
        unnamed(value).is_none()
            && self
                .narrows(state, path)
                .all(|to| to.iter().any(|v| v == value))
    }

    /// The values a picker offers: the legal set less what no panel position writes,
    /// plus the current value however unofferable.
    ///
    /// ⚠️ The current value is always last and always present. Dropping it would trap a
    /// program in the one state it already holds.
    pub fn choices(
        &self,
        state: &State,
        path: &str,
        legal: &[String],
        current: &str,
    ) -> Vec<Value> {
        let mut out: Vec<Value> = legal
            .iter()
            .filter(|value| self.offers(state, path, value))
            .cloned()
            .collect();
        if !out.iter().any(|value| value == current) {
            out.push(current.to_string());
        }
        out
    }

    /// What an edit to `path` also writes: `(path, value)` for each coupled field.
    pub fn coupled(&self, edit: &str) -> Vec<(&str, &str)> {
        self.rules
            .iter()
            .filter_map(|rule| match rule {
                Rule::Couple {
                    edit: named,
                    also,
                    value,
                    ..
                } if named == edit => Some((also.as_str(), value.as_str())),
                _ => None,
            })
            .collect()
    }

    // ── internals ───────────────────────────────────────────────────────────────

    fn gates(&self) -> impl Iterator<Item = (&Cond, &[PathPattern])> {
        self.rules.iter().filter_map(|rule| match rule {
            Rule::Gate { when, controls, .. } => Some((when, controls.as_slice())),
            _ => None,
        })
    }

    fn gates_within<'a>(
        &'a self,
        section: &'a str,
        path: &'a str,
    ) -> impl Iterator<Item = (&'a Cond, &'a [PathPattern])> {
        self.gates().filter(move |(_, controls)| {
            path.starts_with(section)
                && controls
                    .iter()
                    .any(|p| p.matches(path) && !p.covers_all_under(section))
        })
    }

    /// The value sets every narrow in force imposes on `path`.
    fn narrows<'a>(&'a self, state: &'a State, path: &'a str) -> impl Iterator<Item = &'a [Value]> {
        self.rules.iter().filter_map(move |rule| match rule {
            Rule::Narrow {
                path: narrowed,
                when,
                to,
                ..
            } if narrowed == path && when.holds(state) => Some(to.as_slice()),
            _ => None,
        })
    }
}
