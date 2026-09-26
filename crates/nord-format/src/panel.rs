//! How a body's fields appear to a player: which controls sit together, and which of them
//! the instrument uses for the state the file holds.
//!
//! The field registry says where a field sits, what it accepts, and what kind of control
//! it is. It cannot say three things, because none of them is a property of a placement:
//!
//! - **Order.** The registry lists fields in bit order, so an organ layer's nine drawbars
//!   need not be adjacent and a knob need not follow the switch that arms it.
//! - **Grouping.** A dotted prefix is the only structure a path carries, and a body as
//!   flat as the Electro 5 program has one prefix per panel and nothing below it.
//! - **Relevance.** Which controls the instrument uses depends on state: an Electro 5
//!   keeps every organ model's registration and plays one, and a Stage keeps every layer
//!   and enables some.
//!
//! A [`Panel`] states all three as data, per format. It is written by hand, because
//! semantics cannot be derived from bit placement, and as data it can be inspected and
//! tested.
//!
//! # What a caller may rely on
//!
//! - [`of`] returns a decoded file's layout, or `None` where none has been written.
//!   Absence is normal: no format is required to have a layout, and a caller falls back
//!   to its own presentation.
//! - [`Panel::resolve`] is the whole render path: one call over a body's `fields()`
//!   returns the groups with their fields, their effective relevance, and whatever no
//!   group named. It indexes the field list once. [`Panel::named`] and
//!   [`Panel::leftovers`] ask the same questions of a body's specs, for a caller
//!   inspecting a layout without a file.
//! - A [`Group`] names its members in reading order: the order the panel shows them, not
//!   the bit order.
//! - A path a group names is a real registry path of that body, and no path is named
//!   twice. A test in this module checks both for every layout the crate ships, so a
//!   layout cannot drift as a body gains fields.
//! - [`Panel::exhaustive`] says whether the groups account for every registered field. If
//!   it is false, the leftovers can be non-empty, and a caller needs somewhere to put
//!   them.
//! - No group names a morph slot: the slot belongs to the parameter it morphs, which the
//!   group names, and [`FieldSpec::morph_parent`] resolves the relation. That is why a
//!   Stage body, most of whose fields are morph slots, needs a layout of about a hundred
//!   lines instead of one line per field.
//!
//! # Relevance is not visibility
//!
//! [`Group::is_relevant`] answers one question: for the state this file holds, is the
//! instrument using these controls? A group that is not relevant is still state the file
//! carries, and still writable: an organ registration for a model that is not selected is
//! kept. Whether an irrelevant group is hidden, dimmed, or folded is the caller's
//! decision, and it may differ by depth: a whole section nobody is playing is worth
//! hiding, while the second of two registrations is worth showing quietly.
//!
//! A condition is a set of value matches, any one of which satisfies it, and a nested
//! group is relevant only if its parent is. A disjunction is a wider condition, and a
//! conjunction is another level of nesting. This is less than a predicate language, so
//! every condition stays comparable, printable, and checkable against the field's legal
//! values.

use std::collections::{HashMap, HashSet};

use crate::fields::{Field, FieldSpec};
use crate::formats::{ne5, ns4};
use crate::{Entity, Live, Program};

/// One body's controls, grouped the way its instrument groups them.
///
/// ⚠️ Not the Electro 5's `CenterPanel` and similar types, which are bodies: nested
/// `#[bitbody]`s at a byte range. This is the panel as a reader sees it, and it cuts
/// across those bodies freely.
#[derive(Debug)]
pub struct Panel {
    /// The sections, in the order a reader meets them.
    pub groups: &'static [Group],
    /// Whether the groups account for every field the body registers.
    ///
    /// This module's tests check a true value, so an exhaustive layout stays exhaustive
    /// as the body grows: a newly declared field fails the test until it is placed. False
    /// means [`Self::leftovers`] can be non-empty and a caller needs somewhere to put
    /// them.
    pub exhaustive: bool,
}

