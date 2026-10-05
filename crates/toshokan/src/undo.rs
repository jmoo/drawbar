//! Undo and redo of a writer's own intents.
//!
//! Undo never edits the log: it appends compensating entries under a new intent whose
//! `Intent` entry names the intent it reverses. Redo reverses an undo the same way.
//!
//! The intents of the undo window replay as two stacks. An intent that reverses the
//! top of the undo stack is an undo, and moves to the redo stack; one that reverses
//! the top of the redo stack is a redo, and moves back. One that reverses an intent
//! outside the window goes on neither. Any other intent that changes a fact goes on
//! the undo stack and empties the redo stack.
//!
//! A field's write by an undo or redo stands for the write whose value it restores, so
//! undoing two intents in a row is not refused for the first undo's own write.

use std::collections::BTreeMap;
use std::fmt;

use crate::effects::{Effect, Precondition};
use crate::error::{Error, Result};
use crate::fs::RelPath;
use crate::ids::{EntityId, IntentId, Version};
use crate::log::{Entry, Kind, WriterLog};
use crate::merge::State;
use crate::value::{BlobId, Value};
use crate::PATH_FIELD;

/// Why an undo or redo was refused. Nothing was changed.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Refusal {
    /// The writer's undo window holds nothing to undo, or nothing to redo.
    Nothing,
    /// The field was written again after the intent wrote it.
    FieldChanged {
        entity: EntityId,
        name: String,
        wrote: Version,
        current: Option<Version>,
    },
    /// The bytes the intent displaced are no longer in the blob store.
    BlobGone { blob: BlobId },
}

impl fmt::Display for Refusal {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Nothing => f.write_str("there is nothing to reverse"),
            Self::FieldChanged { entity, name, .. } => {
                write!(f, "field {name:?} of {entity} has changed since")
            }
            Self::BlobGone { blob } => write!(f, "blob {blob} is no longer kept"),
        }
    }
}

/// A writer's own intents inside its undo window.
pub struct History {
    intents: BTreeMap<IntentId, Vec<Entry>>,
    undo: Vec<IntentId>,
    redo: Vec<IntentId>,
    /// For each field write of an undo or redo, the write it restores; `None` when
    /// it restores a field no write in the window decided.
    restores: BTreeMap<Version, Option<Version>>,
    /// Each field's deciding write as this writer's history alone would have it.
    current: BTreeMap<FieldKey, Option<Version>>,
    /// Those writes as they were before each intent.
    before: BTreeMap<IntentId, BTreeMap<FieldKey, Option<Version>>>,
}

type FieldKey = (EntityId, String);

impl History {
    pub fn new(own: &WriterLog) -> Self {
        let mut entries: Vec<&Entry> = own
            .all_entries()
            .filter(|entry| entry.intent.writer == own.writer)
            .collect();
        entries.sort_by_key(|entry| entry.version);
        entries.dedup_by_key(|entry| entry.version);
        let mut order = Vec::new();
        let mut intents: BTreeMap<IntentId, Vec<Entry>> = BTreeMap::new();
        for entry in entries {
            let group = intents.entry(entry.intent).or_insert_with(|| {
                order.push(entry.intent);
                Vec::new()
            });
            group.push(entry.clone());
        }
        let mut history = Self {
            intents,
            undo: Vec::new(),
            redo: Vec::new(),
            restores: BTreeMap::new(),
            current: BTreeMap::new(),
            before: BTreeMap::new(),
        };
        for intent in order {
            history.replay(intent);
        }
        history
    }

    fn replay(&mut self, intent: IntentId) {
        let entries = &self.intents[&intent];
        if !entries.iter().any(|entry| reversible(&entry.kind)) {
            return;
        }
        let reverses = entries.iter().find_map(|entry| match entry.kind {
            Kind::Intent { reverses, .. } => reverses,
            _ => None,
        });
        let writes = last_writes(entries);
        let target = match reverses {
            Some(target) if self.undo.last() == Some(&target) => {
                self.undo.pop();
                self.redo.push(intent);
                Some(target)
            }
            Some(target) if self.redo.last() == Some(&target) => {
                self.redo.pop();
                self.undo.push(intent);
                Some(target)
            }
            Some(target) if !self.intents.contains_key(&target) => None,
            _ => {
                self.undo.push(intent);
                self.redo.clear();
                None
            }
        };
        let mut before = BTreeMap::new();
        for (key, version) in writes {
            before.insert(key.clone(), self.current.get(&key).copied().flatten());
            let now = match target {
                Some(target) => {
                    let restored = self.before[&target].get(&key).copied().flatten();
                    self.restores.insert(version, restored);
                    restored
                }
                None => Some(version),
            };
            self.current.insert(key, now);
        }
        self.before.insert(intent, before);
    }

    /// The write `version` stands for.
    fn restored(&self, version: Version) -> Option<Version> {
        self.restores
            .get(&version)
            .copied()
            .unwrap_or(Some(version))
    }

    /// The intent undo would reverse next.
    pub fn undoable(&self) -> Option<IntentId> {
        self.undo.last().copied()
    }

    /// The undo redo would reverse next.
    pub fn redoable(&self) -> Option<IntentId> {
        self.redo.last().copied()
    }
}

/// The version of each field's last write in `entries`.
fn last_writes(entries: &[Entry]) -> BTreeMap<FieldKey, Version> {
    entries
        .iter()
        .filter_map(|entry| match &entry.kind {
            Kind::Field { entity, name, .. } => Some(((*entity, name.clone()), entry.version)),
            _ => None,
        })
        .collect()
}

/// Whether reversing an entry of this kind changes a fact. Blob store bookkeeping
/// does not, so an intent of only that is not undone.
pub(crate) fn reversible(kind: &Kind) -> bool {
    match kind {
        Kind::Create { .. }
        | Kind::Delete { .. }
        | Kind::Field { .. }
        | Kind::SetAdd { .. }
        | Kind::SetRemove { .. }
        | Kind::File { .. } => true,
        Kind::Intent { .. }
        | Kind::BlobAdded { .. }
        | Kind::BlobRemoved { .. }
        | Kind::Unknown { .. } => false,
    }
}

/// What reversing an intent takes: entries to append under a new intent, and file
/// effects to apply with them.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Plan {
    pub reverses: IntentId,
    /// The new intent's entries, its `Intent` entry first.
    pub entries: Vec<Kind>,
    pub effects: Vec<Effect>,
}

/// The plan that undoes the latest undoable intent, or [`crate::Error::Refused`].
pub fn plan_undo(history: &History, state: &State) -> Result<Plan> {
    plan(history, history.undoable(), state)
}

/// The plan that redoes the latest undo, or [`crate::Error::Refused`].
pub fn plan_redo(history: &History, state: &State) -> Result<Plan> {
    plan(history, history.redoable(), state)
}

fn refuse(refusal: Refusal) -> Error {
    Error::Refused(Box::new(refusal))
}

/// Every write an intent made to one field, as one change.
struct FieldChange {
    /// The prior of the intent's first write: what undo restores.
    before: Option<Value>,
    /// The intent's last write.
    version: Version,
    after: Option<Value>,
}