/// How a group is chosen when it is one of several stored alternatives: writing `value`
/// to `field` makes the instrument play this one.
///
/// Not a relevance condition. The other alternatives stay relevant, because a stored
/// preset the instrument is not playing is still state the panel offers. A group whose
/// [`Group::when`] fails is one the instrument is not using at all.
#[derive(Debug)]
pub struct Selection {
    /// The field that chooses between the sibling groups.
    pub field: &'static str,
    /// The value that selects this group, spelled as [`Field::value`] spells it.
    pub value: &'static str,
}

impl Selection {
    /// Whether the body's current value selects this group.
    pub fn selected(&self, fields: &[Field]) -> bool {
        fields
            .iter()
            .find(|field| field.path == self.field)
            .is_some_and(|field| field.value == self.value)
    }
}

/// One run of controls under a title, and the state that makes them relevant.
#[derive(Debug)]
pub struct Group {
    pub title: &'static str,
    /// Registry paths, in reading order.
    ///
    /// A member ending in `.*` is a nested body's prefix and stands for every field that
    /// body registers, in registry order: `organ_a.*` covers the whole organ body
    /// without naming its fields.
    pub members: &'static [&'static str],
    /// Groups within this one. Nesting has no depth limit, so a caller should recurse.
    pub groups: &'static [Group],
    /// What makes this group relevant, or `None` for a group that always is.
    pub when: Option<Relevance>,
    /// How this group is selected, where it is one of several stored alternatives such
    /// as the two organ presets; `None` for a group that is not an alternative.
    pub selected_by: Option<Selection>,
}

/// A condition on the body's own values: satisfied when any match holds.
#[derive(Debug)]
pub struct Relevance {
    pub any_of: &'static [Match],
}

/// One field holding one of a set of values.
#[derive(Debug)]
pub struct Match {
    /// A registry path of the same body.
    pub field: &'static str,
    /// The values that satisfy it, spelled as [`Field::value`] spells them, which is
    /// also what `set_field` takes. A test checks each against the field's legal values,
    /// so a renamed variant fails the test instead of never matching.
    pub is: &'static [&'static str],
}

/// A body's fields by path, so resolving a layout takes one pass.
type Index<'a> = HashMap<&'a str, &'a Field>;

impl Match {
    /// Whether the field holds one of the values.
    ///
    /// A path the body does not register holds nothing, so an unknown field never
    /// satisfies a match. Scans `fields`; [`Panel::resolve`] answers the same question
    /// from an index when a whole layout is being drawn.
    pub fn holds(&self, fields: &[Field]) -> bool {
        fields
            .iter()
            .find(|field| field.path == self.field)
            .is_some_and(|field| self.matched(field))
    }

    fn holds_in(&self, index: &Index) -> bool {
        index
            .get(self.field)
            .is_some_and(|field| self.matched(field))
    }

    fn matched(&self, field: &Field) -> bool {
        self.is.iter().any(|value| *value == field.value)
    }
}

impl Relevance {
    /// Whether any match holds. An empty condition is satisfied.
    pub fn holds(&self, fields: &[Field]) -> bool {
        self.any_of.is_empty() || self.any_of.iter().any(|m| m.holds(fields))
    }

    fn holds_in(&self, index: &Index) -> bool {
        self.any_of.is_empty() || self.any_of.iter().any(|m| m.holds_in(index))
    }
}

impl Group {
    /// Whether the instrument is using this group's controls, for the state `fields`
    /// holds.
    ///
    /// ⚠️ This answers for the group alone. A nested group is relevant only if its parent
    /// is too, and nothing here checks the parent; a caller recursing top-down already
    /// knows. [`Panel::resolve`] combines both.
    pub fn is_relevant(&self, fields: &[Field]) -> bool {
        self.when.as_ref().is_none_or(|when| when.holds(fields))
    }