fn plan(history: &History, intent: Option<IntentId>, state: &State) -> Result<Plan> {
    let intent = intent.ok_or_else(|| refuse(Refusal::Nothing))?;
    let entries = &history.intents[&intent];
    let mut changes: BTreeMap<(EntityId, &str), FieldChange> = BTreeMap::new();
    for entry in entries {
        if let Kind::Field {
            entity,
            name,
            value,
            prior,
        } = &entry.kind
        {
            changes
                .entry((*entity, name.as_str()))
                .and_modify(|change| {
                    change.version = entry.version;
                    change.after = value.clone();
                })
                .or_insert_with(|| FieldChange {
                    before: prior.clone(),
                    version: entry.version,
                    after: value.clone(),
                });
        }
    }
    for (&(entity, name), change) in &changes {
        let current = state.field_version(entity, name);
        let stands_for = current.and_then(|version| history.restored(version));
        if stands_for != history.restored(change.version) {
            return Err(refuse(Refusal::FieldChanged {
                entity,
                name: name.to_owned(),
                wrote: change.version,
                current,
            }));
        }
    }
    let (label, files) = entries
        .iter()
        .find_map(|entry| match &entry.kind {
            Kind::Intent { label, files, .. } => Some((label.clone(), *files)),
            _ => None,
        })
        .unwrap_or_default();
    let effects = match files {
        true => file_effects(entries, &changes, state)?,
        false => Vec::new(),
    };
    let mut kinds = vec![Kind::Intent {
        label,
        reverses: Some(intent),
        files,
    }];
    for entry in entries.iter().rev() {
        kinds.extend(compensate(entry, &mut changes));
    }
    Ok(Plan {
        reverses: intent,
        entries: kinds,
        effects,
    })
}

/// The entry that reverses `entry`. A field's writes are reversed once, at the last.
fn compensate<'a>(
    entry: &'a Entry,
    changes: &mut BTreeMap<(EntityId, &'a str), FieldChange>,
) -> Option<Kind> {
    match &entry.kind {
        Kind::Create { entity } => Some(Kind::Delete { entity: *entity }),
        Kind::Delete { entity } => Some(Kind::Create { entity: *entity }),
        Kind::Field { entity, name, .. } => {
            let change = changes.remove(&(*entity, name.as_str()))?;
            Some(Kind::Field {
                entity: *entity,
                name: name.clone(),
                value: change.before,
                prior: change.after,
            })
        }
        Kind::SetAdd {
            entity,
            name,
            value,
        } => Some(Kind::SetRemove {
            entity: *entity,
            name: name.clone(),
            value: value.clone(),
            observed: [entry.version].into(),
        }),
        Kind::SetRemove {
            entity,
            name,
            value,
            ..
        } => Some(Kind::SetAdd {
            entity: *entity,
            name: name.clone(),
            value: value.clone(),
        }),
        Kind::Intent { .. }
        | Kind::File { .. }
        | Kind::BlobAdded { .. }
        | Kind::BlobRemoved { .. }
        | Kind::Unknown { .. } => None,
    }
}

/// The file changes that reverse an intent that changed files: each file it changed
/// put back as it was, newest first, from the blob store, then its renames reversed.
fn file_effects(
    entries: &[Entry],
    changes: &BTreeMap<(EntityId, &str), FieldChange>,
    state: &State,
) -> Result<Vec<Effect>> {
    let mut effects = Vec::new();
    for entry in entries.iter().rev() {
        let Kind::File {
            path,
            before,
            after,
        } = &entry.kind
        else {
            continue;
        };
        if let Some(blob) = before.filter(|blob| !kept(*blob, state)) {
            return Err(refuse(Refusal::BlobGone { blob }));
        }
        effects.push(Effect::Restore {
            path: path.clone(),
            contents: *before,
            expect: after.map_or(Precondition::Absent, Precondition::Holds),
        });
    }
    for (&(entity, _), change) in changes.iter().filter(|((_, name), _)| *name == PATH_FIELD) {
        if let (Some(Value::Text(to)), Some(Value::Text(from))) = (&change.before, &change.after) {
            if to != from {
                effects.push(Effect::Rename {
                    entity,
                    from: RelPath::new(from)?,
                    to: RelPath::new(to)?,
                });
            }
        }
    }
    Ok(effects)
}