    /// This group's own members, in reading order, with any `prefix.*` expanded against
    /// the registry. Members of nested groups are not included.
    ///
    /// A `prefix.*` names that body's controls. A morph slot whose parameter the same
    /// body declares is left out, because it is drawn on that parameter; a slot whose
    /// parameter is missing is included like any other field.
    ///
    /// A member the body does not register is skipped. The tests require layouts to name
    /// only real fields, so a caller need not handle the case.
    pub fn members_of<'a>(&self, specs: &'a [FieldSpec]) -> Vec<&'a str> {
        members_in(self.members, specs, |member| {
            specs.iter().find(|spec| spec.name == member)
        })
        .into_iter()
        .map(|spec| spec.name.as_str())
        .collect()
    }

    /// Every group under this one, this one included, depth first.
    fn walk(&self) -> Vec<&Group> {
        let mut out = vec![self];
        for group in self.groups {
            out.extend(group.walk());
        }
        out
    }
}

/// What a layout reads from a registered field, whether from a body's specs or from its
/// values: the field's path, and the parameter it morphs if it is a morph slot.
trait Placed {
    fn path(&self) -> &str;
    fn morph_parent(&self) -> Option<String>;
}

impl Placed for FieldSpec {
    fn path(&self) -> &str {
        &self.name
    }

    fn morph_parent(&self) -> Option<String> {
        FieldSpec::morph_parent(self)
    }
}

impl Placed for Field {
    fn path(&self) -> &str {
        &self.path
    }

    fn morph_parent(&self) -> Option<String> {
        self.spec.morph_parent()
    }
}

/// The items `members` names, in reading order, with any `prefix.*` expanded to that
/// body's controls; morph slots are left to the parameters they are drawn on.
///
/// `find` resolves a plain member: a scan where a caller holds only the list, an index
/// lookup where a whole layout is being resolved against one body.
fn members_in<'a, T: Placed>(
    members: &[&str],
    items: &'a [T],
    find: impl Fn(&str) -> Option<&'a T>,
) -> Vec<&'a T> {
    let mut out = Vec::new();
    for member in members {
        match member.strip_suffix(".*") {
            Some(prefix) => out.extend(
                items
                    .iter()
                    .filter(|item| under(item.path(), prefix))
                    .filter(|item| item.morph_parent().is_none()),
            ),
            None => out.extend(find(member)),
        }
    }
    out
}

/// The items `claimed` does not cover, in registry order. A morph slot whose
/// parameter is claimed is not among them: it is drawn on that parameter's control.
fn unclaimed<T: Placed>(items: &[T], claimed: impl Fn(&str) -> bool) -> Vec<&T> {
    items
        .iter()
        .filter(|item| !claimed(item.path()))
        .filter(|item| !item.morph_parent().is_some_and(|parent| claimed(&parent)))
        .collect()
}

/// Whether `path` is a field of the body at `prefix`: one dotted segment deeper, not
/// merely sharing a text prefix.
fn under(path: &str, prefix: &str) -> bool {
    path.strip_prefix(prefix)
        .and_then(|rest| rest.strip_prefix('.'))
        .is_some_and(|leaf| !leaf.contains('.'))
}

impl Panel {
    /// Every group in the layout, sections and their nested clusters alike, depth first.
    ///
    /// ⚠️ Not [`groups`](Self::groups), which is the top level alone.
    pub fn walk(&self) -> Vec<&Group> {
        self.groups.iter().flat_map(Group::walk).collect()
    }

    /// Every path the layout names, in layout order, globs expanded.
    pub fn named<'a>(&self, specs: &'a [FieldSpec]) -> Vec<&'a str> {
        self.walk()
            .into_iter()
            .flat_map(|group| group.members_of(specs))
            .collect()
    }

    /// The registered fields no group names, in registry order.
    ///
    /// A morph slot whose parameter is named is not among them: it is drawn on that
    /// parameter's control, so a caller that has rendered the parameter has rendered it.
    pub fn leftovers<'a>(&self, specs: &'a [FieldSpec]) -> Vec<&'a str> {
        let named = self.named(specs);
        unclaimed(specs, |path| named.contains(&path))
            .into_iter()
            .map(|spec| spec.name.as_str())
            .collect()
    }
}

/// One group with the fields it names, resolved against a body: what a caller draws.
pub struct Section<'a> {
    pub group: &'a Group,
    /// Whether the instrument is using these controls: this group's own condition and
    /// every ancestor's. [`Group::is_relevant`] on `group` answers for this level alone,
    /// for a caller that wants to tell "the section is off" from "this cluster is not the
    /// selected one".
    pub relevant: bool,
    /// This group's own fields, in reading order, `prefix.*` expanded.
    pub fields: Vec<&'a Field>,
    /// The nested groups, resolved the same way.
    pub groups: Vec<Section<'a>>,
}

/// A whole layout resolved against one body: the render path, in one call.
pub struct Resolved<'a> {
    pub sections: Vec<Section<'a>>,
    /// The fields no group named, in registry order; empty for an exhaustive layout.
    /// A morph slot whose parameter was named is not among them; it is drawn on that
    /// parameter's control.
    pub leftovers: Vec<&'a Field>,
}

impl Panel {
    /// The layout against one body's field values: every group with its own fields and
    /// its effective relevance, plus whatever no group named.
    ///
    /// One pass builds an index of the body's paths, so drawing a layout costs about one
    /// walk of the field list, however many members the groups name. The Stage bodies are
    /// large enough for this to matter.
    pub fn resolve<'a>(&'a self, fields: &'a [Field]) -> Resolved<'a> {
        let index: Index<'a> = fields.iter().map(|f| (f.path.as_str(), f)).collect();
        let mut claimed: HashSet<&'a str> = HashSet::new();
        let sections = self
            .groups
            .iter()
            .map(|group| resolve_group(group, fields, &index, true, &mut claimed))
            .collect();
        let leftovers = unclaimed(fields, |path| claimed.contains(path));
        Resolved {
            sections,
            leftovers,
        }
    }
}

fn resolve_group<'a>(
    group: &'a Group,
    fields: &'a [Field],
    index: &Index<'a>,
    parent_relevant: bool,
    claimed: &mut HashSet<&'a str>,
) -> Section<'a> {
    let relevant = parent_relevant && group.when.as_ref().is_none_or(|when| when.holds_in(index));

    let own = members_in(group.members, fields, |member| index.get(member).copied());
    claimed.extend(own.iter().map(|field| field.path.as_str()));

    let groups = group
        .groups
        .iter()
        .map(|nested| resolve_group(nested, fields, index, relevant, claimed))
        .collect();

    Section {
        group,
        relevant,
        fields: own,
        groups,
    }
}

/// The layout for a decoded file's body, or `None` where none has been authored.
///
/// A live buffer is its model's program body under another tag, so the two share a
/// layout.
pub fn of(entity: &Entity) -> Option<&'static Panel> {
    match entity {
        Entity::Program(Program::Electro5(_)) | Entity::Live(Live::Electro5(_)) => {
            Some(&ne5::program::PANEL)
        }
        Entity::Program(Program::Stage4(_)) | Entity::Live(Live::Stage4(_)) => {
            Some(&ns4::program::PANEL)
        }
        _ => None,
    }
}

/// A layout and the registry it describes.
///
/// Every layout the crate ships is listed in [`AUTHORED`], which the consistency tests
/// walk, so every layout is checked without anyone having to remember.
pub struct Authored {
    pub name: &'static str,
    pub panel: &'static Panel,
    pub specs: fn() -> Vec<FieldSpec>,
}