/// Whether some writer's latest record of `blob` is an add.
fn kept(blob: BlobId, state: &State) -> bool {
    state
        .blob_adds()
        .get(&blob)
        .is_some_and(|adds| adds.values().any(|add| !add.removed))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ids::WriterId;
    use crate::log::testing::{field, text, writer, Session};
    use crate::MemFs;
    use crate::CONTENT_FIELD;

    fn reverse(session: &mut Session, redo: bool) -> Result<Plan> {
        let history = History::new(&session.own());
        let state = session.state();
        let plan = match redo {
            false => plan_undo(&history, &state)?,
            true => plan_redo(&history, &state)?,
        };
        let intent = session.log.new_intent();
        session.write(intent, plan.entries.clone()).unwrap();
        Ok(plan)
    }

    fn refusal(result: Result<Plan>) -> Refusal {
        match result {
            Err(Error::Refused(refusal)) => *refusal,
            other => panic!("expected a refusal, got {other:?}"),
        }
    }

    fn name(session: &Session, entity: EntityId) -> Option<Value> {
        session.state().field(entity, "name").cloned()
    }

    fn named(session: &mut Session, entity: EntityId, value: &str) -> IntentId {
        session.act(vec![field(entity, "name", Some(text(value)))])
    }

    fn setup(id: u128) -> (MemFs, Session, EntityId) {
        let fs = MemFs::new();
        let mut session = Session::open(&fs, writer(id));
        let entity = session.log.new_entity();
        session.act(vec![Kind::Create { entity }]);
        (fs, session, entity)
    }

    #[test]
    fn undo_and_redo_walk_back_and_forth_through_intents() {
        let (_, mut session, e) = setup(1);
        named(&mut session, e, "one");
        named(&mut session, e, "two");
        reverse(&mut session, false).unwrap();
        assert_eq!(name(&session, e), Some(text("one")));
        reverse(&mut session, false).unwrap();
        assert_eq!(name(&session, e), None);
        reverse(&mut session, true).unwrap();
        assert_eq!(name(&session, e), Some(text("one")));
        reverse(&mut session, true).unwrap();
        assert_eq!(name(&session, e), Some(text("two")));
        assert_eq!(refusal(reverse(&mut session, true)), Refusal::Nothing);
    }

    #[test]
    fn an_undo_names_the_intent_it_reverses() {
        let (_, mut session, e) = setup(1);
        let intent = session.act(vec![Kind::Intent {
            label: Some("rename".into()),
            reverses: None,
            files: false,
        }]);
        let named = named(&mut session, e, "one");
        let plan = reverse(&mut session, false).unwrap();
        assert_eq!(plan.reverses, named);
        assert_ne!(plan.reverses, intent);
        assert_eq!(
            plan.entries[0],
            Kind::Intent {
                label: None,
                reverses: Some(named),
                files: false,
            }
        );
    }

    #[test]
    fn a_new_intent_after_an_undo_ends_redo() {
        let (_, mut session, e) = setup(1);
        named(&mut session, e, "one");
        reverse(&mut session, false).unwrap();
        named(&mut session, e, "other");
        assert_eq!(refusal(reverse(&mut session, true)), Refusal::Nothing);
    }

    #[test]
    fn there_is_nothing_to_undo_before_any_change() {
        let fs = MemFs::new();
        let mut session = Session::open(&fs, writer(1));
        assert_eq!(refusal(reverse(&mut session, false)), Refusal::Nothing);
        session.act(vec![Kind::BlobAdded {
            blob: BlobId::of(b"x"),
            len: 1,
        }]);
        assert_eq!(refusal(reverse(&mut session, false)), Refusal::Nothing);
    }

    #[test]
    fn undo_is_refused_when_the_field_was_written_since() {
        let (fs, mut session, e) = setup(1);
        named(&mut session, e, "mine");
        let wrote = session.state().field_version(e, "name").unwrap();
        let mut other = Session::open(&fs, writer(2));
        named(&mut other, e, "theirs");
        let current = other.state().field_version(e, "name");
        let before = fs.files();
        assert_eq!(
            refusal(reverse(&mut session, false)),
            Refusal::FieldChanged {
                entity: e,
                name: "name".into(),
                wrote,
                current
            }
        );
        assert_eq!(fs.files(), before);
        assert_eq!(name(&session, e), Some(text("theirs")));
    }

    #[test]
    fn undoing_an_intent_that_wrote_a_field_twice_restores_the_first_prior() {
        let (_, mut session, e) = setup(1);
        named(&mut session, e, "before");
        session.act(vec![
            field(e, "name", Some(text("draft"))),
            field(e, "name", Some(text("final"))),
        ]);
        reverse(&mut session, false).unwrap();
        assert_eq!(name(&session, e), Some(text("before")));
    }

    #[test]
    fn undo_reverses_existence() {
        let (_, mut session, e) = setup(1);
        reverse(&mut session, false).unwrap();
        assert!(!session.state().exists(e));
        reverse(&mut session, true).unwrap();
        assert!(session.state().exists(e));
        session.act(vec![Kind::Delete { entity: e }]);
        reverse(&mut session, false).unwrap();
        assert!(session.state().exists(e));
    }

    #[test]
    fn undoing_a_set_add_spares_another_writers_add() {
        let (fs, mut session, e) = setup(1);
        let add = Kind::SetAdd {
            entity: e,
            name: "tags".into(),
            value: text("x"),
        };
        session.act(vec![add.clone()]);
        Session::open(&fs, writer(2)).act(vec![add]);
        reverse(&mut session, false).unwrap();
        let tags = session.state().tags(e, "tags", &text("x"));
        assert_eq!(
            tags.iter().map(|tag| tag.writer).collect::<Vec<WriterId>>(),
            [writer(2)]
        );
        reverse(&mut session, true).unwrap();
        assert_eq!(session.state().tags(e, "tags", &text("x")).len(), 2);
    }

    /// The `Intent` entry of an intent that changed files.
    const FILES: Kind = Kind::Intent {
        label: None,
        reverses: None,
        files: true,
    };

    fn saved(contents: &[u8]) -> (BlobId, Kind) {
        let blob = BlobId::of(contents);
        let added = Kind::BlobAdded {
            blob,
            len: contents.len() as u64,
        };
        (blob, added)
    }

    fn file(at: &str, before: Option<BlobId>, after: Option<BlobId>) -> Kind {
        Kind::File {
            path: RelPath::new(at).unwrap(),
            before,
            after,
        }
    }

    fn content(entity: EntityId, blob: BlobId) -> Kind {
        field(entity, CONTENT_FIELD, Some(Value::Blob(blob)))
    }

    #[test]
    fn undoing_a_save_restores_the_bytes_it_displaced() {
        let (_, mut session, e) = setup(1);
        let (old, added_old) = saved(b"old");
        let new = BlobId::of(b"newer");
        session.act_as(
            FILES,
            vec![
                file("a/b.txt", None, Some(old)),
                field(e, PATH_FIELD, Some(text("a/b.txt"))),
                content(e, old),
            ],
        );
        session.act_as(
            FILES,
            vec![
                added_old,
                file("a/b.txt", Some(old), Some(new)),
                content(e, new),
            ],
        );
        let plan = reverse(&mut session, false).unwrap();
        assert_eq!(
            plan.effects,
            [Effect::Restore {
                path: RelPath::new("a/b.txt").unwrap(),
                contents: Some(old),
                expect: Precondition::Holds(new),
            }]
        );
    }

    #[test]
    fn undoing_a_save_over_a_file_changed_outside_restores_that_file_and_not_the_content() {
        let (_, mut session, e) = setup(1);
        let (bound, theirs, mine) = (BlobId::of(b"bound"), b"theirs", BlobId::of(b"mine"));
        session.act(vec![
            field(e, PATH_FIELD, Some(text("f"))),
            content(e, bound),
        ]);
        let (theirs, added_theirs) = saved(theirs);
        session.act_as(
            FILES,
            vec![
                added_theirs,
                file("f", Some(theirs), Some(mine)),
                content(e, mine),
            ],
        );
        let plan = reverse(&mut session, false).unwrap();
        assert_eq!(
            plan.effects,
            [Effect::Restore {
                path: RelPath::new("f").unwrap(),
                contents: Some(theirs),
                expect: Precondition::Holds(mine),
            }]
        );
        assert_eq!(
            session.state().field(e, CONTENT_FIELD),
            Some(&Value::Blob(bound)),
            "the content register is restored apart from the file"
        );
    }

    #[test]
    fn undo_is_refused_when_the_displaced_bytes_were_collected() {
        let (_, mut session, e) = setup(1);
        let (old, added_old) = saved(b"old");
        let new = BlobId::of(b"newer");
        session.act_as(
            FILES,
            vec![
                file("f", None, Some(old)),
                field(e, PATH_FIELD, Some(text("f"))),
                content(e, old),
            ],
        );
        session.act_as(
            FILES,
            vec![added_old, file("f", Some(old), Some(new)), content(e, new)],
        );
        session.act(vec![Kind::BlobRemoved { blob: old }]);
        assert_eq!(
            refusal(reverse(&mut session, false)),
            Refusal::BlobGone { blob: old }
        );
    }

    #[test]
    fn undoing_a_new_file_deletes_it_and_undoing_a_rename_moves_it_back() {
        let (_, mut session, e) = setup(1);
        let blob = BlobId::of(b"bytes");
        session.act_as(
            FILES,
            vec![
                file("a", None, Some(blob)),
                field(e, PATH_FIELD, Some(text("a"))),
                content(e, blob),
            ],
        );
        session.act_as(FILES, vec![field(e, PATH_FIELD, Some(text("b")))]);
        let path = |text: &str| RelPath::new(text).unwrap();
        let plan = reverse(&mut session, false).unwrap();
        assert_eq!(
            plan.effects,
            [Effect::Rename {
                entity: e,
                from: path("b"),
                to: path("a")
            }]
        );
        let plan = reverse(&mut session, false).unwrap();
        assert_eq!(
            plan.effects,
            [Effect::Restore {
                path: path("a"),
                contents: None,
                expect: Precondition::Holds(blob),
            }]
        );
    }

    #[test]
    fn undoing_a_bind_reverses_its_fields_and_leaves_the_file() {
        let (_, mut session, e) = setup(1);
        let blob = BlobId::of(b"theirs");
        session.act(vec![
            field(e, PATH_FIELD, Some(text("user.txt"))),
            field(e, CONTENT_FIELD, Some(Value::Blob(blob))),
        ]);
        let plan = reverse(&mut session, false).unwrap();
        assert_eq!(plan.effects, []);
        assert_eq!(session.state().field(e, PATH_FIELD), None);
        let plan = reverse(&mut session, true).unwrap();
        assert_eq!(plan.effects, []);
        assert_eq!(
            session.state().field(e, PATH_FIELD),
            Some(&text("user.txt"))
        );
    }

    #[test]
    fn undoing_a_file_delete_restores_the_file_where_it_was() {
        let (_, mut session, e) = setup(1);
        let (blob, added) = saved(b"bytes");
        session.act_as(
            FILES,
            vec![
                file("a", None, Some(blob)),
                field(e, PATH_FIELD, Some(text("a"))),
                content(e, blob),
            ],
        );
        session.act_as(
            FILES,
            vec![
                added,
                file("a", Some(blob), None),
                field(e, PATH_FIELD, None),
                field(e, CONTENT_FIELD, None),
            ],
        );
        let plan = reverse(&mut session, false).unwrap();
        assert_eq!(
            plan.effects,
            [Effect::Restore {
                path: RelPath::new("a").unwrap(),
                contents: Some(blob),
                expect: Precondition::Absent,
            }]
        );
    }
}