pub const AUTHORED: &[Authored] = &[
    Authored {
        name: "ne5::Program",
        panel: &ne5::program::PANEL,
        specs: ne5::Program::field_specs,
    },
    Authored {
        name: "ns4::Program",
        panel: &ns4::program::PANEL,
        specs: ns4::Program::field_specs,
    },
];

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    /// A layout may only name fields the body registers, and a `prefix.*` must reach at
    /// least one.
    #[test]
    fn every_named_path_is_a_real_field() {
        for authored in AUTHORED {
            let specs = (authored.specs)();
            let known: HashSet<&str> = specs.iter().map(|spec| spec.name.as_str()).collect();
            for group in authored.panel.walk() {
                for member in group.members {
                    match member.strip_suffix(".*") {
                        Some(prefix) => assert!(
                            specs.iter().any(|spec| under(&spec.name, prefix)),
                            "{}: {} names no field under {prefix}",
                            authored.name,
                            group.title,
                        ),
                        None => assert!(
                            known.contains(member),
                            "{}: {} names {member}, which is not a field",
                            authored.name,
                            group.title,
                        ),
                    }
                }
            }
        }
    }

    /// A morph slot is drawn on the parameter it moves, so a layout never names one, and
    /// a `prefix.*` leaves them out.
    #[test]
    fn no_group_names_a_morph_slot_whose_parameter_is_declared() {
        for authored in AUTHORED {
            let specs = (authored.specs)();
            for path in authored.panel.named(&specs) {
                let spec = specs.iter().find(|spec| spec.name == path).expect(path);
                assert!(
                    spec.morph_parent().is_none(),
                    "{}: {path} is a morph slot of {:?}",
                    authored.name,
                    spec.morph_parent(),
                );
            }
        }
    }

    /// One field, one group. Two groups naming the same field would draw it twice and
    /// disagree about when it is relevant.
    #[test]
    fn no_field_is_named_twice() {
        for authored in AUTHORED {
            let specs = (authored.specs)();
            let mut seen = HashSet::new();
            for path in authored.panel.named(&specs) {
                assert!(
                    seen.insert(path),
                    "{}: {path} is in two groups",
                    authored.name
                );
            }
        }
    }

    /// A condition is checked against the field's legal values, so a renamed variant
    /// fails here and cannot become a condition that never holds.
    #[test]
    fn every_condition_names_a_field_and_values_it_accepts() {
        for authored in AUTHORED {
            let specs = (authored.specs)();
            for group in authored.panel.walk() {
                let Some(when) = &group.when else { continue };
                for m in when.any_of {
                    let spec = specs
                        .iter()
                        .find(|spec| spec.name == m.field)
                        .unwrap_or_else(|| {
                            panic!(
                                "{}: {} tests {}, which is not a field",
                                authored.name, group.title, m.field
                            )
                        });
                    let legal = (spec.legal)();
                    // A field too wide to enumerate lists nothing; its values are its
                    // stored bits and there is nothing to check them against.
                    if legal.is_empty() {
                        continue;
                    }
                    for value in m.is {
                        assert!(
                            legal.iter().any(|l| l == value),
                            "{}: {} tests {} for {value}, which it does not accept",
                            authored.name,
                            group.title,
                            m.field,
                        );
                    }
                }
            }
        }
    }

    /// A selection is written back through `set_field`, so it names a registered field
    /// and a value that field accepts. The selector is not a member of the group it
    /// selects, or a caller drawing only the selected group would lose the switch.
    #[test]
    fn every_selection_names_a_field_and_a_value_it_accepts() {
        for authored in AUTHORED {
            let specs = (authored.specs)();
            for group in authored.panel.walk() {
                let Some(selection) = &group.selected_by else {
                    continue;
                };
                let spec = specs
                    .iter()
                    .find(|spec| spec.name == selection.field)
                    .unwrap_or_else(|| {
                        panic!(
                            "{}: {} is selected by {}, which is not a field",
                            authored.name, group.title, selection.field
                        )
                    });
                assert!(
                    (spec.legal)().iter().any(|value| value == selection.value),
                    "{}: {} does not accept {}",
                    authored.name,
                    selection.field,
                    selection.value,
                );
                assert!(
                    !group.members_of(&specs).contains(&selection.field),
                    "{}: {} contains its own selector {}",
                    authored.name,
                    group.title,
                    selection.field,
                );
            }
        }
    }

    #[test]
    fn an_exhaustive_layout_leaves_nothing_out() {
        for authored in AUTHORED {
            if !authored.panel.exhaustive {
                continue;
            }
            let specs = (authored.specs)();
            assert_eq!(
                authored.panel.leftovers(&specs),
                Vec::<&str>::new(),
                "{} claims to be exhaustive",
                authored.name,
            );
        }
    }

    /// A prefix member reaches that body's own fields and no deeper.
    #[test]
    fn a_prefix_names_one_bodys_fields() {
        assert!(under("organ_a.drawbar_1", "organ_a"));
        assert!(!under("organ_ab.drawbar_1", "organ_a"));
        assert!(!under("organ_a.inner.leaf", "organ_a"));
        assert!(!under("drawbar_1", "organ_a"));
    }
}
